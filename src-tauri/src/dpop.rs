//! `DPoP` — proof-of-possession for AT Protocol OAuth.
//!
//! Every request in that flow carries a short-lived `JWT`, signed by a key this
//! app holds, that names the exact method and URL being called. The access token
//! is *bound* to that key: a stolen token is useless without it.
//!
//! Hand-rolled on `p256` rather than through a `JWT` library because the proof is
//! an unusual shape — the public key travels in the header as a `jwk`, and the
//! claims are protocol-specific. Wrapping a library's header model costs more
//! than the sixty lines below.
//!
//! Four details, each of which fails in a way that looks like something else:
//!
//! * **The signature is raw `r‖s`**, 64 bytes — not the DER encoding
//!   `Signature::to_der()` would give. A DER signature is rejected as malformed
//!   with no hint that the encoding is the problem.
//! * **`htu` is the URL with query and fragment stripped**, and `htm` is the
//!   uppercase method. A proof whose `htu` still carries `?foo=bar` does not
//!   match the request the server sees.
//! * **`ath` is required on resource requests** (a hash of the access token) and
//!   must be absent at the token endpoint.
//! * **Nonces are mandatory and server-driven.** The first request to a server
//!   is rejected with `use_dpop_nonce` and a `DPoP-Nonce` header; the same
//!   request must be retried once carrying it. [`send`] does that, so no call
//!   site has to.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};
use crate::http;

/// The key an account's tokens are bound to.
///
/// It MUST outlive the connect flow: a refresh signed by a different key is
/// refused, so the key is serialized into the account's stored secret next to
/// the refresh token. Losing it means the account has to be reconnected.
pub struct Key(SigningKey);

impl Key {
    pub fn generate() -> Self {
        // 32 random bytes rejected until they are a valid scalar. Every value
        // below the curve order works, which is all but ~2^-128 of them, so this
        // effectively never loops — and it keeps the key generation independent
        // of whichever `rand_core` version the curve crates happen to be on.
        loop {
            let bytes = rand::random::<[u8; 32]>();
            if let Ok(key) = SigningKey::from_slice(&bytes) {
                return Self(key);
            }
        }
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0.to_bytes())
    }

    pub fn from_base64(encoded: &str) -> Result<Self> {
        let bytes = B64.decode(encoded).map_err(|e| {
            AppError::Unauthorized(format!("The stored DPoP key is unreadable: {e}"))
        })?;
        SigningKey::from_slice(&bytes)
            .map(Self)
            .map_err(|e| AppError::Unauthorized(format!("The stored DPoP key is invalid: {e}")))
    }

    /// The public half as a `JWK`, which travels in every proof's header.
    pub fn public_jwk(&self) -> serde_json::Value {
        let point = self.0.verifying_key().to_sec1_point(false);
        // Uncompressed SEC1 is `0x04 || x(32) || y(32)`; the `JWK` wants the two
        // coordinates separately.
        let x = point.x().map(|x| B64.encode(x)).unwrap_or_default();
        let y = point.y().map(|y| B64.encode(y)).unwrap_or_default();
        serde_json::json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })
    }

    /// One proof. `access_token` is `Some` for a resource request and `None` at
    /// the token endpoint, where an `ath` claim is not allowed.
    pub fn proof(
        &self,
        method: &str,
        url: &str,
        nonce: Option<&str>,
        access_token: Option<&str>,
    ) -> Result<String> {
        let header = serde_json::json!({
            "typ": "dpop+jwt",
            "alg": "ES256",
            "jwk": self.public_jwk(),
        });

        let mut claims = serde_json::json!({
            // Unique per proof — a replayed `jti` is exactly what this defends
            // against, so it is never derived from anything.
            "jti": B64.encode(rand::random::<[u8; 16]>()),
            "htm": method.to_ascii_uppercase(),
            "htu": htu(url),
            "iat": chrono::Utc::now().timestamp(),
        });
        if let Some(nonce) = nonce {
            claims["nonce"] = serde_json::json!(nonce);
        }
        if let Some(token) = access_token {
            claims["ath"] = serde_json::json!(B64.encode(Sha256::digest(token.as_bytes())));
        }

        let signing_input = format!(
            "{}.{}",
            B64.encode(serde_json::to_vec(&header)?),
            B64.encode(serde_json::to_vec(&claims)?)
        );
        let signature: Signature = self.0.sign(signing_input.as_bytes());
        // Raw r‖s. `to_der()` here is the single most common way to get a
        // signature the server silently refuses.
        Ok(format!(
            "{signing_input}.{}",
            B64.encode(signature.to_bytes())
        ))
    }
}

