use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, OsRng},
};
use anyhow::Result;
use rand::RngCore;

#[derive(Clone)]
pub struct Crypto {
    cipher: Aes256Gcm,
}

impl Crypto {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new_from_slice(&key).expect("32-byte key"),
        }
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| anyhow::anyhow!("encrypt secret"))?;
        let mut out = nonce.to_vec();
        out.append(&mut ciphertext);
        Ok(out)
    }

    pub fn decrypt(&self, payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() < 13 {
            anyhow::bail!("encrypted payload is too short");
        }
        let (nonce, ciphertext) = payload.split_at(12);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow::anyhow!("decrypt secret"))
    }
}
