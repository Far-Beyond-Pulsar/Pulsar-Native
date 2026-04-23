use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,               // Subject (user/peer ID)
    pub session_id: String,        // Session ID
    pub role: Role,                // User role
    pub capabilities: Vec<String>, // Capabilities/permissions
    pub exp: u64,                  // Expiration timestamp
    pub iat: u64,                  // Issued at timestamp
    pub nbf: u64,                  // Not before timestamp
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Host,
    Editor,
    Observer,
}

impl Role {
    pub fn capabilities(&self) -> Vec<String> {
        match self {
            Role::Host => vec![
                "create_session".to_string(),
                "close_session".to_string(),
                "edit".to_string(),
                "read".to_string(),
                "invite".to_string(),
                "kick".to_string(),
            ],
            Role::Editor => vec!["edit".to_string(), "read".to_string()],
            Role::Observer => vec!["read".to_string()],
        }
    }
}

pub struct AuthService {
    jwt_encoding_key: EncodingKey,
    jwt_decoding_key: DecodingKey,
    server_signing_key: SigningKey,
    server_verifying_key: VerifyingKey,
}

impl AuthService {
    pub fn new(config: &Config) -> Result<Self> {
        let jwt_encoding_key = EncodingKey::from_secret(config.jwt_secret.as_bytes());
        let jwt_decoding_key = DecodingKey::from_secret(config.jwt_secret.as_bytes());

        // Generate or load server Ed25519 keys
        let server_signing_key = if let Some(key_b64) = &config.server_ed25519_key {
            let key_bytes = hex_decode(key_b64)
                .map_err(|e| anyhow::anyhow!("Failed to decode server Ed25519 key: {}", e))?;
            SigningKey::from_bytes(
                &key_bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid Ed25519 key length"))?,
            )
        } else {
            tracing::warn!(
                "Generating ephemeral Ed25519 server key - this should be persisted in production"
            );
            SigningKey::generate(&mut OsRng)
        };

        let server_verifying_key = server_signing_key.verifying_key();

        Ok(Self {
            jwt_encoding_key,
            jwt_decoding_key,
            server_signing_key,
            server_verifying_key,
        })
    }

    /// Create a new JWT token for a session
    pub fn create_token(
        &self,
        peer_id: String,
        session_id: String,
        role: Role,
        ttl: Duration,
    ) -> Result<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let exp = now + ttl.as_secs();

        let claims = Claims {
            sub: peer_id,
            session_id,
            capabilities: role.capabilities(),
            role,
            exp,
            iat: now,
            nbf: now,
        };

        let token = encode(&Header::default(), &claims, &self.jwt_encoding_key)
            .context("Failed to encode JWT")?;

        Ok(token)
    }

    /// Verify and decode a JWT token
    pub fn verify_token(&self, token: &str) -> Result<Claims> {
        let token_data = decode::<Claims>(token, &self.jwt_decoding_key, &Validation::default())
            .context("Failed to decode JWT")?;

        Ok(token_data.claims)
    }

    /// Create a session join token (signed and time-limited)
    pub fn create_join_token(
        &self,
        session_id: String,
        role: Role,
        ttl: Duration,
    ) -> Result<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let exp = now + ttl.as_secs();

        let join_data = serde_json::json!({
            "session_id": session_id,
            "role": role,
            "exp": exp,
            "iat": now,
        });

        let payload = serde_json::to_vec(&join_data)?;
        let signature = self.server_signing_key.sign(&payload);

        // Encode as hex: payload || signature
        let mut token_bytes = payload;
        token_bytes.extend_from_slice(&signature.to_bytes());
        Ok(hex_encode(&token_bytes))
    }

    /// Verify a session join token
    pub fn verify_join_token(&self, token: &str) -> Result<(String, Role)> {
        let token_bytes =
            hex_decode(token).map_err(|e| anyhow::anyhow!("Failed to decode join token: {}", e))?;

        if token_bytes.len() < 64 {
            anyhow::bail!("Invalid join token length");
        }

        let (payload, sig_bytes) = token_bytes.split_at(token_bytes.len() - 64);
        let signature = Signature::from_bytes(
            sig_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid signature"))?,
        );

        self.server_verifying_key
            .verify(payload, &signature)
            .context("Invalid signature")?;

        let join_data: serde_json::Value = serde_json::from_slice(payload)?;

        // Check expiration
        let exp = join_data["exp"].as_u64().context("Missing exp field")?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        if now > exp {
            anyhow::bail!("Join token expired");
        }

        let session_id = join_data["session_id"]
            .as_str()
            .context("Missing session_id")?
            .to_string();

        let role: Role = serde_json::from_value(join_data["role"].clone())?;

        Ok((session_id, role))
    }

    /// Sign arbitrary data with server key
    pub fn sign_data(&self, data: &[u8]) -> Vec<u8> {
        self.server_signing_key.sign(data).to_bytes().to_vec()
    }

    /// Verify signature on arbitrary data
    pub fn verify_signature(&self, data: &[u8], signature: &[u8]) -> Result<()> {
        let sig = Signature::from_bytes(
            signature
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid signature length"))?,
        );

        self.server_verifying_key
            .verify(data, &sig)
            .context("Signature verification failed")?;

        Ok(())
    }

    /// Get server public key
    pub fn server_public_key(&self) -> &VerifyingKey {
        &self.server_verifying_key
    }
}

// Hex encoding/decoding helpers
fn hex_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for byte in data {
        write!(&mut result, "{:02x}", byte).unwrap();
    }
    result
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("Invalid hex string length".to_string());
    }

    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("Invalid hex: {}", e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            jwt_secret: "test-secret-key-for-testing".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_create_and_verify_token() {
        let config = test_config();
        let auth = AuthService::new(&config).unwrap();

        let token = auth
            .create_token(
                "peer123".to_string(),
                "session456".to_string(),
                Role::Editor,
                Duration::from_secs(3600),
            )
            .unwrap();

        let claims = auth.verify_token(&token).unwrap();
        assert_eq!(claims.sub, "peer123");
        assert_eq!(claims.session_id, "session456");
        assert_eq!(claims.role, Role::Editor);
    }

    #[test]
    fn test_role_capabilities() {
        assert!(Role::Host.capabilities().contains(&"kick".to_string()));
        assert!(Role::Editor.capabilities().contains(&"edit".to_string()));
        assert!(Role::Observer.capabilities().contains(&"read".to_string()));
        assert!(!Role::Observer.capabilities().contains(&"edit".to_string()));
    }

    #[test]
    fn test_join_token() {
        let config = test_config();
        let auth = AuthService::new(&config).unwrap();

        let token = auth
            .create_join_token(
                "session789".to_string(),
                Role::Host,
                Duration::from_secs(3600),
            )
            .unwrap();

        let (session_id, role) = auth.verify_join_token(&token).unwrap();
        assert_eq!(session_id, "session789");
        assert_eq!(role, Role::Host);
    }

    #[test]
    fn test_sign_and_verify() {
        let config = test_config();
        let auth = AuthService::new(&config).unwrap();

        let data = b"test data to sign";
        let signature = auth.sign_data(data);

        assert!(auth.verify_signature(data, &signature).is_ok());

        // Tampered data should fail
        let tampered = b"tampered data";
        assert!(auth.verify_signature(tampered, &signature).is_err());
    }
}
