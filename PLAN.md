# Innby: design

How Innby is built and why. It's a small, GDPR-compliant site where guests say whether they're coming to a host's events, about 100 guests each. **Guests need no account and no e-mail address.** They respond through a personal invite link, or through a group link where they only give a name. Everyone with a link sees a guest list, with each name shown as its guest chose. E-mail is an optional extra for reminders.

## 1. Goals and non-goals

**Goals**

- Three ways in, all ending at the same personal RSVP page:
  1. **Personal invite link.** You create an invitation per guest or household and share the link or QR code however you like (SMS, Messenger, paper). The guest opens it and taps *Attending* / *Can't come*.
  2. **Group link.** You make an unlisted sign-up link per group of people (`/join/{link}`), e.g. "Climbing club", for its group chat. The guest types a name and how many are coming, taps *I'm coming*, then gets their own private link to change or cancel later.
  3. **Optional e-mail.** From their RSVP page a guest can add an e-mail address (with double opt-in) to get reminders and their link by mail.
- Guests can change their answer, party size and name at any time, and can export or delete their data.
- A guest list behind every personal and group link (§5.6), showing who's coming and who hasn't answered. Each guest chooses whether others see their full name, their first name and initial, or nothing.
- Pages in **Bokmål, Nynorsk, British English ("traditional", a touch posh), American English ("simplified", short and plain), and Simplified and Traditional Chinese**, chosen from the browser's `Accept-Language`, with a switch through `?lang=nb|nn|en|en-us|zh-hans|zh-hant` (no cookie). English is British unless the browser asks for `en-US`; Chinese is Simplified unless it's Traditional or from Taiwan, Hong Kong or Macau.
- **Several events:** you create events in the admin UI, and every invitation and group link belongs to one. An event and everything in it is deleted 30 days after it starts.
- An admin UI for you: create invitations and group links in batches, revoke them, see the headcount, export CSV.
- Built for GDPR from the start: minimal data, clear retention, automatic deletion, no third-party assets, and on the public site no cookies but those a guest asks for, one per invitation (§4).
- A tiny OCI image that holds nothing but the binary, written as plain files by `just image` (no Containerfile, no build tool).
- A **devcontainer** with every tool needed to develop, build the image and deploy, so the host needs nothing but a container runtime (§9, §10).
- Runs on the existing Talos cluster, deployed the same way as the other apps in `vehagn/homelab` (Argo CD and Kustomize).

**Non-goals**

- Payments or ticket QR codes.
- A capacity limit or waitlist. We just count heads.
- Guest accounts or passwords.
- A JavaScript front end. Pages are rendered on the server and work without JS.
- A public list of who's coming.

## 2. Language: Rust

The devcontainer takes away Go's main advantage, that it's usually installed already. Rust gives compile-time checked SQL and templates, strong types for states such as `Response::{Pending, Attending, Declined}`, and a static musl binary that suits a `FROM scratch` image.

| Concern | Crate |
|---|---|
| HTTP | `axum` on `tokio`; `tower-http` for security headers and request tracing |
| Templates | `askama`: compiled into the binary and type-checked at build time |
| Texts | `toml` for `locales/*.toml`, compiled into the binary |
| Postgres | `sqlx` (postgres, rustls, migrate): queries checked against the schema at compile time, with offline `.sqlx/` data so builds don't need a database. `sqlx::migrate!()` embeds the migrations and takes an advisory lock, so there's no custom migrator. |
| Mail | `lettre` (SMTP over rustls) |
| Signed links | `hmac` + `sha2`, `base64` |
| IDs and time | `uuid` (v7), `chrono` + `chrono-tz`: the tz database is compiled in, so the image needs no `/usr/share/zoneinfo` |
| CLI | `clap` (derive) for the subcommands |
| Logging | `tracing` + `tracing-subscriber` (JSON output) |
| Rate limiting | A small fixed-window counter per IP (IPv6 per /64), in memory; no crate needed |
| Admin | `argon2` (password hash), HS512 JWTs with `hmac`, `qrcode` (the module grid, drawn as SVG by `admin/qr.rs`). The CSV is written by hand. |
| Single sign-on | `ring` (ID token signatures), `hyper` client with `tokio-rustls`, `url`, `subtle`: all already in the build through sqlx and lettre, rather than an OIDC crate and reqwest |
| TLS roots | `rustls` with `webpki-roots`, through sqlx's and lettre's features: CA roots compiled in, so the image needs no `/etc/ssl` |

- **CSRF:** axum has no built-in equivalent of Go's `CrossOriginProtection`, so a small middleware does the same thing. For unsafe methods it requires `Sec-Fetch-Site` to be `same-origin` or `none`, or (for older browsers) the `Origin` host to match the `Host` header. This needs no cookies, and works unchanged for both the public and the admin hostname.
- **i18n:** six languages still don't need Fluent. Each language's texts, mails and the admin UI included, live in `locales/{lang}.toml`, compiled into the binary and read into one `Messages` struct that the templates use. A missing or unknown key, or a placeholder like `{name}` that one language has and another lacks, fails the tests and stops the program at start. The privacy notice is a template per language, since it's a document rather than strings.
- **Cross-compiling:** the devcontainer runs arm64 on your Mac, the cluster is amd64, and the image is published for both. `cargo zigbuild --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl` (with Zig as the linker) handles that, including the C code inside `ring`.

## 3. Architecture: one binary, several workloads

For 100 guests one process would do. These were the options:

