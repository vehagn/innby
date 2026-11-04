//! A fake OpenID Connect provider for the tests, shaped like Authelia: discovery, an
//! authorization endpoint that logs in whoever the test says, a token endpoint that
//! checks the client secret, the redirect URI and PKCE, ES256 ID tokens with only the
//! standard claims, and userinfo with the profile and the `innby` roles claim.
//!
//! It listens on a local port, since Innby fetches from it over HTTP; the tests'
//! browsers reach it through its router directly. A test can make it misbehave to see
//! that Innby notices.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ring::digest;
use serde_json::{Value, json};
use url::Url;

use super::{OidcConfig, jws::tests::Signer};

pub const CLIENT_ID: &str = "innby";
/// With characters that Basic authentication must form-encode.
pub const CLIENT_SECRET: &str = "s3cret:with/odd+chars";

#[derive(Clone)]
pub struct Fake {
    /// `http://127.0.0.1:{port}`, also the issuer.
    pub origin: String,
    pub router: Router,
    state: Arc<Mutex<Fakery>>,
}

/// Who's logged in at the provider, and what it gets wrong on purpose.
pub struct Fakery {
    /// The user name and `innby` claim of the person logged in; `None` says no.
    pub user: Option<(String, Value)>,
    /// Claims to put in (or, as null, take out of) the next ID tokens.
    pub id_claims: serde_json::Map<String, Value>,
    /// Another `sub` for userinfo to report.
    pub userinfo_sub: Option<String>,
    /// Another issuer for discovery to report.
    pub issuer: Option<String>,
    signer: Signer,
    codes: HashMap<String, Grant>,
    tokens: HashMap<String, String>,
    /// How many times each endpoint was called.
    pub calls: HashMap<&'static str, usize>,
}

#[derive(Clone)]
struct Grant {
    user: String,
    roles: Value,
    nonce: String,
    challenge: String,
    redirect_uri: String,
}

impl Fake {
    pub async fn start() -> Fake {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Fakery {
            user: Some(("kari".into(), json!(["editor"]))),
            id_claims: serde_json::Map::new(),
            userinfo_sub: None,
            issuer: None,
            signer: Signer::es256("fake-1"),
            codes: HashMap::new(),
            tokens: HashMap::new(),
            calls: HashMap::new(),
        }));
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/userinfo", get(userinfo))
            .route("/jwks", get(jwks))
            .with_state((origin.clone(), Arc::clone(&state)));
        tokio::spawn(axum::serve(listener, router.clone()).into_future());
        Fake {
            origin,
            router,
            state,
        }
    }

    /// Innby's settings for this provider, with the admin UI at `admin_url`.
    pub fn config(&self, admin_url: &str) -> OidcConfig {
        OidcConfig {
            issuer: self.origin.clone(),
            client_id: CLIENT_ID.into(),
            client_secret: CLIENT_SECRET.into(),
            name: "Authelia".into(),
            scopes: "openid profile email innby".into(),
            roles_claim: "innby".into(),
            redirect_uri: format!("{admin_url}/login/oidc/callback").parse().unwrap(),
        }
    }

    pub fn with<T>(&self, change: impl FnOnce(&mut Fakery) -> T) -> T {
        change(&mut self.state.lock().unwrap())
    }

    /// Logs this person in at the provider, with these Innby roles.
    pub fn log_in(&self, user: &str, roles: Value) {
        self.with(|fake| fake.user = Some((user.into(), roles)));
    }

    /// Gives the provider a new signing key, as when it rotates its keys.
    pub fn rotate_key(&self) {
        self.with(|fake| fake.signer = Signer::es256("fake-2"));
    }

    pub fn calls(&self, endpoint: &str) -> usize {
        self.with(|fake| fake.calls.get(endpoint).copied().unwrap_or(0))
    }
}

type FakeState = State<(String, Arc<Mutex<Fakery>>)>;

fn count(state: &Arc<Mutex<Fakery>>, endpoint: &'static str) {
    *state.lock().unwrap().calls.entry(endpoint).or_default() += 1;
}

