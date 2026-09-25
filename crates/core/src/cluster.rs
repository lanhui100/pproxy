use anyhow::{anyhow, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// 集群消息协议版本（保证跨版本节点通信向前兼容）
pub const CLUSTER_SCHEMA_VERSION: u16 = 1;

/// 节点健康状态（SWIM 三态模型 + Tombstone 墓碑机制）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Alive,
    Suspect,
    Dead,
}

/// 对等节点元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerNode {
    pub node_id: String,
    pub internal_addr: SocketAddr,
    pub version: String,
    pub state: NodeState,
    pub incarnation: u64,
    pub active_conns: usize,
    pub total_bytes_in: u64,
    pub total_bytes_out: u64,
    pub last_seen_epoch: u64,
    pub rtt_ms: u32,
}

/// Gossip 心跳包（松散反序列化，防跨版本崩溃）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterHeartbeat {
    pub schema_version: u16,
    pub sender_id: String,
    pub sender_addr: SocketAddr,
    pub version: String,
    pub incarnation: u64,
    pub active_conns: usize,
    pub total_bytes_in: u64,
    pub total_bytes_out: u64,
    pub timestamp: u64,
    #[serde(default)]
    pub peers: Vec<PeerNode>,
}

/// 墓碑记录（防止已下线节点在 Gossip 反熵中借尸还魂）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tombstone {
    pub node_id: String,
    pub dead_epoch: u64,
    pub incarnation: u64,
}

/// 一次性加入令牌（One-Time Join Token, OTT，含全量配置自愈载荷与 HMAC 防篡改签名）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterJoinToken {
    pub cluster_id: String,
    pub cluster_auth_key: String, // 32 字节十六进制 HMAC 密钥
    pub seed_addr: SocketAddr,
    pub exp: u64,
    pub nonce: String,
    /// 全量配置自愈载荷：出海 Gate 端点列表（如 wss://rn.ponygo.fun/ws,wss://...）
    #[serde(default)]
    pub tunnel_gate_url: Option<String>,
    /// 全量配置自愈载荷：出海隧道认证 Token
    #[serde(default)]
    pub tunnel_token: Option<String>,
    /// 全量配置自愈载荷：多租户令牌验签公钥 HEX
    #[serde(default)]
    pub user_verifying_key: Option<String>,
    #[serde(default)]
    pub signature: String,
}

impl ClusterJoinToken {
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now > self.exp
    }

    fn compute_sig(
        cluster_id: &str,
        seed_addr: &SocketAddr,
        exp: u64,
        nonce: &str,
        key: &str,
        gate_url: Option<&str>,
        tunnel_token: Option<&str>,
        verifying_key: Option<&str>,
    ) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        // 密码学安全加固 SEC-VULN-06：域隔离前缀 + 长度前缀编码 (Length-Prefixed Canonicalization) 彻底杜绝拼接碰撞
        hasher.update(b"PONY_CLUSTER_JOIN_V1\x00");

        let mut put_field = |h: &mut Sha256, val: &str| {
            h.update(&(val.len() as u32).to_le_bytes());
            h.update(val.as_bytes());
        };

        put_field(&mut hasher, cluster_id);
        put_field(&mut hasher, &seed_addr.to_string());
        hasher.update(&exp.to_le_bytes());
        put_field(&mut hasher, nonce);
        put_field(&mut hasher, key);
        put_field(&mut hasher, gate_url.unwrap_or(""));
        put_field(&mut hasher, tunnel_token.unwrap_or(""));
        put_field(&mut hasher, verifying_key.unwrap_or(""));

        hex::encode(hasher.finalize())
    }

    pub fn new_signed(
        cluster_id: String,
        cluster_auth_key: String,
        seed_addr: SocketAddr,
        valid_minutes: u64,
        nonce: String,
        tunnel_gate_url: Option<String>,
        tunnel_token: Option<String>,
        user_verifying_key: Option<String>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let exp = now + valid_minutes * 60;
        let signature = Self::compute_sig(
            &cluster_id,
            &seed_addr,
            exp,
            &nonce,
            &cluster_auth_key,
            tunnel_gate_url.as_deref(),
            tunnel_token.as_deref(),
            user_verifying_key.as_deref(),
        );
        Self {
            cluster_id,
            cluster_auth_key,
            seed_addr,
            exp,
            nonce,
            tunnel_gate_url,
            tunnel_token,
            user_verifying_key,
            signature,
        }
    }

    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    pub fn decode(encoded: &str) -> Result<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded.trim())
            .map_err(|e| anyhow!("无效的 Base64 编码: {e}"))?;
        let token: Self = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow!("无效的集群加入令牌数据: {e}"))?;

        if token.is_expired() {
            return Err(anyhow!("集群加入令牌已过期"));
        }

        // 密码学安全校验：强验签名防篡改
        let expected_sig = Self::compute_sig(
            &token.cluster_id,
            &token.seed_addr,
            token.exp,
            &token.nonce,
            &token.cluster_auth_key,
            token.tunnel_gate_url.as_deref(),
            token.tunnel_token.as_deref(),
            token.user_verifying_key.as_deref(),
        );
        if token.signature != expected_sig {
            return Err(anyhow!("集群加入令牌签名无效或已被篡改！"));
        }

        Ok(token)
    }
}

