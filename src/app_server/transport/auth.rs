//! Instance authentication at WebSocket admission, never execution authorization.
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io;

pub const MAX_CAPABILITY_BYTES: usize = 4096;
pub const MAX_JWT_BYTES: usize = 16 * 1024;

/// Loaded once by the process owner. Deliberately neither Debug nor Serialize.
#[derive(Clone)]
pub struct Credential(Mode);

#[derive(Clone)]
enum Mode {
    #[cfg(test)]
    Clock(
        Box<Credential>,
        std::sync::Arc<std::sync::atomic::AtomicI64>,
    ),
    Capability([u8; 32]),
    Signed {
        key: DecodingKey,
        issuer: Option<String>,
        audience: Option<String>,
        skew: i64,
    },
}

#[derive(Clone, Deserialize)]
struct Claims {
    exp: i64,
    nbf: Option<i64>,
    iss: Option<String>,
    aud: Option<Audience>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl Credential {
    #[cfg(test)]
    pub(crate) fn with_clock(self, clock: std::sync::Arc<std::sync::atomic::AtomicI64>) -> Self {
        Self(Mode::Clock(Box::new(self), clock))
    }

    /// Load an opaque capability; the policy retains only its digest.
    /// # Errors
    /// Rejects empty, oversized or non-header-safe tokens.
    pub fn new(value: String) -> io::Result<Self> {
        if !valid_capability(&value) {
            return Err(invalid(
                "capability token must contain 1..4096 bearer characters",
            ));
        }
        let digest = Sha256::digest(value.as_bytes()).into();
        drop(value); // The policy retains no plaintext capability.
        Ok(Self(Mode::Capability(digest)))
    }

    /// Load an alternative SHA-256 capability source.
    /// # Errors
    /// Rejects anything other than a 64-character hexadecimal digest.
    pub fn from_sha256(value: &str) -> io::Result<Self> {
        if value.len() != 64 {
            return Err(invalid(
                "token SHA-256 must contain 64 hexadecimal characters",
            ));
        }
        let mut digest = [0; 32];
        for (byte, pair) in digest.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
            let nibble = |b: u8| {
                char::from(b)
                    .to_digit(16)
                    .and_then(|n| u8::try_from(n).ok())
            };
            *byte = (nibble(pair[0]).ok_or_else(|| invalid("invalid token SHA-256"))? << 4)
                | nibble(pair[1]).ok_or_else(|| invalid("invalid token SHA-256"))?;
        }
        Ok(Self(Mode::Capability(digest)))
    }

    /// Load the trusted host's HS256 verification policy.
    /// # Errors
    /// Rejects short/oversized secrets, empty configured claims and excessive skew.
    pub fn signed(
        secret: &[u8],
        issuer: Option<String>,
        audience: Option<String>,
        skew: u64,
    ) -> io::Result<Self> {
        if !(32..=MAX_CAPABILITY_BYTES).contains(&secret.len())
            || issuer
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 4096)
            || audience
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 4096)
        {
            return Err(invalid("invalid signed bearer configuration"));
        }
        Ok(Self(Mode::Signed {
            key: DecodingKey::from_secret(secret),
            issuer,
            audience,
            skew: i64::try_from(skew).map_err(|_| invalid("clock skew exceeds i64"))?,
        }))
    }

    pub(crate) fn validate(&self, token: &str) -> bool {
        let Ok(duration) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        else {
            return false;
        };
        let Ok(now) = i64::try_from(duration.as_secs()) else {
            return false;
        };
        self.validate_at(token, now)
    }

    fn validate_at(&self, token: &str, now: i64) -> bool {
        match &self.0 {
            #[cfg(test)]
            Mode::Clock(policy, clock) => {
                policy.validate_at(token, clock.load(std::sync::atomic::Ordering::Acquire))
            }
            Mode::Capability(expected) => {
                valid_capability(token)
                    && constant_time_eq::constant_time_eq_32(
                        expected,
                        &Sha256::digest(token.as_bytes()).into(),
                    )
            }
            Mode::Signed {
                key,
                issuer,
                audience,
                skew,
            } => {
                if token.is_empty() || token.len() > MAX_JWT_BYTES {
                    return false;
                }
                // Signature and HS256 are library-owned. Claims use an explicit
                // clock and the pinned Codex inclusive skew boundaries.
                let mut validation = Validation::new(Algorithm::HS256);
                validation.required_spec_claims.clear();
                validation.validate_exp = false;
                validation.validate_nbf = false;
                validation.validate_aud = false;
                let Ok(data) = decode::<Claims>(token, key, &validation) else {
                    return false;
                };
                let claims = data.claims;
                now <= claims.exp.saturating_add(*skew)
                    && claims
                        .nbf
                        .is_none_or(|nbf| now >= nbf.saturating_sub(*skew))
                    && issuer
                        .as_ref()
                        .is_none_or(|expected| claims.iss.as_ref() == Some(expected))
                    && audience.as_ref().is_none_or(|expected| match &claims.aud {
                        Some(Audience::One(actual)) => actual == expected,
                        Some(Audience::Many(actual)) => actual.contains(expected),
                        None => false,
                    })
            }
        }
    }

    pub(crate) fn offered(&self, offered: &[&str], prefix: &str) -> bool {
        let mut values = offered
            .iter()
            .filter_map(|value| value.strip_prefix(prefix));
        let Some(value) = values.next() else {
            return false;
        };
        values.next().is_none() && self.validate(value)
    }

    pub(crate) fn same_secret(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Mode::Capability(a), Mode::Capability(b)) => {
                constant_time_eq::constant_time_eq_32(a, b)
            }
            _ => false,
        }
    }
}

