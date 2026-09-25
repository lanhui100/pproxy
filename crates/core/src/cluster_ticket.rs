use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const TICKET_MAX_AGE_SECS: u64 = 120;
pub const CLUSTER_TICKET_HEADER: &str = "x-pony-cluster-ticket";

/// 安全标准 HMAC-SHA256 实现（严格遵循 RFC 2104 规范）
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];

    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        key_block[..32].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    outer.finalize().into()
}

/// 恒定时间字节比对（抵御微秒级时序侧信道嗅探）
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 校验 node_id 字符集，根除定界符注入
fn sanitize_node_id(node_id: &str) -> Result<()> {
    if node_id.trim().is_empty() {
        return Err(anyhow!("node_id cannot be empty"));
    }
    if node_id.contains('|') || node_id.contains('\n') || node_id.contains('\r') {
        return Err(anyhow!("forbidden character '|' or newline in node_id"));
    }
    Ok(())
}

pub fn create_ticket(cluster_auth_key: &str, node_id: &str) -> Result<String> {
    sanitize_node_id(node_id)?;
    if cluster_auth_key.trim().is_empty() {
        return Err(anyhow!("cluster_auth_key is empty"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(create_ticket_at(cluster_auth_key, node_id, now))
}

pub fn create_ticket_at(cluster_auth_key: &str, node_id: &str, ts: u64) -> String {
    let payload = format!("{node_id}|{ts}");
    let mac = hmac_sha256(cluster_auth_key.as_bytes(), payload.as_bytes());
    format!("{payload}|{}", hex::encode(mac))
}

pub fn verify_ticket(cluster_auth_key: &str, ticket: &str, now: u64) -> Result<String> {
    let ticket = ticket.trim();
    if ticket.is_empty() {
        return Err(anyhow!("empty cluster ticket"));
    }

    // 严苛结构拆解：限制仅切两次，防字段膨胀
    let (node_id, rest) = ticket.split_once('|')
        .ok_or_else(|| anyhow!("malformed cluster ticket: missing node_id delimiter"))?;
    let (ts_str, presented_mac_hex) = rest.split_once('|')
        .ok_or_else(|| anyhow!("malformed cluster ticket: missing timestamp delimiter"))?;

    sanitize_node_id(node_id)?;

    let ts: u64 = ts_str.parse().map_err(|_| anyhow!("invalid ticket timestamp"))?;
    let age = now.abs_diff(ts);
    if age > TICKET_MAX_AGE_SECS {
        return Err(anyhow!("cluster ticket expired: age={age}s > max={TICKET_MAX_AGE_SECS}s"));
    }

    let payload = format!("{node_id}|{ts}");
    let expected_mac = hmac_sha256(cluster_auth_key.as_bytes(), payload.as_bytes());
    let presented_mac = hex::decode(presented_mac_hex)
        .map_err(|_| anyhow!("invalid ticket mac hex"))?;

    if !constant_time_eq(&expected_mac, &presented_mac) {
        return Err(anyhow!("cluster ticket HMAC mismatch (forged or tampered)"));
    }

    Ok(node_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ticket_roundtrip_valid() {
        let key = "aabbccdd1122";
        let ticket = create_ticket(key, "devserver").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let node = verify_ticket(key, &ticket, now).unwrap();
        assert_eq!(node, "devserver");
    }

    #[test]
    fn test_ticket_forged_rejected() {
        let key = "secret-key";
        let ticket = create_ticket(key, "devserver").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let tampered = ticket.replace("devserver", "attacker");
        let err = verify_ticket(key, &tampered, now).unwrap_err();
        assert!(err.to_string().contains("HMAC mismatch"));
    }

    #[test]
    fn test_ticket_wrong_key_rejected() {
        let ticket = create_ticket("correct-key", "devserver").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let err = verify_ticket("wrong-key", &ticket, now).unwrap_err();
        assert!(err.to_string().contains("HMAC mismatch"));
    }

    #[test]
    fn test_ticket_replay_rejected() {
        let key = "secret-key";
        let ts = 1_700_000_000;
        let ticket = create_ticket_at(key, "devserver", ts);
        let now = ts + TICKET_MAX_AGE_SECS + 10;
        let err = verify_ticket(key, &ticket, now).unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn test_ticket_delimiter_injection_rejected() {
        let key = "secret-key";
        let err = create_ticket(key, "node|1899999999").unwrap_err();
        assert!(err.to_string().contains("forbidden character"));
    }
}