| Option | What it looks like | Verdict |
|---|---|---|
| **A. Plain monolith** | One binary, one Deployment. It sends mail inline. | Simplest, but the public pod then holds SMTP credentials and the admin UI is on the same listener. |
| **B. One binary, several workloads** ✅ | One image. Subcommands run as separate Kubernetes workloads. They share no API with each other; Postgres links them, and an **outbox table** carries mail jobs. | Feels like microservices where it helps (each workload has its own permissions and network policy and fails on its own) and costs nothing in RPC, service auth or extra images. |
| **C. Real microservices** | `frontend`, `rsvp-api`, `mailer`, `admin` and `janitor`, talking over HTTP/gRPC or NATS. | You'd still need an outbox for "save and send" to stay consistent, plus mTLS between services and N images and manifests. Each extra service that holds personal data is also another place erasure has to reach. Pure overhead at this size. |

**Chosen: B.** The binary `innby` has these subcommands:

| Subcommand | Kubernetes workload | Responsibility | Needs |
|---|---|---|---|
| `innby serve` | Deployment (public) | Event page, `/join/{link}`, RSVP pages, e-mail confirm, privacy notice | DB, link key. Reachable from the **external** gateway. |
| `innby admin` | Deployment (internal) | Invitations, group links, QR codes, headcount, CSV, reminders | DB, link key, admin password hash and JWT secret. Reachable from the **internal** gateway only. |
| `innby mailer` | Deployment (1 replica) | Processes the outbox and sends e-mail through MailerSend | DB, link key, SMTP credentials, outbound SMTP. No ingress. |
| `innby janitor` | CronJob (hourly) | Enforces retention and deletes expired data, each event's 30 days after it starts | DB only. |
| `innby invite`, `innby hash-password` | run by hand | Create an invitation and print its link; hash the admin password | DB and link key; nothing. |
| `innby migrate` | init container on `innby-web` | Applies the embedded SQL migrations | DB as schema owner. |

Splitting this way buys real least privilege:

- The public pod has no SMTP credentials and no outbound internet access.
- The admin UI, which can create and revoke invitations, is never reachable from outside.
- Cilium network policies can lock each workload down separately.

```
            external gateway (Cilium)                      internal gateway
                     │                                            │
             ┌───────▼───────┐                            ┌───────▼───────┐
             │  innby serve  │                            │  innby admin  │
             └───────┬───────┘                            └───────┬───────┘
                     │  rsvp (+ outbox in same tx)                │ invitations, headcount
                     ▼                                            ▼
             ┌─────────────────────── Postgres 18 (CNPG) ─────────────────┐
             │  rsvp · invite_group · outbox · qr_style · admin logins    │
             └───────▲──────────────────────────────▲─────────────────────┘
                     │ SELECT … FOR UPDATE SKIP LOCKED │ DELETE / scrub expired
             ┌───────┴───────┐               ┌───────┴───────┐
             │  innby mailer │─▶ MailerSend   │ innby janitor │ (CronJob)
             └───────────────┘               └───────────────┘
```

## 4. Links: signed, never stored

Every guest has one **RSVP link**. Possession of the link is the authorisation; there are no logins.

