# Record of processing (GDPR Art. 30) and legitimate-interest assessment

This is the record for **fest**, the RSVP site for one private event. Fill in the
placeholders in *italics* before go-live and keep this file with the deployment.

## 1. Record of processing activities

| Field | Entry |
|---|---|
| Controller | *Host's name*, *contact e-mail* (the same as `FEST_CONTROLLER_NAME` and `FEST_CONTROLLER_EMAIL` on `/privacy`). No representative or DPO is required at this scale. |
| Purposes | 1. Planning one event: who is coming and how many. 2. Optional: sending a guest their own RSVP link and reminders by e-mail. |
| Categories of data subjects | Invited guests; people who sign up through the open RSVP link. |
| Categories of personal data | Name or invitation label; response; party size; language; created, answered and consent timestamps; privacy notice version. Optional: e-mail address and its confirmation status. No special categories (no dietary or health data). IP addresses are never stored: they're held in memory for at most an hour for rate limiting. |
| Legal bases | Purpose 1: legitimate interest, Art. 6(1)(f), see §2. Purpose 2: consent, Art. 6(1)(a), recorded with `email_consent_at` and `privacy_version`, withdrawn by removing the address. |
| Recipients | The host. Processor: MailerSend UAB (Lithuania) for e-mail delivery, under its DPA. |
| Transfers outside the EU/EEA | None intended. *Check MailerSend's sub-processor list before go-live and note any transfer and its safeguard here.* |
| Retention | Unconfirmed e-mail addresses: 48 hours. Records of sent mail: 7 days. Everything else: 30 days after the event starts. Enforced hourly by `fest janitor`. Database backups: *retention ≤ 30 days after the event, or describe the backup policy here*. |
| Security measures (Art. 32) | TLS at the gateway. Access by HMAC-signed links; no secrets stored in the database. `Cache-Control: no-store` on link pages, `Referrer-Policy: no-referrer` everywhere, no third-party resources. Admin UI only on the internal gateway, behind basic auth with an argon2 hash. Separate workloads with least-privilege network policies; only the mailer has SMTP credentials. Non-root, read-only containers. Logs contain route patterns and RSVP ids, never names, addresses or links. |

## 2. Legitimate-interest assessment (purpose 1)

**Purpose test.** The host has a genuine interest in knowing who is coming to their
event and how many, to plan food, space and seating. Without this, the event can't be
organised.

**Necessity test.** The data is the minimum for that: a name (or a household label)
to know who answered, the answer, and the number of people. There is no free-text
field and no dietary information. Asking for consent instead would not fit: the host
creates invitations before the guest has had any say, and an RSVP is something the
guest expects to give.

**Balancing test.**

- *Relationship and expectations.* Guests were invited by the host or chose to sign up
  through a link the host shared. Recording an RSVP is exactly what they expect.
- *Nature of the data.* Low sensitivity: names and attendance at a private social
  event. No special categories.
- *Impact.* Minimal. The data is never published or shared beyond the host and the
  mail processor, there is no profiling, and it's deleted 30 days after the event.
- *Safeguards.* Information on first visit and at sign-up (Art. 13/14), a full
  notice at `/privacy`, self-service access, export, rectification and deletion on
  every RSVP page, and objection handled as deletion.

**Conclusion.** The host's interest is legitimate, the processing is necessary and
minimal, and it doesn't override the guests' interests or rights. Legitimate interest
is an appropriate basis.

*Reviewed by: host's name, date.*

## 3. Before go-live

- [ ] Fill in the placeholders above.
- [ ] Accept MailerSend's DPA; check its sub-processors.
- [ ] Verify the sender domain in MailerSend (SPF, DKIM, DMARC).
- [ ] Set CNPG backup retention to end at most 30 days after the event, or note the policy in §1.
- [ ] Bump `PRIVACY_VERSION` in `src/web/mod.rs` if the notice changes.
