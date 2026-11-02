# fest — event RSVP page

Plan for a small, GDPR-compliant page where ~100 guests say whether they're coming to one event. **Guests need no account and no e-mail address.** They respond through a personal invite link, or through an open RSVP link where they only give a name. E-mail is an optional extra for reminders.

## 1. Goals and non-goals

**Goals**

- Three ways in, all ending at the same personal RSVP page:
  1. **Personal invite link.** You create an invitation per guest or household and share the link or QR code however you like (SMS, Messenger, paper). The guest opens it and taps *Attending* / *Can't come*.
  2. **Open RSVP.** One shared, unlisted link (`/join/{code}`). The guest types a name and how many are coming (up to 2), taps *I'm coming*, then gets their own private link to change or cancel later.
  3. **Optional e-mail.** From their RSVP page a guest can add an e-mail address (with double opt-in) to get reminders and their link by mail.
- Guests can change their answer, party size and name at any time, and can export or delete their data.
- Pages in **Norwegian (bokmål) and English**, chosen from the browser's `Accept-Language`, with a manual switch through `?lang=` (no cookie).
- An admin UI for you: create and revoke invitations, see the headcount, export CSV.
- Built for GDPR from the start: minimal data, clear retention, automatic deletion, no cookies or third-party assets.
- A tiny OCI image that holds nothing but the binary, built with buildah and driven by `just`.
- A **devcontainer** with every tool needed to develop and deploy, so the host needs nothing but a container runtime. Images are built outside it (§10).
- Runs on the existing Talos cluster, deployed the same way as the other apps in `vehagn/homelab` (Argo CD and Kustomize).

**Non-goals**

- Several events, payments or ticket QR codes.
- A capacity limit or waitlist. We just count heads.
- Guest accounts or passwords.
- A JavaScript front end. Pages are rendered on the server and work without JS.
- A public list of who's coming.

## 2. Language: Rust

You had no preference, and the devcontainer removes Go's main advantage (it was already installed). Rust gives compile-time checked SQL and templates, strong types for states such as `Response::{Pending, Attending, Declined}`, and a static musl binary that suits a `FROM scratch` image.

| Concern | Crate |
|---|---|
| HTTP | `axum` on `tokio`; `tower-http` for security headers and request tracing |
| Templates | `askama`: compiled into the binary and type-checked at build time |
| Postgres | `sqlx` (postgres, rustls, migrate): queries checked against the schema at compile time, with offline `.sqlx/` data so builds don't need a database. `sqlx::migrate!()` embeds the migrations and takes an advisory lock, so there's no custom migrator. |
| Mail | `lettre` (SMTP over rustls) |
| Signed links | `hmac` + `sha2`, `base64` |
| IDs and time | `uuid` (v7), `chrono` + `chrono-tz`: the tz database is compiled in, so the image needs no `/usr/share/zoneinfo` |
| CLI | `clap` (derive) for the subcommands |
| Logging | `tracing` + `tracing-subscriber` (JSON output) |
| Rate limiting | A ~40-line fixed-window counter per IP (IPv6 per /64), in memory; no crate needed |
| Admin | `argon2` (basic-auth hash), `csv`, `qrcode` (SVG output) |
| TLS roots | `rustls` + `webpki-roots`: CA roots compiled in, so the image needs no `/etc/ssl` |

- **CSRF:** axum has no built-in equivalent of Go's `CrossOriginProtection`, so we add a ~20-line middleware that does the same thing. For unsafe methods it requires `Sec-Fetch-Site` to be `same-origin` or `none`, or (for older browsers) the `Origin` host to match the `Host` header. This needs no cookies, and works unchanged for both the public and the admin hostname.
- **i18n:** two languages don't need Fluent. Each language gets a `Messages` struct, the templates read strings from it, and the compiler catches a missing translation.
- **Cross-compiling:** the devcontainer runs arm64 on your Mac and the cluster is amd64. `cargo zigbuild --target x86_64-unknown-linux-musl` (with Zig as the linker) handles that, including the C code inside `ring`.