The front page, `/`, is about Innby, not about any one event: event details show only on invitation, sign-up and guest list pages. A guest can ask a device to remember them ("Remember me on this device" on their page). The browser then keeps their link in a cookie for that event (`__Host-innby-{event}` with `Secure` over https, `innby-{event}` over plain http; `HttpOnly`, `SameSite=Lax`, expiring when the event's data is deleted), and the front page shows each invitation the device remembers, the soonest event first, with a button to the guest's page. A cookie from before there were several events, named just `__Host-innby`, gets its event's name the next time the front page sees it. A cookie authorises nothing: every page and form still takes the link from the path, so the cookie can at most show the way back. A link that stops working (revoked, or the reply deleted) is dropped from the cookie the next time the front page sees it, deleting the reply drops it at once, and "Forget this device" drops it on demand. The front page is `no-store`, since it now differs per visitor.

```
/r/{id}.{sig}
  id  = base64url(rsvp.id)                                   -- 22 chars
  sig = base64url(HMAC-SHA256(LINK_KEY, "rsvp" ‖ id ‖ link_nonce)[:16])   -- 128-bit
```

- **Nothing secret is stored in the database.** The server recomputes the HMAC and compares in constant time.
  - A leaked database dump doesn't give anyone working links.
  - The admin UI can still show any guest's link again at any time ("I lost my link" → you resend it).
- **Revoking a link** increments `link_nonce`, so the old link stops working and a new one is issued. Use it for a forwarded or leaked invite.
- **E-mail confirmation links** use the same scheme, but sign the purpose, the expiry and the address itself: `"email" ‖ id ‖ email ‖ exp`. They expire after 48 hours and become invalid as soon as the address changes. There is no token table at all.
- `LINK_KEY` is 32 random bytes in a SealedSecret. Rotating it invalidates **every** link, so it is kept stable for the event's lifetime.
- **Hardening for links in URLs:**
  - `Cache-Control: no-store` on `/r/*`; `Referrer-Policy: no-referrer` and `X-Robots-Tag: noindex, nofollow` on every page, and a `robots.txt` that disallows everything.
  - Chat apps fetch a shared link to build a preview, whatever `robots.txt` says, so the page a link opens has no other guest's name on it. The guest list is a page of its own behind a button (§5.6).
  - No third-party resources on any page.
  - Access logs record the route pattern (`/r/{link}`), never the raw path.

## 5. Guest flows

### 5.1 Personal invitation

1. In `innby admin` you create invitations, one label per line (e.g. "Kari & Ola, 2"), each with a maximum party size. The admin UI then lists the new links; each invitation's page shows its link and a QR code.
2. You share it however you like.
3. The guest opens `/r/{link}`. On the first visit, a short notice explains what is stored, with a link to `/privacy`. The page shows the event, a party-size picker (1…max), how to show their name on the guest list (preset to their full name), **Attending** / **Can't come** buttons, and a button to the guest list.
4. `POST /r/{link}` saves the response, party size, name choice and `responded_at`. The guest can come back and change it any time until the optional RSVP deadline.

### 5.2 Group links

1. `/` is about Innby, and leads back to each invitation the device remembers. To RSVP, people need an invite link or a group link, `/join/{id}.{sig}`. You create groups in `innby admin`, one per line with a descriptive name and how many people each sign-up may cover (e.g. "Neighbours, 4"). A group's link is signed like an RSVP link, and *Make a new link* replaces it if it spreads too far.
2. The guest sees the group's name, a form for their name, party size and how to show their name (preset to full), and a button to the guest list, then taps **I'm coming**.
   - Abuse protection: a honeypot field and a per-IP in-memory rate limit (10 sign-ups per hour, generous for a shared household connection). Behind the gateway, `INNBY_TRUST_FORWARDED=true` takes the client address from the `X-Forwarded-For` entry the gateway appends. You can also delete entries in the admin UI. A group can only be deleted while nobody has signed up through it.
3. The guest is sent straight to their new `/r/{link}`, with a box asking them to **save this link** to change their answer later, holding the link ready to copy. From there it's identical to 5.1, e-mail and remembering the device included.

### 5.3 Optional e-mail

1. On their RSVP page the guest adds an e-mail address and ticks an **unticked** consent box ("send me reminders and my link").
2. The address is stored with `email_status='unconfirmed'` and an `outbox(kind='confirm_email')` row is added in the same transaction.
   - If another RSVP has already confirmed the address, an `email_in_use` mail goes to it instead (pointing to `/lost`, without a link). The address is stored unconfirmed either way, and only confirmed addresses must be unique, so **neither the response nor the RSVP page shows whether someone else has the address.** If two RSVPs hold it unconfirmed, the first to confirm keeps it.
   - The confirmation mail contains nothing a guest typed, not even their name, so it can't carry someone else's message to an address they don't own.
   - Adding an address and `/lost` share a per-IP limit of 5 mails an hour and a site-wide limit of 100 a day, and a mail isn't queued twice while the first is still waiting.
3. The mailer sends the signed confirmation link, `/e/{id}.{exp}.{sig}`, as plain text. The guest presses a **Confirm** button: the GET shows a page and the POST confirms, so mail scanners can't confirm for them. `email_status` becomes `confirmed`.
4. Once confirmed, the guest is taken to their RSVP page and also receives their RSVP link by mail. Reminders go out when you press *Send reminders* in the admin UI: once per guest with a confirmed address who hasn't declined.
5. On `/lost` a guest enters their e-mail and gets their link mailed if the address is confirmed. The page response is identical either way, and the lookup runs after the response, so its timing doesn't tell either.
6. A guest can remove their e-mail at any time, which withdraws consent. An address left unconfirmed for 48 hours is cleared automatically. **The RSVP itself is kept in both cases.**

Mails are sent in the language the guest was using when they added their e-mail address. That language is stored as `rsvp.lang`.

### 5.4 Guest rights (on every RSVP page)

| Action | Request | GDPR article |
|---|---|---|
| View their data | `GET /r/{link}` | Art. 15: access |
| Download their data as JSON | `GET /r/{link}/export` | Art. 20: portability |
| Change answer and party size, name, or e-mail | `POST /r/{link}`, `/r/{link}/name`, `/r/{link}/email` | Art. 16: rectification |
| Delete their RSVP and all data now | `POST /r/{link}/delete` | Art. 17: erasure, and objecting under Art. 21 |

Deletion is a hard `DELETE`. `ON DELETE CASCADE` also removes the guest's pending outbox rows.

### 5.5 Host (`innby admin`)

Two tabs under the header: **Events** and **QR codes**. Each event has four of its own, under `/events/{id}`: **Guests**, **Invitation links**, **Group links** and **Details**, each a page with its own form, and a way back to all events. A guest's page belongs to its event's Guests tab, a group's to Group links.

- **Events:** the list shows each event's date and headcount, the soonest first, with a form for a new one: a title and description in Bokmål and, if you like, in the other languages, a place, the start and its time zone, and an optional answer deadline. **Details** changes them, or deletes the event with everything in it.

- **Invitations:** create them in a batch, one per line, and get a page of their links; edit them (label, maximum party size), show the link and QR code, revoke the link (bumps the nonce), delete.
- **Groups:** create them in a batch, one per line; each has a page with its link, QR code and sign-ups, and can be renamed, given a new link, or deleted while nobody has signed up.
- **QR codes:** one style for the whole event, in the one-row `qr_style` table: square, rounded or dotted modules, a colour dark enough to scan on white (4.5:1), and a heart, a star or an emoji in the middle. A code with something in the middle uses error correction level H and hides about a quarter of its width. Each guest's and group's code downloads as an SVG for printing.
- **Overview:** the Guests tab's headcount is three tiles, coming (people and parties), no answer and not coming; each tile filters the table below to those guests (`?show=attending`). Every guest is in that table, with a CSV export.
- **Reminders:** *Send reminders* queues one mail per guest with a confirmed address who hasn't declined.
- **Authentication:** reachable only through the internal gateway (LAN or VPN), plus a login form checked against an argon2 hash from a SealedSecret, since the admin can change data. As in LLDAP, a login sets two `HttpOnly` cookies: `token`, an HS512 JWT that lasts a day, and `refresh_token`, which gets a new JWT for 30 days and is kept in Postgres by its keyed hash. *Log out* revokes both (the JWT's `jti` goes on a list until it would have expired), so copies of the cookies stop working too. The JWT key is `INNBY_ADMIN_JWT_SECRET` bound to how the host logs in (the user name and password hash, the provider and client), so a new password logs every browser out. Each argon2 check needs 19 MiB, so checks run one at a time, and after 10 checks in 15 minutes logins get 429 (browsers that are logged in keep working).
- **Single sign-on:** optionally, an OpenID Connect provider such as Authelia logs hosts in too, or instead (leaving out the password hash turns the built-in user off, and the login page then goes straight to the provider). It's the authorization code flow for a confidential client, in `admin/oidc/`: PKCE (S256), `state` and `nonce` kept in a short-lived cookie signed with the JWT key, `client_secret_basic` with both halves form-encoded, and the RFC 9207 `iss` parameter when the provider sends it. The discovery document's `issuer` must be the configured one exactly. The ID token's signature is checked with the keys at `jwks_uri` only (fetched again, at most once a minute, when a token names a new one), with asymmetric algorithms only and the key's type and curve matching; then `iss`, `aud` (Innby alone), `azp`, `exp`, `iat`, `nonce` and `at_hash`. Userinfo must be about the ID token's `sub`. The login's JWT lasts 12 hours, without a refresh token, so a role taken away at the provider is gone by the next day. The HTTP client is hyper with rustls (ring and the Mozilla roots), with a timeout, a size limit and no redirects; plain HTTP only reaches this machine.
- **Roles:** the provider lists a host's roles in a claim named after the app, `innby: [editor]`, as the homelab's Grafana and Argo CD clients have theirs. **viewer** sees the events, headcounts and who's coming, but not the personal links, e-mail status or the CSV, since those let you act as a guest. **editor** also runs events: invitations, group links, guests, reminders, the CSV and the details. **admin** also creates and deletes events and styles the QR codes; the built-in user is one. The highest role listed counts; without one there's no login. Routes are grouped by the role they need, so a page can't be reached by typing its address, and pages show only what the role may use.

### 5.6 Guest list

Every personal and group link page has a *See who's coming* button to the guest list, `/r/{link}/guests` or `/join/{link}/guests`. It's a page of its own so that link previews, which fetch only the shared link, never see a name. It has two lists, **Coming** and **Not answered yet**, of everyone invited who hasn't declined. Each name shows as its guest chose:

| Choice | Shown as |
|---|---|
| Full name | "Kari Nordmann" |
| First name and initial | "Kari N.", or the family name for a name in Chinese, Japanese or Korean script: "王**" |
| Hidden | not listed; the list ends with "+ 1 more", which counts everyone a hidden guest brings too |

Until a guest chooses, their name shows as first name and initial before they answer and in full once they're coming. The answer form presets the full name, and the guest list page has buttons to change the choice at any time. Declined guests are never listed. A coming guest who brings others has the count after their name, "Kari Nordmann +1"; the count shows only alongside a name, so a hidden guest's party stays hidden too. Each heading has a total that counts hidden guests too: everyone coming, with those they bring, and how many invitations have yet to be answered.

## 6. GDPR checklist

| Area | Decision |
|---|---|
| Lawful basis | **RSVP data** (name, response, party size) and **the guest list**: **legitimate interest** (Art. 6(1)(f)) in organising an event the guest was invited to or chose to join, and in guests knowing who else is coming. Guests can hide their name from the list. The balancing test is documented in `docs/ropa.md`. Consent would be awkward, because you create invitations before the guest has agreed to anything. **Optional e-mail:** **consent** (Art. 6(1)(a)), with double opt-in and `email_consent_at` and `privacy_version` recorded. (A private party might fall under the household exemption in Art. 2(2)(c), but we don't rely on it.) |
| Transparency | Invitation data is entered by you, not the guest, so Art. 14 information appears on their first visit to the link. Group sign-up and e-mail show the Art. 13 notice at the point where the data is collected. The notice is available in every page language. |
| Data minimisation | Required: a name or label, the response and the party size. E-mail is optional. No free-text notes field, and **no dietary or allergy info**: it can count as health data (Art. 9) and would need explicit consent. Invitation labels should be first names or household names. |
| IP addresses | Never stored. Rate limiting works per IP in memory only, and logs drop or truncate IPs. |
| Cookies | **One per invitation, only when the guest asks for it** (§4): it keeps the guest's link so the front page can lead them back. Storing it is strictly necessary for a service the guest explicitly asks for, so it needs no consent and no banner; the privacy notice describes it. CSRF is handled by the `Sec-Fetch-Site`/`Origin` middleware (§2), authorisation by the signed link, and the language and colour scheme by `?lang=` and `?theme=`. The admin hostname's login cookies are the host's own and never reach guests. |
| Third parties | Nothing loaded from a CDN, Google Fonts, analytics or reCAPTCHA. The stylesheet is embedded in the binary and uses the device's own fonts. A honeypot field and rate limiting replace a CAPTCHA. |
| Retention | Unconfirmed e-mail addresses: cleared after 48h. Sent outbox rows: deleted after 7 days. **All RSVP data: deleted 30 days after the event.** The janitor enforces this. |
| Backups | The CNPG backup retention must be ≤ 30 days after the event, or the backup policy must be documented. Otherwise "deleted" data lives on in backups. |
| Processors | **MailerSend** (MailerSend UAB, Lithuania; your existing account) for e-mail. Before go-live, accept its DPA, check its sub-processor list for transfers outside the EU, and verify the sender domain (SPF, DKIM, DMARC). Nothing sits in front of the site: it is exposed directly through `gateway/external`. |
| Privacy notice | `/privacy`, in all six languages, lists the controller (`INNBY_CONTROLLER_NAME`, `INNBY_CONTROLLER_EMAIL`), purposes, data, legal bases, retention, recipients and processors, the guest's rights (including the right to object), the right to complain to Datatilsynet, and a contact address. It is versioned by `PRIVACY_VERSION`, which is stored with each answer. |
| Security (Art. 32) | TLS at the gateway (cert-manager). HMAC-signed links with nothing secret in the database. Link hygiene as in §4. Non-root, read-only containers. Least-privilege NetworkPolicies. |
| Logging | `tracing` JSON output, with no names, e-mail addresses or links in the logs (log the RSVP UUID instead). |
| Records of processing | `docs/ropa.md`: the Art. 30 record and the legitimate-interest assessment. |

## 7. Data model (Postgres 18)

Postgres 18, as the cluster's other CNPG databases run. Its built-in `uuidv7()` gives sortable IDs.

The schema lives in [`migrations/`](migrations/). In short:

| Table | Columns | Constraints |
|---|---|---|
| `event` | `id` (uuidv7), `title` and `description` in Bokmål with optional `_nn`, `_en`, `_en_us`, `_zh_hans`, `_zh_hant` columns, `location`, `starts_at`, `time_zone`, `rsvp_deadline` | deleting it deletes its invitations, groups and their mail |
| `rsvp` | `id` (uuidv7), `event_id` (cascade delete), `source`, `name`, `response`, `party_size`, `max_party_size`, `link_nonce`, `lang`, `email`, `email_status`, `email_consent_at`, `privacy_version`, `responded_at`, `created_at`, `group_id`, `name_display` | name 1–100 chars; `party_size` within `1..=max_party_size` (max 20); e-mail present ⇔ status ≠ `none` ⇔ consent recorded; a confirmed e-mail is unique within an event; `name_display` null until the guest chooses |
| `invite_group` | `id` (uuidv7), `event_id` (cascade delete), `name`, `max_party_size`, `link_nonce`, `created_at` | can't be deleted while an RSVP references it |
| `outbox` | `id`, `rsvp_id` (cascade delete), `kind`, `attempts`, `next_try_at`, `sent_at`, `last_error` | partial index on unsent rows |
| `qr_style` | `shape`, `color`, `icon`, `emoji` | one row, for every event |
| `admin_refresh_token`, `admin_revoked_jwt` | keyed hashes of refresh tokens, and the `jti` of logged-out JWTs, each with an expiry | expired rows go whenever a new one is written |

- **Enums are Postgres enum types** (`rsvp_source`, `rsvp_response`, `email_status`, `lang`, `outbox_kind`, `name_display`, `qr_shape`, `qr_icon`) rather than `text` + `CHECK`. sqlx maps them straight to Rust enums with `#[derive(sqlx::Type)]`.
- **Queries** in `src/db/` use `query!` and `query_as!`, so they're checked against the schema at compile time. `.sqlx/` holds the offline data, which lets builds and CI compile without a database.

Notes on the model:

- **Headcount** is `sum(party_size) where response = 'attending'`. There is no capacity limit, so no locking is needed.
- **Mailer loop:**
  1. `SELECT … WHERE sent_at IS NULL AND next_try_at <= now() ORDER BY id LIMIT 10 FOR UPDATE SKIP LOCKED`
  2. Re-read the RSVP. Skip the mail if the address was removed or changed in the meantime.
  3. Render the mail with a freshly signed link, send it, and mark the row sent. A failed send gets exponential backoff (1 minute doubling to at most 6 hours) and is given up after 10 attempts. SMTP errors go to `outbox.last_error`, not the logs, since they often quote the address.
- **Events** live in the database, made and changed in the admin UI. A missing language falls back: Nynorsk and British English to Bokmål, American English and Simplified Chinese to British, Traditional Chinese to Simplified. The rest of the config (the base URL, the controller named in the privacy notice, the sender address) comes from environment variables, in the cluster from a ConfigMap; `.env.example` lists them.

## 8. Repository layout

```
innby/
├── .devcontainer/
│   ├── devcontainer.json      # VS Code / RustRover / devcontainer CLI entry point
│   ├── compose.yaml           # dev + db (postgres:18) + mail (mailpit)
│   ├── compose.ports.yaml     # the ports `just dev` publishes on the host
│   └── Containerfile          # Rust, zig, cargo-zigbuild, sqlx-cli, just, psql, kubectl, kubeseal, zsh, Vim, Neovim
├── .github/workflows/ci.yaml  # test, lint, then build and push the image
├── Cargo.toml / Cargo.lock
├── rust-toolchain.toml        # pins the toolchain and both musl targets
├── src/
│   ├── main.rs                # clap: serve | admin | mailer | janitor | migrate | invite | hash-password
│   ├── config.rs              # env parsing and validation
│   ├── link.rs                # HMAC signing for RSVP, group and e-mail links
│   ├── paths.rs               # every route pattern, and links built from them
│   ├── i18n.rs                # Messages, loaded from locales/
│   ├── theme.rs               # the colour scheme from ?theme=
│   ├── db/                    # queries, per table (events, RSVPs, groups, outbox…), and retention
│   ├── web/                   # public pages, CSRF middleware, rate limit, remember-me cookie
│   ├── admin/                 # host UI: events, invitations, groups, QR codes, login
│   ├── mailer/                # outbox worker and mail texts
│   └── janitor.rs             # runs the retention rules
├── locales/*.toml             # texts and mails per language, compiled in
├── templates/                 # askama, compiled in; privacy/ has the notice per language
├── static/style.css           # the one stylesheet, compiled in
├── migrations/*.sql           # sqlx::migrate!(), compiled in
├── .sqlx/                     # offline query metadata (committed)
├── compose.yaml               # a local copy of the published image
├── charts/innby/              # the Helm chart, published as an OCI artifact
├── deploy/                    # the homelab's install of it, mirrored into vehagn/homelab
├── docs/ropa.md               # GDPR record of processing
├── Justfile
└── PLAN.md
```

## 9. Development environment (devcontainer)

The devcontainer is a Compose project with three services:

| Service | Image | Purpose |
|---|---|---|
| `dev` | built from `.devcontainer/Containerfile` (Debian trixie) | Where you work. Contains:<br>• Rust 1.99.1 with rustfmt, clippy, rust-analyzer and both musl targets<br>• Zig and `cargo-zigbuild` for cross-compiling<br>• `sqlx-cli`, `just`, `cargo-deny`<br>• `psql` 18<br>• `kubectl` 1.35 (matching the cluster) and `kubeseal` (matching the controller)<br>• zsh with oh-my-zsh, Vim and Neovim |
| `db` | `postgres:18.6` | Dev database. `DATABASE_URL` is preset. |
| `mail` | `axllent/mailpit` | Fake SMTP server with a web inbox on port 8025. |

- **Kubernetes access:** your `~/.kube/config` is mounted read-only, so `kubectl`/`kubeseal` work from inside the container.
- **Volumes:** Cargo's registry, the build directory and the shell history live in named volumes. This keeps builds fast on macOS bind mounts and keeps build output out of the repository; `target/oci` stays in the workspace so the host can load the image.
- **Editors:** works with VS Code (Dev Containers extension), RustRover/IntelliJ (*Remote Development → Dev Containers*) and the `devcontainer` CLI.
- **Tests:**
  - Unit tests for link signing (tampering, nonce bump, expiry), validation, i18n completeness and retention.
  - Integration tests with `#[sqlx::test]`, which creates a throwaway database per test against `db`.
  - End-to-end journeys in `src/e2e/`, over the real routers of the public site and the admin UI and the real mailer, on a throwaway database. A test `Browser` opens pages, fills in fields by their labels, presses buttons by their text, follows redirects and keeps cookies; mail lands in an inbox the test reads. Labels and texts come from the British locale file, so rewording a text doesn't break a test. Each file covers an area: `guests.rs`, `groups.rs`, `email.rs`, `host.rs`, `events.rs`, `sso.rs`. Single sign-on runs against a fake provider in `admin/oidc/fake.rs`, on a local port, which checks what a real one would and can be made to misbehave.

## 10. Container image (binary only, no build tool)

- Build: `just image` first runs `cargo zigbuild --release --locked` for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`, with `SQLX_OFFLINE=true` so no database is needed.
  - Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`.
  - The cluster is amd64; arm64 is for running a copy on an arm64 machine, such as a Mac.
- `just image [tag]` writes the image as an [OCI image layout](https://github.com/opencontainers/image-spec/blob/main/image-layout.md) in `target/oci`, with `tar`, `gzip`, `sha256sum` and `jq`:
  - Per architecture: one gzipped layer holding `/innby` (root-owned, mode 0555, time 0), a config (entrypoint, `serve`, user 65532, port 8080, OCI labels) and a manifest.
  - An image index over the two, named `<tag>` in `index.json`.
  - A `docker save`-style `manifest.json` for the build machine's architecture, since Docker before 25 can't load OCI layouts.
  - `target/oci.tar` holds it all, for `just up <tag>` on the host.
  - The same binaries always give the same digests.
- It needs no privileges, so it runs in the dev container. Buildah can't run there: it needs user namespaces, which the dev container's default seccomp profile blocks, and we keep the profile as it is.
- CI's `just push` copies the index and both images to the registry with `skopeo copy --all --preserve-digests`; CI checks that the registry serves an OCI image index for amd64 and arm64.
- `just up <tag>`, on the host, loads the image into Docker or Podman first when `target/oci` holds that tag.
- The image contains no shell, no CA bundle and no tzdata: the CA roots and zoneinfo are compiled into the binary. It runs as a numeric non-root UID, so it needs no `/etc/passwd`. Expect about 10–20 MB.
- `.github/workflows/ci.yaml` runs `just test` and `just lint` against a Postgres 18 service on every push and pull request; on pushes it then runs `just image` on `ubuntu-24.04` (which ships skopeo) and pushes `sha-<commit>`, plus `latest` from `main` and the tag name for `v*` tags, to `ghcr.io/vehagn/innby`. A `chart` job runs `just chart` and packages the chart; for `v*` tags, once the image is pushed, it goes to `oci://ghcr.io/vehagn/charts`, after checking that `Chart.yaml` has the tag's version. Tool versions match the devcontainer.
- Probes use `httpGet` on `/healthz` and `/readyz`, since there is no shell for `exec` probes.

## 11. Kubernetes

A **Helm chart** in `charts/innby`, published with each release as an OCI artifact, `oci://ghcr.io/vehagn/charts/innby`, under the app's version. It's general purpose: it needs no CRDs and assumes nothing about the cluster but a Postgres to reach.

| Resource | Notes |
|---|---|
| Deployments | `{release}-web` (`serve`, 2 replicas), `{release}-admin` (`admin`, 1 replica), `{release}-mailer` (`mailer`, 1 replica, `Recreate`). `innby migrate` runs as an **init container** on `web`. Each can be turned off but `web`. |
| CronJob | `{release}-janitor`: hourly, with `concurrencyPolicy: Forbid` and an optional time zone. |
| Settings | A ConfigMap of `INNBY_*` from the values, and a Secret, the chart's or one you made (`secrets.existingSecret`), with the link key, the admin's JWT secret, password hash and OIDC client secret, and the SMTP credentials. Each workload references only the keys it needs. Pods roll when either changes. |
| Postgres | Not the chart's to make. One of three: a **CloudNativePG cluster** by name (`database.cnpg.cluster`), whose operator-made `{cluster}-app` Secret Innby reads and whose pods the network policies let the workloads reach; a Secret with a `postgres://` URL; or the URL itself. |
| Routing | Optional: two HTTPRoutes (Gateway API, the one CRD the chart can use) or two Ingresses, one for the site's hostname and one for the admin UI. The admin UI is under `/admin` on the site's hostname by default, where the longer path wins, or on a hostname of its own, and then can attach to another gateway or ingress class. |
| NetworkPolicies | Optional, of either kind: standard NetworkPolicies, or CiliumNetworkPolicies (a CRD, so off by default like the routes). The site and admin UI accept traffic on their ports, the mailer and janitor nothing, and each workload may reach DNS and Postgres, the mailer SMTP and the admin UI the OIDC provider. Standard policies can't name hosts, so those are open on their ports; Cilium's name the SMTP host and the provider, taken from `mailer.smtpUrl` and `oidc.issuer`, and read DNS answers to learn their addresses. |
| Pod security | `runAsNonRoot`, `readOnlyRootFilesystem`, `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`, `seccompProfile: RuntimeDefault`, no service account token: the `restricted` Pod Security Standard. |
| Checks | `values.schema.json` refuses unknown values, and the templates refuse settings that can't work (no site URL, nobody able to log in, routes without a gateway). `just chart` lints, renders `charts/innby/ci/*.yaml`, checks the manifests with kubeconform, and checks that bad settings fail. |

**The homelab** installs it from `deploy/`, a kustomization that Argo CD renders with `--enable-helm`: the chart with `deploy/values.yaml`, plus what the chart leaves to the cluster.

| Resource | Notes |
|---|---|
| Namespace | `innby`, labelled `pod-security.kubernetes.io/enforce: restricted`. |
| Postgres | A **CNPG `Cluster`** `innby-postgres` in `deploy/cnpg-db.yaml`, which the chart uses by name: 1 instance, image `postgresql:18.6-minimal-trixie` (same as dev), 1 Gi on `proxmox-csi`, created with `bootstrap.initdb` (database `innby`, owner `innby`). No backups, on purpose. |
| Routes | The site on `gateway/external` and `gateway/internal`; the admin UI on `fest-admin.stonegarden.dev`, on `gateway/internal` only. TLS comes from the existing cert-manager setup. |
| Secrets (SealedSecrets) | The `innby` secret, sealed by `just seal` from the gitignored `deploy/secrets.env`. |
| NetworkPolicies (Cilium) | The chart's Cilium policies: `web` and `admin` accept traffic from the gateways (`fromEntities: ingress`). `mailer` has no ingress and egress to `smtp.mailersend.net:587` only, and `admin` to `authelia.stonegarden.dev:443` once single sign-on is on. `janitor` has egress to the DB only. `deploy/network-policies.yaml` adds Postgres's: ingress from the Innby pods and the CNPG operator. |

- Observability: JSON logs. `/metrics` (responses, headcount, rate-limit hits, outbox lag, mail failures) is left for hardening.

## 12. Justfile

```
just                     # list recipes
just dev [down]          # on the host: shell in the dev container, or stop it
just up [tag|janitor]    # dev container: serve this checkout (+ mailer); host: local copy of the image
just down                # stop the local copy
just test                # cargo test (incl. #[sqlx::test] against db)
just lint                # cargo fmt --check, clippy -D warnings, sqlx prepare --check, cargo deny check
just db [prepare]        # psql on the dev db, or cargo sqlx prepare → .sqlx/ (commit it)
just innby invite "Kari & Ola" 2   # any innby command; hash-password asks without echo
just image [tag]         # OCI layout in target/oci: amd64 + arm64, binary only
just release 0.3.0       # bump the crate and the chart, tag (signed), push
just chart               # helm lint, render charts/innby/ci/*.yaml, kubeconform
just seal                # deploy/secrets.env → deploy/sealed-secrets.yaml
```

Hidden from the list, for other recipes and CI: `run`, `migrate`, `build` (cargo zigbuild, static musl, `SQLX_OFFLINE`) and `push` (skopeo copy → ghcr.io/vehagn/innby). `helm template innby charts/innby -f deploy/values.yaml` renders the homelab's manifests.

## 13. History

0. ✅ **Devcontainer:** Containerfile, compose and `devcontainer.json`, verified by building it and running every tool once.
1. ✅ **Skeleton:** `cargo init`, `rust-toolchain.toml`, `Justfile`, config, `innby serve` with `/healthz`, an askama layout with embedded CSS, nb/en `Messages`.
2. ✅ **Schema:** migrations, the `db` module, `.sqlx/` offline data, `#[sqlx::test]` integration tests.
3. ✅ **Links and the RSVP page:** HMAC signing, `/r/{link}` (respond, party size, rename, export, delete), link hygiene headers, CSRF middleware.
4. ✅ **Admin:** invitations (create, link and QR, revoke, delete), headcount, CSV, basic auth, and `innby invite` on the CLI.
5. ✅ **Open RSVP:** `/join/{code}`, honeypot, rate limit, a "save your link" page, and the on/off flag. Replaced by group links in 12.
6. ✅ **Optional e-mail:** add and confirm an address, the outbox and mailer (Mailpit locally, MailerSend in the cluster), `/lost`, reminders.
7. ✅ **Janitor and privacy:** retention rules, `/privacy` (nb/en), `docs/ropa.md`.
8. ✅ **Image:** `just image` writes an OCI layout, plus a CI workflow that pushes to GHCR.
9. ✅ **Deploy:** CNPG cluster, workloads, HTTPRoutes, SealedSecrets, NetworkPolicies, Argo CD app.
10. ✅ **Hardening and polish:** strict CSP (no inline styles or scripts anywhere), HSTS, `Cross-Origin-Opener-Policy` and `-Resource-Policy`, a 15-second request timeout and a 10-second header timeout, `cargo deny` in CI, and a load sanity check (5000 RSVP-page requests at concurrency 50: p50 0.5 ms, p99 4.5 ms). Still optional: `LISTEN/NOTIFY` for the mailer (it polls every 5 seconds), Authelia OIDC for the admin UI, and `/metrics`.
11. ✅ **Pentest fixes:** a bounded admin login check with lockout, a confirmation mail without guest text, a site-wide mail cap, unconfirmed addresses that can be shared, the example secrets from `.env.example` refused on an https site, a header timeout, names without invisible or reordering characters, and `/lost` lookups after the response.
12. ✅ **Groups and guest list:** batch-created invitations and group links (replacing `FEST_OPEN_RSVP_CODE`), and a guest list on every link with a full, partial or hidden name per guest.
13. ✅ **Paths and texts:** every route pattern and link in `src/paths.rs`, admin guest pages at `/guests/{id}`, and the texts and mails in `locales/{nb,en}.toml`.
14. ✅ **More languages:** Nynorsk, and English split into British ("traditional") and American ("simplified"), each with its own texts, mails, privacy notice and date format.
15. ✅ **Chinese:** Simplified and Traditional, with punctuation moved from the templates into the texts, and partial names that keep only the family name for names in Chinese, Japanese or Korean script ("王**").
16. ✅ **Renamed to Innby** (from fest): the crate, binary and commands, the `INNBY_*` settings, the image, the Kubernetes names and the databases. The hostnames stay as they are.
17. ✅ **Multi-arch and a local copy:** CI builds amd64 and arm64 binaries and pushes their scratch images as one OCI image index; `compose.yaml` and `just up` run a local copy of the published image.
18. ✅ **Admin in every language:** the admin UI's texts in `[admin]` in each locale file, its CSV header too, and the same language menu, theme switch and header as the public site, plus a logout.
19. ✅ **Admin login with a JWT:** a login form instead of basic auth. As in LLDAP, an HS512 JWT for a day and a refresh token for 30 days in `HttpOnly` cookies, and a logout that revokes both. Needs `INNBY_ADMIN_JWT_SECRET` in `deploy/secrets.env`.
20. ✅ **Styled QR codes:** module shape, colour and a centre icon or emoji for every code, set once for the event, with SVG downloads.
21. ✅ **Admin tabs and end-to-end tests:** the admin's sections as tabs, a headcount that filters the guests, and journeys through the site, the admin UI and the mailer in `src/e2e/`.
22. ✅ **Several events:** events in the database, made in the admin UI, with everything else belonging to one; a front page about Innby that leads back to each remembered invitation, one cookie per event; retention per event.
23. ✅ **Single sign-on and roles:** optional OpenID Connect login (Authelia), the built-in user optional, and viewer, editor and admin roles from an `innby` claim.
24. ✅ **Helm chart:** `charts/innby`, general purpose and without CRDs, with optional HTTPRoutes, Ingresses, standard or Cilium NetworkPolicies, and a CloudNativePG cluster by name, published to GHCR as an OCI artifact with each release. The admin UI can live under `/admin` on the site's hostname. `deploy/` is now the homelab's values for it.

## 14. Decisions

- **Language:** Rust (§2).
- **RSVP style:** both personal invite links and name-only sign-up through group links the host makes (§5.1, §5.2).
- **E-mail:** an optional extra for guests (§5.3), sent by its own `mailer` workload.
- **Page languages:** Bokmål, Nynorsk, British English, American English, Simplified Chinese and Traditional Chinese.
- **Capacity:** no limit and no waitlist.
- **Group sign-up party size:** set per group, 2 by default (the guest plus one).
- **Guest list:** the whole event's, behind every link; names as each guest chose (§5.6).
- **Events:** as many as you like, made in the admin UI; one QR code style for all of them.
- **Root page `/`:** about Innby, and a way back to each invitation the device remembers; RSVP needs an invite link or a group link.
- **RSVP deadline:** optional, unset by default, so answers can change until the event.
- **Retention:** all guest data is deleted 30 days after the event.
- **Exposure:** directly through `gateway/external`.
- **E-mail provider:** MailerSend (existing account).
- **Registry and CI:** GHCR, with GitHub Actions building and pushing the image.
