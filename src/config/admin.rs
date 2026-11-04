//! Settings only `innby admin` needs, on top of `Config`: where it listens and where
//! hosts reach it, the built-in user, single sign-on, and the secret its logins are
//! signed with. A public site refuses the example ones in `.env.example`.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, ensure};

use super::{Config, Env, process_env};
use crate::admin::auth::{jwt, oidc::OidcConfig};

/// The example password and JWT secret in `.env.example`, which a public site refuses.
pub(super) const EXAMPLE_ADMIN_PASSWORD: &str = "innby";
pub(super) const EXAMPLE_ADMIN_JWT_SECRET: &str =
    "ZGV2LW9ubHktYWRtaW4tand0LXNlY3JldC1kby1ub3QtdXNlLWluLXByb2Q=";

/// Settings only `innby admin` needs, on top of `Config`.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    pub listen: SocketAddr,
    /// The path the admin UI is under, such as `/admin`, or empty for the root. From
    /// `INNBY_ADMIN_URL`.
    pub path: String,
    /// The built-in user, unless `INNBY_ADMIN_PASSWORD_HASH` is left out.
    pub login: Option<PasswordLogin>,
    /// Single sign-on, when `INNBY_OIDC_ISSUER` is set.
    pub oidc: Option<OidcConfig>,
    /// Signs the login's JWTs.
    pub jwt_secret: jwt::Secret,
}

#[derive(Debug, Clone)]
pub struct PasswordLogin {
    pub user: String,
    /// An argon2 PHC string from `innby hash-password`.
    pub password_hash: String,
}

/// Where the provider sends the host back to, under `INNBY_ADMIN_URL`.
pub const OIDC_CALLBACK: &str = "/login/oidc/callback";

impl AdminConfig {
    pub fn from_env(config: &Config) -> Result<Self> {
        Self::from_lookup(process_env, config)
    }

    pub(super) fn from_lookup(
        get: impl Fn(&str) -> Option<String>,
        config: &Config,
    ) -> Result<Self> {
        let env = Env(get);
        let login = env
            .optional("INNBY_ADMIN_PASSWORD_HASH")
            .map(|password_hash| -> Result<PasswordLogin> {
                crate::admin::auth::check_hash(&password_hash)
                    .context("INNBY_ADMIN_PASSWORD_HASH")?;
                ensure!(
                    !config.is_https()
                        || !crate::admin::auth::verify(&password_hash, EXAMPLE_ADMIN_PASSWORD),
                    "INNBY_ADMIN_PASSWORD_HASH: that's a hash of the example password from \
                     .env.example; make one for a real password with `just innby hash-password`"
                );
                Ok(PasswordLogin {
                    user: env
                        .optional("INNBY_ADMIN_USER")
                        .unwrap_or_else(|| "admin".into()),
                    password_hash,
                })
            })
            .transpose()?;
        let admin_url = env.admin_url()?;
        let oidc = env.oidc(admin_url.as_ref())?;
        ensure!(
            login.is_some() || oidc.is_some(),
            "nobody could log in: set INNBY_ADMIN_PASSWORD_HASH, INNBY_OIDC_ISSUER or both"
        );
        let jwt_secret = env.required("INNBY_ADMIN_JWT_SECRET")?;
        ensure!(
            !config.is_https() || jwt_secret.trim() != EXAMPLE_ADMIN_JWT_SECRET,
            "INNBY_ADMIN_JWT_SECRET: that's the example from .env.example, which anyone can read"
        );
        Ok(Self {
            listen: env.socket_address("INNBY_ADMIN_LISTEN", "0.0.0.0:8081")?,
            path: admin_url
                .map(|url| url.path().trim_end_matches('/').to_owned())
                .unwrap_or_default(),
            login,
            oidc,
            jwt_secret: jwt::Secret::from_base64(&jwt_secret).context("INNBY_ADMIN_JWT_SECRET")?,
        })
    }
}

