//! gematik:zero PoC core in Rust — a drop-in replacement for the Go core
//! (packages/zero/core-go, which uses gematik libzero): same WIT package, same
//! exported symbols (`gematik_zero_*`), same observable behaviour.
//!
//! Builds RFC 9449 DPoP proofs; the private key stays in the APP. Every
//! signature is a host call to the imported WIT interface `host-keys`
//! (ABI 1.1), e.g. into the Secure Enclave or the Android Keystore.
//!
//!   generated.rs  from packages/zero/wit (wabi-wit-gen `rust` target)
//!   lib.rs        this file: business logic only

mod generated;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use generated::{host_keys, DpopSignerResource, ZeroError};
use sha2::{Digest, Sha256};
use std::future::Future;

/// Binds the generated contract to this core's types (`crate::Core` is what
/// the generated exports reference).
pub struct Core;

impl generated::Implementation for Core {
    const CORE_INFO: &'static std::ffi::CStr = c"gematik-zero-rust-core/0.1.0 (RFC 9449 in Rust, tokio)";
    type DpopSigner = Signer;
}

pub struct Signer {
    key_id: String,
}

fn crypto(m: impl Into<String>) -> ZeroError {
    ZeroError::Crypto(m.into())
}

/// SubjectPublicKeyInfo header for id-ecPublicKey / prime256v1, followed by
/// the 65-byte uncompressed point 0x04 || X || Y.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
    0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// Public EC P-256 key of the app, as JWK coordinates (base64url).
struct Jwk {
    x: String,
    y: String,
}

impl Jwk {
    fn from_spki(der: &[u8]) -> Result<Jwk, ZeroError> {
        if der.len() != 91 || der[..26] != P256_SPKI_PREFIX || der[26] != 0x04 {
            return Err(crypto("public key: expected EC P-256 SubjectPublicKeyInfo"));
        }
        Ok(Jwk { x: B64.encode(&der[27..59]), y: B64.encode(&der[59..91]) })
    }

    /// RFC 7638: members in lexicographic order, no whitespace.
    fn thumbprint(&self) -> String {
        let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#, self.x, self.y);
        B64.encode(Sha256::digest(canonical.as_bytes()))
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "kty": "EC", "crv": "P-256", "x": self.x, "y": self.y })
    }
}

/// A failed host call becomes a domain error for the app.
fn host_error(e: host_keys::HostError) -> ZeroError {
    crypto(format!("host-keys: {e}"))
}

async fn public_jwk(key_id: String) -> Result<Jwk, ZeroError> {
    let der = host_keys::public_key(key_id).await.map_err(host_error)??;
    Jwk::from_spki(&der)
}

/// ASN.1 DER `SEQUENCE { INTEGER r, INTEGER s }` -> JOSE `r || s` (32 + 32 bytes).
fn der_to_jose(der: &[u8]) -> Result<[u8; 64], ZeroError> {
    let bad = || crypto("signature: expected an ASN.1 DER ECDSA signature");
    let mut out = [0u8; 64];
    let body = match der {
        [0x30, len, rest @ ..] if *len as usize == rest.len() => rest,
        _ => return Err(bad()),
    };
    let mut pos = 0;
    for half in 0..2 {
        if body.get(pos) != Some(&0x02) {
            return Err(bad());
        }
        let n = *body.get(pos + 1).ok_or_else(bad)? as usize;
        let int = body.get(pos + 2..pos + 2 + n).ok_or_else(bad)?;
        let int = &int[int.iter().take_while(|b| **b == 0).count()..]; // strip sign padding
        if int.len() > 32 {
            return Err(bad());
        }
        out[half * 32 + 32 - int.len()..half * 32 + 32].copy_from_slice(int);
        pos += 2 + n;
    }
    if pos != body.len() {
        return Err(bad());
    }
    Ok(out)
}

fn random_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS random source");
    B64.encode(b)
}

impl DpopSignerResource for Signer {
    fn new(key_id: String) -> Self {
        Signer { key_id }
    }

    fn thumbprint(&self) -> impl Future<Output = Result<String, ZeroError>> + Send {
        let key_id = self.key_id.clone();
        async move { Ok(public_jwk(key_id).await?.thumbprint()) }
    }

    fn create_proof(
        &self,
        http_method: String,
        http_url: String,
        access_token: Option<String>,
        nonce: Option<String>,
    ) -> impl Future<Output = Result<String, ZeroError>> + Send {
        let key_id = self.key_id.clone();
        async move {
            if http_method.is_empty() || !http_url.starts_with("http") {
                return Err(ZeroError::InvalidInput("http-method and an absolute http(s) http-url are required".into()));
            }
            let jwk = public_jwk(key_id.clone()).await?;
            let header = serde_json::json!({ "typ": "dpop+jwt", "alg": "ES256", "jwk": jwk.json() });
            let mut claims = serde_json::json!({
                "jti": random_id(),
                "htm": http_method,
                "htu": http_url,
                "iat": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            });
            if let Some(token) = access_token {
                claims["ath"] = B64.encode(Sha256::digest(token.as_bytes())).into();
            }
            if let Some(n) = nonce {
                claims["nonce"] = n.into();
            }
            let input = format!("{}.{}", B64.encode(header.to_string()), B64.encode(claims.to_string()));
            let digest = Sha256::digest(input.as_bytes()).to_vec();
            let der = host_keys::sign(key_id, digest).await.map_err(host_error)??; // the app signs
            Ok(format!("{input}.{}", B64.encode(der_to_jose(&der)?)))
        }
    }
}
