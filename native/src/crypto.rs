use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce, aead::Aead};
use aes_siv::siv::Aes256Siv;
use anyhow::{Context, Result, bail};
use hkdf::Hkdf;
use rand::RngCore;
use scrypt::{Params, scrypt};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroize;

/// The interoperable v3 codec. All key derivation happens on the client.
pub struct VaultCrypto {
    key_hash: String,
    body_key: [u8; 32],
    siv_key: [u8; 64],
}

impl Drop for VaultCrypto {
    fn drop(&mut self) {
        self.body_key.zeroize();
        self.siv_key.zeroize();
    }
}

impl VaultCrypto {
    pub fn derive(password: &str, salt: &str) -> Result<Self> {
        let password: String = password.nfkc().collect();
        let normalized_salt: String = salt.nfkc().collect();
        let mut base = [0u8; 32];
        scrypt(
            password.as_bytes(),
            normalized_salt.as_bytes(),
            &Params::new(15, 8, 1, 32)?,
            &mut base,
        )
        .map_err(|_| anyhow::anyhow!("scrypt failed"))?;
        Self::from_base_key(&base, salt)
    }

    pub fn from_base_key(base: &[u8; 32], salt: &str) -> Result<Self> {
        let key_hash = hex::encode(Self::expand(base, salt.as_bytes(), b"ObsidianKeyHash")?);
        let body_key = Self::expand(base, &[], b"ObsidianAesGcm")?;
        let mac = Self::expand(base, salt.as_bytes(), b"ObsidianAesSivMac")?;
        let ctr = Self::expand(base, salt.as_bytes(), b"ObsidianAesSivEnc")?;
        let mut siv_key = [0u8; 64];
        siv_key[..32].copy_from_slice(&mac);
        siv_key[32..].copy_from_slice(&ctr);
        Ok(Self {
            key_hash,
            body_key,
            siv_key,
        })
    }

    fn expand(base: &[u8; 32], salt: &[u8], info: &[u8]) -> Result<[u8; 32]> {
        let mut out = [0u8; 32];
        Hkdf::<Sha256>::new(Some(salt), base)
            .expand(info, &mut out)
            .map_err(|_| anyhow::anyhow!("HKDF failed"))?;
        Ok(out)
    }

    pub fn key_hash(&self) -> &str {
        &self.key_hash
    }

    pub fn encode_path(&self, path: &str) -> Result<String> {
        let mut cipher = Aes256Siv::new_from_slice(&self.siv_key)
            .map_err(|_| anyhow::anyhow!("invalid SIV key"))?;
        Ok(hex::encode(
            cipher
                .encrypt(std::iter::empty::<&[u8]>(), path.as_bytes())
                .map_err(|_| anyhow::anyhow!("path encryption failed"))?,
        ))
    }

    pub fn decode_path(&self, encoded: &str) -> Result<String> {
        let bytes = hex::decode(encoded).context("invalid encrypted path hex")?;
        let mut cipher = Aes256Siv::new_from_slice(&self.siv_key)
            .map_err(|_| anyhow::anyhow!("invalid SIV key"))?;
        let plain = cipher
            .decrypt(std::iter::empty::<&[u8]>(), &bytes)
            .map_err(|_| anyhow::anyhow!("path authentication failed"))?;
        String::from_utf8(plain).context("path is not UTF-8")
    }

    pub fn content_hash(&self, content: &[u8]) -> Result<String> {
        self.encode_path(&hex::encode(Sha256::digest(content)))
    }

    pub fn encrypt_body(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        self.encrypt_body_with_nonce(plain, nonce)
    }

    fn encrypt_body_with_nonce(&self, plain: &[u8], nonce: [u8; 12]) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(&self.body_key)
            .map_err(|_| anyhow::anyhow!("invalid GCM key"))?;
        let mut out = nonce.to_vec();
        out.extend(
            cipher
                .encrypt(Nonce::from_slice(&nonce), plain)
                .map_err(|_| anyhow::anyhow!("body encryption failed"))?,
        );
        Ok(out)
    }

    pub fn decrypt_body(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < 28 {
            bail!("truncated encrypted body");
        }
        let cipher = Aes256Gcm::new_from_slice(&self.body_key)
            .map_err(|_| anyhow::anyhow!("invalid GCM key"))?;
        cipher
            .decrypt(Nonce::from_slice(&ciphertext[..12]), &ciphertext[12..])
            .map_err(|_| anyhow::anyhow!("body authentication failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_tamper_rejection() {
        let key = VaultCrypto::derive("passphrase", "synthetic-salt").unwrap();
        let path = "Folder/é.md";
        let encoded = key.encode_path(path).unwrap();
        assert_eq!(key.decode_path(&encoded).unwrap(), path);
        assert_eq!(key.encode_path(path).unwrap(), encoded);
        let mut body = key.encrypt_body(b"hello").unwrap();
        assert_eq!(key.decrypt_body(&body).unwrap(), b"hello");
        body[20] ^= 1;
        assert!(key.decrypt_body(&body).is_err());
    }
}
