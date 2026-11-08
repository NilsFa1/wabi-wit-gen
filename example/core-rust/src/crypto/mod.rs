//! Body encryption and request digests.
//!
//! Every request and response body is `nonce (12 bytes) || AES-256-GCM
//! ciphertext + tag`. The key is derived from the shared secret (`api-key`),
//! which itself never travels over the wire. The additional data binds a body
//! to its role, so a response cannot be replayed as a request or an event.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const KEY_LABEL: &[u8] = b"demo-client/v1 body key";
const NONCE_LEN: usize = 12;

/// Additional authenticated data of the three kinds of bodies.
pub const AAD_REQUEST: &[u8] = b"req";
pub const AAD_RESPONSE: &[u8] = b"res";
pub const AAD_EVENT: &[u8] = b"evt";

#[derive(Clone)]
pub struct BodyKey(Aes256Gcm);

impl BodyKey {
    /// HMAC-SHA256(api-key, "demo-client/v1 body key").
    pub fn derive(api_key: &str) -> BodyKey {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(api_key.as_bytes()).expect("HMAC accepts any key length");
        mac.update(KEY_LABEL);
        BodyKey(Aes256Gcm::new_from_slice(&mac.finalize().into_bytes()).expect("32-byte key"))
    }

    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("OS random number generator unavailable");
        self.seal_with(nonce, aad, plaintext)
    }

    fn seal_with(&self, nonce: [u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let ciphertext = self.0.encrypt(&Nonce::from(nonce), Payload { msg: plaintext, aad }).expect("AES-GCM encryption");
        [&nonce[..], &ciphertext].concat()
    }

    /// `None` if the body was sealed with another key, for another role, or modified.
    pub fn open(&self, aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let (nonce, ciphertext) = sealed.split_at_checked(NONCE_LEN)?;
        let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
        self.0.decrypt(&Nonce::from(nonce), Payload { msg: ciphertext, aad }).ok()
    }
}

/// What the app-held key signs: SHA-256 over method, path, session and the
/// (already encrypted) body. The session id is empty while the session is opened.
pub fn request_digest(method: &str, path: &str, session: &str, body: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    for part in [method, path, session] {
        h.update(part.as_bytes());
        h.update(b"\n");
    }
    h.update(body);
    h.finalize().to_vec()
}

/// Hex SHA-256 of a SubjectPublicKeyInfo.
pub fn fingerprint(spki: &[u8]) -> String {
    hex(&Sha256::digest(spki))
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("OS random number generator unavailable");
    hex(&b)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open() {
        let key = BodyKey::derive("demo-key");
        let sealed = key.seal(AAD_REQUEST, b"hello");
        assert_eq!(key.open(AAD_REQUEST, &sealed).as_deref(), Some(&b"hello"[..]));
        assert_eq!(key.open(AAD_RESPONSE, &sealed), None, "bound to its role");
        assert_eq!(BodyKey::derive("other").open(AAD_REQUEST, &sealed), None);
        assert_eq!(key.open(AAD_REQUEST, &sealed[..5]), None);
        assert_ne!(key.seal(AAD_REQUEST, b"hello"), sealed, "fresh nonce per body");
    }

    /// Pins the construction, so that other cores (core-go) stay interoperable.
    #[test]
    fn known_answers() {
        let sealed = BodyKey::derive("demo-key").seal_with([0; NONCE_LEN], AAD_REQUEST, b"hello");
        assert_eq!(hex(&sealed), "000000000000000000000000e01f22429807d0ba0d6b42659676e605a1545ae5f4");
        assert_eq!(hex(&request_digest("POST", "/v1/customers", "abc", b"body")), "4807155a56ea86d92eae1a05098bd891b459ea1d82e4c0c9c915b7d3b47237d7");
    }
}
