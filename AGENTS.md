# Innby, for coding agents

Innby is an RSVP site that keeps as little about its guests as it can: a public site for
guests, an admin UI for the host, a mailer and a janitor. It's one Rust binary on
Postgres 18, with no JavaScript and server-rendered HTML.

Read as needed: [docs/architecture.md](docs/architecture.md) (how it works),
[docs/development.md](docs/development.md) (how to add a page, text, query, setting or
test), [docs/translating.md](docs/translating.md), [docs/decisions.md](docs/decisions.md)
(why), [docs/games.md](docs/games.md) (minigames and their API), and the chart's
[README](charts/innby/README.md).

## Commands

Run these in the dev container (`just dev` on the host opens a shell in it).

| Command | Does |
|---|---|
| `just test` | All tests, against the dev database, and the games' with `node --test` |
| `just lint` | rustfmt, clippy with `-D warnings`, the `.sqlx/` query data, cargo-deny |
| `just chart` | Lints and renders the Helm chart, checks it with kubeconform |
| `just db prepare` | Regenerates `.sqlx/` after a query changes (commit it) |
| `just db reset` | Rebuilds the dev database from `migrations/` |
| `just db seed` | Loads the sample data in `seed/sample.sql` (`just db fresh`: reset, then seed) |
| `just up` | Serves this checkout: site on :8080, admin on :8081 (`admin`/`innby`), example games on :8082, Mailpit on :8025 |
| `just bench` | Benchmarks a release build under load, with a memory budget (`bench/run.sh`) |

`just test`, `just lint` and `just chart` are what CI runs; all three must pass. CI also
runs `just bench` on every release.

## Map

```
src/
  main.rs            the CLI: serve | admin | mailer | janitor | migrate | invite | event | languages | hash-password
  alloc.rs           the allocator: mimalloc, giving freed memory back at once
  icons.rs           the tabs' icons, as inline SVG
  http/              what both listeners share
    mod.rs           listen (timeouts, graceful shutdown), harden (security headers), no_store, ready,
                     the Lang and Asked extractors
    cookie.rs        reading Cookie headers, writing Set-Cookie
    csrf.rs          refuses cross-site form posts (Sec-Fetch-Site / Origin)
    rate_limit.rs    in-memory limits per client or for everyone
    render.rs        askama templates to responses (render_form: 422 for a form to fix), the stylesheet
    icon.rs          the site's icon: favicon.svg, and the .ico and iPhone PNG `just icons` draws from it
  web/               the public site; mod.rs has the router and every route
    rsvp.rs          a guest's pages /r/{link}: the link's guest, the Reply tab (event, answer)
                     and the Guest list tab
    you.rs           the Your details tab: name, e-mail, this device, export, delete
    calendar.rs      /r/{link}/event.ics, for a guest who's coming
    join.rs          group sign-up /join/{link}
    email.rs         the optional e-mail address: consent, double opt-in, "I lost my link"
    remember.rs      "remember me on this device": one cookie per event
    pages.rs         front page, privacy notice, message pages
    languages.rs     the languages the site offers guests, read from the database and
                     kept a few seconds; each request is handled with them on offer
    games.rs         minigames: the Games tab and its high scores, the scores API /api/, and
                     the internal API for games' servers on its own port
    board.rs         the Board tab: the event's message board, posting, replies, votes
    views.rs         what personal and group pages show alike (event, guest list, choices,
                     the host's question, a guest's tabs)
  admin/             the host's UI; mod.rs has the router, grouped by the role each page needs;
                     a module per tab or page, its handlers named list, show, create, edit, revoke, delete
    page.rs          shared: AdminState, tabs, PageError, load_event, redirect
    forms.rs         shared forms: a name and party size, a batch a line each, delete, ?saved
    events/          the events page, the list; new.rs is the New event tab (typed, copied or imported)
      details.rs     an event's Details tab: change it, download its file, delete it
      form.rs        the event form, checked into an event's details
      file.rs        an event as a versioned TOML file (apiVersion innby/v1, kind Event)
    guests.rs        an event's Guests tab, a guest's page, CSV export, reminders
    invitations.rs   the Invitation links tab: invitations in a batch
    groups.rs        the Group links tab, and each group's page
    qr/              an event's QR code style tab and the SVG downloads; draw.rs draws them
    games.rs         an event's Games tab: the minigames it offers, and their scores
    board.rs         an event's Board tab: what its board is, and hosts' posts and deletions
    access.rs        an event's Access tab: roles for that event alone, by login name
    users.rs         the Users tab, for admins: password users kept in the database
    languages.rs     the Languages tab, for admins: which languages the site offers
                     guests, its default, and how far each is translated
    role.rs          viewer < editor < admin, and the role a request has for its event
    lang.rs          the admin page's language
    auth/            logging in: sessions (mod.rs), login.rs, password.rs (argon2), jwt.rs
      oidc/          single sign-on: routes.rs, provider.rs, jws.rs (ID tokens), http.rs, fake.rs (tests)
    cli.rs           the host's commands: `innby invite`, `innby event list|export|import`
    testing.rs       a logged-in test host of any role
  db/                queries per table (access.rs: roles for one event; admin_user.rs:
                     password users made in the admin UI; board.rs: the message board's
                     posts and votes, and its threads in page order; admin_form.rs: create forms
                     sent, so one sent twice makes once; languages.rs: the languages
                     the site offers); rsvp/ splits into the row,
                     names.rs and guest_list.rs
  mailer/            the outbox worker; compose.rs writes the mails
  janitor.rs         deletes what retention says is due
  config/            settings from INNBY_* variables: mod.rs (site), admin.rs, mail.rs,
                     games.rs (INNBY_GAMES), env.rs
  i18n/              Lang (mod.rs), language.rs (a locale file and its fallbacks), messages.rs,
                     texts.rs, fill.rs, offer.rs (which languages a page is offered in, and
                     picking among them for a request)
  calendar.rs        an event as an iCalendar file, for guests' pages and reminders
  board.rs           a message board post as both sites show it (_board_thread.html)
  markdown.rs        the little Markdown posts are written in, everything else escaped
  link.rs            HMAC-signed links: possession is the authorisation
  paths.rs           every route pattern, and links built from them
  theme.rs           the colour scheme from ?theme=
  version.rs         which build this is: release, commit and date, as build.rs stamps them
  metrics.rs         anonymous metrics for Prometheus, on a port of their own; a Label
                     can't be made from what a request carries
  random.rs          tokens from the OS
  testing.rs         HTTP helpers for tests (body, location, escape, form posts, Set-Cookie)
  e2e/               end-to-end journeys through both sites and the mailer, by a test Browser
locales/*.toml       one file per language, found by build.rs: what it is, its fallbacks,
                     its texts; templates/privacy/{code}.html its privacy notice
templates/           askama pages: base.html and admin/base.html, partials named _*.html
static/style.css     the one stylesheet: tokens first, then base, layout, components, pages
static/favicon.*     the icon, drawn in favicon.svg; `just icons` makes the rest
migrations/          the schema, applied by `innby migrate`: 0001_schema.sql, the whole of
                     it until Innby is live;
                     seed/sample.sql is sample data for dev
bench/               `just bench`: run.sh, and data.sql, an event of 1500 guests
games/               example minigames on an origin of their own (docs/games.md): reaction/
                     in the browser; snake/ and tetris/ with a server that replays the
                     moves to score them; balloon/ the same, its rules in Rust built to
                     WebAssembly (`just games`); lib/ what they share; serve.js serves them
charts/innby/        the Helm chart; deploy/ is one homelab's install of it
docs/                the guides for people, and ropa.md, the GDPR record of processing
```

