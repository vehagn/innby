//! ID tokens: their signatures, checked with the keys the provider publishes, and their
//! claims, checked as OpenID Connect Core §3.1.3.7 says.
//!
//! The usual mistakes, and what this module does instead:
//! - `alg` comes from the token, so only asymmetric algorithms are accepted (never
//!   `none`, never HMAC with a public key as the secret), and the key must be of the
//!   algorithm's type and curve, and name the same algorithm if it names one.
//! - Keys come only from the provider's `jwks_uri`, never from the token's `jku`, `jwk`
//!   or `x5u` headers, and a token whose `crit` header asks for anything is refused.
//! - `aud` must be Innby alone, `nonce` must be this login's, and `iss` must be the
//!   provider's, exactly.

use base64::{
    Engine, alphabet,
    engine::{
        DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig, general_purpose::URL_SAFE_NO_PAD,
    },
};
use ring::{
    digest,
    signature::{self, RsaPublicKeyComponents, UnparsedPublicKey, VerificationAlgorithm},
};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Base64url as keys sometimes have it: padded or not.
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// How far the provider's clock may be from ours.
pub const LEEWAY: i64 = 60;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not three base64url parts with a JSON header and claims.
    Malformed,
    /// An algorithm Innby doesn't accept, or a `crit` or `typ` header it doesn't know.
    Unsupported(String),
    /// No published key fits the token: maybe the provider has a new one.
    UnknownKey,
    BadSignature,
    /// A claim that doesn't hold, by name.
    Claim(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Malformed => f.write_str("the ID token is malformed"),
            Error::Unsupported(what) => {
                write!(f, "the ID token uses {what}, which Innby doesn't accept")
            }
            Error::UnknownKey => f.write_str("no published key fits the ID token"),
            Error::BadSignature => f.write_str("the ID token's signature is wrong"),
            Error::Claim(claim) => write!(f, "the ID token's {claim} claim doesn't hold"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    Rs256,
    Rs384,
    Rs512,
    Ps256,
    Ps384,
    Ps512,
    Es256,
    Es384,
    EdDsa,
}

impl Alg {
    fn from_name(name: &str) -> Option<Alg> {
        Some(match name {
            "RS256" => Alg::Rs256,
            "RS384" => Alg::Rs384,
            "RS512" => Alg::Rs512,
            "PS256" => Alg::Ps256,
            "PS384" => Alg::Ps384,
            "PS512" => Alg::Ps512,
            "ES256" => Alg::Es256,
            "ES384" => Alg::Es384,
            "EdDSA" => Alg::EdDsa,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Alg::Rs256 => "RS256",
            Alg::Rs384 => "RS384",
            Alg::Rs512 => "RS512",
            Alg::Ps256 => "PS256",
            Alg::Ps384 => "PS384",
            Alg::Ps512 => "PS512",
            Alg::Es256 => "ES256",
            Alg::Es384 => "ES384",
            Alg::EdDsa => "EdDSA",
        }
    }

    /// The hash `at_hash` takes the left half of.
    fn hash(self) -> &'static digest::Algorithm {
        match self {
            Alg::Rs256 | Alg::Ps256 | Alg::Es256 => &digest::SHA256,
            Alg::Rs384 | Alg::Ps384 | Alg::Es384 => &digest::SHA384,
            // Ed25519 hashes with SHA-512.
            Alg::Rs512 | Alg::Ps512 | Alg::EdDsa => &digest::SHA512,
        }
    }

    fn verify(self, key: &Jwk, message: &[u8], signature: &[u8]) -> bool {
        let rsa = |params: &'static signature::RsaParameters| {
            let (Some(n), Some(e)) = (decode(&key.n), decode(&key.e)) else {
                return false;
            };
            key.kty == "RSA"
                && RsaPublicKeyComponents { n, e }
                    .verify(params, message, signature)
                    .is_ok()
        };
        let ec = |curve: &str, size: usize, algorithm: &'static dyn VerificationAlgorithm| {
            let (Some(x), Some(y)) = (decode(&key.x), decode(&key.y)) else {
                return false;
            };
            if key.kty != "EC" || key.crv.as_deref() != Some(curve) {
                return false;
            }
            if x.len() != size || y.len() != size {
                return false;
            }
            let point = [&[4], x.as_slice(), y.as_slice()].concat();
            UnparsedPublicKey::new(algorithm, point)
                .verify(message, signature)
                .is_ok()
        };
        match self {
            Alg::Rs256 => rsa(&signature::RSA_PKCS1_2048_8192_SHA256),
            Alg::Rs384 => rsa(&signature::RSA_PKCS1_2048_8192_SHA384),
            Alg::Rs512 => rsa(&signature::RSA_PKCS1_2048_8192_SHA512),
            Alg::Ps256 => rsa(&signature::RSA_PSS_2048_8192_SHA256),
            Alg::Ps384 => rsa(&signature::RSA_PSS_2048_8192_SHA384),
            Alg::Ps512 => rsa(&signature::RSA_PSS_2048_8192_SHA512),
            Alg::Es256 => ec("P-256", 32, &signature::ECDSA_P256_SHA256_FIXED),
            Alg::Es384 => ec("P-384", 48, &signature::ECDSA_P384_SHA384_FIXED),
            Alg::EdDsa => {
                let Some(x) = decode(&key.x) else {
                    return false;
                };
                key.kty == "OKP"
                    && key.crv.as_deref() == Some("Ed25519")
                    && UnparsedPublicKey::new(&signature::ED25519, x)
                        .verify(message, signature)
                        .is_ok()
            }
        }
    }

    /// The key type a key for this algorithm has.
    fn kty(self) -> &'static str {
        match self {
            Alg::Es256 | Alg::Es384 => "EC",
            Alg::EdDsa => "OKP",
            _ => "RSA",
        }
    }
}