/// The `htu` claim: the request URL without query or fragment.
fn htu(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .to_string()
}

/// Last nonce seen per server origin. Servers rotate them every few minutes and
/// hand the next one back on every response, so this is a cache rather than a
/// session: a stale entry costs one extra round trip, never a failure.
fn nonce_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn origin_of(url: &str) -> String {
    url::Url::parse(url).map_or_else(
        |_| url.to_string(),
        |parsed| parsed.origin().ascii_serialization(),
    )
}

fn remembered_nonce(url: &str) -> Option<String> {
    nonce_cache().lock().ok()?.get(&origin_of(url)).cloned()
}

fn remember_nonce(url: &str, nonce: &str) {
    if let Ok(mut cache) = nonce_cache().lock() {
        cache.insert(origin_of(url), nonce.to_string());
    }
}

/// Sends a `DPoP`-signed request, retrying once when the server asks for a nonce.
///
/// `build` produces a fresh `RequestBuilder` each time rather than the request
/// being cloned: a retry needs a different proof in its header, and rebuilding
/// is both simpler and correct for bodies that cannot be cloned.
///
/// Returns the status and body, as everywhere else in this app.
pub fn send(
    key: &Key,
    method: &str,
    url: &str,
    access_token: Option<&str>,
    build: impl Fn() -> reqwest::blocking::RequestBuilder,
) -> Result<(u16, String)> {
    let mut nonce = remembered_nonce(url);

    for attempt in 0..2 {
        let proof = key.proof(method, url, nonce.as_deref(), access_token)?;
        let mut request = build().header("DPoP", proof);
        if let Some(token) = access_token {
            // `DPoP`, not `Bearer`: the scheme is what tells the server the token
            // is bound to a key it should check.
            request = request.header(reqwest::header::AUTHORIZATION, format!("DPoP {token}"));
        }

        let response = request.send()?;
        let served_nonce = response
            .headers()
            .get("DPoP-Nonce")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = http::read_body(response);

        if let Some(fresh) = &served_nonce {
            remember_nonce(url, fresh);
        }

        // The one retryable case: the server issued a nonce and wants the
        // request signed again with it. Anything else is the caller's to read.
        let wants_nonce = matches!(status, 400 | 401) && body.contains("use_dpop_nonce");
        if attempt == 0 && wants_nonce && served_nonce.is_some() {
            nonce = served_nonce;
            continue;
        }
        return Ok((status, body));
    }
    unreachable!("the loop returns on its second pass")
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;

    fn decode_part(part: &str) -> serde_json::Value {
        serde_json::from_slice(&B64.decode(part).expect("base64url")).expect("json")
    }

    fn parts(proof: &str) -> (serde_json::Value, serde_json::Value, Vec<u8>) {
        let segments: Vec<&str> = proof.split('.').collect();
        assert_eq!(segments.len(), 3, "a JWT has three segments");
        (
            decode_part(segments[0]),
            decode_part(segments[1]),
            B64.decode(segments[2]).expect("signature"),
        )
    }

    #[test]
    fn a_proof_verifies_against_the_key_that_signed_it() {
        let key = Key::generate();
        let proof = key
            .proof("POST", "https://bsky.social/oauth/token", None, None)
            .expect("proof");
        let (_, _, signature) = parts(&proof);

        assert_eq!(signature.len(), 64, "ES256 is raw r‖s, not DER");
        let signing_input = proof.rsplit_once('.').expect("segments").0;
        let signature = Signature::from_slice(&signature).expect("signature");
        key.0
            .verifying_key()
            .verify(signing_input.as_bytes(), &signature)
            .expect("the proof must verify against its own key");
    }

    #[test]
    fn the_header_carries_the_public_key_and_the_right_type() {
        let key = Key::generate();
        let (header, _, _) = parts(
            &key.proof("GET", "https://example.test/", None, None)
                .expect("proof"),
        );
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["jwk"]["kty"], "EC");
        assert_eq!(header["jwk"]["crv"], "P-256");
        // Both coordinates present, 32 bytes each — a `JWK` missing `y` is
        // accepted by serde and rejected by every server.
        for coordinate in ["x", "y"] {
            let raw = header["jwk"][coordinate].as_str().expect(coordinate);
            assert_eq!(B64.decode(raw).expect("base64url").len(), 32);
        }
        // A private key must never appear in a proof.
        assert!(header["jwk"].get("d").is_none());
    }

    #[test]
    fn htu_drops_the_query_and_fragment() {
        assert_eq!(htu("https://a.test/x?y=1#z"), "https://a.test/x");
        assert_eq!(htu("https://a.test/x"), "https://a.test/x");
    }

    #[test]
    fn the_method_is_upper_cased_and_the_url_normalised_in_the_claims() {
        let key = Key::generate();
        let (_, claims, _) = parts(
            &key.proof("post", "https://a.test/token?x=1", None, None)
                .expect("proof"),
        );
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://a.test/token");
    }

    #[test]
    fn a_token_endpoint_proof_has_no_ath_and_a_resource_proof_does() {
        let key = Key::generate();
        let (_, at_token, _) = parts(
            &key.proof("POST", "https://a.test/t", None, None)
                .expect("proof"),
        );
        assert!(
            at_token.get("ath").is_none(),
            "an `ath` at the token endpoint is rejected"
        );

        let (_, at_resource, _) = parts(
            &key.proof("POST", "https://a.test/xrpc/x", None, Some("the-token"))
                .expect("proof"),
        );
        let expected = B64.encode(Sha256::digest(b"the-token"));
        assert_eq!(at_resource["ath"], expected);
    }

    #[test]
    fn a_nonce_is_included_only_when_the_server_gave_one() {
        let key = Key::generate();
        let (_, without, _) = parts(
            &key.proof("GET", "https://a.test/", None, None)
                .expect("proof"),
        );
        assert!(without.get("nonce").is_none());

        let (_, with, _) = parts(
            &key.proof("GET", "https://a.test/", Some("abc123"), None)
                .expect("proof"),
        );
        assert_eq!(with["nonce"], "abc123");
    }

    #[test]
    fn every_proof_gets_its_own_jti() {
        let key = Key::generate();
        let first = parts(
            &key.proof("GET", "https://a.test/", None, None)
                .expect("proof"),
        )
        .1;
        let second = parts(
            &key.proof("GET", "https://a.test/", None, None)
                .expect("proof"),
        )
        .1;
        assert_ne!(
            first["jti"], second["jti"],
            "a reused jti is exactly the replay DPoP exists to stop"
        );
    }

    #[test]
    fn a_key_survives_the_round_trip_through_storage() {
        // The account's tokens are bound to this key, so a refresh a day later
        // has to be signed by the very same one.
        let key = Key::generate();
        let restored = Key::from_base64(&key.to_base64()).expect("restore");
        assert_eq!(key.public_jwk(), restored.public_jwk());
    }

    #[test]
    fn a_corrupt_stored_key_is_an_auth_error_not_a_panic() {
        assert!(Key::from_base64("not base64!!").is_err());
        assert!(Key::from_base64(&B64.encode([0u8; 8])).is_err());
    }

    #[test]
    fn nonces_are_remembered_per_origin() {
        remember_nonce("https://one.test/a?x=1", "nonce-one");
        remember_nonce("https://two.test/b", "nonce-two");
        assert_eq!(
            remembered_nonce("https://one.test/other").as_deref(),
            Some("nonce-one")
        );
        assert_eq!(
            remembered_nonce("https://two.test/b").as_deref(),
            Some("nonce-two")
        );
    }
}