Every module starts with a `//!` comment saying what it holds, and a `mod.rs` lists its
parts too, so the top of a file is the way into it.

## Rules

- `just test`, `just lint` and `just chart` must pass; CI runs them.
- After changing a query, run `just db prepare` and commit `.sqlx/`.
- Until Innby is live, change the schema in `0001_schema.sql` itself, and break
  compatibility freely: databases are made again. Once it's live, add a migration
  instead. Keep `seed/sample.sql` in step with the schema (`src/e2e/sample.rs` loads it).
- A new text goes in `src/i18n/messages.rs` and the base language's file,
  `locales/en.toml`, at least; placeholders must match English's. Fill them with
  `i18n::fill` or `filled`, never `format!`.
- A handler of a guest's or a group's page goes by `guest.lang` or `joining.lang`, the
  language among those the event offers, not a `Lang` of its own, which knows only the
  site's.
- Admin routes go in the router group of the role they need (`admin/mod.rs`), and
  templates hide what `Role::can_edit()` / `Role::is_admin()` rule out. Handlers are
  named for what they do (`list`, `show`, `create`, `edit`, `revoke`, `delete`), in a
  module named after their tab or page.
- Keep personal data out of logs (route patterns and ids only). New personal data needs
  a retention rule in `db/retention.rs`, and updates to `docs/ropa.md`, the privacy
  notices in `templates/privacy/` and `web::PRIVACY_VERSION`.
- Every module starts with a `//!` comment saying what it holds.
- Tests go in a `tests` module beside the code: inline when short, in a sibling
  `tests.rs` past about 150 lines (`foo.rs` → `foo/tests.rs`). Journeys go in
  `src/e2e/`. There's no `tests/` directory: integration tests there only see a
  library's public API, and Innby is a binary.
- Style with the components in `static/style.css`; change the look through its tokens.
  No inline styles, and no class without a rule (a test checks both ways).
- No JavaScript in Innby; the example games in `games/` are apart from it, on another
  origin. Say Containerfile and Compose, not Dockerfile and Docker Compose. Docs
  and comments are plain British English and say why rather than what. One change per
  commit, imperative subject, a body that says why.
