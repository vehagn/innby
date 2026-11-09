# Innby

An RSVP site for a host's events that keeps as little about its guests as it can.

- **Guests need nothing but a link.** No account, no password, no e-mail address. They
  answer through a personal invitation link, or sign up by name through a link the host
  makes for a group, such as a club's chat. Once they're coming, they can add the event
  to their calendar.
- **Everyone sees who's coming**, each name shown as its guest chose: in full, as first
  name and initial, or not at all.
- **E-mail is optional**, for reminders and a guest's own link, with double opt-in.
- **Everything goes 30 days after each event.** No tracking, no third parties, and on the
  public site no cookies but those a guest asks for.
- **Six languages:** Bokmål, Nynorsk, British and American English, and Simplified and
  Traditional Chinese. No JavaScript.
- **For the host:** an admin UI for events, invitations and group links in batches, QR
  codes, the headcount and reminders, with a built-in login or single sign-on. An event
  and its group links go into a versioned TOML file, to keep, or to import as next
  year's.
- **Minigames, if the host wants them:** any web page on an origin of its own can be one,
  with a high score table on each guest's page.

It's one Rust binary on Postgres, run as a few small workloads, with a Helm chart for
Kubernetes.

## Try it

You need Docker (or Podman, with `ENGINE=podman`) and
[`just`](https://github.com/casey/just).

```bash
just up
```

That runs the published image with Postgres and a mail catcher. Then open:

- http://localhost:8081, the admin UI (user `admin`, password `innby`): create an event
  on the *New event* tab, then invitations on its *Invitation links* tab;
- an invitation's link, to answer as a guest;
- http://localhost:8025, to read the mail Innby sends.

`just down` stops it.

## Documentation

| Read | To |
|---|---|
| [How Innby works](docs/architecture.md) | understand it: guests, hosts, links, e-mail, data and privacy |
| [Working on Innby](docs/development.md) | develop: the dev container, commands, tests and releases |
| [Translating](docs/translating.md) | change texts or add a language |
| [The Helm chart](charts/innby/README.md) | run it on Kubernetes |
| [Single sign-on](docs/single-sign-on.md) | log hosts in through Authelia or another OpenID Connect provider |
| [Minigames](docs/games.md) | offer games with high scores, or write one |
| [Decisions](docs/decisions.md) | see why it's built this way, and how it got here |
| [Record of processing](docs/ropa.md) | the GDPR record, to fill in before going live |
| [AGENTS.md](AGENTS.md) | find your way in the code (written for coding agents, useful to people) |

[`deploy/`](deploy/README.md) is one homelab's installation, with Argo CD.

## Licence

MIT; see [LICENCE](LICENCE).
