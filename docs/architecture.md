# How Innby works

Innby is a small RSVP site: guests say whether they're coming to a host's events, about a
hundred guests each. This page describes how it works today. [decisions.md](decisions.md)
explains why it's built this way, and [development.md](development.md) how to work on it.

## Words used here

| Word | Means |
|---|---|
| **Host** | Whoever runs the events, and the admin UI. Several people can be hosts, each with a role. |
| **Guest** | Anyone invited, or who signed up through a group link. Guests have no accounts. |
| **Event** | One party, with a title, a place, a start and perhaps an end. Everything else belongs to an event. |
| **Invitation** | A guest or household the host invites by name. It has a **personal link**. |
| **Group link** | A link the host makes for a group of people, such as a club's chat. Anyone with it can sign up by giving a name, and then gets a personal link of their own. |
| **RSVP**, **answer** | A guest's reply: coming or not, how many, and how their name shows. One row in `rsvp` per invitation or sign-up. |
| **Guest list** | Who's coming and who hasn't answered, shown to every guest. |

## What guests and hosts do

**A guest** opens their personal link (`/r/{link}`). Nothing else is needed: no account,
no password, no e-mail address. Their pages are tabs, each a page of its own under the
link: **Your reply** (the event and the answer), **Guest list**, **Games** when the event
offers any, **Board** when its board is on for guests, and **Your details** (name,
e-mail, this device, their data). There they:

- answer **Attending** or **Can't come**, with how many are coming, and the host's own
  question if the event asks one, by picking one of its choices;
- choose how their name shows on the guest list: in full, as first name and initial, or
  not at all;
- see the guest list, on a page of its own (`/r/{link}/guests`);
- once coming, add the event to their calendar, as a file (`/r/{link}/event.ics`) with
  the event alone in it, never their name or link; an event with no set end lasts three
  hours there;
