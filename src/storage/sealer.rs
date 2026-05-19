//! Symmetric sealer for secrets at rest.
//!
//! Wraps `XChaCha20Poly1305` (24-byte nonce, 32-byte key) and produces a
//! self-contained sealed blob laid out as `[nonce(24) || ciphertext+tag]`.
//! Nonces are generated from the OS RNG on every seal — the 24-byte
//! XChaCha nonce gives us comfortable headroom for random nonces.
//!
//! The key can be sourced from raw bytes, a hex/base64 string, or
//! derived from a passphrase via SHA-256.

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    Key, XChaCha20Poly1305, XNonce,
};
use eyre::{eyre, Report, Result};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// AEAD-backed sealer. The key never leaves this struct.
#[derive(Clone)]
pub struct Sealer {
    key: Key,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealer")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl Sealer {
    /// Construct from a raw 32-byte key.
    pub fn from_key_bytes(bytes: &[u8]) -> Result<Self, Report> {
        if bytes.len() != 32 {
            return Err(eyre!(
                "sealer key must be exactly 32 bytes (got {})",
                bytes.len()
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(Self {
            key: Key::from(arr),
        })
    }

    /// Derive a 32-byte key from an arbitrary passphrase via SHA-256.
    pub fn from_passphrase(passphrase: &str) -> Self {
        let digest = Sha256::digest(passphrase.as_bytes());
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&digest);
        Self {
            key: Key::from(arr),
        }
    }

    /// Load a sealer from the named env var. Accepts:
    ///  - 64 hex chars  -> raw 32 bytes
    ///  - 32-byte base64 (standard or url-safe, with or without padding)
    ///  - anything else -> treated as a passphrase (SHA-256).
    pub fn from_env(var_name: &str) -> Result<Self, Report> {
        let raw = std::env::var(var_name).map_err(|_| eyre!("env var `{}` not set", var_name))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(eyre!("env var `{}` is empty", var_name));
        }
        if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut bytes = [0u8; 32];
            for (i, chunk) in trimmed.as_bytes().chunks_exact(2).enumerate() {
                let hi = hex_nibble(chunk[0])?;
                let lo = hex_nibble(chunk[1])?;
                bytes[i] = (hi << 4) | lo;
            }
            return Self::from_key_bytes(&bytes);
        }
        if let Some(bytes) = try_base64_32(trimmed) {
            return Self::from_key_bytes(&bytes);
        }
        Ok(Self::from_passphrase(trimmed))
    }

    /// Seal a plaintext. Output is `[nonce(24) || ciphertext+tag]`.
    pub fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        let cipher = XChaCha20Poly1305::new(&self.key);
        let mut nonce_bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from(nonce_bytes);
        let ct = cipher
            .encrypt(&nonce, plaintext)
            .expect("XChaCha20Poly1305 encrypt is infallible for valid key");
        let mut out = Vec::with_capacity(24 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        out
    }

    /// Open a sealed blob. Errors on tampering, wrong key, or short input.
    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, Report> {
        if sealed.len() < 24 + 16 {
            return Err(eyre!("sealed blob too short ({} bytes)", sealed.len()));
        }
        let (nonce_bytes, ct) = sealed.split_at(24);
        let mut arr = [0u8; 24];
        arr.copy_from_slice(nonce_bytes);
        let nonce = XNonce::from(arr);
        let cipher = XChaCha20Poly1305::new(&self.key);
        cipher
            .decrypt(&nonce, ct)
            .map_err(|_| eyre!("sealer open failed: auth tag mismatch or wrong key"))
    }
}

fn hex_nibble(b: u8) -> Result<u8, Report> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(eyre!("invalid hex digit")),
    }
}

fn try_base64_32(s: &str) -> Option<[u8; 32]> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    use base64::Engine;
    let attempts: [Result<Vec<u8>, _>; 4] = [
        STANDARD.decode(s),
        STANDARD_NO_PAD.decode(s),
        URL_SAFE.decode(s),
        URL_SAFE_NO_PAD.decode(s),
    ];
    for v in attempts.into_iter().flatten() {
        if v.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&v);
            return Some(arr);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_a() -> Sealer {
        Sealer::from_key_bytes(&[7u8; 32]).unwrap()
    }
    fn key_b() -> Sealer {
        Sealer::from_key_bytes(&[42u8; 32]).unwrap()
    }

    #[test]
    fn roundtrip() {
        let s = key_a();
        let pt = b"hello submux secrets";
        let sealed = s.seal(pt);
        assert_ne!(&sealed[24..], pt, "ciphertext must differ from plaintext");
        let opened = s.open(&sealed).expect("open must succeed");
        assert_eq!(opened, pt);
    }

    #[test]
    fn wrong_key_fails() {
        let sealed = key_a().seal(b"top secret");
        let err = key_b().open(&sealed);
        assert!(err.is_err(), "different key must fail to open");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let s = key_a();
        let mut sealed = s.seal(b"do not flip me");
        let idx = sealed.len() - 1;
        sealed[idx] ^= 0x01;
        let err = s.open(&sealed);
        assert!(err.is_err(), "tampered ciphertext must fail to open");
    }

    #[test]
    fn passphrase_is_deterministic() {
        let a = Sealer::from_passphrase("hunter2");
        let b = Sealer::from_passphrase("hunter2");
        let sealed = a.seal(b"x");
        let opened = b.open(&sealed).expect("same passphrase must roundtrip");
        assert_eq!(opened, b"x");
    }

    #[test]
    fn credentials_roundtrip_via_serde_json() {
        use crate::core::Credentials;
        use chrono::Utc;
        use uuid::Uuid;

        let creds = Credentials::AnthropicOAuth {
            access_token: "at-123".into(),
            refresh_token: "rt-456".into(),
            expires_at: Utc::now(),
            scopes: vec!["user:inference".into()],
            device_id: Uuid::new_v4(),
            account_uuid: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
        };
        let s = key_a();
        let plaintext = serde_json::to_vec(&creds).unwrap();
        let sealed = s.seal(&plaintext);
        let opened = s.open(&sealed).unwrap();
        let back: Credentials = serde_json::from_slice(&opened).unwrap();
        match (&creds, &back) {
            (
                Credentials::AnthropicOAuth {
                    access_token: a, ..
                },
                Credentials::AnthropicOAuth {
                    access_token: b, ..
                },
            ) => assert_eq!(a, b),
            _ => panic!("credential kind changed across roundtrip"),
        }
    }
}
