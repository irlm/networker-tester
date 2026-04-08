use aes_gcm::{aead::Aead, Aes256Gcm, Key, KeyInit, Nonce};
use rand::RngExt;
/// Encrypt plaintext with AES-256-GCM. Returns (ciphertext, 12-byte nonce).
pub fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> anyhow::Result<(Vec<u8>, [u8; 12])> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("Encryption failed: {e}"))?;
    Ok((ciphertext, nonce_bytes))
}

/// Decrypt ciphertext with AES-256-GCM.
pub fn decrypt(ciphertext: &[u8], nonce: &[u8; 12], key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {e}"))
}

/// Try to decrypt with primary key, fall back to old key (for rotation).
pub fn decrypt_with_fallback(
    ciphertext: &[u8],
    nonce: &[u8; 12],
    key: &[u8; 32],
    old_key: Option<&[u8; 32]>,
) -> anyhow::Result<Vec<u8>> {
    match decrypt(ciphertext, nonce, key) {
        Ok(v) => Ok(v),
        Err(_) if old_key.is_some() => decrypt(ciphertext, nonce, old_key.unwrap()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = [0x42u8; 32];
        let plaintext = b"hello world";
        let (ct, nonce) = encrypt(plaintext, &key).unwrap();
        let pt = decrypt(&ct, &nonce, &key).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = [0x42u8; 32];
        let key2 = [0x43u8; 32];
        let (ct, nonce) = encrypt(b"secret", &key1).unwrap();
        assert!(decrypt(&ct, &nonce, &key2).is_err());
    }

    #[test]
    fn fallback_key_works() {
        let old_key = [0x42u8; 32];
        let new_key = [0x43u8; 32];
        let (ct, nonce) = encrypt(b"data", &old_key).unwrap();
        // Primary key fails, fallback succeeds
        let pt = decrypt_with_fallback(&ct, &nonce, &new_key, Some(&old_key)).unwrap();
        assert_eq!(pt, b"data");
    }
}