## 3. Architecture: should we split it into microservices?

For 100 guests, one process is more than enough. Since this is for fun, here are the options.

| Option | What it looks like | Verdict |
|---|---|---|
| **A. Plain monolith** | One binary, one Deployment. It sends mail inline. | Simplest, but the public pod then holds SMTP credentials and the admin UI is on the same listener. |
| **B. One binary, several workloads** ✅ | One image. Subcommands run as separate Kubernetes workloads. They share no API with each other; Postgres links them, and an **outbox table** carries mail jobs. | Feels like microservices where it helps (each workload has its own permissions and network policy and fails on its own) and costs nothing in RPC, service auth or extra images. |
| **C. Real microservices** | `frontend`, `rsvp-api`, `mailer`, `admin` and `janitor`, talking over HTTP/gRPC or NATS. | You'd still need an outbox for "save and send" to stay consistent, plus mTLS between services and N images and manifests. Each extra service that holds personal data is also another place erasure has to reach. Pure overhead at this size. |

**Recommendation: B.** The binary `fest` has these subcommands:

| Subcommand | Kubernetes workload | Responsibility | Needs |
|---|---|---|---|
| `fest serve` | Deployment (public) | Event page, `/join/{code}`, RSVP pages, e-mail confirm, privacy notice | DB, link key. Reachable from the **external** gateway. |
| `fest admin` | Deployment (internal) | Invitations, headcount, CSV | DB, link key. Reachable from the **internal** gateway only. |
| `fest mailer` | Deployment (1 replica) | Processes the outbox and sends e-mail through MailerSend | DB, link key, SMTP credentials, outbound SMTP. No ingress. |
| `fest janitor` | CronJob (hourly) | Enforces retention and deletes expired data | DB and the event start (`FEST_EVENT_START`, `FEST_EVENT_TIMEZONE`) only. |
| `fest migrate` | init container / Argo CD PreSync Job | Applies the embedded SQL migrations | DB as schema owner. |

Splitting this way buys real least privilege:

- The public pod has no SMTP credentials and no outbound internet access.
- The admin UI, which can create and revoke invitations, is never reachable from outside.
- Cilium network policies can lock each workload down separately.

```
            external gateway (Cilium)                      internal gateway
                     │                                            │
             ┌───────▼───────┐                            ┌───────▼───────┐
             │  fest serve   │                            │  fest admin   │
             └───────┬───────┘                            └───────┬───────┘
                     │  rsvp (+ outbox in same tx)                │ invitations, headcount
                     ▼                                            ▼
             ┌─────────────────────── Postgres 18 (CNPG) ─────────────────┐
             │  rsvp · outbox · _sqlx_migrations                          │
             └───────▲──────────────────────────────▲─────────────────────┘
                     │ SELECT … FOR UPDATE SKIP LOCKED │ DELETE / scrub expired
             ┌───────┴───────┐               ┌───────┴───────┐
             │  fest mailer  │─▶ MailerSend   │  fest janitor │ (CronJob)
             └───────────────┘               └───────────────┘
```

## 4. Links: signed, never stored

Every guest has one **RSVP link**. Possession of the link is the authorisation; there are no logins or cookies.

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
  - `Cache-Control: no-store` on `/r/*`; `Referrer-Policy: no-referrer` and `X-Robots-Tag: noindex` on every page.
  - No third-party resources on any page.
  - Access logs record the route pattern (`/r/{link}`), never the raw path.

## 5. Guest flows

### 5.1 Personal invitation

1. In `fest admin` you create an invitation with a label (e.g. "Kari & Ola") and a maximum party size. The admin UI shows the link, a copy button and a QR code.
2. You share it however you like.
3. The guest opens `/r/{link}`. On the first visit, a short notice explains what is stored, with a link to `/privacy`. The page shows the event, a party-size picker (1…max), and **Attending** / **Can't come** buttons.
4. `POST /r/{link}` saves the response, party size and `responded_at`. The guest can come back and change it any time until the optional RSVP deadline.

