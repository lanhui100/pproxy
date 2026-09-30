use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 用户凭据 Claims（自包含在 Token 中）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserTokenClaims {
    /// 唯一令牌 ID，用于防重放与单凭据黑名单撤销
    pub jti: String,
    /// 用户主体唯一标识 (UID)
    pub sub: String,
    /// 显示昵称
    #[serde(default)]
    pub name: String,
    /// 总周期配额字节数 (Bytes)
    pub quota_bytes: u64,
    /// 默认每次租约分配额度 (Lease Step)，默认 500MB
    #[serde(default = "default_lease_bytes")]
    pub lease_bytes: u64,
    /// 硬过期时间戳 (UNIX 秒)
    pub exp: u64,
    /// 签发时间戳 (UNIX 秒)
    pub iat: u64,
    /// 最大允许并发连接数（商业规范硬编码或签发时指定，默认 3）
    #[serde(default = "default_max_conns")]
    pub max_conns: usize,
    /// 用户角色："admin"（管理员）或 "user"（普通用户，默认）
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "user".to_string()
}

fn default_lease_bytes() -> u64 {
    500 * 1024 * 1024 // 500MB
}

fn default_max_conns() -> usize {
    3
}

impl UserTokenClaims {
    pub fn is_expired(&self) -> bool {
        self.is_time_valid().is_err()
    }

    /// 严格时间有效性校验（防未来穿越、时钟回拨、非法生命周期）
    pub fn is_time_valid(&self) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 允许最大 60 秒的时钟倾斜 (Clock Skew Tolerance)
        const CLOCK_SKEW_TOLERANCE_SECS: u64 = 60;

        if now > self.exp {
            anyhow::bail!("token has expired at {} (current: {})", self.exp, now);
        }

        // 防未来时间穿越：签发时间不能大于当前时间加时钟倾斜容忍度
        if self.iat > now.saturating_add(CLOCK_SKEW_TOLERANCE_SECS) {
            anyhow::bail!("token issued in the future (iat: {}, current: {})", self.iat, now);
        }

        // 基本合理性检查：过期时间不能早于签发时间
        if self.exp <= self.iat {
            anyhow::bail!("malformed token lifetime: exp <= iat");
        }

        Ok(())
    }
}

/// 密钥对管理器（非对称隔离：管理端有私钥，所有边缘节点仅有公钥）
pub struct TokenSigner {
    signing_key: SigningKey,
}

impl TokenSigner {
    pub fn new(signing_key: SigningKey) -> Self {
        Self { signing_key }
    }

    pub fn generate() -> (Self, VerifyingKey) {
        let mut rng = rand::rngs::OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        let verifying_key = signing_key.verifying_key();
        (Self { signing_key }, verifying_key)
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(bytes);
        Self { signing_key }
    }

