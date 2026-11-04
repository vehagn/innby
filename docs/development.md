# Working on Innby

Everything you need runs in a dev container, so your machine only needs Docker (or
Podman) and [`just`](https://github.com/casey/just). [architecture.md](architecture.md)
explains how Innby works; read it first if you're new.

## Get going

On your machine:

```bash
just dev
```

That builds and starts the dev container, with Postgres 18 and
[Mailpit](https://mailpit.axllent.org/) beside it, and opens a shell in it. The first
time takes a while. It makes a `.env` from `.env.example` if there's none. In that
shell:

```bash
just up
```

That builds and runs your checkout:

| | |
|---|---|
| http://localhost:8080 | the public site |
| http://localhost:8081 | the admin UI; user `admin`, password `innby` |
| http://localhost:8025 | Mailpit, the inbox for every mail the site sends |

Ctrl-C stops it. `just dev down`, on your machine, stops the dev container; its database
and build cache are kept for next time.

To try it, log in to the admin UI, create an event, make an invitation on its
*Invitation links* tab, and open the link it gives you.

VS Code, RustRover and the `devcontainer` CLI open the same container
(`.devcontainer/`). The shell is zsh with completion for `just`, `cargo` and `kubectl`;
Vim and Neovim have rust-analyzer, and `just dev` mounts your own Vim and Neovim
settings read-only if you have them.

## Commands

`just` lists them all. The ones you'll use:

| Command | Does |
|---|---|
| `just up` | Builds and runs your checkout (above) |
| `just test` | Every test, against the dev database |
| `just lint` | rustfmt, clippy with warnings as errors, the `.sqlx/` query data, and cargo-deny (licences and advisories) |
| `just chart` | Lints and renders the Helm chart, and checks the manifests |
| `just db` | psql on the dev database |
| `just db prepare` | Regenerates `.sqlx/` after you change a query (commit it) |
| `just db reset` | Drops the dev database and rebuilds it from `migrations/` |
| `just innby …` | Runs an `innby` command, e.g. `just innby invite "Kari & Ola" 2` or `just innby hash-password` |

`just test`, `just lint` and `just chart` are what CI runs; all three must pass.

## Find your way in the code

[AGENTS.md](../AGENTS.md) has a one-line map of every module. It's written for coding
agents, but it's the quickest tour for people too. In short:

- `src/web/` is the public site and `src/admin/` the admin UI; each has a `router` that
  lists every page. `src/http/` is what both share.
- `src/db/` has the queries, one module per table. `src/mailer/` and `src/janitor.rs`
  are the other two workloads.
- `templates/` has the pages, `locales/` the texts, `static/style.css` the one
  stylesheet, `migrations/` the schema.
- Every `mod.rs` starts with a comment listing its parts.

## How to…

- **Add a page:** a handler and an askama `Template` struct in the module for its area,
  a template in `templates/` (`templates/admin/` for the admin), its path in
  `src/paths.rs`, and a route in that site's `router`. An admin page goes in the router
  group of the role it needs, and its template hides what the host's role can't use.
- **Add a text:** a field in `src/i18n/messages.rs`, the text in `locales/nb.toml`, and
  its translations; see [translating.md](translating.md). Fill in placeholders with
  `i18n::fill` or `filled`.
- **Change the schema:** add a migration in `migrations/` (never edit one that's been
  released), run `just db prepare`, and commit `.sqlx/`.
- **Add a setting:** read it in `src/config/`, describe it in `.env.example`, and, if the
  cluster needs it, add it to the chart (`values.yaml`, `values.schema.json` and
  `templates/configmap.yaml`).
- **Change the event file format:** keep reading `innby/v1` as it is. A new field that
  older Innbys can ignore safely still needs a new version, since files are read with
  unknown fields refused; add `innby/v2` beside `v1` in `src/admin/event_file.rs`, write
  the new one, and keep reading both.
- **Keep personal data:** keep it out of the logs, give it a retention rule in
  `src/db/retention.rs`, and update [ropa.md](ropa.md) and the privacy notices in
  `templates/privacy/`, bumping `PRIVACY_VERSION` in `src/web/mod.rs`.

## Tests

`just test` runs three kinds:

- **Unit tests**, in a `tests` module beside the code they test: at the bottom of the
  file, or in a sibling `tests.rs` when they're long (`rsvp.rs`'s are in
  `rsvp/tests.rs`).
- **Database tests**, marked `#[sqlx::test]`, each on a fresh, throwaway database.
- **End-to-end journeys** in `src/e2e/`, one file per area. A test `Browser` uses the
  real site and admin UI as a person would: it opens links, fills in fields by their
  labels, presses buttons by their text, keeps cookies, and has a client address of its
  own. Mail lands in an inbox the test reads, and single sign-on runs against a fake
  provider. `src/e2e/mod.rs` explains how to add a journey.

Router-level helpers: `web::testing::Site` for the public site, `admin::testing::Admin`
for a logged-in host of any role, and `crate::testing` for reading responses.

## Releases, images and CI

- **CI** (`.github/workflows/ci.yaml`) runs `just test` and `just lint` against Postgres,
  and `just chart`, on every push and pull request.
- **Images:** on every push, CI builds static binaries for amd64 and arm64
  (`cargo zigbuild`) and `just image` packs each into an image holding nothing else: no
  shell, no CA bundle, no tzdata, all of which are compiled in. They're pushed to
  `ghcr.io/vehagn/innby` as one multi-arch image, tagged `sha-<commit>`, `latest` from
  `main`, and the version for a release.
- **Releasing:** `just release 0.3.0` bumps the version in `Cargo.toml` and the chart,
  makes a signed tag and pushes it. CI then publishes the image as `:v0.3.0` and the
  chart as `oci://ghcr.io/vehagn/charts/innby` version `0.3.0`.
- **A local copy of the published image:** on your machine (not in the dev container),
  `just up` runs it with [`compose.yaml`](../compose.yaml), Postgres and Mailpit, on the
  same ports; `just up v0.3.0` runs a given release, and `just down` stops it. To run
  your own build that way, run `just image` in the dev container first, then
  `just up dev` on your machine.

## Conventions

- `cargo fmt`, and clippy with no warnings.
- No JavaScript: forms and links only, under a strict Content Security Policy.
- Docs, comments and texts are plain British English, and comments say why rather than
  what.
- One change per commit, with an imperative subject and a body that says why.
