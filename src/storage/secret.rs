use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    XChaCha20Poly1305,
};
use serde::{de::DeserializeOwned, Serialize};
use zeroize::Zeroize;

pub struct SecretBox<T> {
    sealed: Vec<u8>,
    nonce: [u8; 24],
    _marker: std::marker::PhantomData<T>,
}

impl<T> SecretBox<T>
where
    T: Serialize + DeserializeOwned,
{
    pub fn seal(value: &T, key: &[u8; 32]) -> Result<Self, SealError> {
        let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| SealError::InvalidKey)?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut plaintext = serde_json::to_vec(value).map_err(|_| SealError::Encode)?;
        let sealed = cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|_| SealError::Encrypt)?;
        plaintext.zeroize();
        Ok(Self {
            sealed,
            nonce: nonce.into(),
            _marker: std::marker::PhantomData,
        })
    }

    pub fn open(&self, key: &[u8; 32]) -> Result<T, SealError> {
        let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| SealError::InvalidKey)?;
        let nonce = chacha20poly1305::XNonce::from(self.nonce);
        let mut plaintext = cipher
            .decrypt(&nonce, self.sealed.as_ref())
            .map_err(|_| SealError::Decrypt)?;
        let out: T = serde_json::from_slice(&plaintext).map_err(|_| SealError::Decode)?;
        plaintext.zeroize();
        Ok(out)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SealError {
    #[error("invalid key length (must be 32 bytes)")]
    InvalidKey,
    #[error("encrypt failed")]
    Encrypt,
    #[error("decrypt failed (auth tag mismatch?)")]
    Decrypt,
    #[error("encode failed")]
    Encode,
    #[error("decode failed")]
    Decode,
}
