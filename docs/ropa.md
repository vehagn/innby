# Record of processing (GDPR Art. 30) and legitimate-interest assessment

This is the record for **Innby**, the RSVP site for a host's private events. Fill in the
placeholders in *italics* before go-live and keep this file with the deployment.

## 1. Record of processing activities

| Field | Entry |
|---|---|
| Controller | *Host's name*, *contact e-mail* (the same as `INNBY_CONTROLLER_NAME` and `INNBY_CONTROLLER_EMAIL` on `/privacy`). No representative or DPO is required at this scale. |
| Purposes | 1. Planning the host's events: who is coming to each, and how many. 2. Optional: sending a guest their own RSVP link, reminders, and word of a change to the event's time or place by e-mail. 3. Letting guests see who else is invited (the guest list). 4. Optional: high score tables for minigames the host offers. |
| Categories of data subjects | Invited guests; people who sign up through a group link the host made; people the host gives a role for one event, to help run it; hosts who log in with a password made in the admin UI. |
| Categories of personal data | Name or invitation label; response; party size; how the name shows on the guest list; the group link used to sign up; the answer to the host's own question, if the event asks one; language; created, answered and consent timestamps; privacy notice version. Optional: e-mail address and its confirmation status. Optional, on the guest's own device only: their link in a cookie, one per invitation they ask the device to remember; the host stores nothing for it. Optional, when the host offers minigames and the guest plays one from their page: their best score in it. For a helper with a role for one event: the user name they log in with, and that role. For a host made in the admin UI: their user name, role, an argon2 hash of their password, and when they were made. No special categories (no dietary or health data). IP addresses are never stored: they're held in memory for at most an hour for rate limiting. |
| Legal bases | Purposes 1, 3 and 4: legitimate interest, Art. 6(1)(f), see §2; a guest takes part in 4 only by playing from their page, and only their best score in each game is kept. Purpose 2: consent, Art. 6(1)(a), recorded with `email_consent_at` and `privacy_version`, withdrawn by removing the address. |
| Recipients | The host, and anyone the host gives an Innby role, in the config, at their identity provider or for one event: a viewer sees the guest list and headcount, an editor or admin also the e-mail status and invitation links. Other guests (anyone with a personal or group link) see the guest list: names of guests who haven't declined, as each guest chose (full name, first name and initial, or not shown), defaulting to first name and initial before answering and the full name once coming, with how many others a coming guest brings ("+1"). With minigames, other guests see each guest's best score in each game, under their name as on the guest list (or "a guest"); the games run on a separate origin of the host's and receive only a token that reads and saves that guest's scores, never their link. Processor: MailerSend UAB (Lithuania) for e-mail delivery, under its DPA. |
| Transfers outside the EU/EEA | None intended. *Check MailerSend's sub-processor list before go-live and note any transfer and its safeguard here.* |
| Retention | Unconfirmed e-mail addresses: 48 hours. Records of sent mail: 7 days. Everything else: 30 days after its event starts, when the event is deleted with it. Enforced hourly by `innby janitor`. Hosts made in the admin UI: until an admin deletes them. Database backups: *retention ≤ 30 days after the event, or describe the backup policy here*. |
| Security measures (Art. 32) | TLS at the gateway. Access by HMAC-signed links; no secrets stored in the database. `Cache-Control: no-store` on link pages and the front page, `Referrer-Policy: no-referrer` everywhere, no third-party resources. Admin UI behind a login against argon2 hashes, or through the host's OpenID Connect provider, that sets an HttpOnly JWT cookie; logging out revokes it. Roles, for every event or for one, limit what each host may see and change. The admin UI can be kept to an internal network (the homelab's is). Separate workloads with least-privilege network policies; only the mailer has SMTP credentials. Non-root, read-only containers. Logs contain route patterns and RSVP ids, never guests' names, addresses or links; a login logs the host's user name and role. |

## 2. Legitimate-interest assessment (purposes 1 and 3)

**Purpose test.** The host has a genuine interest in knowing who is coming to their
event and how many, to plan food, space and seating. Without this, the event can't be
organised. Guests have an interest in seeing who else is invited, as they would on a
guest list or in a group chat.

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
- *Impact.* Minimal. Other guests see only names on the guest list: a first name and
  initial until the guest answers, the full name once they're coming unless they choose
  otherwise, and nothing for those who decline. Next to a coming guest's name they see
  how many others the guest brings ("+1"), which the guest can hide with their name.
  Answers beyond "coming" or "not answered" and e-mail addresses are never shown to
  other guests. There is no profiling, and everything is deleted 30 days after the
  event. Group links can be forwarded beyond the group, so they carry the same
  minimal defaults, and the host can replace a link that spreads too far.
- *Safeguards.* Information on first visit and at sign-up (Art. 13/14), a choice of
  how the name shows on the guest list, including not at all (Art. 21), the guest list on
  a page of its own so that link previews in chat apps never receive names, a
  `robots.txt` and `noindex` that keep crawlers out, a full notice at `/privacy`,
  self-service access, export, rectification and deletion on every RSVP page, and
  objection handled as deletion.

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