### 5.2 Open RSVP

1. `/` shows only the event info. To RSVP, people need an invite link or the unlisted join link `/join/{code}`. The code comes from config and can be rotated if the link spreads too far.
2. The guest enters a name and party size (1 or 2), then taps **I'm coming**.
   - Abuse protection: a honeypot field and a per-IP in-memory rate limit (10 sign-ups per hour, generous for a shared household connection). Behind the gateway, `FEST_TRUST_FORWARDED=true` takes the client address from the `X-Forwarded-For` entry the gateway appends. You can also delete entries in the admin UI, or switch the feature off by unsetting `FEST_OPEN_RSVP_CODE` (the join link then 404s).
3. The guest is sent straight to their new `/r/{link}`, with a clear "**Bookmark or save this link** to change your answer later" box, plus copy and share buttons and an optional "add your e-mail" field. From there it's identical to 5.1.

### 5.3 Optional e-mail

1. On their RSVP page the guest adds an e-mail address and ticks an **unticked** consent box ("send me reminders and my link").
2. The address is stored with `email_status='unconfirmed'` and an `outbox(kind='confirm_email')` row is added in the same transaction.
   - If the address already belongs to another RSVP, an `email_in_use` mail goes to that address instead (pointing to `/lost`, without a link). **The immediate response is identical either way.** The RSVP page afterwards shows whether the address was stored; anyone who can see that already holds this guest's link, so we accept it.
   - Adding an address and `/lost` share a per-IP limit of 5 mails an hour, and a mail isn't queued twice while the first is still waiting.
3. The mailer sends the signed confirmation link, `/e/{id}.{exp}.{sig}`, as plain text. The guest presses a **Confirm** button: the GET shows a page and the POST confirms, so mail scanners can't confirm for them. `email_status` becomes `confirmed`.
4. Once confirmed, the guest is taken to their RSVP page and also receives their RSVP link by mail. Reminders go out when you press *Send reminders* in the admin UI: once per guest with a confirmed address who hasn't declined.
5. On `/lost` a guest enters their e-mail and gets their link mailed if the address is confirmed. The page response is identical either way.
6. A guest can remove their e-mail at any time, which withdraws consent. An address left unconfirmed for 48 hours is cleared automatically. **The RSVP itself is kept in both cases.**

Mails are sent in the language the guest was using when they added their e-mail address. That language is stored as `rsvp.lang`.

### 5.4 Guest rights (on every RSVP page)

| Action | Request | GDPR article |
|---|---|---|
| View their data | `GET /r/{link}` | Art. 15: access |
| Download their data as JSON | `GET /r/{link}/export` | Art. 20: portability |
| Change name, party size or e-mail | `POST /r/{link}` | Art. 16: rectification |
| Delete their RSVP and all data now | `POST /r/{link}/delete` | Art. 17: erasure, and objecting under Art. 21 |

Deletion is a hard `DELETE`. `ON DELETE CASCADE` also removes the guest's pending outbox rows.

### 5.5 Host (`fest admin`)

- **Invitations:** create and edit them (label, maximum party size), show the link and QR code, revoke the link (bumps the nonce), delete. Optionally import invitations from a CSV.
- **Overview:** attending, declined and no-answer counts, a total headcount, and a CSV export.
- **Authentication:** reachable only through the internal gateway (LAN or VPN), plus basic auth with an argon2 hash from a SealedSecret, since the admin can change data. Later, for fun: OIDC login against Authelia.

## 6. GDPR checklist

