# Record of processing (GDPR Art. 30) and legitimate-interest assessment

This is the record for **Innby**, the RSVP site for a host's private events. Fill in the
placeholders in *italics* before go-live and keep this file with the deployment.

## 1. Record of processing activities

| Field | Entry |
|---|---|
| Controller | *Host's name*, *contact e-mail* (the same as `INNBY_CONTROLLER_NAME` and `INNBY_CONTROLLER_EMAIL` on `/privacy`). No representative or DPO is required at this scale. Supervisory authority: *the host's* (`INNBY_AUTHORITY_NAME` and `_URL` on `/privacy`). |
| Purposes | 1. Planning the host's events: who is coming to each, and how many. 2. Optional: sending a guest their own RSVP link, reminders, and word of a change to the event's time or place by e-mail. 3. Letting guests see who else is invited (the guest list). 4. Optional: high score tables for minigames the host offers. 5. Optional: a message board where guests and hosts write to each other before the event. |
| Categories of data subjects | Invited guests; people who sign up through a group link the host made; people the host gives a role for one event, to help run it; hosts who log in with a password made in the admin UI. |
| Categories of personal data | Name or invitation label; response; party size; how the name shows on the guest list; the group link used to sign up; the answer to the host's own question, if the event asks one; the answers to the host's open questions, in the guest's own words, if the event asks any; language; created, answered and consent timestamps; privacy notice version. Optional: e-mail address and its confirmation status. Optional, on the guest's own device only: their link in a cookie, one per invitation they ask the device to remember; the host stores nothing for it. Optional, when the host offers minigames and the guest plays one from their page: their best score in it. A host who chooses to play an event's minigames from the admin UI: their user name, and their best score in each game with their role for the event when they set it. Optional, when the host has a message board and the guest uses it: the text of their posts and when they were posted, and their up or down votes on others' posts. A host's posts carry their user name and their role for the event when they wrote them, and a host's votes on posts their user name. For a helper with a role for one event: the user name they log in with, and that role. For each event: the user name of the host who made it. For a host made in the admin UI: their user name, role, an argon2 hash of their password, and when they were made; optionally their e-mail address, for links to set a password, and the mail sent with one. A generated UUID also distinguishes each stored host account from a later account with the same name; it is an internal pseudonymous identifier, not guest data, and is deleted with the account. No special categories are asked for (no dietary or health data); a guest could still write about them in an answer to an open question or on a message board, which the form, the board and the notice ask them not to, the host's form asks hosts not to ask about, and the host can delete. IP addresses are never stored: they're held in memory for at most an hour for rate limiting. |
| Legal bases | Purposes 1, 3, 4 and 5: legitimate interest, Art. 6(1)(f), see §2; a guest takes part in 4 only by playing from their page, and a host only by choosing to play in the admin UI, and only the best score in each game is kept, and in 5 only by posting or voting, and may delete their posts. Purpose 2: consent, Art. 6(1)(a), recorded with `email_consent_at` and `privacy_version`, withdrawn by removing the address. |
| Recipients | The host, and anyone the host gives an Innby role, in the config, at their identity provider or for one event: a viewer sees the guest list and headcount, with the answers to the host's questions, an editor or admin also the e-mail status and invitation links. Other guests (anyone with a personal or group link) see the guest list: names of guests who haven't declined, as each guest chose (full name, first name and initial, or not shown), defaulting to first name and initial before answering and the full name once coming, with how many others a coming guest brings ("+1"). With minigames, other guests see each guest's best score in each game, under their name as on the guest list (or "a guest"); the games run on a separate origin of the host's and receive only a token that reads and saves that guest's scores, never their link. A host who plays is ranked with the guests, signed with their login name, up to any `@`, and their role for the event, as "Hagen (Editor)"; their token names only a row of their own, never their name, and lasts 12 hours. With a message board, other guests see each post's text, signed with its guest's name as on the guest list, which a guest must be on to post, or their first name and initial if they leave it later; hosts' posts signed with the host's login name, up to any `@`, and their role for the event, as "Hagen (Editor)"; and each post's total of guests' and hosts' votes, never who voted how; the host sees who wrote each post. Processor: the service mail is sent through, for e-mail delivery to guests and to hosts with an address, under its DPA, as `INNBY_MAIL_PROCESSOR` names it on `/privacy` (in the homelab, MailerSend UAB, Lithuania); none if the host runs their own mail server. The hosting provider, if the site runs on one's servers, as `INNBY_HOSTING` names it; none on the host's own. |
| Transfers outside the EU/EEA | None intended. *Check the mail processor's sub-processor list before go-live and note any transfer and its safeguard here.* |
| Retention | Unconfirmed e-mail addresses: 48 hours. Records of sent mail: 7 days. Everything else: 30 days after its event starts, when the event is deleted with it. A guest's board votes and the text of their posts go with the guest; a post with replies stays as an empty "[deleted]" so the replies keep their place. A host's name and role on a post, and their votes on it, go with the post, which a host may delete, or with the event. Turning a board off keeps its posts until the event is deleted. A playing host's user name and scores go when they stop playing, which they may at any time, or with the event; their scores in a game also go when the event stops offering it. Deleting a host's account leaves both, as it leaves the name of the host who made an event. Enforced hourly by `innby janitor`. Hosts made in the admin UI, with their address: until an admin deletes them, or they or an admin remove the address. Their account UUID is deleted at the same time as their account; it has no separate retention period. Database backups: *retention ≤ 30 days after the event, or describe the backup policy here*. |
| Security measures (Art. 32) | TLS at the gateway. Access by HMAC-signed links; no secrets stored in the database. `Cache-Control: no-store` on link pages and the front page, `Referrer-Policy: no-referrer` everywhere, no third-party resources. Admin UI behind a login against argon2 hashes, or through the host's OpenID Connect provider, that sets an HttpOnly JWT cookie; logging out revokes it. Roles, for every event or for one, limit what each host may see and change. The admin UI can be kept to an internal network: *say whether it is*. Separate workloads with least-privilege network policies; only the mailer has SMTP credentials. Non-root, read-only containers. Logs contain route patterns, RSVP ids, playing hosts' row ids, job ids, actions and bounded roles or outcomes, never guests' or hosts' names, addresses, passwords, bearer tokens or signed links. An error is logged by its kind, a database's by its code and the constraint or table it names, never by its own message, which could quote a value. SMTP errors stay in the outbox, not the logs. Metrics, if turned on, are counts by route pattern and status, with no personal data, served only inside the cluster. |

