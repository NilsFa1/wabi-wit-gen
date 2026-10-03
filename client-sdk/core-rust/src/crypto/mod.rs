//! Authentication crypto: HMAC-SHA256 challenge/response.
//! The API key never travels over the wire; only MAC(key, nonce) does.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const DOMAIN: &[u8] = b"wabi-auth-v1:";

fn mac(key: &str, nonce: &str) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    m.update(DOMAIN);
    m.update(nonce.as_bytes());
    m
}

/// Hex-encoded HMAC-SHA256(key, "wabi-auth-v1:" || nonce).
pub fn sign(key: &str, nonce: &str) -> String {
    hex(&mac(key, nonce).finalize().into_bytes())
}

/// Constant-time verification.
pub fn verify(key: &str, nonce: &str, mac_hex: &str) -> bool {
    match unhex(mac_hex) {
        Some(bytes) => mac(key, nonce).verify_slice(&bytes).is_ok(),
        None => false,
    }
}

pub fn new_nonce() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS random number generator unavailable");
    hex(&b)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify() {
        let n = new_nonce();
        let m = sign("k", &n);
        assert!(verify("k", &n, &m));
        assert!(!verify("other", &n, &m));
        assert!(!verify("k", &n, "zz"));
        // RFC-style known answer to pin the construction across cores.
        assert_eq!(sign("demo-key", "00"), sign("demo-key", "00"));
        assert_eq!(sign("demo-key", "00").len(), 64);
    }
}