fn decode(value: &Option<String>) -> Option<Vec<u8>> {
    LENIENT.decode(value.as_deref()?).ok()
}

/// The provider's published keys.
#[derive(Debug, Clone, Deserialize)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    kty: String,
    kid: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
    typ: Option<String>,
    crit: Option<Value>,
}

/// A token that has been split and decoded, but not yet trusted.
pub struct Token<'a> {
    alg: Alg,
    kid: Option<String>,
    signed: &'a str,
    signature: Vec<u8>,
    claims: Map<String, Value>,
}

impl<'a> Token<'a> {
    pub fn parse(token: &'a str) -> Result<Self, Error> {
        let mut parts = token.split('.');
        let (Some(header), Some(claims), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Malformed);
        };
        let json = |part: &str| URL_SAFE_NO_PAD.decode(part).map_err(|_| Error::Malformed);
        let header: Header =
            serde_json::from_slice(&json(header)?).map_err(|_| Error::Malformed)?;
        if header.crit.is_some() {
            return Err(Error::Unsupported("a crit header".into()));
        }
        if let Some(typ) = &header.typ
            && !typ.eq_ignore_ascii_case("JWT")
        {
            return Err(Error::Unsupported(format!("typ {typ}")));
        }
        let alg = Alg::from_name(&header.alg)
            .ok_or_else(|| Error::Unsupported(format!("alg {}", header.alg)))?;
        Ok(Token {
            alg,
            kid: header.kid,
            signed: &token[..token.rfind('.').expect("three parts")],
            signature: json(signature)?,
            claims: serde_json::from_slice(&json(claims)?).map_err(|_| Error::Malformed)?,
        })
    }

    /// The claims, once one of the provider's keys has verified the signature.
    pub fn verify(self, keys: &JwkSet) -> Result<Claims, Error> {
        let candidates: Vec<&Jwk> = keys
            .keys
            .iter()
            .filter(|key| key.usage.as_deref().is_none_or(|usage| usage == "sig"))
            .filter(|key| key.alg.as_deref().is_none_or(|alg| alg == self.alg.name()))
            .filter(|key| key.kty == self.alg.kty())
            .filter(|key| self.kid.is_none() || key.kid == self.kid)
            .collect();
        if candidates.is_empty() {
            return Err(Error::UnknownKey);
        }
        let message = self.signed.as_bytes();
        if !candidates
            .iter()
            .any(|key| self.alg.verify(key, message, &self.signature))
        {
            return Err(Error::BadSignature);
        }
        Ok(Claims {
            alg: self.alg,
            claims: self.claims,
        })
    }
}

/// The claims of a token whose signature checked out, not yet checked themselves.
#[derive(Debug)]
pub struct Claims {
    alg: Alg,
    claims: Map<String, Value>,
}

/// What this login expects of its ID token.
pub struct Expected<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    /// The access token that came with it, for `at_hash`.
    pub access_token: &'a str,
    /// Seconds since the epoch.
    pub now: i64,
}

impl Claims {
    /// The claims, if they're for this login.
    pub fn check(self, expected: &Expected) -> Result<Map<String, Value>, Error> {
        let string = |name: &'static str| {
            self.claims
                .get(name)
                .and_then(Value::as_str)
                .ok_or(Error::Claim(name))
        };
        let time = |name: &'static str| {
            self.claims
                .get(name)
                .and_then(Value::as_f64)
                .map(|seconds| seconds as i64)
                .ok_or(Error::Claim(name))
        };
        if string("iss")? != expected.issuer {
            return Err(Error::Claim("iss"));
        }
        if string("sub")?.is_empty() {
            return Err(Error::Claim("sub"));
        }
        // Innby must be the audience, and the only one: another client's token isn't
        // for logging in here.
        let only_us = match self.claims.get("aud") {
            Some(Value::String(aud)) => aud == expected.client_id,
            Some(Value::Array(auds)) => {
                !auds.is_empty()
                    && auds
                        .iter()
                        .all(|aud| aud.as_str() == Some(expected.client_id))
            }
            _ => false,
        };
        if !only_us {
            return Err(Error::Claim("aud"));
        }
        if let Some(azp) = self.claims.get("azp")
            && azp.as_str() != Some(expected.client_id)
        {
            return Err(Error::Claim("azp"));
        }
        if time("exp")? + LEEWAY <= expected.now {
            return Err(Error::Claim("exp"));
        }
        if time("iat")? - LEEWAY > expected.now {
            return Err(Error::Claim("iat"));
        }
        let nonce_ok = string("nonce").is_ok_and(|nonce| {
            subtle::ConstantTimeEq::ct_eq(nonce.as_bytes(), expected.nonce.as_bytes()).into()
        });
        if !nonce_ok {
            return Err(Error::Claim("nonce"));
        }
        if let Some(at_hash) = self.claims.get("at_hash") {
            let hash = digest::digest(self.alg.hash(), expected.access_token.as_bytes());
            let half = &hash.as_ref()[..hash.as_ref().len() / 2];
            if at_hash.as_str() != Some(URL_SAFE_NO_PAD.encode(half).as_str()) {
                return Err(Error::Claim("at_hash"));
            }
        }
        Ok(self.claims)
    }
}

#[cfg(test)]
pub mod tests {
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
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let key =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
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
        serde_json::from_value(
            json!({ "keys": signers.iter().map(|s| s.jwk()).collect::<Vec<_>>() }),
        )
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
}