impl<F: Fn(&str) -> Option<String>> Env<F> {
    /// Where hosts reach the admin UI: on a hostname of its own, or under a path of the
    /// site's, such as `https://fest.example.com/admin`.
    fn admin_url(&self) -> Result<Option<url::Url>> {
        let Some(value) = self.optional("INNBY_ADMIN_URL") else {
            return Ok(None);
        };
        let url = value
            .trim()
            .parse::<url::Url>()
            .ok()
            .filter(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host().is_some()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.username().is_empty()
            })
            .ok_or_else(|| {
                anyhow!(
                    "INNBY_ADMIN_URL: expected an address like https://admin.example.com \
                     or https://example.com/admin, got {value:?}"
                )
            })?;
        Ok(Some(url))
    }

    fn oidc(&self, admin_url: Option<&url::Url>) -> Result<Option<OidcConfig>> {
        let Some(issuer) = self.optional("INNBY_OIDC_ISSUER") else {
            return Ok(None);
        };
        let admin_url = admin_url.ok_or_else(|| anyhow!("INNBY_ADMIN_URL must be set"))?;
        let redirect_uri = format!(
            "{}{OIDC_CALLBACK}",
            admin_url.as_str().trim_end_matches('/')
        )
        .parse::<url::Url>()?;
        let config = OidcConfig {
            issuer: issuer.trim().to_owned(),
            client_id: self.required("INNBY_OIDC_CLIENT_ID")?,
            client_secret: self.required("INNBY_OIDC_CLIENT_SECRET")?,
            name: self
                .optional("INNBY_OIDC_NAME")
                .unwrap_or_else(|| "SSO".into()),
            scopes: self
                .optional("INNBY_OIDC_SCOPES")
                .unwrap_or_else(|| "openid profile email innby".into()),
            roles_claim: self
                .optional("INNBY_OIDC_ROLES_CLAIM")
                .unwrap_or_else(|| "innby".into()),
            redirect_uri,
        };
        config.check().context("INNBY_OIDC_ISSUER")?;
        Ok(Some(config))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::tests::{config, parse};

    fn admin_with(hash: &str, jwt_secret: &str, config: &Config) -> Result<AdminConfig> {
        AdminConfig::from_lookup(
            |key| match key {
                "INNBY_ADMIN_PASSWORD_HASH" => Some(hash.to_owned()),
                "INNBY_ADMIN_JWT_SECRET" => Some(jwt_secret.to_owned()),
                _ => None,
            },
            config,
        )
    }

    const OIDC: &[(&str, &str)] = &[
        ("INNBY_ADMIN_JWT_SECRET", crate::link::tests::KEY),
        ("INNBY_ADMIN_URL", "https://admin.fest.example.com/"),
        ("INNBY_OIDC_ISSUER", "https://authelia.example.com"),
        ("INNBY_OIDC_CLIENT_ID", "innby"),
        ("INNBY_OIDC_CLIENT_SECRET", "s3cret"),
    ];

    fn admin_oidc(overrides: &[(&str, &str)]) -> Result<AdminConfig> {
        let mut env: HashMap<&str, &str> = OIDC.iter().copied().collect();
        env.extend(overrides.iter().copied());
        AdminConfig::from_lookup(|key| env.get(key).map(|value| value.to_string()), &config())
    }

    #[test]
    fn single_sign_on_can_replace_the_built_in_user() {
        let admin = admin_oidc(&[]).unwrap();
        assert!(admin.login.is_none());
        assert_eq!(admin.path, "");
        let oidc = admin.oidc.unwrap();
        assert_eq!(
            oidc.redirect_uri.as_str(),
            "https://admin.fest.example.com/login/oidc/callback"
        );
        assert_eq!(
            (
                oidc.scopes.as_str(),
                oidc.roles_claim.as_str(),
                oidc.name.as_str()
            ),
            ("openid profile email innby", "innby", "SSO")
        );
        assert!(!format!("{oidc:?}").contains("s3cret"));

        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let both = admin_oidc(&[("INNBY_ADMIN_PASSWORD_HASH", &hash)]).unwrap();
        assert!(both.login.is_some() && both.oidc.is_some());

        let err = |overrides: &[(&str, &str)]| format!("{:#}", admin_oidc(overrides).unwrap_err());
        assert!(err(&[("INNBY_OIDC_ISSUER", "")]).starts_with("nobody could log in"));
        assert!(err(&[("INNBY_ADMIN_URL", "")]).contains("INNBY_ADMIN_URL"));
        assert!(err(&[("INNBY_ADMIN_URL", "admin.example.com")]).contains("expected"));
        assert!(err(&[("INNBY_ADMIN_URL", "https://example.com/?admin")]).contains("expected"));
        // Under a path of the site's.
        let under = admin_oidc(&[("INNBY_ADMIN_URL", "https://fest.example.com/admin/")]).unwrap();
        assert_eq!(under.path, "/admin");
        assert_eq!(
            under.oidc.unwrap().redirect_uri.as_str(),
            "https://fest.example.com/admin/login/oidc/callback"
        );
        assert!(err(&[("INNBY_OIDC_CLIENT_SECRET", "")]).contains("INNBY_OIDC_CLIENT_SECRET"));
        assert!(err(&[("INNBY_OIDC_ISSUER", "http://authelia.example.com")]).contains("https"));
        assert!(err(&[("INNBY_OIDC_SCOPES", "profile innby")]).contains("openid"));
        // A provider on this machine, while developing.
        admin_oidc(&[("INNBY_OIDC_ISSUER", "http://127.0.0.1:9091")]).unwrap();
    }

    fn admin_with_hash(hash: &str, config: &Config) -> Result<AdminConfig> {
        admin_with(hash, crate::link::tests::KEY, config)
    }

    #[test]
    fn admin_config_needs_a_valid_hash() {
        let config = config();
        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let admin = admin_with_hash(&hash, &config).unwrap();
        assert_eq!(admin.login.unwrap().user, "admin");
        assert_eq!(admin.listen, "0.0.0.0:8081".parse().unwrap());

        assert!(AdminConfig::from_lookup(|_| None, &config).is_err());
        let err = admin_with_hash("hunter2", &config).unwrap_err();
        assert!(format!("{err:#}").starts_with("INNBY_ADMIN_PASSWORD_HASH"));
        let err = admin_with(&hash, "c2hvcnQ=", &config).unwrap_err();
        assert!(format!("{err:#}").starts_with("INNBY_ADMIN_JWT_SECRET"));
    }

    #[test]
    fn admin_refuses_the_example_secrets_on_a_public_site() {
        let local = parse(&[("INNBY_BASE_URL", "http://localhost:8080")]).unwrap();
        let hash = crate::admin::auth::tests::cheap_hash(EXAMPLE_ADMIN_PASSWORD);
        assert!(admin_with_hash(&hash, &config()).is_err());
        admin_with_hash(&hash, &local).unwrap();

        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let err = admin_with(&hash, EXAMPLE_ADMIN_JWT_SECRET, &config()).unwrap_err();
        assert!(err.to_string().contains(".env.example"), "{err}");
        admin_with(&hash, EXAMPLE_ADMIN_JWT_SECRET, &local).unwrap();
    }
}
