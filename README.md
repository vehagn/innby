# Innby

An RSVP site for one event and about 100 guests. Guests answer through a personal link,
with no account or e-mail address needed, or sign up through a link the host makes for
their group. Every link leads to a guest list, with each name shown as its guest chose.
E-mail reminders are optional. Pages are in Bokmål, Nynorsk, Simplified and Traditional
Chinese, and English in a traditional (British) and a simplified (American) flavour, and
use no JavaScript. The public site's only cookie is one a guest asks for, so that the
front page leads them back to their own page. All guest data is deleted 30 days after
the event.

One Rust binary, run as several workloads:

| Command | What it does |
|---|---|
| `innby serve` | The public site: event page, `/r/{link}` RSVP pages, `/join/{link}` group sign-up, `/lost`, `/privacy` |
| `innby admin` | The host's UI: invitations and group links in batches, QR codes in the event's style, headcount, CSV, reminders |
| `innby mailer` | Sends queued mail (confirmations, links, reminders) |
| `innby janitor` | Deletes data past its retention; run hourly |
| `innby migrate` | Applies the database migrations |
| `innby invite NAME [MAX]` | Creates an invitation and prints its link |
| `innby hash-password` | Hashes the admin password read from stdin |

## Develop

Run `just dev` on the host. It starts the devcontainer with Postgres 18 and Mailpit and
every tool the Justfile needs, then opens a zsh in it. In there:

```bash
just up
```

That builds and runs this checkout: the site on http://localhost:8080, the admin UI on
http://localhost:8081 (user `admin`, password `innby`), and Mailpit's inbox on
http://localhost:8025, all in your own browser. Ctrl-C stops it, and `just dev down` on
the host stops the devcontainer, keeping its database and build cache. `just dev` makes a
`.env` from `.env.example` if there is none.

`just` lists the other recipes. `just test` and `just lint` are what CI runs, `just db`
opens psql, `just db prepare` updates the offline query data in `.sqlx/` after a query
changes, and `just innby` runs the binary's commands, e.g.
`just innby invite "Kari & Ola" 2`.

The shell is zsh with oh-my-zsh and the devcontainers prompt, which shows the git
branch. It completes `just`, `cargo`, `rustup` and `kubectl`, has `k`, `kz`
(`kubectl kustomize`) and `yl` (`yamllint`) aliases, and keeps its history in a volume,
so it survives rebuilds. Vim and Neovim are there too, both with rust-analyzer.
`just dev` mounts those of your `~/.vimrc`, `~/.vim` and `~/.config/nvim` that you have,
read-only, and Neovim falls back to your Vim settings.

VS Code, RustRover and the `devcontainer` CLI open the same devcontainer and forward the
ports themselves.

## Translate

Every text on the public site, the admin UI and in the mails is in
[`locales/`](locales/): `nb.toml` (Bokmål), `nn.toml` (Nynorsk), `en.toml` (British,
posh), `en-us.toml` (American, simplified), `zh-hans.toml` (Simplified Chinese) and
`zh-hant.toml` (Traditional Chinese). The admin UI's texts are under `[admin]`. All files
have the same keys, and placeholders such as `{name}` mark where a value goes; keep them
as they are. Punctuation that differs between scripts, like "：" for ": ", is part of the
texts. `just test` checks that the files agree.

The privacy notice is an HTML template per language, in
[`templates/privacy/`](templates/privacy/). Bump `PRIVACY_VERSION` in
[`src/web/mod.rs`](src/web/mod.rs) when it changes; each answer records the version the
guest saw.

## Run a local copy

```bash
just up
```

That runs the published image with [`compose.yaml`](compose.yaml): Postgres, Mailpit, and
the site, admin UI and mailer. The site is on http://localhost:8080, the admin UI on
http://localhost:8081 (user `admin`, password `innby`), and the mail it sends on
http://localhost:8025. Settings come from `.env.example`, overridden by `.env` if there is
one. `just up v1.0.0` runs a given tag, `just up janitor` applies the retention rules,
and `just down` stops it all. It uses Docker unless `ENGINE` says otherwise, e.g.
`ENGINE=podman`.

To run your own build instead, run `just image` in the dev container, then `just up dev`
on the host, which loads it first.

## Deploy

CI builds `FROM scratch` images holding only the static binary, for amd64 and arm64, and
pushes them to `ghcr.io/vehagn/innby` as one OCI image index. Every push to `main` is
`latest`; `just release 0.3.0` bumps `Cargo.toml`, tags `v0.3.0` and pushes, which
publishes `:v0.3.0`. [`deploy/`](deploy/README.md) has the Kubernetes manifests and the
steps to deploy them.

[PLAN.md](PLAN.md) explains the design, and [docs/ropa.md](docs/ropa.md) is the GDPR
record of processing.
