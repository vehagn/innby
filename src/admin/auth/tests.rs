use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
use sqlx::postgres::PgPoolOptions;

use super::*;

/// A hash with tiny argon2 parameters, so tests stay fast.
pub fn cheap_hash(password: &str) -> String {
    let params = Params::new(8, 1, 1, None).unwrap();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string()
}

/// `admin` with the password `secret`, the editor `ola.n` with `ola`, `nils` with no
/// role of their own with `nils`, and no provider.
pub fn admin_config() -> AdminConfig {
    let user = |user: &str, role, password| PasswordLogin {
        user: user.into(),
        role,
        password_hash: cheap_hash(password),
    };
    AdminConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        path: String::new(),
        users: vec![
            user("admin", Some(Role::Admin), "secret"),
            user("ola.n", Some(Role::Editor), "ola"),
            user("nils", None, "nils"),
        ],
        oidc: None,
        jwt_secret: jwt::Secret::from_base64(jwt::tests::SECRET).unwrap(),
    }
}

/// A JWT for this user and role that's valid now, as the provider would vouch for
/// them, for tests that don't log in through a form.
pub fn token(config: &AdminConfig, db: PgPool, user: &str, role: Option<Role>) -> String {
    let login = Login::Provider { role };
    Auth::new(config, link_key(), true, db).token(Timestamp::now(), user, login, TOKEN_LIFETIME)
}

/// The tests' link key, as `config::tests::config` has it.
fn link_key() -> LinkKey {
    LinkKey::from_base64(crate::link::tests::KEY).unwrap()
}

fn auth() -> Auth {
    // Checking passwords never touches the database.
    let db = PgPoolOptions::new()
        .connect_lazy("postgres://innby@127.0.0.1:1/innby")
        .unwrap();
    Auth::new(&admin_config(), link_key(), true, db)
}

#[test]
fn refresh_tokens_are_versioned_and_keep_dotted_usernames_intact() {
    let identity = PasswordIdentity::Stored(AccountCredential {
        id: Uuid::nil(),
        version: 3,
    });
    let token = refresh_token("ola.n", identity);
    assert!(token.starts_with("v1."));
    assert_eq!(
        parse_refresh_token(&token),
        Some(("ola.n".into(), identity))
    );
    assert_eq!(parse_refresh_token("ola.n.3.old-secret"), None);
}

#[test]
fn configured_password_claims_have_a_distinct_identity() {
    let now = Timestamp::now();
    let claims = jwt::Claims {
        iat: now.as_second(),
        exp: (now + TOKEN_LIFETIME).as_second(),
        user: "admin".into(),
        login: Login::Password {
            identity: PasswordIdentity::Configured,
        },
        jti: Uuid::nil(),
    };
    let key = jwt::Key::new(
        &jwt::Secret::from_base64(jwt::tests::SECRET).unwrap(),
        &[b"admin", b"hash"],
    );
    let token = key.sign(&claims);
    assert_eq!(key.verify(&token, now), Some(claims));
}

#[sqlx::test]
async fn old_password_credentials_do_not_authenticate_a_recreated_account(db: PgPool) {
    let config = admin_config();
    let auth = Auth::new(&config, link_key(), true, db.clone());
    admin_user::create(
        &db,
        "same.name",
        Some("editor"),
        Some(&cheap_hash("secret")),
        Some("same@example.com"),
    )
    .await
    .unwrap()
    .unwrap();
    let predecessor = admin_user::find(&db, "same.name").await.unwrap().unwrap();
    let identity = PasswordIdentity::Stored(predecessor.credential());
    let cookies = auth.start_session("same.name", identity).await.unwrap();
    let token = cookies[0]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("token=")
        .unwrap()
        .to_owned();
    let refresh = cookies[1]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("refresh_token=")
        .unwrap()
        .to_owned();
    let link = auth.link_key.password_link(
        &predecessor.name,
        predecessor.id,
        predecessor.email.as_deref().unwrap(),
        predecessor.version,
        Timestamp::now() + SignedDuration::from_hours(1),
    );
    let link: crate::link::PasswordLink = link.parse().unwrap();

    assert!(admin_user::delete(&db, "same.name").await.unwrap());
    admin_user::create(
        &db,
        "same.name",
        Some("editor"),
        Some(&cheap_hash("secret")),
        Some("same@example.com"),
    )
    .await
    .unwrap()
    .unwrap();
    let replacement = admin_user::find(&db, "same.name").await.unwrap().unwrap();
    assert_eq!(replacement.version, predecessor.version);
    assert_ne!(replacement.id, predecessor.id);
    assert!(!auth.link_key.verify_password(
        &link,
        &replacement.name,
        replacement.id,
        replacement.email.as_deref().unwrap(),
        replacement.version,
    ));

    for value in [format!("token={token}"), format!("refresh_token={refresh}")] {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, value.parse().unwrap());
        assert!(matches!(
            auth.session(&headers).await.unwrap(),
            Session::LoggedOut
        ));
    }
}

#[tokio::test]
async fn limits_guessing_per_client() {
    let auth = auth();
    let client = |last: u8| ClientIp([192, 0, 2, last].into());
    let check =
        |last: u8, password: &str| auth.check(client(last), "admin".into(), password.into());
    let admin = Outcome::Accepted(
        "admin".into(),
        Some(Role::Admin),
        PasswordIdentity::Configured,
    );
    assert_eq!(check(1, "secret").await, admin);
    // The login above was the first of the attempts.
    for i in 1..ATTEMPTS {
        assert_eq!(
            check(1, &format!("guess{i}")).await,
            Outcome::Rejected,
            "{i}"
        );
    }
    assert_eq!(check(1, "secret").await, Outcome::TooManyAttempts);
    // Someone else still gets in.
    assert_eq!(check(2, "secret").await, admin);
}

#[tokio::test]
async fn limits_guessing_from_many_clients() {
    let auth = auth();
    // 100 checks so far, from ten clients, ten each.
    for client in 0..10u8 {
        for _ in 0..10 {
            let ip = ClientIp([192, 0, 2, client].into());
            assert_eq!(
                auth.check(ip, "admin".into(), "guess".into()).await,
                Outcome::Rejected
            );
        }
    }
    let fresh = ClientIp([198, 51, 100, 1].into());
    assert_eq!(
        auth.check(fresh, "admin".into(), "secret".into()).await,
        Outcome::TooManyAttempts
    );
}

#[tokio::test]
async fn cookies_are_http_only_and_secure_over_https() {
    let cookie = auth().cookie(TOKEN, "t", TOKEN_LIFETIME);
    assert_eq!(
        cookie,
        "token=t; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax; Secure"
    );
}
