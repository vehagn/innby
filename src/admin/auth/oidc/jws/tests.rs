use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RsaKeyPair},
};
use serde_json::json;

use super::*;

/// Signs tokens for the tests, as a provider would.
pub struct Signer {
    alg: &'static str,
    kid: &'static str,
    key: SigningKey,
}

enum SigningKey {
    Ec(EcdsaKeyPair),
    Rsa(RsaKeyPair),
}

/// A 2048-bit RSA key (PKCS #1 DER, base64) made with `openssl genpkey`, for the tests
/// only.
const RSA_KEY: &str = include_str!("test_rsa_key.b64");

impl Signer {
    /// A new P-256 key, for ES256.
    pub fn es256(kid: &'static str) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        Signer {
            alg: "ES256",
            kid,
            key: SigningKey::Ec(key),
        }
    }

    /// The tests' RSA key, for RS256.
    pub fn rs256(kid: &'static str) -> Self {
        let der = base64::engine::general_purpose::STANDARD
            .decode(RSA_KEY.split_whitespace().collect::<String>())
            .unwrap();
        Signer {
            alg: "RS256",
            kid,
            key: SigningKey::Rsa(RsaKeyPair::from_der(&der).unwrap()),
        }
    }

    /// The public key as the provider publishes it.
    pub fn jwk(&self) -> Value {
        match &self.key {
            SigningKey::Ec(key) => {
                let point = key.public_key().as_ref();
                json!({
                    "kty": "EC", "crv": "P-256", "kid": self.kid, "use": "sig", "alg": "ES256",
                    "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                    "y": URL_SAFE_NO_PAD.encode(&point[33..]),
                })
            }
            SigningKey::Rsa(key) => {
                let public = key.public();
                let components: RsaPublicKeyComponents<Vec<u8>> = public.into();
                json!({
                    "kty": "RSA", "kid": self.kid, "use": "sig", "alg": "RS256",
                    "n": URL_SAFE_NO_PAD.encode(&components.n),
                    "e": URL_SAFE_NO_PAD.encode(&components.e),
                })
            }
        }
    }

    pub fn sign(&self, claims: &Value) -> String {
        self.sign_with(
            &json!({ "alg": self.alg, "kid": self.kid, "typ": "JWT" }),
            claims,
        )
    }

    pub fn sign_with(&self, header: &Value, claims: &Value) -> String {
        let message = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let rng = SystemRandom::new();
        let signature = match &self.key {
            SigningKey::Ec(key) => key
                .sign(&rng, message.as_bytes())
                .unwrap()
                .as_ref()
                .to_vec(),
            SigningKey::Rsa(key) => {
                let mut signature = vec![0; key.public().modulus_len()];
                key.sign(
                    &signature::RSA_PKCS1_SHA256,
                    &rng,
                    message.as_bytes(),
                    &mut signature,
                )
                .unwrap();
                signature
            }
        };
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
}

pub fn set(signers: &[&Signer]) -> JwkSet {
    serde_json::from_value(json!({ "keys": signers.iter().map(|s| s.jwk()).collect::<Vec<_>>() }))
        .unwrap()
}

const NOW: i64 = 1_800_000_000;

fn claims() -> Value {
    json!({
        "iss": "https://auth.example.com",
        "sub": "c0ffee",
        "aud": "innby",
        "exp": NOW + 300,
        "iat": NOW,
        "nonce": "n-0S6",
        "preferred_username": "kari",
    })
}

fn expected() -> Expected<'static> {
    Expected {
        issuer: "https://auth.example.com",
        client_id: "innby",
        nonce: "n-0S6",
        access_token: "an access token",
        now: NOW,
    }
}

fn check(signer: &Signer, keys: &JwkSet, claims: &Value) -> Result<Map<String, Value>, Error> {
    Token::parse(&signer.sign(claims))?
        .verify(keys)?
        .check(&expected())
}

#[test]
fn accepts_es256_and_rs256_tokens_for_this_login() {
    for signer in [Signer::es256("ec"), Signer::rs256("rsa")] {
        let claims = check(&signer, &set(&[&signer]), &claims()).unwrap();
        assert_eq!(claims["preferred_username"], "kari");
    }
}