| Area | Decision |
|---|---|
| Lawful basis | **RSVP data** (name, response, party size): **legitimate interest** (Art. 6(1)(f)) in organising an event the guest was invited to or chose to join. The balancing test is documented in `docs/ropa.md`. Consent would be awkward, because you create invitations before the guest has agreed to anything. **Optional e-mail:** **consent** (Art. 6(1)(a)), with double opt-in and `email_consent_at` and `privacy_version` recorded. (A private party might fall under the household exemption in Art. 2(2)(c), but we don't rely on it.) |
| Transparency | Invitation data is entered by you, not the guest, so Art. 14 information appears on their first visit to the link. Open RSVP and e-mail show the Art. 13 notice at the point where the data is collected. The notice is available in Norwegian and English. |
| Data minimisation | Required: a name or label, the response and the party size. E-mail is optional. No free-text notes field, and **no dietary or allergy info**: it can count as health data (Art. 9) and would need explicit consent. Invitation labels should be first names or household names. |
| IP addresses | Never stored. Rate limiting works per IP in memory only, and logs drop or truncate IPs. |
| Cookies | **None.** CSRF is handled by the `Sec-Fetch-Site`/`Origin` middleware (§2), authorisation by the signed link, and the language choice by `?lang=`, so no cookie banner is needed. |
| Third parties | Nothing loaded from a CDN, Google Fonts, analytics or reCAPTCHA. All CSS and fonts are embedded in the binary. A honeypot field and rate limiting replace a CAPTCHA. |
| Retention | Unconfirmed e-mail addresses: cleared after 48h. Sent outbox rows: deleted after 7 days. **All RSVP data: deleted 30 days after the event.** The janitor enforces this. |
| Backups | The CNPG backup retention must be ≤ 30 days after the event, or the backup policy must be documented. Otherwise "deleted" data lives on in backups. |
| Processors | **MailerSend** (MailerSend UAB, Lithuania; your existing account) for e-mail. Before go-live, accept its DPA, check its sub-processor list for transfers outside the EU, and verify the sender domain (SPF, DKIM, DMARC). Nothing sits in front of the site: it is exposed directly through `gateway/external`. |
| Privacy notice | `/privacy` (nb/en) lists the controller (`FEST_CONTROLLER_NAME`, `FEST_CONTROLLER_EMAIL`), purposes, data, legal bases, retention, recipients and processors, the guest's rights (including the right to object), the right to complain to Datatilsynet, and a contact address. It is versioned by `PRIVACY_VERSION`, which is stored with each answer. |
| Security (Art. 32) | TLS at the gateway (cert-manager). HMAC-signed links with nothing secret in the database. Link hygiene as in §4. Non-root, read-only containers. Least-privilege NetworkPolicies. |
| Logging | `tracing` JSON output, with no names, e-mail addresses or links in the logs (log the RSVP UUID instead). |
| Records of processing | `docs/ropa.md`: the Art. 30 record and the legitimate-interest assessment. |

## 7. Data model (Postgres 18)

PostgreSQL 18 is the newest major version (18.6). The cluster already runs `ghcr.io/cloudnative-pg/postgresql:18.x` for Authelia. PG18's built-in `uuidv7()` gives sortable IDs.

The schema lives in [`migrations/0001_rsvp_and_outbox.sql`](migrations/0001_rsvp_and_outbox.sql). In short:

| Table | Columns | Constraints |
|---|---|---|
| `rsvp` | `id` (uuidv7), `source`, `name`, `response`, `party_size`, `max_party_size`, `link_nonce`, `lang`, `email`, `email_status`, `email_consent_at`, `privacy_version`, `responded_at`, `created_at` | name 1–100 chars; `party_size` within `1..=max_party_size` (max 20); e-mail present ⇔ status ≠ `none` ⇔ consent recorded; e-mail unique |
| `outbox` | `id`, `rsvp_id` (cascade delete), `kind`, `attempts`, `next_try_at`, `sent_at`, `last_error` | partial index on unsent rows |

- **Enums are Postgres enum types** (`rsvp_source`, `rsvp_response`, `email_status`, `lang`, `outbox_kind`) rather than `text` + `CHECK`. sqlx maps them straight to Rust enums with `#[derive(sqlx::Type)]`.
- **Queries** in `src/db/rsvp.rs` use `query_as!`, so they're checked against the schema at compile time. `.sqlx/` holds the offline data, which lets builds and CI compile without a database.

Notes on the model:

- **Headcount** is `sum(party_size) where response = 'attending'`. There is no capacity limit, so no locking is needed.
- **Mailer loop:**
  1. `SELECT … WHERE sent_at IS NULL AND next_try_at <= now() ORDER BY id LIMIT 10 FOR UPDATE SKIP LOCKED`
  2. Re-read the RSVP. Skip the mail if the address was removed or changed in the meantime.
  3. Render the mail with a freshly signed link, send it, and mark the row sent. A failed send gets exponential backoff (1 minute doubling to at most 6 hours) and is given up after 10 attempts. SMTP errors go to `outbox.last_error`, not the logs, since they often quote the address.
- **Event config** (title and description in both languages, date, place, optional RSVP deadline, open-RSVP code, open maximum party size = 2) comes from environment variables or a ConfigMap. It doesn't live in the database.

## 8. Repository layout

```
fest/
├── .devcontainer/
│   ├── devcontainer.json      # VS Code / RustRover / devcontainer CLI entry point
│   ├── compose.yaml           # dev + db (postgres:18) + mail (mailpit)
│   └── Containerfile          # Rust, zig, cargo-zigbuild, sqlx-cli, just, psql, kubectl, kubeseal
├── Cargo.toml / Cargo.lock
├── rust-toolchain.toml        # pins the toolchain + musl target
├── src/
│   ├── main.rs                # clap: serve | admin | mailer | janitor | migrate
│   ├── config.rs              # env parsing + validation
│   ├── db/                    # sqlx pool, migrator, queries
│   ├── link.rs                # HMAC sign/verify for rsvp + email links
│   ├── i18n.rs                # nb/en Messages
│   ├── web/                   # public pages, RSVP page, CSRF middleware, rate limit
│   ├── admin/                 # invitations, headcount, CSV, QR
│   ├── mailer.rs              # outbox worker + mail rendering
│   └── janitor.rs             # retention rules, reminders
├── templates/*.html           # askama, compiled in
├── static/                    # CSS (+ self-hosted font if any), compiled in
├── migrations/*.sql           # sqlx::migrate!(), compiled in
├── .sqlx/                     # offline query metadata (committed)
├── deploy/                    # Kustomize, mirrored into vehagn/homelab k8s/apps/…
├── docs/ropa.md
├── Justfile
└── PLAN.md
```

## 9. Development environment (devcontainer)

The devcontainer is a Compose project with three services:

| Service | Image | Purpose |
|---|---|---|
| `dev` | built from `.devcontainer/Containerfile` (Debian trixie) | Where you work. Contains:<br>• Rust 1.99.1 with rustfmt, clippy and the `x86_64-unknown-linux-musl` target<br>• Zig and `cargo-zigbuild` for cross-compiling<br>• `sqlx-cli`, `just`, `cargo-deny`<br>• `psql` 18<br>• `kubectl` 1.35 (matching the cluster) and `kubeseal` (matching the controller) |
| `db` | `postgres:18.6` | Dev database. `DATABASE_URL` is preset. |
| `mail` | `axllent/mailpit` | Fake SMTP server with a web inbox on port 8025. |

- **Kubernetes access:** your `~/.kube/config` is mounted read-only, so `kubectl`/`kubeseal` work from inside the container.
- **Volumes:** Cargo's registry and the `target/` directory live in named volumes. This keeps builds fast on macOS bind mounts and keeps build output out of the repository.
- **Editors:** works with VS Code (Dev Containers extension), RustRover/IntelliJ (*Remote Development → Dev Containers*) and the `devcontainer` CLI.
- **Tests:**
  - Unit tests for link signing (tampering, nonce bump, expiry), validation, i18n completeness and retention.
  - Integration tests with `#[sqlx::test]`, which creates a throwaway database per test against `db`.
  - End-to-end tests over axum's `Router` with `tower::ServiceExt::oneshot`:
    - create invite → respond → change → delete.
    - open RSVP → link shown → add e-mail → confirm in Mailpit → `/lost`.

## 10. Container image (buildah, binary only)

- Build: `cargo zigbuild --release --locked --target x86_64-unknown-linux-musl`, with `SQLX_OFFLINE=true` so no database is needed.
  - Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`.
  - All nodes are **amd64**, so a single-architecture image is enough.
- Image steps:

  ```
  ctr=$(buildah from --arch amd64 --os linux scratch)   # explicit: dev machines may be arm64
  buildah copy  $ctr "$CARGO_TARGET_DIR"/x86_64-unknown-linux-musl/release/fest /fest
  buildah config --entrypoint '["/fest"]' --cmd serve --user 65532:65532 \
                 --port 8080 --label org.opencontainers.image.source=… $ctr
  buildah commit --rm $ctr ghcr.io/vehagn/fest:<tag>
  ```

- The image contains no shell, no CA bundle and no tzdata: the CA roots and zoneinfo are compiled into the binary. It runs as a numeric non-root UID, so it needs no `/etc/passwd`. Expect about 10–20 MB.
- **Images are built in CI, not in the devcontainer.** Rootless buildah there needs user namespaces, which means loosening the dev container's seccomp and AppArmor profiles. We tried that, and it builds a correct image, but chose to keep the dev container locked down. `.github/workflows/ci.yaml` runs `just test` and `just lint` against a Postgres 18 service on every push and pull request; on pushes it then runs `just image` on `ubuntu-24.04` (which ships buildah) and pushes `sha-<commit>`, plus `latest` from `main` and the tag name for `v*` tags, to `ghcr.io/vehagn/fest`. Tool versions match the devcontainer. `just image` and `just push` also run anywhere buildah works natively.
- Probes use `httpGet` on `/healthz` and `/readyz`, since there is no shell for `exec` probes.

## 11. Kubernetes (fits the existing homelab conventions)

| Resource | Notes |
|---|---|
| Namespace | `fest` |
| Postgres | A **CNPG `Cluster`** `fest-postgres`: 1 instance, image `postgresql:18.6-minimal-trixie` (same as dev), 1 Gi on `proxmox-csi`, created with `bootstrap.initdb` (database `fest`, owner `fest`). The CNPG-generated `fest-postgres-app` secret provides `uri`. No backups, on purpose. |
| Deployments | `fest-web` (`serve`, 2 replicas), `fest-admin` (`admin`, 1 replica), `fest-mailer` (`mailer`, 1 replica, `Recreate`). `fest migrate` runs as an **init container** on `fest-web`. |
| CronJob | `fest-janitor`: hourly, with `concurrencyPolicy: Forbid`. |
| HTTPRoutes | Public routes on `gateway/external`. The admin hostname on `gateway/internal`. TLS comes from the existing cert-manager setup. |
| Secrets (SealedSecrets) | One `fest` secret, sealed by `just seal` from the gitignored `deploy/secrets.env`: the link key, the open-RSVP code, the admin hash, and the MailerSend SMTP credentials. Each workload references only the keys it needs. |
| NetworkPolicies (Cilium) | `web` and `admin` accept traffic from the gateways (`fromEntities: ingress`); which gateway reaches which is decided by the HTTPRoutes. `mailer` has no ingress and egress to `smtp.mailersend.net:587` only. `janitor` has egress to the DB only. Postgres accepts ingress from the fest pods and the CNPG operator. |
| Pod security | `runAsNonRoot`, `readOnlyRootFilesystem`, `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`, `seccompProfile: RuntimeDefault`, and the namespace labelled `pod-security.kubernetes.io/enforce: restricted`. |
| Observability | JSON logs. `/metrics` (responses, headcount, rate-limit hits, outbox lag, mail failures) is left for hardening. |
| Delivery | Kustomize in `deploy/` (see `deploy/README.md`), copied to `vehagn/homelab` → `k8s/apps/<group>/fest`, where the group's ApplicationSet picks it up. |

## 12. Justfile (draft)

```
just                     # list recipes
just run                 # serve (:8080) + admin (:8081) locally (+ mailer from milestone 6)
just migrate             # apply migrations to $DATABASE_URL
just prepare             # cargo sqlx prepare → .sqlx/ (commit it)
just psql                # shell into dev db
just invite "Kari & Ola" 2   # create an invitation from the CLI, print link
just hash-password       # argon2 hash for FEST_ADMIN_PASSWORD_HASH
just test                # cargo test (incl. #[sqlx::test] against db)
just lint                # cargo fmt --check, clippy -D warnings, sqlx prepare --check, cargo deny check
just build               # static x86_64 musl binary via cargo zigbuild (SQLX_OFFLINE)
just image [tag]         # buildah: scratch + binary
just push [tag]          # buildah push → ghcr.io/vehagn/fest
just seal                # deploy/secrets.env → deploy/sealed-secrets.yaml
just manifests           # render deploy/ with kustomize (Argo CD does the deploying)
```

## 13. Milestones

0. ✅ **Devcontainer:** Containerfile, compose and `devcontainer.json`, verified by building it and running every tool once.
1. ✅ **Skeleton:** `cargo init`, `rust-toolchain.toml`, `Justfile`, config, `fest serve` with `/healthz`, an askama layout with embedded CSS, nb/en `Messages`.
2. ✅ **Schema:** migrations, the `db` module, `.sqlx/` offline data, `#[sqlx::test]` integration tests.
3. ✅ **Links and the RSVP page:** HMAC signing, `/r/{link}` (respond, party size, rename, export, delete), link hygiene headers, CSRF middleware.
4. ✅ **Admin:** invitations (create, link and QR, revoke, delete), headcount, CSV, basic auth, and `fest invite` on the CLI.
5. ✅ **Open RSVP:** `/join/{code}`, honeypot, rate limit, a "save your link" page, and the on/off flag.
6. ✅ **Optional e-mail:** add and confirm an address, the outbox and mailer (Mailpit locally, MailerSend in the cluster), `/lost`, reminders.
7. ✅ **Janitor and privacy:** retention rules, `/privacy` (nb/en), `docs/ropa.md`.
8. ✅ **Image:** buildah recipe, plus a CI workflow that pushes to GHCR.
9. ✅ **Deploy:** CNPG cluster, workloads, HTTPRoutes, SealedSecrets, NetworkPolicies, Argo CD app.
10. ✅ **Hardening and polish:** strict CSP (no inline styles or scripts anywhere), HSTS, `Cross-Origin-Opener-Policy` and `-Resource-Policy`, a 15-second request timeout, `cargo deny` in CI, and a load sanity check (5000 RSVP-page requests at concurrency 50: p50 0.5 ms, p99 4.5 ms). Still optional: `LISTEN/NOTIFY` for the mailer (it polls every 5 seconds), Authelia OIDC for the admin UI, and `/metrics`.

## 14. Decisions

- **Language:** Rust (no preference given; the devcontainer makes the toolchain a non-issue).
- **RSVP style:** both personal invite links and an open name-only RSVP (§5.1, §5.2).
- **E-mail:** an optional extra for guests (§5.3); the `mailer` workload is part of v1.
- **Page languages:** Norwegian (bokmål) and English.
- **Capacity:** no limit and no waitlist.
- **Open RSVP party size:** up to 2 (the guest plus one).
- **Root page `/`:** event info only; RSVP needs an invite link or the join link.
- **Retention:** all guest data is deleted 30 days after the event.
- **Exposure:** directly through `gateway/external`.
- **E-mail provider:** MailerSend (existing account).

## 15. Remaining defaults (say if you want them changed)

1. **RSVP deadline:** an optional config value, unset by default (answers can change until the event).
2. **Registry and CI:** GHCR, with GitHub Actions building and pushing the image.
