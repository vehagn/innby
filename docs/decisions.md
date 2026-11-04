# Decisions

Why Innby is built the way it is. [architecture.md](architecture.md) describes the result.

## What it's for, and not for

Innby is for a host's private events, a hundred guests or so each. It aims to:

- let guests answer with **no account and no e-mail address**, through a personal link
  or a group's link;
- show every guest who's coming, with each name shown as its guest chose;
- keep as little as it can, and delete all of it 30 days after each event;
- work in Bokmål, Nynorsk, two kinds of English and two scripts of Chinese, with no
  JavaScript.

It deliberately leaves out payments and tickets, capacity limits and waitlists, guest
accounts, a JavaScript front end, and a public list of who's coming.

## Choices

| Question | Choice | Why |
|---|---|---|
| Language | Rust | Queries and templates checked at compile time, strong types for states such as an answer, and a static binary for an image with nothing else in it. The dev container makes setup a non-issue. |
| Shape | One binary, several workloads | A single process would cope with the load, but splitting by task keeps credentials apart: only the mailer has SMTP, and the admin UI can be kept off the internet. Real microservices would add RPC, service auth and more images for nothing. Postgres links the workloads, and an outbox table carries mail. |
| Authorisation for guests | The link itself | No accounts to forget, nothing to type. Links are HMAC-signed, so nothing secret is stored, and a leaked one can be replaced. |
| Getting in | Personal links, and group links | Personal links for people the host knows; group links for a club's chat, where people sign up by name. |
| E-mail | Optional, with double opt-in | Most guests never need it; those who do get reminders and their link. |
| Guest list | Behind every link, names as chosen | Guests like to see who's coming; each chooses full name, initial, or hidden. |
| Capacity | No limit, no waitlist | It's a party: count heads, no locking needed. |
| Party size for group sign-up | Set per group, 2 by default | The guest plus one, unless the host says otherwise. |
| Answer deadline | Optional, unset by default | Answers can change until the event. |
| Events | As many as the host likes, made in the admin UI | Each with its own QR code style, so each party's printed invitations look its own. |
| Front page | About Innby, plus a way back to remembered invitations | Event details only on link pages, so the front page says nothing about anyone's party. |
| Retention | 30 days after each event | Long enough for afterthoughts, and then gone. No backups, on purpose. |
| Languages | Six, in TOML files with fallbacks | Plain strings don't need Fluent; each language is a file that names its fallbacks. |
| CSRF | `Sec-Fetch-Site` and `Origin` | No tokens or cookies needed, as Go's `CrossOriginProtection` does it. |
| Admin login | Built-in user, single sign-on, or both | A password for a small install; an OpenID Connect provider, with roles, where there is one. |
| Image | Written by `just image`, no Containerfile | Just the binary in an OCI layout, with reproducible digests and no build tool needed. |
| Deployment | A general Helm chart, published to GHCR | No CRDs needed; routes, policies and CloudNativePG are options. |
| Saving | With a Save button | Saving as you type needs JavaScript. Instead, forms are open rather than folded away, Save is the main button, and the QR code's preview says it shows the saved style. |
| Styling | One stylesheet of tokens and components | The look changes in CSS alone: colours, type and spacing are tokens at the top, and each kind of thing is one class. A test keeps the templates and the stylesheet in step. |
| Events as code | Import and export, not sync | A file per event, versioned like a Kubernetes manifest, read into the form for a new event. Syncing events from a ConfigMap or Secret was considered, but would put addresses in a public repository or need a watcher; a file the host keeps and imports when needed does what's wanted. Never guests, whose names mustn't outlive the event. |
| Mail provider | MailerSend, in the homelab | An existing account in the EU. Any SMTP server works. |

## History

Innby began as `fest`, a single-event RSVP page for one party, and grew from there. The
main steps, oldest first:

1. A dev container, the skeleton, the schema, signed links and the RSVP page.
2. The admin UI, a group sign-up page, optional e-mail with the outbox and mailer, the
   janitor and the privacy notice.
3. The container image and CI; the first deployment, with Cilium policies.
4. Hardening: a strict CSP, timeouts, `cargo deny`, and a first pentest's fixes.
5. Group links in place of the open sign-up page, and the guest list with name choices.
6. Nynorsk, British and American English, and Simplified and Traditional Chinese.
7. Renamed from `fest` to Innby; multi-arch images and a local copy with Compose.
8. The admin UI in every language, a JWT login, styled QR codes, tabs, and end-to-end
   tests.
9. Several events, each deleted 30 days after it starts, and a front page about Innby.
10. Single sign-on with roles.
11. The Helm chart, published to GHCR, with the admin UI under `/admin` if wanted.
12. Restructuring into modules, with a map for people and agents; languages as modules
    with fallbacks.
13. A second pentest: stricter names, capped headers, and a login limit per address.
14. Event files: export, import and copying events, with their group links.
15. The stylesheet as tokens and components, a banner on every page, and sample data.
16. The code organised by page, with handlers named for what they do and every module
    described.
17. A third pentest: a cap on the remembered links the front page reads, control
    characters refused in event texts, a stricter QR colour, and tests dated years
    ahead.
18. QR code styles per event.

`git log` has the details.
