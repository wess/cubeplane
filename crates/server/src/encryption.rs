//! Protocol encryption for online mode.
//!
//! Implements the vanilla login encryption handshake: a per-server RSA keypair,
//! PKCS#1 v1.5 decryption of the client's shared secret, the Mojang
//! authentication hash, and AES-128-CFB8 streaming.
//!
//! CFB8 is implemented directly (one AES block encryption per byte, feeding the
//! ciphertext back into the IV) so the cipher is *stateful* across the many
//! small reads/writes of the packet stream — the property a socket needs.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};

/// A server's RSA keypair plus its SubjectPublicKeyInfo DER encoding (the form
/// sent in Encryption Request).
pub struct ServerKey {
    private: RsaPrivateKey,
    public_der: Vec<u8>,
}

impl ServerKey {
    /// Generate a fresh 1024-bit keypair (the size vanilla uses).
    pub fn generate() -> anyhow::Result<ServerKey> {
        let mut rng = rand::rngs::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 1024)?;
        let public = RsaPublicKey::from(&private);
        let public_der = public.to_public_key_der()?.as_bytes().to_vec();
        Ok(ServerKey { private, public_der })
    }

    /// The DER-encoded public key for the Encryption Request packet.
    pub fn public_der(&self) -> &[u8] {
        &self.public_der
    }

    /// Decrypt an RSA/PKCS1v15 ciphertext (shared secret or verify token).
    pub fn decrypt(&self, ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        Ok(self.private.decrypt(Pkcs1v15Encrypt, ciphertext)?)
    }
}

/// Compute Mojang's authentication hash for `sessionserver` `hasJoined`.
pub fn auth_hash(server_id: &str, secret: &[u8], public_key_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(secret);
    hasher.update(public_key_der);
    let digest = hasher.finalize();

    // Interpret the 20-byte digest as a signed big-endian integer rendered in
    // hex, with a leading '-' (two's complement) for negative values.
    let negative = digest[0] & 0x80 != 0;
    let mut bytes = digest.to_vec();
    if negative {
        let mut carry = true;
        for b in bytes.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (v, c) = b.overflowing_add(1);
                *b = v;
                carry = c;
            }
        }
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let hex = hex.trim_start_matches('0');
    if negative {
        format!("-{hex}")
    } else {
        hex.to_string()
    }
}

/// A stateful AES-128-CFB8 cipher (separate instances for read and write).
pub struct Cfb8 {
    aes: Aes128,
    iv: [u8; 16],
}

impl Cfb8 {
    /// Create a cipher; the shared secret is used as both key and IV, per spec.
    pub fn new(secret: &[u8]) -> Cfb8 {
        let key = GenericArray::from_slice(secret);
        let mut iv = [0u8; 16];
        iv.copy_from_slice(secret);
        Cfb8 {
            aes: Aes128::new(key),
            iv,
        }
    }

    fn keystream_byte(&self) -> u8 {
        let mut block = GenericArray::clone_from_slice(&self.iv);
        self.aes.encrypt_block(&mut block);
        block[0]
    }

    /// Encrypt a buffer in place, advancing cipher state.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let cipher = *byte ^ self.keystream_byte();
            // Feedback: shift IV left one byte, append the ciphertext byte.
            self.iv.copy_within(1.., 0);
            self.iv[15] = cipher;
            *byte = cipher;
        }
    }

    /// Decrypt a buffer in place, advancing cipher state.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let cipher = *byte;
            let plain = cipher ^ self.keystream_byte();
            self.iv.copy_within(1.., 0);
            self.iv[15] = cipher;
            *byte = plain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mojang_auth_hash_vectors() {
        // Canonical vectors from the protocol documentation.
        assert_eq!(auth_hash("Notch", b"", b""), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(auth_hash("jeb_", b"", b""), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(auth_hash("simon", b"", b""), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    #[test]
    fn cfb8_roundtrips() {
        let secret = [0x42u8; 16];
        let mut enc = Cfb8::new(&secret);
        let mut dec = Cfb8::new(&secret);
        let original = b"cubeplane encrypted packet stream \x00\x01\x02\xff".to_vec();
        let mut buf = original.clone();
        // Encrypt in two chunks to prove state persists across calls.
        let (a, b) = buf.split_at_mut(10);
        enc.encrypt(a);
        enc.encrypt(b);
        assert_ne!(buf, original);
        dec.decrypt(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn rsa_secret_roundtrips() {
        let key = ServerKey::generate().unwrap();
        let public = RsaPublicKey::from(&key.private);
        let secret = [0x11u8; 16];
        let mut rng = rand::rngs::OsRng;
        let ct = public.encrypt(&mut rng, Pkcs1v15Encrypt, &secret).unwrap();
        assert_eq!(key.decrypt(&ct).unwrap(), secret);
        assert!(!key.public_der().is_empty());
    }
}
