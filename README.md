# fest

An RSVP site for one event and about 100 guests. Guests answer through a personal link,
with no account or e-mail address needed; an open sign-up link and e-mail reminders are
optional. Pages are in Norwegian and English, use no cookies or JavaScript, and all
guest data is deleted 30 days after the event.

One Rust binary, run as several workloads:

| Command | What it does |
|---|---|
| `fest serve` | The public site: event page, `/r/{link}` RSVP pages, `/join/{code}`, `/lost`, `/privacy` |
| `fest admin` | The host's UI: invitations with links and QR codes, headcount, CSV, reminders |
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

## Deploy

CI builds a `FROM scratch` image holding only the static binary and pushes it to
`ghcr.io/vehagn/fest`. [`deploy/`](deploy/README.md) has the Kubernetes manifests.

[PLAN.md](PLAN.md) explains the design, and [docs/ropa.md](docs/ropa.md) is the GDPR
record of processing.