fn random() -> String {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).unwrap();
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn discovery(State((origin, state)): FakeState) -> Json<Value> {
    count(&state, "discovery");
    let issuer = state
        .lock()
        .unwrap()
        .issuer
        .clone()
        .unwrap_or(origin.clone());
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{origin}/authorize"),
        "token_endpoint": format!("{origin}/token"),
        "userinfo_endpoint": format!("{origin}/userinfo"),
        "jwks_uri": format!("{origin}/jwks"),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["plain", "S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "id_token_signing_alg_values_supported": ["ES256", "RS256"],
        "authorization_response_iss_parameter_supported": true,
    }))
}

async fn authorize(
    State((origin, state)): FakeState,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    count(&state, "authorize");
    let param = |name: &str| query.get(name).cloned().unwrap_or_default();
    assert_eq!(param("response_type"), "code");
    assert_eq!(param("client_id"), CLIENT_ID);
    assert_eq!(param("code_challenge_method"), "S256");
    assert!(param("scope").split(' ').any(|scope| scope == "openid"));
    let mut redirect: Url = param("redirect_uri").parse().unwrap();
    let mut fake = state.lock().unwrap();
    {
        let mut pairs = redirect.query_pairs_mut();
        match fake.user.clone() {
            Some((user, roles)) => {
                let code = random();
                fake.codes.insert(
                    code.clone(),
                    Grant {
                        user,
                        roles,
                        nonce: param("nonce"),
                        challenge: param("code_challenge"),
                        redirect_uri: param("redirect_uri"),
                    },
                );
                pairs.append_pair("code", &code);
            }
            None => {
                pairs.append_pair("error", "access_denied");
            }
        }
        pairs.append_pair("state", &param("state"));
        pairs.append_pair("iss", &origin);
    }
    Redirect::to(redirect.as_str()).into_response()
}

async fn token(State((origin, state)): FakeState, headers: HeaderMap, body: String) -> Response {
    count(&state, "token");
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let error =
        |error: &str| (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    // RFC 6749 §2.3.1: each half form-encoded, then base64.
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|value| STANDARD.decode(value).ok())
        .and_then(|value| String::from_utf8(value).ok());
    let expected = format!(
        "{}:{}",
        CLIENT_ID,
        url::form_urlencoded::byte_serialize(CLIENT_SECRET.as_bytes()).collect::<String>()
    );
    if basic.as_deref() != Some(expected.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_client" })),
        )
            .into_response();
    }
    let mut fake = state.lock().unwrap();
    // Codes are good once.
    let Some(grant) = form.get("code").and_then(|code| fake.codes.remove(code)) else {
        return error("invalid_grant");
    };
    if form.get("grant_type").map(String::as_str) != Some("authorization_code")
        || form.get("redirect_uri") != Some(&grant.redirect_uri)
    {
        return error("invalid_grant");
    }
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let challenge = URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
    if challenge != grant.challenge {
        return error("invalid_grant");
    }
    let access_token = random();
    let hash = digest::digest(&digest::SHA256, access_token.as_bytes());
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "iss": origin,
        "sub": format!("sub-{}", grant.user),
        "aud": [CLIENT_ID],
        "azp": CLIENT_ID,
        "exp": now + 3600,
        "iat": now,
        "auth_time": now,
        "nonce": grant.nonce,
        "at_hash": URL_SAFE_NO_PAD.encode(&hash.as_ref()[..16]),
    });
    for (name, value) in &fake.id_claims {
        if value.is_null() {
            claims.as_object_mut().unwrap().remove(name);
        } else {
            claims[name] = value.clone();
        }
    }
    let id_token = fake.signer.sign(&claims);
    let userinfo = json!({
        "sub": fake.userinfo_sub.clone().unwrap_or(format!("sub-{}", grant.user)),
        "preferred_username": grant.user,
        "name": grant.user,
        "innby": grant.roles,
    });
    fake.tokens
        .insert(access_token.clone(), userinfo.to_string());
    Json(json!({
        "access_token": access_token,
        "token_type": "bearer",
        "expires_in": 3600,
        "id_token": id_token,
        "scope": "openid profile email innby",
    }))
    .into_response()
}

async fn userinfo(State((_, state)): FakeState, headers: HeaderMap) -> Response {
    count(&state, "userinfo");
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let fake = state.lock().unwrap();
    match token.and_then(|token| fake.tokens.get(token)) {
        Some(claims) => (
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            claims.clone(),
        )
            .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn jwks(State((_, state)): FakeState) -> Json<Value> {
    count(&state, "jwks");
    Json(json!({ "keys": [state.lock().unwrap().signer.jwk()] }))
}
