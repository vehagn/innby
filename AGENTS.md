# Innby, for coding agents

Innby is an RSVP site that keeps as little about its guests as it can: a public site for
guests, an admin UI for the host, a mailer and a janitor. It's one Rust binary on
Postgres 18, with no JavaScript and server-rendered HTML. [PLAN.md](PLAN.md) explains
the design and its reasons; [README.md](README.md) is for people running it.

## Commands

Run these in the dev container (`just dev` on the host opens a shell in it).

| Command | Does |
|---|---|
| `just test` | All tests, against the dev database |
| `just lint` | rustfmt, clippy with `-D warnings`, the `.sqlx/` query data, cargo-deny |
| `just chart` | Lints and renders the Helm chart, checks it with kubeconform |
| `just db prepare` | Regenerates `.sqlx/` after a query changes (commit it) |
| `just db reset` | Rebuilds the dev database from `migrations/` |
| `just up` | Serves this checkout: site on :8080, admin on :8081 (`admin`/`innby`), Mailpit on :8025 |

`just test`, `just lint` and `just chart` are what CI runs; all three must pass.

## Map

```
src/
  main.rs            the CLI: serve | admin | mailer | janitor | migrate | invite | hash-password
  http/              what both listeners share
    mod.rs           listen (timeouts, graceful shutdown), harden (security headers), no_store, ready, Lang extractor
    cookie.rs        reading Cookie headers, writing Set-Cookie
    csrf.rs          refuses cross-site form posts (Sec-Fetch-Site / Origin)
    rate_limit.rs    in-memory limits per client or for everyone
    render.rs        askama templates to responses, the stylesheet
  web/               the public site; mod.rs has the router and every route
    rsvp.rs          a guest's personal page /r/{link}: answer, rename, export, delete
    join.rs          group sign-up /join/{link}
    email.rs         the optional e-mail address: consent, double opt-in, "I lost my link"
    remember.rs      "remember me on this device": one cookie per event
    pages.rs         front page, privacy notice, message pages
    views.rs         what personal and group pages show alike (event, guest list, choices)
  admin/             the host's UI; mod.rs has the router, grouped by the role each page needs
    page.rs          shared: AdminState, tabs, PageError, load_event, redirect
    events.rs        the events list, a new event, an event's details
    guests.rs        an event's Guests tab, a guest's page, CSV export, reminders
    invitations.rs   invitations in a batch, and `innby invite`
    groups.rs        group sign-up links
    qr/              the QR code style page and SVG downloads; draw.rs draws them
    role.rs          viewer < editor < admin, and what each may do
    lang.rs          the admin page's language
    auth/            logging in: sessions (mod.rs), login.rs, password.rs (argon2), jwt.rs
      oidc/          single sign-on: routes.rs, provider.rs, jws.rs (ID tokens), http.rs, fake.rs (tests)
    testing.rs       a logged-in test host of any role
  db/                queries per table; rsvp/ splits into the row, names.rs and guest_list.rs
  mailer/            the outbox worker; compose.rs writes the mails
  janitor.rs         deletes what retention says is due
  config/            settings from INNBY_* variables: mod.rs (site), admin.rs, mail.rs, env.rs
  i18n/              Lang (mod.rs), messages.rs (locales/*.toml), texts.rs (host-written texts), fill.rs
  link.rs            HMAC-signed links: possession is the authorisation
  paths.rs           every route pattern, and links built from them
  theme.rs           the colour scheme from ?theme=
  random.rs          tokens from the OS
  testing.rs         HTTP helpers for tests (body, location, escape, form posts, Set-Cookie)
  e2e/               end-to-end journeys through both sites and the mailer, by a test Browser
locales/*.toml       every text, in six languages; templates/ askama pages (admin/, privacy/)
migrations/*.sql     the schema, applied by `innby migrate`
charts/innby/        the Helm chart; deploy/ is one homelab's install of it
docs/ropa.md         the GDPR record of processing
```

Every `mod.rs` starts with a `//!` comment listing its parts, so reading it is the way
into a module.

## Where things go

- **A page:** a handler and an askama `Template` struct in the module for its area, the
  template in `templates/` (`templates/admin/` for the admin), the route in that site's
  `router` and its path in `paths.rs`. Admin routes go in the router group of the role
  they need, and templates hide what `Role::can_edit()` / `Role::is_admin()` rule out.
- **A text:** a key in all six `locales/*.toml` files (tests fail if one lacks it, or
  their `{placeholders}` differ), and a field in `i18n/messages.rs`. Admin texts are
  under `[admin]`. Fill placeholders with `i18n::fill` or `filled`, never `format!`.
- **A query:** in the `db` module for its table, as a `sqlx` macro; then `just db prepare`.
- **A schema change:** a new file in `migrations/`; never edit one that's released.
- **A setting:** read it in `config/`, list it in `.env.example`, and in the chart's
  `values.yaml`, `values.schema.json` and `templates/configmap.yaml` if it's for the cluster.
- **Personal data:** keep it out of logs (route patterns and ids only), give it a
  retention rule in `db/retention.rs`, and update `docs/ropa.md` and the privacy
  notices in `templates/privacy/`, bumping `web::PRIVACY_VERSION`.

## Tests

- Unit and database tests sit in a `tests` module at the bottom of the file they test;
  `#[sqlx::test]` gives each a fresh database.
- Router-level tests: `web::testing::Site` for the public site, `admin::testing::Admin`
  for a logged-in host. Generic helpers are in `crate::testing`.
- Journeys go in `src/e2e/`, one file per area; `src/e2e/mod.rs` says how to add one.
  They find fields by label and buttons by text, from the British locale (`en()`,
  `admin()`).

## Conventions

- Rust 2024 edition, `cargo fmt`, clippy clean with `-D warnings`.
- No JavaScript, and a strict CSP: forms and links only. Theme and language travel in
  `?theme=` and `?lang=`.
- Docs and comments are plain, British English, and say why rather than what.
- Say Containerfile and Compose, not Dockerfile and Docker Compose.
- Commits: one change each, imperative subject, a body that says why.
