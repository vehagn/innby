# fest

An RSVP site for one event and about 100 guests. Guests answer through a personal link,
with no account or e-mail address needed, or sign up through a link the host makes for
their group. Every link shows a guest list, with each name shown as its guest chose. E-mail
reminders are optional. Pages are in Bokmål, Nynorsk, and English in a
traditional (British) and a simplified (American) flavour, use no cookies or JavaScript, and all
guest data is deleted 30 days after the event.

One Rust binary, run as several workloads:

| Command | What it does |
|---|---|
| `fest serve` | The public site: event page, `/r/{link}` RSVP pages, `/join/{link}` group sign-up, `/lost`, `/privacy` |
| `fest admin` | The host's UI: invitations and group links in batches, QR codes, headcount, CSV, reminders |
| `fest mailer` | Sends queued mail (confirmations, links, reminders) |
| `fest janitor` | Deletes data past its retention; run hourly |
| `fest migrate` | Applies the database migrations |
| `fest invite NAME [MAX]` | Creates an invitation and prints its link |
| `fest hash-password` | Hashes the admin password read from stdin |

## Develop

Open the repository in the devcontainer (VS Code, RustRover or the `devcontainer` CLI).
It comes with Postgres 18 and Mailpit, and every tool the Justfile needs.

```bash
cp .env.example .env
```

```bash
just run
```

The site is on port 8080, the admin UI on 8081 (user `admin`, password `fest`), and
Mailpit's inbox on 8025. `just` lists the other recipes; `just test` and `just lint` are
what CI runs.

## Translate

Every text on the public site and in the mails is in [`locales/`](locales/): `nb.toml`
(Bokmål), `nn.toml` (Nynorsk), `en.toml` (British, posh) and `en-us.toml` (American,
simplified). All files have the same keys; keep placeholders like `{name}` as they are.
`just test` checks that the files agree. The privacy notice is
an HTML template per language, in [`templates/privacy/`](templates/privacy/). The admin
UI is English only.

## Deploy

CI builds a `FROM scratch` image holding only the static binary and pushes it to
`ghcr.io/vehagn/fest`. [`deploy/`](deploy/README.md) has the Kubernetes manifests.

[PLAN.md](PLAN.md) explains the design, and [docs/ropa.md](docs/ropa.md) is the GDPR
record of processing.
