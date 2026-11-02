# Innby

An RSVP site for one event and about 100 guests. Guests answer through a personal link,
with no account or e-mail address needed, or sign up through a link the host makes for
their group. Every link shows a guest list, with each name shown as its guest chose. E-mail
reminders are optional. Pages are in Bokmål, Nynorsk, Simplified and
Traditional Chinese, and English in a traditional (British) and a simplified (American)
flavour, use no cookies or JavaScript, and all
guest data is deleted 30 days after the event.

One Rust binary, run as several workloads:

| Command | What it does |
|---|---|
| `innby serve` | The public site: event page, `/r/{link}` RSVP pages, `/join/{link}` group sign-up, `/lost`, `/privacy` |
| `innby admin` | The host's UI: invitations and group links in batches, QR codes, headcount, CSV, reminders |
| `innby mailer` | Sends queued mail (confirmations, links, reminders) |
| `innby janitor` | Deletes data past its retention; run hourly |
| `innby migrate` | Applies the database migrations |
| `innby invite NAME [MAX]` | Creates an invitation and prints its link |
| `innby hash-password` | Hashes the admin password read from stdin |

## Develop

Open the repository in the devcontainer (VS Code, RustRover or the `devcontainer` CLI).
It comes with Postgres 18 and Mailpit, and every tool the Justfile needs.

```bash
cp .env.example .env
```

```bash
just run
```

The site is on port 8080, the admin UI on 8081 (user `admin`, password `innby`), and
Mailpit's inbox on 8025. `just` lists the other recipes; `just test` and `just lint` are
what CI runs.

## Translate

Every text on the public site and in the mails is in [`locales/`](locales/): `nb.toml`
(Bokmål), `nn.toml` (Nynorsk), `en.toml` (British, posh), `en-us.toml` (American,
simplified), `zh-hans.toml` (Simplified Chinese) and `zh-hant.toml` (Traditional
Chinese). Punctuation that differs between scripts, like "：" for ": ", is part of the
texts, with placeholders such as `{name}` where a value goes. All files have the same keys; keep placeholders like `{name}` as they are.
`just test` checks that the files agree. The privacy notice is
an HTML template per language, in [`templates/privacy/`](templates/privacy/). The admin
UI is English only.

## Run a local copy

```bash
just up
```

That runs the published image with [`compose.yaml`](compose.yaml): Postgres, Mailpit, and
the site, admin UI and mailer. The site is on http://localhost:8080, the admin UI on
http://localhost:8081 (user `admin`, password `innby`), and the mail it sends on
http://localhost:8025. Settings come from `.env.example`, overridden by `.env` if there is
one. `just up v1.0.0` runs a given tag, `just up-janitor` applies the retention rules,
and `just down` stops it all. It uses `docker compose` unless `COMPOSE` says otherwise,
e.g. `COMPOSE="podman compose"`.

## Deploy

CI builds `FROM scratch` images holding only the static binary, for amd64 and arm64, and
pushes them to `ghcr.io/vehagn/innby` as one OCI image index. [`deploy/`](deploy/README.md) has the Kubernetes manifests.

[PLAN.md](PLAN.md) explains the design, and [docs/ropa.md](docs/ropa.md) is the GDPR
record of processing.