#[test]
fn checks_the_signature_with_the_right_key() {
    let signer = Signer::es256("ec");
    let other = Signer::es256("other");
    // The key the token names isn't published (yet).
    assert_eq!(
        check(&signer, &set(&[&other]), &claims()),
        Err(Error::UnknownKey)
    );
    // A published key under the token's name that didn't sign it.
    let impostor = Signer {
        kid: "other",
        ..Signer::es256("x")
    };
    assert_eq!(
        check(&impostor, &set(&[&other]), &claims()),
        Err(Error::BadSignature)
    );
    // Tampered claims.
    let token = signer.sign(&claims());
    let mut parts: Vec<&str> = token.split('.').collect();
    let mut forged = claims();
    forged["sub"] = json!("admin");
    let forged = URL_SAFE_NO_PAD.encode(forged.to_string());
    parts[1] = &forged;
    let token = parts.join(".");
    let verified = Token::parse(&token).unwrap().verify(&set(&[&signer]));
    assert_eq!(verified.unwrap_err(), Error::BadSignature);
}

#[test]
fn refuses_algorithms_and_headers_it_shouldnt_trust() {
    let signer = Signer::es256("ec");
    let keys = set(&[&signer]);
    let header = |header: Value| {
        let token = signer.sign_with(&header, &claims());
        Token::parse(&token)
            .and_then(|token| token.verify(&keys))
            .map(|_| ())
    };
    assert!(matches!(
        header(json!({ "alg": "none" })),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        header(json!({ "alg": "HS256" })),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        header(json!({ "alg": "ES256", "crit": ["exp"] })),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        header(json!({ "alg": "ES256", "typ": "at+jwt" })),
        Err(Error::Unsupported(_))
    ));
    // A key the token brings along is no reason to trust it.
    let attacker = Signer::es256("ec");
    let token = attacker.sign_with(
        &json!({ "alg": "ES256", "kid": "ec", "jwk": attacker.jwk() }),
        &claims(),
    );
    let verified = Token::parse(&token).unwrap().verify(&keys);
    assert_eq!(verified.unwrap_err(), Error::BadSignature);
    // An RSA token can't use an EC key, or a key meant for another algorithm.
    let rsa = Signer::rs256("ec");
    assert_eq!(check(&rsa, &keys, &claims()), Err(Error::UnknownKey));
    // No kid: any fitting key may have signed it.
    let token = signer.sign_with(&json!({ "alg": "ES256" }), &claims());
    assert!(Token::parse(&token).unwrap().verify(&keys).is_ok());
    assert_eq!(Token::parse("a.b").err(), Some(Error::Malformed));
}

#[test]
fn checks_every_claim() {
    let signer = Signer::es256("ec");
    let keys = set(&[&signer]);
    let with = |name: &str, value: Value| {
        let mut claims = claims();
        if value.is_null() {
            claims.as_object_mut().unwrap().remove(name);
        } else {
            claims[name] = value;
        }
        check(&signer, &keys, &claims).err()
    };
    assert_eq!(
        with("iss", json!("https://auth.example.com/")),
        Some(Error::Claim("iss"))
    );
    assert_eq!(with("sub", Value::Null), Some(Error::Claim("sub")));
    assert_eq!(with("aud", json!("grafana")), Some(Error::Claim("aud")));
    assert_eq!(
        with("aud", json!(["innby", "grafana"])),
        Some(Error::Claim("aud"))
    );
    assert_eq!(with("aud", json!(["innby"])), None);
    assert_eq!(with("azp", json!("grafana")), Some(Error::Claim("azp")));
    assert_eq!(with("azp", json!("innby")), None);
    assert_eq!(with("exp", json!(NOW - LEEWAY)), Some(Error::Claim("exp")));
    assert_eq!(with("exp", json!(NOW - LEEWAY + 1)), None);
    assert_eq!(
        with("iat", json!(NOW + LEEWAY + 1)),
        Some(Error::Claim("iat"))
    );
    assert_eq!(with("iat", json!(NOW as f64 + 0.5)), None);
    assert_eq!(
        with("nonce", json!("replayed")),
        Some(Error::Claim("nonce"))
    );
    assert_eq!(with("nonce", Value::Null), Some(Error::Claim("nonce")));
    // at_hash, as the spec's example computes it.
    let hash = digest::digest(&digest::SHA256, b"an access token");
    let at_hash = URL_SAFE_NO_PAD.encode(&hash.as_ref()[..16]);
    assert_eq!(with("at_hash", json!(at_hash)), None);
    assert_eq!(
        with("at_hash", json!("AAAA")),
        Some(Error::Claim("at_hash"))
    );
}
