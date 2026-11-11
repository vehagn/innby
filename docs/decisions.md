# Decisions

Why Innby is built the way it is. [architecture.md](architecture.md) describes the result.

## What it's for, and not for

Innby is for a host's private events, a hundred guests or so each. It aims to:

- let guests answer with **no account and no e-mail address**, through a personal link
  or a group's link;
- show every guest who's coming, with each name shown as its guest chose;
- keep as little as it can, and delete all of it 30 days after each event;
- work in whichever languages it has a file for, with no JavaScript.

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
| Languages | A TOML file each, with fallbacks, found when built | Plain strings don't need Fluent; each language is a module that holds the texts it has and names where the rest come from, in the end English, the base, which a translator into anything can read. Adding one is adding its file: no list of languages in the code, the schema or the templates, so a translator needn't touch Rust. |
| Which languages are offered | The admin picks the site's, a host narrows their event's | A file being there doesn't mean a host wants it: a formal party needn't offer English (Simplified), and an event written in two languages needn't offer seven. The standards cover naming and matching, which Innby follows: BCP 47 tags, and RFC 4647's lookup of the browser's `Accept-Language` among the languages on offer. Which ones are on offer is each site's to say, so that's a setting in the admin UI, kept in the database, where the event's is too. |
| CSRF | `Sec-Fetch-Site` and `Origin` | No tokens or cookies needed, as Go's `CrossOriginProtection` does it. |
| Admin login | Passwords, single sign-on, or both | A user or a few with passwords and roles in the config, for a small install, and more that an admin makes in the admin UI, without a restart; an OpenID Connect provider, with roles, where there is one. Config users stay read-only in the UI, so the config can always let someone in. |
| Roles for one event | In the database, given on the event's Access tab | Events come and go in the admin UI, so neither the config nor the provider can name them. A role by login name, deleted with the event, lets a helper run one party without seeing the others. Admins give it, as they make and delete events. |
| Image | Written by `just image`, no Containerfile | Just the binary in an OCI layout, with reproducible digests and no build tool needed. The example games' image is the same idea: one layer, `games/`, added to a distroless Node.js base by digest, with skopeo and jq. |
| Minigames | Any web page on an origin of its own, with a score token | Plug-and-play: Innby lists the games, links to them, and keeps scores through a small API, and the games stay apart from it. On the site's own origin a game's JavaScript could read a guest's link and act as them, so a game gets a token that only saves that guest's scores in that game. Only each guest's best is kept, shown by their name as on the guest list. Using the remember-me cookie to know the player was considered, but would have needed the games on the site's origin. |
| Scores that can't be made up | The game's own server works the score out, and sends it with the game's key | A browser can send any score. Limits, server-timed rounds and signed clients only raise the bar; a server that replays the player's moves with the game's own rules, one file shared with the page, decides the score itself, and Innby, which knows no game's rules, takes scores for such a game only from it. Keys are derived from the link key, so there's no new secret to keep, and the internal port is reachable only from the games' pods. |
| Allocator | mimalloc, purging freed memory at once | The images are static musl binaries, and musl's own allocator keeps what it frees: benchmarked, `innby serve` held 113 MiB and `innby admin` 171 MiB after load, over the chart's 64 MiB, and never shrank. With mimalloc purging at once they stayed under 20 and 34 MiB, as fast or faster (the 1500-guest list went from 990 to 1350 requests a second). jemalloc did about as well, but its arm64 builds must be told the system's page size. `just bench` checks it on every release. |
| Deployment | A general Helm chart, published to GHCR | No CRDs needed; routes, policies and CloudNativePG are options. |
| Saving | With a Save button | Saving as you type needs JavaScript. Instead, forms are open rather than folded away, but for the parts few need (other languages, the host's own question, reading an event file), Save is the main button, and the QR code's preview says it shows the saved style. |
| The host's own question | One per event, with set choices | Hosts want to know who brings what, or who stays for dinner. Free text would bring back the notes Innby leaves out, and whatever guests chose to write in them, health among it; a pick among the host's choices can't. The form asks hosts not to ask about health, diet or faith. Other guests never see the picks. |
| Calendar | A file from Innby, for guests who are coming | A link to a calendar service would send the event to that service; an iCalendar file reaches any calendar with no third party and no JavaScript. It holds the event alone, never the guest's name or link, which would let whoever sees their calendar answer for them. An end is optional, so without one the entry lasts three hours. |
| Compression | Gzip for the stylesheet and the privacy notice alone | Every other page holds a link, a guest's key, and many show what someone typed, such as a name signed up through a group link. Compressing both together lets whoever chooses that text and sees the response's size guess the link (BREACH). The stylesheet is most of what a first visit fetches, so gzipping it, once, gets most of the gain. |
| Copying a link | A link beside its field | A copy button needs JavaScript. Browsers already offer to copy a link that's held or right-clicked, so each link field has an Open link beside it, and a hint saying so. |
| Styling | One stylesheet of tokens and components | The look changes in CSS alone: colours, type and spacing are tokens at the top, and each kind of thing is one class. A test keeps the templates and the stylesheet in step. |
| Events as code | Import and export, not sync | A file per event, versioned like a Kubernetes manifest, read into the form for a new event. Syncing events from a ConfigMap or Secret was considered, but would put addresses in a public repository or need a watcher; a file the host keeps and imports when needed does what's wanted. Never guests, whose names mustn't outlive the event. |
| Mail provider | Any SMTP server, named on `/privacy` by a setting | The notice must name who handles guests' addresses, so `INNBY_MAIL_PROCESSOR` says it, and without it the notice says the host's own server. The homelab uses MailerSend, an existing account in the EU. |
| Compatibility before going live | None kept | Until Innby is live, the one install is a test, so the schema changes in place in its one file and databases are made again, and the event file's format changes within `innby/v1`. Migrations and new format versions come once it's live. |

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
19. Before going live: the migrations squashed into one schema, event texts as maps by
    language, and the database checking what the app checks.
20. More password users with roles, roles for one event, and a benchmark with a memory
    budget, run on every release, which led to mimalloc.
21. Remembering a device with a box on the answer, and minigames with high scores, with
    Snake, Tetris and Balloon (in Rust) scored by their own server.
22. Links to copy without JavaScript, remembering a device on group sign-up, a New event
    tab, icons on the admin's tabs, an event's end, and the event for guests' calendars;
    the schema in one file again until going live.
23. Telling guests of a new time or place, the host's own question, the calendar file in
    reminders, and gzip where it's safe.
24. Guests' tabs, events in the host's own language, forms that are safe to send twice,
    and a device that remembers each invitation; the admin token's claims named as the
    standards name them.
25. The site's handlers ending in an error with `?`, as the admin's do, and what was
    said twice said once. A fourth pentest: a cap on the client addresses a rate limit
    remembers, the scheme in the `Origin` check, no caching of an unknown path under a
    link, user names with a letter or digit, and the janitor sweeping expired tokens.
26. Times on the 24-hour clock, a trail of where an admin page is, and languages as
    files alone: a new one is its file in `locales/`, with no list to change.
27. English as the base language, with its Traditional and Simplified variants as
    modules beside the others; a score of how far each is translated, in `innby
    languages` and the benchmark's results; and the admin choosing which languages the
    site offers, each host which their event does.

`git log` has the details.