- optionally add an e-mail address, to get reminders and their link by mail;
- optionally have the device remember them, so the front page leads back here
  ([below](#the-front-page-and-remembered-devices));
- play the minigames the host offers, if any, each with a high score table
  ([games.md](games.md));
- read the event's message board, if the host has one on, and vote posts up or down;
  where it's open to guests, post and reply too, signed with their name as on the guest
  list, and delete their own posts ([below](#the-message-board));
- change any of it until the event, rename themselves, download everything kept about
  them as JSON, or delete it all.

Someone with a **group link** (`/join/{link}`) types their name and party size, answers
the host's question if there is one, may have the device remember them, and is then sent
to a personal link of their own, with a reminder to keep it. The form carries a random
token of its own: sent again, as when the answer to the first was lost on the way, it
leads to that first sign-up rather than making a second.

**The host** works in the admin UI (`innby admin`). Its first page lists the **Events**;
beside it, admins have a **New event** tab, a **Users** tab and a **Languages** tab, for
the languages the site offers guests. Each tab has an icon, and
a host whose role allows only one tab gets no tab bar, just the page. An event has up to
eight tabs:

- **Guests:** the headcount (coming, no answer, not coming), every guest in a table,
  how many of those coming picked each of the host's choices, a CSV export, and *Send
  reminders*. A guest's page edits their name and party size,
  makes them a new link (if theirs leaked), shows their QR code, or deletes them.
- **Invitation links:** many invitations at once, one per line (`Kari & Ola, 2`), each
  link in a field with an *Open* link beside it: held or right-clicked, the browser offers
  to copy it, which a copy button couldn't do without JavaScript.
- **Group links:** the same for groups, each with its link, QR code and sign-ups.
- **QR codes:** the style every QR code of the event is drawn in: the module shape, a
  colour, and a heart, star or emoji in the middle. Each event has its own, so each
  party's printed invitations and posters look its own; a new event starts plain.
- **Games**, when `INNBY_GAMES` lists any: which minigames the event offers, and each
  one's high scores. Turning a game off deletes its scores.
- **Board:** what the event's message board is, and while there is one, its posts, with
  every guest's name in full, to post and reply as a host, or delete any post.
- **Details:** the language the event is written in, its title and description (in that
  language, and optionally in each other), place, start and end, time zone and answer
  deadline; the event as a file; or deleting the event. The host may ask guests one
  question of their own, with two to twenty choices; other guests never see the picks,
  and changing the choices in the event's language clears them, since a pick is a place
  in the list. A new start, end or place is mailed to every guest with a confirmed
  address, whatever their answer, and noted on the pages of those who answered before it
  until they answer again.
- **Access**, for admins: who else may see or run this event alone, by the name they log
  in with, as a viewer or an editor.

**New events** are typed into a form on the New event tab, which also takes group links,
a line each. The form can be filled in for you: from an **event file** (below), or by
*Copy to a new event* on another event's Details tab, for next year's party. Either way
the new event's QR codes keep the style they had. A start more than 30 days ago is
refused, new or changed, since the janitor would delete the event at once: an old event
file imported without a new date, say. An end is optional, and must come after the
start; guests see it beside the start, and without one their calendars show the event as
three hours long.

**Event files** hold an event's details, its group links and its QR codes' style, in
TOML, but never its guests. *Download as a file* on the Details tab saves one; *Import
an event* on the New event tab reads one, uploaded or pasted, into the new event's form,
to check and change before creating it. Like a Kubernetes manifest, a file says what it is:

```toml
apiVersion = "innby/v1"
kind = "Event"
starts = "2026-12-12T19:00"
ends = "2026-12-13T01:00"
time_zone = "Europe/Oslo"
location = "Storgata 1, Oslo"

[title]
nb = "Julebord"
en = "Christmas party"

[qr]
shape = "dots"
color = "#b71c1c"
icon = "star"

[[groups]]
name = "Climbing club"
max_party_size = 2
```

Innby reads `apiVersion` and `kind` first, so a file in a later format, or of another
kind, is refused by name rather than misread. Once Innby is live, a change to the format
gets a new version, read alongside the old one; until then, `innby/v1` changes in place.
`innby event list`, `export` and `import` do the same from the command line.

**Languages:** each language is a file in `locales/`, a module that holds the texts it
has and falls back to others for the rest, in the end to English, the base. Innby comes
with English, its Traditional (British, formal) and Simplified (American) variants,
Bokmål, Nynorsk, and Simplified and Traditional Chinese.
[translating.md](translating.md) has the details. A page's language comes from `?lang=`
or the browser's `Accept-Language`, matched as BCP 47's lookup does (RFC 4647), among
the languages on offer:

- **The site** offers the languages an admin ticks on the **Languages** tab, with one of
  them the default for a browser that asks for none. A file may be there without the
  site using it. Until an admin picks, it offers them all, with English the default.
  `innby serve` reads the choice from the database and keeps it for ten seconds.
- **An event** is written in one language, the host's own when they make it, and may
  have its texts in others; a guest reading a language it lacks gets the nearest it
  has, as the site's texts fall back, and in the end the event's own. Its host can
  narrow its pages to some of the site's languages, in its form. Its own language is
  always offered there, and is those pages' default.
- **The admin UI** stays in every language.

A guest's language is kept with their answer, for their mail: the language of the page
they answered on.

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
| `innby event list`, `export ID`, `import FILE` | by hand | Lists events, prints one as an event file, or makes one from a file | the database |
| `innby hash-password` | by hand | Hashes a host's password, read from stdin | nothing |

Splitting it this way keeps each part to what it needs: the public site has no SMTP
credentials, the admin UI can be kept off the internet, and each workload can have its
own network policy.

```
         public site                                   admin UI
        (innby serve)                                (innby admin)
              │ answers, and mail to send                    │ events, invitations
              ▼                                               ▼
      ┌──────────────────────────── Postgres ───────────────────────────────┐
      │  event · rsvp · invite_group · outbox · admin logins              │
      └──────────────▲────────────────────────────────────▲────────────────┘
                     │ takes mail jobs                     │ deletes what's due
              ┌──────┴───────┐                     ┌───────┴───────┐
              │ innby mailer │──▶ SMTP server       │ innby janitor │
              └──────────────┘                     └───────────────┘
```

Pages are rendered on the server from [askama](https://github.com/askama-rs/askama)
templates, with no JavaScript, and the stylesheet, templates, texts and migrations are all
compiled into the binary. The container image holds nothing but that binary. One
stylesheet styles both sites, built from tokens (colours, type, spacing) and a handful
of components, so the look changes there alone.

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
comes by mail, and later any reminders the host sends, and word when the event's start,
end or place changes. A reminder or a change notice to a guest who's coming carries the
event as a calendar file, as their page offers it.

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
asks a device to remember them, by ticking the box above the answer buttons or on the
group sign-up form (unticked until they do, since a box ticked for them wouldn't be
asking; once answers close, a button does it), the browser keeps their link in one
cookie per invitation (`__Host-innby-{invitation}` over HTTPS, so two sign-ups to one
event through a group's link are both remembered; `HttpOnly`, `SameSite=Lax`,
expiring when the event's data is deleted). The front page then lists each remembered
invitation, those to come or under way first, the soonest first, and those over after
them, under a heading of their own, each with a button back. The cookie gives no access by
itself: pages still read the link from the URL. A link that stops working is dropped
from the cookie the next time the front page sees it, and answering with the box
unticked, *Forget it here* on the front page or *Forget this device* on the guest's
details drops it on demand. The front page reads ten such
cookies at most, and looks them all up in one query. These are the public site's only
cookies; language and colour scheme travel in `?lang=` and `?theme=`.

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

- **A user name and password**, checked against an argon2 hash. The users are the
  built-in admin (`INNBY_ADMIN_USER` and `INNBY_ADMIN_PASSWORD_HASH`; leave the hash out
  to turn it off) and any in `INNBY_ADMIN_USERS`, each `name:role:hash`, as
  `ola:editor:$argon2id$…`, apart by spaces or lines; the role may be `none`; and those
  an admin makes on the **Users** tab, beside Events, with a role and a first password,
  kept in `admin_user`. A login sets an HS512 JWT good for a day and a refresh token good
  for 30 days, both `HttpOnly` cookies; logging out revokes both. Changing a config user,
  their role or their password logs every browser out. A user made on the Users tab has
  their role read on each request, so a new one counts at once, and a new password or
  their deletion logs them out everywhere. An admin can't change or delete themselves
  there.
  Ten attempts in 15 minutes from one address lock that address out for the rest of the
  window, and 100 from all addresses together lock everyone out, which bounds guessing
  from many addresses.
- **Single sign-on** through an OpenID Connect provider such as Authelia, which also says
  what role each host has. [single-sign-on.md](single-sign-on.md) explains the roles and
  the setup.

The JWT uses the registered claim names where there are any: `sub` for the user, and for
a password login `amr: ["pwd"]` (RFC 8176) with no role, since a password user's is read
on each request. A provider's login has `provider: true`, Innby's own, as no standard
names it, and the role the provider gave in `roles` (RFC 9068), which Innby takes at its
word until the token expires.

**Roles:** a *viewer* sees events, headcounts and who's coming, but not links, e-mail
status or the guest export, since those let you act as a guest. An *editor* also runs
events: invitations, group links, guests, reminders, the guest export, the details, the
QR codes' style and the event file. A *creator* also makes new events, by hand, by
copying one or from a file, and deletes the events they made and gives roles for them.
An *admin* does that for every event, makes password users and picks the site's
languages. Each event keeps the name of the host who made it for this, which goes with
the event. Pages check the role, and only show
what it allows.

A host's own role, from the config (`none` there for no role) or the provider, is for
every event. On an event's Access tab, an admin can give anyone a role for that event
alone, viewer or editor, by the name they log in with: a password user's name, or the
provider's `preferred_username` (else `email`, else `sub`). On that event's pages, and its
guests' and groups', a host has the higher of their own role and the event's. Without a
role of their own, they see only the events that give them one; through the provider,
they can't log in until one does. The role for an event is looked up on each request, so
taking it away works at once.

## The message board

An event can have a message board (`event.board`), which its editors set on the Board tab
to one of four states:

| State | Hosts | Guests |
|---|---|---|
| Off | no board; its posts are kept | no board |
| Hidden | read, post, reply, delete | no board |
| Hosts post | read, post, reply, delete | read and vote |
| Open | read, post, reply, delete | read, vote, post and reply |

Threads show the best score first, then the newest; replies the best, then the oldest,
as a conversation reads, nested six deep and lined up below that. A page holds whole
threads until it has 50 posts, and only its posts' text is read from the database; a vote,
post or deletion leads back to the page the thread is on now. A board holds 500 posts
at most, and a thread 100, so the largest page is bounded: the benchmark draws a full
thread of the longest posts, in characters of three bytes each, under the memory budget. A post whose votes add
up below zero is folded away with its replies, one click to open, and nothing else is
moderated: no filters, no automatic hiding beyond that. Guests vote with their personal
link, one vote a post, not on their own; the same vote again takes it back, and nobody
sees who voted how, only the sum.

A post is at most 2000 characters, in a little Markdown (`src/markdown.rs`): bold,
italics, code, links to the web or an e-mail address, lists and quotes, and line breaks
where the writer made them. Everything else is escaped, so a post holds no HTML of its
own; links are marked `nofollow ugc`, and characters that turn text around, with which a
link's address could show back to front, are refused. A guest may post or reply 30 times
an hour, counted
in memory by their RSVP rather than their address, so a household on one network doesn't
share it; hosts aren't counted.

Guests sign as the guest list shows them ("A guest" when it doesn't), and see hosts only
as "The hosts"; hosts see everyone in full. A guest may delete their own posts, and a
host any. A deleted post with replies stays as "[deleted]", so the replies keep their
place; without replies it goes, with any empty posts above it that were only kept for
it. Deleting a guest does the same to their posts, in the database (`board_post_forget`),
and deletes their votes. Forms post and the page comes back, with no JavaScript: a reply
form folds out under each post, and the vote buttons say which way the guest voted.
Everything on the board, posts, replies and votes, is deleted with its event.

## Data and retention

The schema is one commented file,
[`migrations/0001_schema.sql`](../migrations/0001_schema.sql), which changes in place
until Innby is live. In short:

| Table | Holds |
|---|---|
| `event` | the language it's written in and those its pages are offered in, title and description (maps by language code, with that language always there), place, start and any end, time zone, answer deadline, when its time or place last changed, the host's question and its choices, the style of its QR codes |
| `rsvp` | each invitation or sign-up: name, answer, party size and its maximum, link nonce, language, optional e-mail and its status and consent time, privacy notice version, how the name shows, the group it came through, the pick for the host's question |
| `invite_group` | group links: name, maximum party size, link nonce |
| `event_access` | roles for one event: the name a host logs in with, viewer or editor |
| `event_game`, `game_score` | the minigames an event offers, and each guest's best score in each |
| `board_post`, `board_vote` | the event's message board: each post's text, whose it is (a guest, or a host by login name) and what it answers; each guest's vote on a post, up or down |
| `outbox` | mail to send: kind, attempts, next try, when sent, last error |
| `admin_user` | password users an admin makes: name, role, argon2 hash, and a version that a new password bumps |
| `site_languages` | the languages the site offers guests and its default, once an admin has picked: one row |
| `admin_form_sent` | for a day, the token of each admin form that made something, and where it led, so the same form sent twice makes it once |
| `admin_refresh_token`, `admin_revoked_jwt` | keyed hashes of refresh tokens, and logged-out JWTs, until they'd expire |

Deleting an event deletes everything in it, and deleting an answer deletes its mail. Enums
such as the answer are Postgres enum types, and queries are checked against the schema at
compile time (`sqlx`). The database checks what the app checks, too: a title in the event's language,
texts only in the site's languages and within their lengths, no control characters, a
lower-case address, and a sign-up's group in the sign-up's own event.

**Retention**, enforced hourly by `innby janitor`:

| What | Kept |
|---|---|
| An event and everything in it | until 30 days after it starts |
| An e-mail address not confirmed | 48 hours |
| A record of sent mail | 7 days |
| Client addresses for rate limits | in memory only, for the limit's window, 20 000 at most per limit |
| Hosts' refresh tokens (as keyed hashes), logged-out JWTs and the admin forms' tokens | until they expire; a day for a form's |

Nothing is backed up on purpose: nothing should outlive those 30 days.

## Privacy and security

Innby is built to keep as little as it can. [ropa.md](ropa.md) is the GDPR record of
processing, with the legal bases; the privacy notice is at `/privacy` in every language
(`templates/privacy/`).

- **Kept:** a name or label, the answer and party size, the language, the pick for the
  host's question if there is one, and, only if the guest adds one, an e-mail address.
  No notes, no dietary needs (that would be health data), no IP addresses.
- **Shown to others:** the guest list, as each guest chose. Hosts see everything; how much
  of it depends on their role.
- **No third parties:** no CDNs, fonts, analytics or CAPTCHAs; a honeypot field and rate
  limits keep bots out of group sign-up.
- **Headers:** a strict Content Security Policy (no scripts at all, no framing),
  `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, cross-origin
  isolation, `noindex`, and HSTS over HTTPS.
- **Forms:** cross-site form posts are refused using the browser's `Sec-Fetch-Site`
  header (or `Origin` on older browsers, which must be the site's own, scheme and
  host), so no CSRF tokens are needed.
- **Compression:** only the stylesheet and the privacy notice are gzipped. A page that
  holds a link is never compressed, so its size can't give the link away (BREACH).
- **Limits:** requests time out after 15 seconds and their headers after 10, headers are
  capped at 16 KiB and bodies at 2 MB.
- **Containers:** non-root, read-only, no capabilities, no service account token.
- **Metrics:** anonymous, on a port of their own that no route reaches (see below).

## Metrics

With `INNBY_METRICS_LISTEN` set, the site, the admin UI and the mailer each serve
metrics for Prometheus at `/metrics` on that address, a port apart from the pages, which
no route or Ingress leads to and the network policies open only to whoever scrapes them.
The chart's `metrics` turns them on, with a ClusterIP Service each, a
ServiceMonitor for the Prometheus Operator, and a dashboard for Grafana
(`charts/innby/dashboards/innby.json`).

They say how Innby is doing, never who did what:

| Metric | What |
|---|---|
| `innby_http_requests_total` | Requests, by route pattern (`/r/{link}`, never the link), method and status |
| `innby_http_request_duration_seconds` | How long they took, by route pattern and method |
| `innby_mails_total` | Mail jobs sent, failed or dropped (mailer) |
| `innby_outbox_jobs` | Mail not sent: waiting, or given up on (mailer) |
| `innby_db_connections` | The database pool's connections, idle or in use |
| `innby_process_resident_memory_bytes`, `innby_process_start_time_seconds` | The process |
| `innby_build_info` | The release and commit |

Nothing is counted per event or per guest, so no number can single anyone out, and a
label's value can only come from the code, a route's pattern or a status code
(`metrics::Label`): a request's path, with the link in it, its address and its headers
can't reach one. A method outside GET, HEAD and POST counts as `other`, so a client
can't invent label values either. A test sends a guest's request and checks the metrics
hold its route and nothing it carried.

## Running it

The Helm chart in [`charts/innby`](../charts/innby/README.md) runs the workloads above on
any cluster with a Postgres to reach. It can add HTTPRoutes or Ingresses, and standard or
Cilium network policies. [`deploy/`](../deploy/README.md) is one homelab's installation
of it.
