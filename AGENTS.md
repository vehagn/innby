# Innby, for coding agents

Innby is an RSVP site that keeps as little about its guests as it can: a public site for
guests, an admin UI for the host, a mailer and a janitor. It's one Rust binary on
Postgres 18, with no JavaScript and server-rendered HTML.

Read as needed: [docs/architecture.md](docs/architecture.md) (how it works),
[docs/development.md](docs/development.md) (how to add a page, text, query, setting or
test), [docs/translating.md](docs/translating.md), [docs/decisions.md](docs/decisions.md)
(why), and the chart's [README](charts/innby/README.md).

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
  main.rs            the CLI: serve | admin | mailer | janitor | migrate | invite | event | hash-password
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
    events.rs        the events list, a new event (typed, copied or imported), an event's details and export
    event_file.rs    an event as a versioned TOML file (apiVersion innby/v1, kind Event)
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
  i18n/              Lang (mod.rs), language.rs (a locale file and its fallbacks), messages.rs, texts.rs, fill.rs
  link.rs            HMAC-signed links: possession is the authorisation
  paths.rs           every route pattern, and links built from them
  theme.rs           the colour scheme from ?theme=
  random.rs          tokens from the OS
  testing.rs         HTTP helpers for tests (body, location, escape, form posts, Set-Cookie)
  e2e/               end-to-end journeys through both sites and the mailer, by a test Browser
locales/*.toml       one file per language: what it is, its fallbacks, its texts; templates/ askama pages
static/style.css     the one stylesheet: tokens first, then base, layout, components, pages
migrations/*.sql     the schema, applied by `innby migrate`
charts/innby/        the Helm chart; deploy/ is one homelab's install of it
docs/                the guides for people, and ropa.md, the GDPR record of processing
```

Every `mod.rs` starts with a `//!` comment listing its parts, so reading it is the way
into a module.

## Rules

- `just test`, `just lint` and `just chart` must pass; CI runs them.
- After changing a query, run `just db prepare` and commit `.sqlx/`.
- Never edit a migration that has been released; add a new one.
- A new text goes in `src/i18n/messages.rs` and `locales/nb.toml` at least; placeholders
  must match Bokmål's. Fill them with `i18n::fill` or `filled`, never `format!`.
- Admin routes go in the router group of the role they need (`admin/mod.rs`), and
  templates hide what `Role::can_edit()` / `Role::is_admin()` rule out.
- Keep personal data out of logs (route patterns and ids only). New personal data needs
  a retention rule in `db/retention.rs`, and updates to `docs/ropa.md`, the privacy
  notices in `templates/privacy/` and `web::PRIVACY_VERSION`.
- Tests go in a `tests` module beside the code: inline when short, in a sibling
  `tests.rs` past about 150 lines (`foo.rs` → `foo/tests.rs`). Journeys go in
  `src/e2e/`. There's no `tests/` directory: integration tests there only see a
  library's public API, and Innby is a binary.
- Style with the components in `static/style.css`; change the look through its tokens.
  No inline styles, and no class without a rule (a test checks both ways).
- No JavaScript. Say Containerfile and Compose, not Dockerfile and Docker Compose. Docs
  and comments are plain British English and say why rather than what. One change per
  commit, imperative subject, a body that says why.