## 2. Legitimate-interest assessment (purposes 1, 3 and 5)

**Purpose test.** The host has a genuine interest in knowing who is coming to their
event and how many, to plan food, space and seating. Without this, the event can't be
organised. Guests have an interest in seeing who else is invited, as they would on a
guest list or in a group chat.

**Necessity test.** The data is the minimum for that: a name (or a household label)
to know who answered, the answer, and the number of people. The RSVP asks for no
dietary information. Two parts are free text, which guests choose to write: the optional
answers to a host's open questions, and the optional message board, see below. Asking for consent instead would not fit: the host
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
  Answers beyond "coming" or "not answered", including those to the host's questions,
  and e-mail addresses are never shown to other guests. There is no profiling, and everything is deleted 30 days after the
  event. Group links can be forwarded beyond the group, so they carry the same
  minimal defaults, and the host can replace a link that spreads too far.
- *Safeguards.* Information on first visit and at sign-up (Art. 13/14), a choice of
  how the name shows on the guest list, including not at all (Art. 21), the guest list on
  a page of its own so that link previews in chat apps never receive names, a
  `robots.txt` and `noindex` that keep crawlers out, a full notice at `/privacy`,
  self-service access, export, rectification and deletion on every RSVP page, and
  objection handled as deletion.
- *The message board (purpose 5).* Off unless the host turns it on, for one event. Guests
  write only if they choose to, and only while their name is on the guest list, which
  they choose; they sign as it shows them, or by first name and initial if they leave it
  later, so nobody writes anonymously. They may delete their posts at any time; deleting
  their RSVP deletes the text of every post and every vote. Votes are never shown, only
  their sum. Free text can hold anything, special categories too, so the notice asks
  guests not to write about anyone's health, beliefs or other sensitive matters, and the
  host can delete any post. Nothing is moderated by machine.
- *Open questions.* Off unless the host asks some, at most five for one event, and every
  answer is optional and at most 500 characters. Only the hosts see them, on the Guests
  tab, the guest's page and in the CSV export, and the guest sees, changes and exports
  their own. Like posts they can hold anything, so the host's form asks hosts not to
  ask about health, diet, faith or the like, and the form and the notice ask guests not
  to write about anyone's health or beliefs. A reworded question loses its answers, and
  the answers go with the RSVP: when the guest deletes it, or with the event.

**Conclusion.** The host's interest is legitimate, the processing is necessary and
minimal, and it doesn't override the guests' interests or rights. Legitimate interest
is an appropriate basis.

*Reviewed by: host's name, date.*

## 3. Before go-live

- [ ] Fill in the placeholders above.
- [ ] Set `INNBY_MAIL_PROCESSOR` (the chart's `controller.mailProcessor`) to the mail
      service, as the privacy notice names it, or leave it unset for your own server.
- [ ] Set `INNBY_HOSTING` (`controller.hosting`) if the site runs on a provider's
      servers, and accept its DPA too.
- [ ] Set `INNBY_AUTHORITY_NAME` and `INNBY_AUTHORITY_URL` (`controller.authority`) to
      your data protection authority, or leave them unset for the notice's general
      wording.
- [ ] Accept the mail service's DPA; check its sub-processors.
- [ ] Verify the sender domain with the mail service (SPF, DKIM, DMARC).
- [ ] Set CNPG backup retention to end at most 30 days after the event, or note the policy in §1.
- [ ] Bump `PRIVACY_VERSION` in `src/web/mod.rs` if the notice changes.