    /// 从 32 字节十六进制种子（`~/.pony/cluster_signing_key.hex` 格式）恢复签名器
    pub fn from_seed_hex(hex_str: &str) -> Result<Self> {
        let bytes = hex::decode(hex_str.trim()).map_err(|e| anyhow!("无效的种子 HEX: {e}"))?;
        if bytes.len() != 32 {
            return Err(anyhow!("种子必须为 32 字节（64 个 HEX 字符）"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self::from_bytes(&arr))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// 对任意消息做 Ed25519 签名，返回 64 字节签名。
    ///
    /// 升级包签名约定：签名对象为**升级包的 SHA-256 摘要**（先摘要后签名，
    /// 与 `pproxy user sign` / `pproxy cluster upgrade` 双侧一致）。
    pub fn sign_bytes(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    /// 签发自包含令牌：格式为 `usr_live_<payload_b64url>.<signature_b64url>`
    pub fn sign_token(&self, claims: &UserTokenClaims) -> Result<String> {
        let payload_json = serde_json::to_vec(claims)?;
        let payload_b64 = URL_SAFE_NO_PAD.encode(&payload_json);

        let signature: Signature = self.signing_key.sign(payload_b64.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

        Ok(format!("usr_live_{}.{}", payload_b64, sig_b64))
    }
}

/// 边缘验签器（仅持有公钥，可在任何单机/轻量 VPS 极速离线验签）
#[derive(Clone)]
pub struct TokenVerifier {
    verifying_key: VerifyingKey,
}

impl TokenVerifier {
    pub fn new(verifying_key: VerifyingKey) -> Self {
        Self { verifying_key }
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self> {
        let verifying_key = VerifyingKey::from_bytes(bytes)
            .map_err(|e| anyhow!("invalid verifying key bytes: {e}"))?;
        Ok(Self { verifying_key })
    }

    pub fn from_hex(hex_str: &str) -> Result<Self> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(anyhow!("verifying key must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Self::from_bytes(&arr)
    }

    /// 提取未经验签的 Payload 内容（仅用于提取 jti/sub 识别令牌，不可替代验签）
    pub fn peek_claims_unverified(token: &str) -> Result<UserTokenClaims> {
        let token = token.trim();
        let stripped = token
            .strip_prefix("usr_live_")
            .ok_or_else(|| anyhow!("token must start with usr_live_"))?;

        let (payload_b64, _) = stripped
            .split_once('.')
            .ok_or_else(|| anyhow!("invalid token format: missing signature dot delimiter"))?;

        let payload_json = URL_SAFE_NO_PAD.decode(payload_b64)
            .map_err(|e| anyhow!("invalid payload base64: {e}"))?;

        let claims: UserTokenClaims = serde_json::from_slice(&payload_json)
            .map_err(|e| anyhow!("invalid claims json: {e}"))?;

        Ok(claims)
    }

    /// 校验令牌并返回解析出的用户 Claims
    pub fn verify_token(&self, token: &str) -> Result<UserTokenClaims> {
        let token = token.trim();
        let stripped = token
            .strip_prefix("usr_live_")
            .ok_or_else(|| anyhow!("token must start with usr_live_"))?;

        let (payload_b64, sig_b64) = stripped
            .split_once('.')
            .ok_or_else(|| anyhow!("invalid token format: missing signature dot delimiter"))?;

        let sig_bytes = URL_SAFE_NO_PAD.decode(sig_b64)
            .map_err(|e| anyhow!("invalid signature base64: {e}"))?;

        if sig_bytes.len() != 64 {
            return Err(anyhow!("invalid signature length: expected 64 bytes"));
        }

        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let signature = Signature::from_bytes(&sig_arr);

        // 强校验公钥签名（Payload_b64 作为签名原文）
        self.verifying_key
            .verify(payload_b64.as_bytes(), &signature)
            .map_err(|e| anyhow!("cryptographic signature verification failed: {e}"))?;

        let payload_json = URL_SAFE_NO_PAD.decode(payload_b64)
            .map_err(|e| anyhow!("invalid payload base64: {e}"))?;

        let claims: UserTokenClaims = serde_json::from_slice(&payload_json)
            .map_err(|e| anyhow!("invalid claims json: {e}"))?;

        claims.is_time_valid()?;

        Ok(claims)
    }

    /// 校验任意消息的 Ed25519 签名（升级包签名约定：对升级包 SHA-256 摘要签名）。
    ///
    /// 供 `pproxy cluster upgrade` 在替换二进制前做防篡改硬校验。
    pub fn verify_bytes(&self, message: &[u8], sig_bytes: &[u8]) -> Result<()> {
        if sig_bytes.len() != 64 {
            return Err(anyhow!("Ed25519 签名长度必须为 64 字节"));
        }
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(sig_bytes);
        let signature = Signature::from_bytes(&sig_arr);
        self.verifying_key
            .verify_strict(message, &signature)
            .map_err(|e| anyhow!("升级包签名校验失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify_token() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = TokenVerifier::new(vk);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = UserTokenClaims {
            jti: "token-001".into(),
            sub: "usr_alice".into(),
            name: "Alice".into(),
            quota_bytes: 50 * 1024 * 1024 * 1024,
            lease_bytes: 500 * 1024 * 1024,
            exp: now + 3600,
            iat: now,
            max_conns: 3,
            role: "user".into(),
        };

        let token = signer.sign_token(&claims).unwrap();
        assert!(token.starts_with("usr_live_"));

        let decoded = verifier.verify_token(&token).unwrap();
        assert_eq!(decoded.sub, "usr_alice");
        assert_eq!(decoded.name, "Alice");
        assert_eq!(decoded.quota_bytes, 50 * 1024 * 1024 * 1024);
        assert_eq!(decoded.max_conns, 3);
        assert_eq!(decoded.role, "user");
    }

    #[test]
    fn test_expired_token_rejected() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = TokenVerifier::new(vk);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = UserTokenClaims {
            jti: "token-expired".into(),
            sub: "usr_bob".into(),
            name: "Bob".into(),
            quota_bytes: 1024,
            lease_bytes: 512,
            exp: now - 10, // 已过期
            iat: now - 100,
            max_conns: 3,
            role: "user".into(),
        };

        let token = signer.sign_token(&claims).unwrap();
        let err = verifier.verify_token(&token).unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn test_tampered_payload_rejected() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = TokenVerifier::new(vk);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        let claims = UserTokenClaims {
            jti: "token-tamper".into(),
            sub: "usr_charlie".into(),
            name: "Charlie".into(),
            quota_bytes: 1024,
            lease_bytes: 512,
            exp: now + 3600,
            iat: now,
            max_conns: 3,
            role: "user".into(),
        };

        let token = signer.sign_token(&claims).unwrap();
        // 篡改 payload 部分
        let parts: Vec<&str> = token.split('.').collect();
        let tampered_token = format!("{}.{}", parts[0].replace("a", "b"), parts[1]);

        let err = verifier.verify_token(&tampered_token).unwrap_err();
        assert!(err.to_string().contains("signature verification failed"));
    }

    #[test]
    fn test_sign_bytes_and_verify_bytes_roundtrip() {
        // 升级包签名约定：对 SHA-256 摘要签名，双侧 verify_bytes 硬校验
        use sha2::{Digest, Sha256};
        let (signer, vk) = TokenSigner::generate();
        let verifier = TokenVerifier::new(vk);

        let package = b"mock pproxy binary bytes v0.3.57";
        let digest = Sha256::digest(package);
        let sig = signer.sign_bytes(&digest);
        assert_eq!(sig.len(), 64);

        verifier.verify_bytes(&digest, &sig).expect("valid signature passes");
    }

    #[test]
    fn test_verify_bytes_rejects_tampered_message_and_sig() {
        use sha2::{Digest, Sha256};
        let (signer, vk) = TokenSigner::generate();
        let verifier = TokenVerifier::new(vk);

        let package = b"mock pproxy binary bytes";
        let digest = Sha256::digest(package);
        let sig = signer.sign_bytes(&digest);

        // 篡改消息（换个摘要）→ 拒绝
        let tampered_digest = Sha256::digest(b"tampered bytes");
        assert!(verifier.verify_bytes(&tampered_digest, &sig).is_err());

        // 签名长度非法 → 拒绝
        assert!(verifier.verify_bytes(&digest, &sig[..63]).is_err());
    }

    #[test]
    fn test_signer_from_seed_hex_roundtrip() {
        let (signer, _vk) = TokenSigner::generate();
        let seed_hex = hex::encode(signer.to_bytes());
        let restored = TokenSigner::from_seed_hex(&seed_hex).unwrap();
        assert_eq!(restored.to_bytes(), signer.to_bytes());
        assert_eq!(
            hex::encode(restored.verifying_key().to_bytes()),
            hex::encode(signer.verifying_key().to_bytes())
        );

        assert!(TokenSigner::from_seed_hex("abcd").is_err());
        assert!(TokenSigner::from_seed_hex("zz").is_err());
    }
}
