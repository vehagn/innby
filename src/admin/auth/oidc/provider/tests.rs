use axum::{body::Body, http::Request};
use serde_json::json;
use tower::ServiceExt;

use std::collections::HashMap;

use axum::http::header;

use super::*;
use crate::admin::auth::oidc::fake::Fake;

const ADMIN: &str = "https://admin.fest.example.com";

async fn provider() -> (Fake, Provider) {
    let fake = Fake::start().await;
    let mut provider = Provider::new(fake.config(ADMIN));
    provider.keys_min_age = Duration::ZERO;
    (fake, provider)
}

/// The browser's trip to the provider and back: what it brings to the callback.
async fn authorize(fake: &Fake, url: &Url) -> Callback {
    let path = format!("{}?{}", url.path(), url.query().unwrap());
    let request = Request::get(path).body(Body::empty()).unwrap();
    let response = fake.router.clone().oneshot(request).await.unwrap();
    let location: Url = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        location.as_str().split('?').next().unwrap(),
        format!("{ADMIN}/login/oidc/callback")
    );
    let query: HashMap<String, String> = location.query_pairs().into_owned().collect();
    let field = |name: &str| query.get(name).cloned();
    Callback {
        code: field("code"),
        state: field("state"),
        iss: field("iss"),
        error: field("error"),
        error_description: field("error_description"),
    }
}

async fn log_in(fake: &Fake, provider: &Provider) -> Result<Login, LoginError> {
    let (url, flow) = provider.start(Utc::now()).await.unwrap();
    let callback = authorize(fake, &url).await;
    provider.finish(&flow, &callback, Utc::now()).await
}

fn failed(result: Result<Login, LoginError>) -> String {
    match result {
        Err(LoginError::Failed(err)) => format!("{err:#}"),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn logs_in_with_the_role_from_userinfo() {
    let (fake, provider) = provider().await;
    let login = log_in(&fake, &provider).await.unwrap();
    assert_eq!(
        login,
        Login {
            user: "kari".into(),
            role: Role::Editor
        }
    );
    // The ID token has no roles, as Authelia's by default, so userinfo was asked.
    assert_eq!(fake.calls("userinfo"), 1);

    // Discovery and keys are kept for the next login.
    fake.log_in("ola", json!(["viewer", "grafana_admin", "admin"]));
    let login = log_in(&fake, &provider).await.unwrap();
    assert_eq!(login.role, Role::Admin);
    assert_eq!((fake.calls("discovery"), fake.calls("jwks")), (1, 1));
}

#[tokio::test]
async fn sends_pkce_state_and_nonce() {
    let (_fake, provider) = provider().await;
    let (url, flow) = provider.start(Utc::now()).await.unwrap();
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(query["state"], flow.state);
    assert_eq!(query["nonce"], flow.nonce);
    assert_eq!(query["code_challenge_method"], "S256");
    assert_ne!(query["code_challenge"], flow.verifier);
    assert_eq!(query["scope"], "openid profile email innby");
    assert_eq!(
        query["redirect_uri"],
        format!("{ADMIN}/login/oidc/callback")
    );
    let (_, again) = provider.start(Utc::now()).await.unwrap();
    assert_ne!(flow.state, again.state);
}

#[tokio::test]
async fn turns_away_hosts_without_a_role_and_hears_a_no() {
    let (fake, provider) = provider().await;
    fake.log_in("per", json!(["grafana_admin"]));
    assert!(matches!(
        log_in(&fake, &provider).await,
        Err(LoginError::NoRole { user }) if user == "per"
    ));
    fake.with(|fake| fake.user = None);
    assert!(matches!(
        log_in(&fake, &provider).await,
        Err(LoginError::Cancelled)
    ));
}

#[tokio::test]
async fn the_callback_must_belong_to_this_login() {
    let (fake, provider) = provider().await;
    let (url, flow) = provider.start(Utc::now()).await.unwrap();
    let callback = authorize(&fake, &url).await;

    // Another browser's login, or none.
    let (_, other) = provider.start(Utc::now()).await.unwrap();
    let result = provider.finish(&other, &callback, Utc::now()).await;
    assert!(matches!(result, Err(LoginError::Expired)));
    // Too late.
    let late = Utc::now() + FLOW_LIFETIME;
    let result = provider.finish(&flow, &callback, late).await;
    assert!(matches!(result, Err(LoginError::Expired)));
    // From another provider (RFC 9207), or without saying.
    let elsewhere = Callback {
        iss: Some("https://evil.example.com".into()),
        ..authorize(&fake, &url).await
    };
    let result = provider.finish(&flow, &elsewhere, Utc::now()).await;
    assert!(failed(result).contains("issuer"));
    let unnamed = Callback {
        iss: None,
        ..authorize(&fake, &url).await
    };
    let result = provider.finish(&flow, &unnamed, Utc::now()).await;
    assert!(failed(result).contains("iss"));

    // The right one works, once.
    provider.finish(&flow, &callback, Utc::now()).await.unwrap();
    let result = provider.finish(&flow, &callback, Utc::now()).await;
    assert!(failed(result).contains("invalid_grant"));
}

#[tokio::test]
async fn checks_the_id_token_and_userinfo() {
    let (fake, provider) = provider().await;
    fake.with(|fake| {
        fake.id_claims
            .insert("aud".into(), json!(["innby", "grafana"]))
    });
    assert!(failed(log_in(&fake, &provider).await).contains("aud"));
    fake.with(|fake| fake.id_claims.clear());
    fake.with(|fake| fake.id_claims.insert("nonce".into(), json!("another")));
    assert!(failed(log_in(&fake, &provider).await).contains("nonce"));
    fake.with(|fake| fake.id_claims.clear());
    fake.with(|fake| fake.id_claims.insert("at_hash".into(), json!("AAAA")));
    assert!(failed(log_in(&fake, &provider).await).contains("at_hash"));
    fake.with(|fake| fake.id_claims.clear());

    fake.with(|fake| fake.userinfo_sub = Some("sub-someone-else".into()));
    assert!(failed(log_in(&fake, &provider).await).contains("sub"));
    fake.with(|fake| fake.userinfo_sub = None);

    // Roles in the ID token are used as they are.
    fake.with(|fake| fake.id_claims.insert("innby".into(), json!(["admin"])));
    fake.with(|fake| {
        fake.id_claims
            .insert("preferred_username".into(), json!("kari"))
    });
    let asked = fake.calls("userinfo");
    let login = log_in(&fake, &provider).await.unwrap();
    assert_eq!(login.role, Role::Admin);
    assert_eq!(fake.calls("userinfo"), asked);
}

#[tokio::test]
async fn fetches_new_keys_when_the_provider_rotates_them() {
    let (fake, provider) = provider().await;
    log_in(&fake, &provider).await.unwrap();
    fake.rotate_key();
    log_in(&fake, &provider).await.unwrap();
    assert_eq!(fake.calls("jwks"), 2);
}

#[tokio::test]
async fn the_provider_must_be_the_configured_issuer() {
    let (fake, provider) = provider().await;
    fake.with(|fake| fake.issuer = Some("https://evil.example.com".into()));
    let err = provider.start(Utc::now()).await.unwrap_err();
    assert!(format!("{err:#}").contains("INNBY_OIDC_ISSUER"), "{err:#}");

    let wrong_secret = Provider::new(OidcConfig {
        client_secret: "wrong".into(),
        ..fake.config(ADMIN)
    });
    fake.with(|fake| fake.issuer = None);
    assert!(failed(log_in(&fake, &wrong_secret).await).contains("invalid_client"));
}
