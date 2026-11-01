create type rsvp_source as enum ('invite', 'open');
create type rsvp_response as enum ('pending', 'attending', 'declined');
create type email_status as enum ('none', 'unconfirmed', 'confirmed');
create type lang as enum ('nb', 'en');

-- One row per invitation or open RSVP: a single guest or a household.
create table rsvp (
    id               uuid primary key default uuidv7(),
    source           rsvp_source not null,
    -- Label for invitations, self-given name for open RSVPs.
    name             text not null check (length(name) between 1 and 100),
    response         rsvp_response not null default 'pending',
    party_size       int not null default 1,
    max_party_size   int not null default 1 check (max_party_size between 1 and 20),
    -- Bumped to revoke the guest's link; part of the HMAC input.
    link_nonce       int not null default 0,
    lang             lang not null default 'nb',
    -- Optional, normalised (trimmed, lower-case), only kept with consent.
    email            text,
    email_status     email_status not null default 'none',
    email_consent_at timestamptz,
    -- Version of the privacy notice the guest saw when they last acted.
    privacy_version  text,
    responded_at     timestamptz,
    created_at       timestamptz not null default now(),
    check (party_size between 1 and max_party_size),
    check ((email is null) = (email_status = 'none')),
    check ((email_consent_at is null) = (email is null))
);

create unique index rsvp_email_uq on rsvp (email) where email is not null;

create type outbox_kind as enum ('confirm_email', 'email_in_use', 'rsvp_link', 'reminder');

-- Mail jobs for `fest mailer`, written in the same transaction as the change that causes them.
create table outbox (
    id          bigint generated always as identity primary key,
    rsvp_id     uuid not null references rsvp on delete cascade,
    kind        outbox_kind not null,
    created_at  timestamptz not null default now(),
    attempts    int not null default 0,
    next_try_at timestamptz not null default now(),
    sent_at     timestamptz,
    last_error  text
);

create index outbox_due on outbox (next_try_at) where sent_at is null;
create index outbox_rsvp on outbox (rsvp_id);