fn valid_capability(token: &str) -> bool {
    (1..=MAX_CAPABILITY_BYTES).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
}

/// Reject ambiguous carriers before validating either. No fallback on failure.
pub(super) fn authorize(
    credential: &Credential,
    headers: &http::HeaderMap,
    offered: &[&str],
) -> bool {
    if offered
        .iter()
        .any(|value| value.starts_with("rustx-") && !value.starts_with("rustx-token."))
    {
        return false;
    }
    let mut authorization = headers.get_all(http::header::AUTHORIZATION).iter();
    let header = authorization.next();
    if authorization.next().is_some() {
        return false;
    }
    let mut protocols = offered
        .iter()
        .filter_map(|value| value.strip_prefix("rustx-token."));
    let protocol = protocols.next();
    if protocols.next().is_some() || (header.is_some() && protocol.is_some()) {
        return false;
    }
    match (header, protocol) {
        (Some(header), None) => header
            .to_str()
            .ok()
            .and_then(|value| {
                let (scheme, token) = value.split_once(' ')?;
                (scheme.eq_ignore_ascii_case("Bearer")
                    && !token.is_empty()
                    && !token.bytes().any(|b| b.is_ascii_whitespace()))
                .then_some(token)
            })
            .is_some_and(|token| credential.validate(token)),
        (None, Some(encoded)) => {
            if encoded.len() > MAX_JWT_BYTES.div_ceil(3) * 4 {
                return false;
            }
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .is_some_and(|token| credential.validate(&token))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    const SECRET: &[u8] = b"test-host-secret-with-at-least-32-bytes";
    fn jwt(claims: &serde_json::Value, algorithm: Algorithm) -> String {
        encode(
            &Header::new(algorithm),
            &claims,
            &EncodingKey::from_secret(SECRET),
        )
        .unwrap()
    }

    #[test]
    fn capability_sources_and_carriers_fail_closed() {
        let token = "opaque/host+capability==";
        let browser = format!(
            "rustx-token.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
        );
        let policy = Credential::new(token.into()).unwrap();
        let digest = format!("{:x}", Sha256::digest(token));
        let hashed = Credential::from_sha256(&digest).unwrap();
        assert!(policy.same_secret(&hashed));
        assert!(hashed.validate_at(token, 0));
        for value in [
            "",
            "wrong",
            "has spaces",
            &"x".repeat(MAX_CAPABILITY_BYTES + 1),
        ] {
            assert!(!hashed.validate_at(value, 0));
        }
        assert!(Credential::from_sha256(&"g".repeat(64)).is_err());
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert!(authorize(&policy, &headers, &[]));
        assert!(!authorize(&policy, &headers, &["rustx-token.wrong"]));
        headers.append(
            http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert!(!authorize(&policy, &headers, &[]));
        assert!(!authorize(
            &policy,
            &http::HeaderMap::new(),
            &[&format!("rustx-token.{token}")]
        ));
        assert!(!authorize(
            &policy,
            &http::HeaderMap::new(),
            &[&format!("{browser}=")]
        ));
        assert!(authorize(&policy, &http::HeaderMap::new(), &[&browser]));
        assert!(!authorize(
            &policy,
            &http::HeaderMap::new(),
            &[&browser, &browser]
        ));
    }

    #[test]
    fn hs256_signature_claims_and_exact_skew_boundaries() {
        let policy =
            Credential::signed(SECRET, Some("host".into()), Some("rustx".into()), 30).unwrap();
        let valid = json!({"exp":100,"nbf":90,"iss":"host","aud":["other","rustx"],"role":"ignored","scope":"ignored"});
        let token = jwt(&valid, Algorithm::HS256);
        for now in [60, 90, 100, 130] {
            assert!(policy.validate_at(&token, now));
        }
        for now in [59, 131] {
            assert!(!policy.validate_at(&token, now));
        }
        for claims in [
            json!({}),
            json!({"exp":"100"}),
            json!({"exp":100,"iss":"wrong","aud":"rustx"}),
            json!({"exp":100,"iss":"host","aud":"wrong"}),
            json!({"exp":100,"iss":"host"}),
        ] {
            assert!(!policy.validate_at(&jwt(&claims, Algorithm::HS256), 100));
        }
        assert!(!policy.validate_at(&jwt(&valid, Algorithm::HS384), 100));
        let mut tampered = token.into_bytes();
        tampered[10] ^= 1;
        assert!(!policy.validate_at(std::str::from_utf8(&tampered).unwrap(), 100));
        assert!(!policy.validate_at(&"x".repeat(MAX_JWT_BYTES + 1), 100));
        assert!(Credential::signed(b"short", None, None, 30).is_err());
    }
}