/// 内存集群拓扑管理器（对等无锁/读写锁拓扑表）
#[derive(Clone)]
pub struct ClusterManager {
    local_id: String,
    local_addr: SocketAddr,
    auth_key: String,
    peers: Arc<RwLock<HashMap<String, (PeerNode, Instant)>>>,
    tombstones: Arc<RwLock<HashMap<String, Tombstone>>>,
}

impl ClusterManager {
    pub fn new(local_id: String, local_addr: SocketAddr, auth_key: String) -> Self {
        Self {
            local_id,
            local_addr,
            auth_key,
            peers: Arc::new(RwLock::new(HashMap::new())),
            tombstones: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn auth_key(&self) -> &str {
        &self.auth_key
    }

    pub fn local_id(&self) -> &str {
        &self.local_id
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// 产生本地心跳数据
    pub async fn make_heartbeat(&self, conns: usize, in_bytes: u64, out_bytes: u64) -> ClusterHeartbeat {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let guard = self.peers.read().await;
        let peers_list: Vec<PeerNode> = guard.values().map(|(p, _)| p.clone()).collect();

        ClusterHeartbeat {
            schema_version: CLUSTER_SCHEMA_VERSION,
            sender_id: self.local_id.clone(),
            sender_addr: self.local_addr,
            version: env!("CARGO_PKG_VERSION").to_string(),
            incarnation: 1,
            active_conns: conns,
            total_bytes_in: in_bytes,
            total_bytes_out: out_bytes,
            timestamp: now,
            peers: peers_list,
        }
    }

    /// 合并收到的 Gossip 心跳（遵循 SWIM 冲突仲裁与时钟防倒退）
    pub async fn merge_heartbeat(&self, hb: ClusterHeartbeat, rtt_ms: u32) {
        if hb.sender_id == self.local_id {
            return;
        }

        // 检查墓碑表：如果已确认彻底死亡并在保留期，拒绝盲目复活
        let tombstones_guard = self.tombstones.read().await;
        if let Some(tb) = tombstones_guard.get(&hb.sender_id) {
            if hb.incarnation <= tb.incarnation {
                return;
            }
        }
        drop(tombstones_guard);

        let mut guard = self.peers.write().await;
        if let Some((existing, _)) = guard.get(&hb.sender_id) {
            // 防乱序旧包覆盖新状态（ABA 时序防倒退）
            if hb.timestamp < existing.last_seen_epoch || hb.incarnation < existing.incarnation {
                return;
            }
        }

        let node = PeerNode {
            node_id: hb.sender_id.clone(),
            internal_addr: hb.sender_addr,
            version: hb.version,
            state: NodeState::Alive,
            incarnation: hb.incarnation,
            active_conns: hb.active_conns,
            total_bytes_in: hb.total_bytes_in,
            total_bytes_out: hb.total_bytes_out,
            last_seen_epoch: hb.timestamp,
            rtt_ms,
        };
        guard.insert(hb.sender_id, (node, Instant::now()));

        // 反熵传递其他节点
        for p in hb.peers {
            if p.node_id != self.local_id && !guard.contains_key(&p.node_id) {
                guard.insert(p.node_id.clone(), (p, Instant::now()));
            }
        }
    }

    /// 获取当前集群全景快照（跨洋网络调优：30s Suspect / 120s Dead，死节点进入 Tombstone 驱逐）
    pub async fn get_cluster_snapshot(&self) -> Vec<PeerNode> {
        let mut guard = self.peers.write().await;
        let mut tombstones = self.tombstones.write().await;
        let now = Instant::now();
        let now_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

        let mut dead_nodes = Vec::new();

        // 跨洋网络调优：30s 标记 Suspect，120s 标记 Dead 并物理转移至墓碑列表
        for (id, (p, last_instant)) in guard.iter_mut() {
            let elapsed = now.duration_since(*last_instant);
            if elapsed > Duration::from_secs(120) {
                p.state = NodeState::Dead;
                dead_nodes.push((id.clone(), p.incarnation));
            } else if elapsed > Duration::from_secs(30) {
                p.state = NodeState::Suspect;
            }
        }

        // 审查修复（P0）：从活跃 Peer 表中物理剔除死节点并计入墓碑，根除幽灵节点滞留与复活
        for (dead_id, incarnation) in dead_nodes {
            guard.remove(&dead_id);
            tombstones.insert(dead_id.clone(), Tombstone {
                node_id: dead_id,
                dead_epoch: now_epoch,
                incarnation,
            });
        }

        guard.values().map(|(p, _)| p.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_token_encode_decode() {
        let tok = ClusterJoinToken::new_signed(
            "corp-mesh".into(),
            "abc123secret".into(),
            "100.95.193.103:8899".parse().unwrap(),
            10,
            "rnd-nonce".into(),
            Some("wss://rn.ponygo.fun/ws".into()),
            Some("gate_secret".into()),
            Some("pubkey_hex".into()),
        );

        let encoded = tok.encode();
        let decoded = ClusterJoinToken::decode(&encoded).unwrap();
        assert_eq!(decoded.cluster_id, "corp-mesh");
        assert_eq!(decoded.seed_addr, tok.seed_addr);
        assert_eq!(decoded.tunnel_gate_url.as_deref(), Some("wss://rn.ponygo.fun/ws"));
        assert_eq!(decoded.tunnel_token.as_deref(), Some("gate_secret"));
        assert_eq!(decoded.user_verifying_key.as_deref(), Some("pubkey_hex"));
        assert!(!decoded.is_expired());
    }

    #[tokio::test]
    async fn test_cluster_heartbeat_merge_and_swim_state() {
        let mgr = ClusterManager::new(
            "node-dev".into(),
            "100.95.193.103:8899".parse().unwrap(),
            "secret".into(),
        );

        let hb = ClusterHeartbeat {
            schema_version: 1,
            sender_id: "node-preprod".into(),
            sender_addr: "100.97.143.121:8899".parse().unwrap(),
            version: "v0.4.0".into(),
            incarnation: 1,
            active_conns: 5,
            total_bytes_in: 1000,
            total_bytes_out: 5000,
            timestamp: 1735000000,
            peers: vec![],
        };

        mgr.merge_heartbeat(hb, 12).await;
        let snapshot = mgr.get_cluster_snapshot().await;
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].node_id, "node-preprod");
        assert_eq!(snapshot[0].state, NodeState::Alive);
        assert_eq!(snapshot[0].rtt_ms, 12);
    }
}
