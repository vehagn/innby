# How Innby works

Innby is a small RSVP site: guests say whether they're coming to a host's events, about a
hundred guests each. This page describes how it works today. [decisions.md](decisions.md)
explains why it's built this way, and [development.md](development.md) how to work on it.

## Words used here

| Word | Means |
|---|---|
| **Host** | Whoever runs the events, and the admin UI. Several people can be hosts, each with a role. |
| **Guest** | Anyone invited, or who signed up through a group link. Guests have no accounts. |
| **Event** | One party, with a title, a place and a start. Everything else belongs to an event. |
| **Invitation** | A guest or household the host invites by name. It has a **personal link**. |
| **Group link** | A link the host makes for a group of people, such as a club's chat. Anyone with it can sign up by giving a name, and then gets a personal link of their own. |
| **RSVP**, **answer** | A guest's reply: coming or not, how many, and how their name shows. One row in `rsvp` per invitation or sign-up. |
| **Guest list** | Who's coming and who hasn't answered, shown to every guest. |

## What guests and hosts do

**A guest** opens their personal link (`/r/{link}`). Nothing else is needed: no account,
no password, no e-mail address. On that page they:

- answer **Attending** or **Can't come**, with how many are coming;
- choose how their name shows on the guest list: in full, as first name and initial, or
  not at all;
- see the guest list, on a page of its own (`/r/{link}/guests`);
- optionally add an e-mail address, to get reminders and their link by mail;
- optionally have the device remember them, so the front page leads back here;
- change any of it until the event, rename themselves, download everything kept about
  them as JSON, or delete it all.

Someone with a **group link** (`/join/{link}`) types their name and party size, and is
then sent to a personal link of their own, with a reminder to keep it.

**The host** works in the admin UI (`innby admin`). An event has four tabs:

- **Guests:** the headcount (coming, no answer, not coming), every guest in a table,
  a CSV export, and *Send reminders*. A guest's page edits their name and party size,
  makes them a new link (if theirs leaked), shows their QR code, or deletes them.
- **Invitation links:** many invitations at once, one per line (`Kari & Ola, 2`), with
  their links to copy.
- **Group links:** the same for groups, each with its link, QR code and sign-ups.
- **Details:** the event's title and description (in Bokmål, and optionally in each other
  language), place, start, time zone and answer deadline; or deleting the event.

Besides the events, there's one QR code style for every code: the module shape, a colour,
and a heart, star or emoji in the middle.

**Languages:** every page is in Bokmål, Nynorsk, British English (formal), American
English (plain), and Simplified and Traditional Chinese, from the browser's languages or
`?lang=`. [translating.md](translating.md) has the details.

## One binary, several workloads

Innby is one Rust binary, `innby`, run as separate workloads. They don't call each other;
Postgres links them.

| Command | Runs as | Does | Needs |
|---|---|---|---|
| `innby serve` | Deployment, 2 replicas | The public site, on port 8080 | the database, the link key |
| `innby admin` | Deployment | The admin UI, on port 8081 | the database, the link key, the admin's secrets |
| `innby mailer` | Deployment, 1 replica | Sends the mail queued in the database, over SMTP | the database, the link key, SMTP |
| `innby janitor` | CronJob, hourly | Deletes what the retention rules say is due | the database |
| `innby migrate` | init container of `serve` | Brings the database schema up to date | the database |
| `innby invite [--event ID] NAME [MAX]` | by hand | Makes an invitation and prints its link | the database, the link key |
| `innby hash-password` | by hand | Hashes the admin password, read from stdin | nothing |

Splitting it this way keeps each part to what it needs: the public site has no SMTP
credentials, the admin UI can be kept off the internet, and each workload can have its
own network policy.

```
         public site                                   admin UI
        (innby serve)                                (innby admin)
              │ answers, and mail to send                    │ events, invitations
              ▼                                               ▼
      ┌──────────────────────────── Postgres ───────────────────────────────┐
      │  event · rsvp · invite_group · outbox · qr_style · admin logins     │
      └──────────────▲────────────────────────────────────▲────────────────┘
                     │ takes mail jobs                     │ deletes what's due
              ┌──────┴───────┐                     ┌───────┴───────┐
              │ innby mailer │──▶ SMTP server       │ innby janitor │
              └──────────────┘                     └───────────────┘
```

