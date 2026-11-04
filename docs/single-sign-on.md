# Single sign-on

The admin UI can log hosts in through an OpenID Connect provider, such as
[Authelia](https://www.authelia.com/), as well as or instead of its built-in user. The
provider says who each host is, and what they may do in Innby.

## Roles

The provider lists a host's Innby roles in a claim named after the app, `innby`, as a
list such as `["editor"]`. The highest role listed counts, and a host with none can't log
in. Values Innby doesn't know are ignored, so the same claim can carry other things.

| Role | May |
|---|---|
| `viewer` | See the events, their headcounts and who's coming. Not the personal links, e-mail status or the guest export, since those let you act as a guest. |
| `editor` | Also run the events: invitations, group links, guests, reminders, the export and each event's details |
| `admin` | Also create and delete events, and style the QR codes |

The built-in user is always an admin.

## Turning it on

Set these, in `.env` or in the chart's values (`oidc` and `admin.url`):

| Setting | |
|---|---|
| `INNBY_OIDC_ISSUER` | The provider, exactly as its discovery document names itself, e.g. `https://auth.example.com` |
| `INNBY_OIDC_CLIENT_ID` | Innby's client ID at the provider, e.g. `innby` |
| `INNBY_OIDC_CLIENT_SECRET` | Its client secret (a secret, so in the chart's Secret) |
| `INNBY_ADMIN_URL` | Where hosts reach the admin UI, e.g. `https://fest-admin.example.com` or `https://fest.example.com/admin` |
| `INNBY_OIDC_NAME` | Optional: the name on the login button, e.g. `Authelia` |
| `INNBY_OIDC_SCOPES`, `INNBY_OIDC_ROLES_CLAIM` | Optional: the scopes asked for (`openid profile email innby`) and the claim with the roles (`innby`) |

Register `{INNBY_ADMIN_URL}/login/oidc/callback` as the client's redirect URI. Leave out
`INNBY_ADMIN_PASSWORD_HASH` to turn the built-in user off; the admin UI then sends hosts
straight to the provider.

## Authelia

With LLDAP holding a multi-valued string attribute `innby` per user, set to their roles:

```yaml
authentication_backend:
  ldap:
    attributes:
      extra:
        innby: { multi_valued: true, value_type: string }
identity_providers:
  oidc:
    claims_policies:
      innby:
        custom_claims: { innby: { attribute: innby } }
    scopes:
      innby:
        claims: [ innby ]
    clients:
      - client_id: innby
        client_secret: { path: /secrets/client-innby/client_secret.txt }
        client_name: Innby
        public: false
        authorization_policy: two_factor
        claims_policy: innby
        pre_configured_consent_duration: 3 months
        require_pkce: true
        pkce_challenge_method: S256
        redirect_uris: [ https://fest-admin.example.com/login/oidc/callback ]
        scopes: [ openid, profile, email, innby ]
        response_types: [ code ]
        grant_types: [ authorization_code ]
        token_endpoint_auth_method: client_secret_basic
        id_token_signed_response_alg: ES256
        userinfo_signed_response_alg: none
```

Innby needs `client_secret_basic`, and unsigned userinfo. In a cluster with network
policies, the admin UI must be allowed to reach the provider; the chart's Cilium policies
allow the host in `oidc.issuer`.

## How a login works

1. `/login/oidc` makes a `state`, a `nonce` and a PKCE verifier, keeps them in a signed
   cookie for ten minutes, and sends the browser to the provider.
2. The provider sends it back to `/login/oidc/callback` with a code. The `state` must
   match the cookie's, so nobody can log a browser in to their own account.
3. Innby trades the code for tokens, with its client secret and the PKCE verifier, and
   checks the ID token: its signature, with the keys the provider publishes (asymmetric
   algorithms only, never keys from the token itself), and its `iss`, `aud` (Innby alone),
   `azp`, `exp`, `iat`, `nonce` and `at_hash`.
4. The roles come from the ID token, or else from userinfo, which must be about the same
   user. No role, no login.

The provider's settings come from its discovery document and are kept for an hour; its
keys are fetched again when a token names one Innby hasn't seen. A login lasts 12 hours,
with no refresh token, after which the provider is asked again (seamlessly, while the
host is logged in there). So a role taken away at the provider is gone by the next day.
Logging out of Innby doesn't log out of the provider.

The code is in [`src/admin/auth/oidc/`](../src/admin/auth/oidc/mod.rs), and its tests run
against a fake provider that can be made to misbehave.