Pages are rendered on the server from [askama](https://github.com/askama-rs/askama)
templates, with no JavaScript, and the stylesheet, templates, texts and migrations are all
compiled into the binary. The container image holds nothing but that binary.

## Links are the keys

A guest's link *is* their access: whoever has it can answer for them. So links are hard to
guess, and nothing secret about them is stored.

```
/r/{id}.{sig}
  id  = base64url(rsvp.id)                                              22 characters
  sig = base64url(HMAC-SHA256(INNBY_LINK_KEY, "rsvp" ‖ id ‖ link_nonce)[:16])   128 bits
```

- The server recomputes the signature and compares it in constant time. A copy of the
  database gives nobody a working link, yet the host can show a guest's link again at any
  time.
- **A new link** for a guest bumps `link_nonce`, which makes the old one stop working.
- **Group links** (`/join/…`) are signed the same way, with their own purpose.
- **E-mail confirmation links** (`/e/…`) also sign the address and an expiry 48 hours
  ahead, so they stop working when the address changes.
- `INNBY_LINK_KEY` signs them all. Changing it breaks every link.

Because links sit in URLs, the site takes care with them: link pages are `no-store`, no
page sends a `Referer`, nothing is loaded from other sites, logs record the route pattern
(`/r/{link}`) and never the path, and the guest list is a page of its own, so a chat
app's link preview never shows another guest's name.

## E-mail, optional

A guest can add an address on their page, ticking an unticked consent box. The address is
saved unconfirmed, and a confirmation mail is queued in the same transaction (the
**outbox**). The confirmation mail contains nothing a guest typed, so nobody can use it to
send a message to someone else. The guest confirms by pressing a button on the page the
mail links to: mail scanners that open links can't confirm for them. Then their link
comes by mail, and later any reminders the host sends.

- Only a *confirmed* address must be unique within an event. If someone else already has
  it, that person gets a mail pointing to `/lost`, and the page looks the same either way,
  so it never reveals whose address is whose.
- `/lost` mails a guest their links for every event where the address is confirmed. It
  answers the same whether or not the address is known, and looks it up after answering,
  so its timing doesn't tell either.
- Mail is in the language the guest used when adding the address.
- The **mailer** checks the outbox every 5 seconds, taking jobs with `FOR UPDATE SKIP
  LOCKED`, re-reads the guest (skipping the mail if the address changed), signs a fresh
  link, and sends it. A failed send is retried with backoff, from a minute up to six
  hours, ten times at most. SMTP errors go into the outbox row, not the logs, since they
  often quote the address.
- Rate limits: adding an address and `/lost` allow 5 mails an hour per client address and
  100 a day in all.

## The front page and remembered devices

`/` is about Innby, not any one event: event details show only on link pages. If a guest
asks a device to remember them, the browser keeps their link in one cookie per event
(`__Host-innby-{event}` over HTTPS; `HttpOnly`, `SameSite=Lax`, expiring when the
event's data is deleted). The front page then lists each remembered invitation, the
soonest event first, with a button back. The cookie gives no access by itself: pages
still read the link from the URL. A link that stops working is dropped from the cookie
the next time the front page sees it, and *Forget this device* drops it on demand. These
are the public site's only cookies; language and colour scheme travel in `?lang=` and
`?theme=`.

## The guest list

Every link page has a *See who's coming* button to the guest list. It lists everyone
invited who hasn't declined, under **Coming** and **Not answered yet**, each name as its
guest chose:

| Choice | Shown as |
|---|---|
| Full name | Kari Nordmann |
| First name and initial | Kari N. (or the family name alone in Chinese, Japanese or Korean script: 王**) |
| Hidden | not listed; the list ends "+ 1 more", counting those a hidden guest brings too |

Until a guest chooses, their name shows as first name and initial before they answer,
and in full once they're coming. A guest who brings others shows as "Kari Nordmann +1".
Each heading's total counts hidden guests too.

Names are letters in any script, with their accents, spaces, and the punctuation names
use (`- ' . , &` and middle dots); a group's name may also have digits. Anything else,
such as `<`, emoji or digits in a person's name, is refused with a message saying what's
allowed.

## The admin UI

**Where:** on a hostname of its own, or under a path of the site's, such as `/admin`
(`INNBY_ADMIN_URL`). Under a path, its pages, links and cookies stay under that path.

**Logging in**, either way or both:

- **The built-in user**, checked against an argon2 hash (`INNBY_ADMIN_PASSWORD_HASH`;
  leave it out to turn this off). A login sets an HS512 JWT good for a day and a refresh
  token good for 30 days, both `HttpOnly` cookies; logging out revokes both. Ten attempts
  in 15 minutes from one address lock that address out for the rest of the window, and
  100 from all addresses together lock everyone out, which bounds guessing from many
  addresses. The built-in user is an admin.
- **Single sign-on** through an OpenID Connect provider such as Authelia, which also says
  what role each host has. [single-sign-on.md](single-sign-on.md) explains the roles and
  the setup.

**Roles:** a *viewer* sees events, headcounts and who's coming, but not links, e-mail
status or the export, since those let you act as a guest. An *editor* also runs events:
invitations, group links, guests, reminders, the export and the details. An *admin* also
creates and deletes events and styles the QR codes. Pages check the role, and only show
what it allows.

## Data and retention

The schema is in [`migrations/`](../migrations/). In short:

| Table | Holds |
|---|---|
| `event` | title and description (Bokmål, plus optional columns per language), place, start, time zone, answer deadline |
| `rsvp` | each invitation or sign-up: name, answer, party size and its maximum, link nonce, language, optional e-mail and its status and consent time, privacy notice version, how the name shows, the group it came through |
| `invite_group` | group links: name, maximum party size, link nonce |
| `outbox` | mail to send: kind, attempts, next try, when sent, last error |
| `qr_style` | the one QR code style |
| `admin_refresh_token`, `admin_revoked_jwt` | keyed hashes of refresh tokens, and logged-out JWTs, until they'd expire |

Deleting an event deletes everything in it, and deleting an answer deletes its mail. Enums
such as the answer are Postgres enum types, and queries are checked against the schema at
compile time (`sqlx`).

**Retention**, enforced hourly by `innby janitor`:

| What | Kept |
|---|---|
| An event and everything in it | until 30 days after it starts |
| An e-mail address not confirmed | 48 hours |
| A record of sent mail | 7 days |
| Client addresses for rate limits | in memory only, for the limit's window |

Nothing is backed up on purpose: nothing should outlive those 30 days.

## Privacy and security

Innby is built to keep as little as it can. [ropa.md](ropa.md) is the GDPR record of
processing, with the legal bases; the privacy notice is at `/privacy` in every language
(`templates/privacy/`).

- **Kept:** a name or label, the answer and party size, the language, and, only if the
  guest adds one, an e-mail address. No notes, no dietary needs (that would be health
  data), no IP addresses.
- **Shown to others:** the guest list, as each guest chose. Hosts see everything; how much
  of it depends on their role.
- **No third parties:** no CDNs, fonts, analytics or CAPTCHAs; a honeypot field and rate
  limits keep bots out of group sign-up.
- **Headers:** a strict Content Security Policy (no scripts at all, no framing),
  `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, cross-origin
  isolation, `noindex`, and HSTS over HTTPS.
- **Forms:** cross-site form posts are refused using the browser's `Sec-Fetch-Site`
  header (or `Origin` on older browsers), so no CSRF tokens are needed.
- **Limits:** requests time out after 15 seconds and their headers after 10, headers are
  capped at 16 KiB and bodies at 2 MB.
- **Containers:** non-root, read-only, no capabilities, no service account token.

## Running it

The Helm chart in [`charts/innby`](../charts/innby/README.md) runs the workloads above on
any cluster with a Postgres to reach. It can add HTTPRoutes or Ingresses, and standard or
Cilium network policies. [`deploy/`](../deploy/README.md) is one homelab's installation
of it.
