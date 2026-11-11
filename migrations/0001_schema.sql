-- Innby's schema. Events hold everything else: their guests' answers, the group links
-- guests sign up through, the mail queued for those guests, who besides the hosts may
-- see the event, the games it offers with their scores, and its message board; deleting
-- an event deletes all of it. The admin's login tables stand apart; of them, only the password users an
-- admin makes hold personal data.
--
-- Until Innby is live, this file is the whole schema and changes in place, and a
-- database made from an earlier one is dropped and made again. Once it's live, changes
-- go in migrations of their own, beside this file.

-- A language, by the code its file in locales/ has, as `en-us`. The languages are
-- whatever files there are, so the database checks the shape of a code rather than a
-- list; the app reads a code it no longer has as its base language.
create function lang_ok(code text) returns boolean
language sql immutable as $$
    select code ~ '^[a-z0-9]+(-[a-z0-9]+)*$' and length(code) <= 20
$$;
-- Whether these are all language codes: a set of languages, as the site and each event
-- keep the ones they offer.
create function langs_ok(codes text[]) returns boolean
language sql immutable as $$
    select coalesce(bool_and(lang_ok(code)), true) from unnest(codes) code
$$;
-- Whether a map of texts by language code holds only language codes, each a text
-- of at most `longest` characters with no control characters, but for line breaks and
-- tabs when `lines` allows them. The app checks the same; this keeps a stray query or
-- a crafted import from storing what the pages can't show.
create function texts_ok(texts jsonb, longest int, lines boolean) returns boolean
language sql immutable as $$
    select jsonb_typeof(texts) = 'object' and coalesce(bool_and(
        lang_ok(key)
        and jsonb_typeof(value) = 'string'
        and length(value #>> '{}') <= longest
        and (value #>> '{}') !~ case
            when lines then '[\x01-\x08\x0b\x0c\x0e-\x1f\x7f]' else '[[:cntrl:]]' end
    ), true)
    from jsonb_each(case jsonb_typeof(texts) when 'object' then texts else '{}' end)
$$;

create type qr_shape as enum ('square', 'rounded', 'dots');
-- An event's message board: off, none at all, its posts kept; hidden, for hosts alone;
-- hosts, where hosts post and guests read and vote; open, where guests post too.
create type board_state as enum ('off', 'hidden', 'hosts', 'open');
create type qr_icon as enum ('none', 'heart', 'star', 'emoji');

-- One party, as the host describes it, with the style its QR codes are drawn in.
create table event (
    id                  uuid primary key default uuidv7(),
    -- The language the host writes the event in: it has a title in it, and the other
    -- languages fall back to it.
    lang                text not null check (lang_ok(lang)),
    -- The name the host who made it logs in with, so a creator may delete it and give
    -- roles for it; null for one made from the command line, or by a user since deleted.
    created_by          text check (length(created_by) between 1 and 254
                                    and created_by !~ '[[:cntrl:]]'),
    -- The languages its pages are offered in, of those the site offers: empty for all
    -- of them. Its own language always is.
    langs               text[] not null default '{}' check (langs_ok(langs)),
    -- Each by language code, as `{"nb": "Julebord", "en": "Christmas party"}`, so a new
    -- language needs no new column.
    title               jsonb not null
        check (coalesce(title ->> lang, '') <> '' and texts_ok(title, 200, false)),
    description         jsonb not null default '{}' check (texts_ok(description, 5000, true)),
    location            text not null default ''
        check (length(location) <= 200 and location !~ '[[:cntrl:]]'),
    starts_at           timestamptz not null,
    -- Null for an event with no set end.
    ends_at             timestamptz check (ends_at > starts_at),
    -- When its start, end or place last changed, once it was made. Guests who answered
    -- before are told, on their page and by mail.
    changed_at          timestamptz,
    -- The host's own question to guests, by language like the title, and its choices,
    -- a line each; none when it's empty in the event's language.
    question            jsonb not null default '{}' check (texts_ok(question, 200, false)),
    choices             jsonb not null default '{}' check (texts_ok(choices, 2000, true)),
    -- Dates show in the event's time zone, wherever the guest is.
    time_zone           text not null default 'Europe/Oslo',
    -- Guests can answer until then; null means until the event starts.
    rsvp_deadline       timestamptz,
    qr_shape            qr_shape not null default 'rounded',
    -- The dark modules' colour, as #rrggbb. The light ones are always white.
    qr_color            text not null default '#00695c' check (qr_color ~ '^#[0-9a-f]{6}$'),
    qr_icon             qr_icon not null default 'none',
    -- The emoji in the middle, when qr_icon is 'emoji'.
    qr_emoji            text not null default '' check (length(qr_emoji) <= 8),
    board               board_state not null default 'off'
);

-- Sign-up links the host makes, one per group of people, e.g. "Climbing club". Anyone
-- with a group's link can sign up with a name and gets their own RSVP link back.
create table invite_group (
    id             uuid primary key default uuidv7(),
    event_id       uuid not null references event on delete cascade,
    name           text not null
        check (length(name) between 1 and 100 and name !~ '[[:cntrl:]]'),
    -- How many people each sign-up may cover.
    max_party_size int not null default 2 check (max_party_size between 1 and 20),
    -- Bumped to revoke the group's link; part of the HMAC input.
    link_nonce     int not null default 0,
    created_at     timestamptz not null default now(),
    -- For sign-ups to name their group and its event together.
    unique (id, event_id)
);

-- An event's groups, in the order they were made (the ids are UUIDv7).
create index invite_group_event on invite_group (event_id, id);

create type rsvp_response as enum ('pending', 'attending', 'declined');
create type email_status as enum ('none', 'unconfirmed', 'confirmed');
-- How the guest's name shows in the guest list that every link shows. Null until the
-- guest chooses: their first name and an initial before they answer, and their full name
-- once they're coming. Declined guests are never listed.
create type name_display as enum ('full', 'partial', 'hidden');

-- One row per invitation or group sign-up: a single guest or a household.
create table rsvp (
    id               uuid primary key default uuidv7(),
    event_id         uuid not null references event on delete cascade,
    -- The group link a sign-up came through, which must be the same event's; none for an
    -- invitation. A group with sign-ups can't be deleted.
    group_id         uuid,
    -- Label for invitations, self-given name for sign-ups.
    name             text not null
        check (length(name) between 1 and 100 and name !~ '[[:cntrl:]]'),
    response         rsvp_response not null default 'pending',
    party_size       int not null default 1,
    max_party_size   int not null default 1 check (max_party_size between 1 and 20),
    name_display     name_display,
    -- Bumped to revoke the guest's link; part of the HMAC input.
    link_nonce       int not null default 0,
    -- The guest's language, for their mail.
    lang             text not null check (lang_ok(lang)),
    -- Optional, normalised (trimmed, lower-case), only kept with consent.
    email            text check (email = lower(email) and length(email) <= 254),
    email_status     email_status not null default 'none',
    email_consent_at timestamptz,
    -- Version of the privacy notice the guest saw when they last acted.
    privacy_version  text,
    responded_at     timestamptz,
    created_at       timestamptz not null default now(),
    -- Their pick among the event's choices, by its place in the list; cleared when the
    -- host changes the list.
    choice           smallint check (choice between 0 and 19),
    -- For a sign-up: the random token of the form it was sent from, so that the same
    -- form sent again, as when the answer to the first was lost on the way, leads to
    -- this sign-up rather than making another.
    sign_up_token    text check (sign_up_token ~ '^[A-Za-z0-9_-]{16,64}$'),
    foreign key (group_id, event_id) references invite_group (id, event_id),
    -- For posts on the board to name their guest and its event together.
    unique (id, event_id),
    check (party_size between 1 and max_party_size),
    check ((email is null) = (email_status = 'none')),
    check ((email_consent_at is null) = (email is null))
);

-- An event's guests, in the order they came (the ids are UUIDv7).
create index rsvp_event on rsvp (event_id, id);
create index rsvp_group on rsvp (group_id) where group_id is not null;
create unique index rsvp_sign_up_token on rsvp (group_id, sign_up_token)
    where sign_up_token is not null;
-- An address is unique within an event once it's confirmed, not before. Until then
-- several RSVPs may hold it, so adding an address never shows whether another guest
-- already has it. One guest can be invited to several events. Address first, so the
-- index also finds every event an address is confirmed for, as /lost asks.
create unique index rsvp_confirmed_email_uq on rsvp (email, event_id)
    where email_status = 'confirmed';

create type outbox_kind as enum (
    'confirm_email', 'email_in_use', 'rsvp_link', 'reminder', 'event_changed'
);

-- Mail jobs for `innby mailer`, written in the same transaction as the change that
-- causes them.
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

-- A role for one event: an admin's role is for every event, so it's not among them.
create type event_role as enum ('viewer', 'editor');

-- Hosts who may see or run one event, by the name they log in with, which an admin gives
-- them on the event's Access tab. Someone with a role of their own (in the config, or at
-- the identity provider) has it for every event; this gives others one event, or more
-- than their own role allows for it. Deleted with the event.
create table event_access (
    event_id  uuid not null references event on delete cascade,
    user_name text not null
        check (length(user_name) between 1 and 254 and user_name !~ '[[:cntrl:]]'),
    role      event_role not null,
    primary key (event_id, user_name)
);

-- The events a host has access to, for their list of events.
create index event_access_user on event_access (user_name);

-- What an admin has picked on the admin UI's Languages tab: the languages the site
-- offers guests, of those it has files for, and the one a guest gets whose browser asks
-- for none of them. One row at most; without it, the site offers every language it has,
-- and English by default. No personal data.
create table site_languages (
    only_row     boolean primary key default true check (only_row),
    offered      text[] not null check (cardinality(offered) > 0 and langs_ok(offered)),
    default_lang text not null check (default_lang = any (offered))
);

-- Admin forms that made something, by the random token each form carries, for a day:
-- the same form sent again, as when the answer to the first was lost on the way, goes
-- where the first went rather than making it all again. No personal data.
create table admin_form_sent (
    token   text primary key check (token ~ '^[A-Za-z0-9_-]{16,64}$'),
    -- The admin page the form led to.
    page    text not null check (length(page) <= 200),
    sent_at timestamptz not null default now()
);

-- The host's logins, kept as LLDAP keeps them. A login gets a JWT that lasts a day and a
-- refresh token that gets new JWTs for a month.

-- Refresh tokens, by their keyed hash.
create table admin_refresh_token (
    digest     bytea primary key,
    expires_at timestamptz not null
);

-- JWTs logged out before they expired.
create table admin_revoked_jwt (
    jti        uuid primary key,
    expires_at timestamptz not null
);

-- Hosts who log in with a password, made by an admin on the admin UI's Users tab, beside
-- those in the config (INNBY_ADMIN_USER, INNBY_ADMIN_USERS), whose names they can't take.
-- Unlike the other login tables, this holds personal data, a host's user name, kept until
-- an admin deletes them.
create table admin_user (
    -- With a letter or digit, so its page's address is never `/users/..`.
    name          text primary key
        check (name ~ '^[A-Za-z0-9._@-]{1,100}$' and name ~ '[A-Za-z0-9]'),
    -- Their role for every event; null for none, so they see only the events that give
    -- them one.
    role          text check (role in ('admin', 'creator', 'editor', 'viewer')),
    -- argon2, as `innby hash-password` makes it.
    password_hash text not null check (password_hash like '$argon2%'),
    -- Bumped by a new password. Sessions carry it, so a new password logs the user out
    -- everywhere.
    version       int not null default 0,
    created_at    timestamptz not null default now()
);

-- The minigames an event offers, by their id in INNBY_GAMES. The games themselves are
-- served elsewhere; see docs/games.md.
create table event_game (
    event_id uuid not null references event on delete cascade,
    game     text not null check (game ~ '^[a-z0-9-]{1,40}$'),
    primary key (event_id, game)
);

-- Each guest's best score in each game, and only that: points, or milliseconds taken.
-- Deleted with the guest, and when the event stops offering the game.
create table game_score (
    rsvp_id   uuid not null references rsvp on delete cascade,
    game      text not null check (game ~ '^[a-z0-9-]{1,40}$'),
    score     bigint not null check (score between 0 and 1000000000000),
    scored_at timestamptz not null default now(),
    primary key (rsvp_id, game)
);

-- Posts on an event's message board (`event.board`), each a thread's first or a reply to
-- another of the same event's. A guest's or a host's: a guest by their RSVP, shown as on
-- the guest list; a host by the name they log in with, shown to guests only as a host.
create table board_post (
    id         uuid primary key default uuidv7(),
    event_id   uuid not null references event on delete cascade,
    -- The post it answers; none for a thread's first.
    parent_id  uuid,
    rsvp_id    uuid,
    host_name  text check (length(host_name) between 1 and 254 and host_name !~ '[[:cntrl:]]'),
    -- Lines and tabs allowed. None for a post deleted, or whose guest was, which stays,
    -- empty, while there are replies to it.
    body       text check (length(body) between 1 and 2000
                           and body !~ '[\x01-\x08\x0b\x0c\x0d\x0e-\x1f\x7f]'),
    created_at timestamptz not null default now(),
    unique (id, event_id),
    foreign key (parent_id, event_id) references board_post (id, event_id) on delete cascade,
    foreign key (rsvp_id, event_id) references rsvp (id, event_id) on delete set null (rsvp_id),
    check (rsvp_id is null or host_name is null),
    check ((body is null) = (rsvp_id is null and host_name is null))
);

create index board_post_event on board_post (event_id, id);
create index board_post_parent on board_post (parent_id) where parent_id is not null;
create index board_post_rsvp on board_post (rsvp_id) where rsvp_id is not null;

-- A post whose guest is deleted loses its text with them.
create function board_post_forget() returns trigger
language plpgsql as $$
begin
    if new.rsvp_id is null and old.rsvp_id is not null then
        new.body := null;
    end if;
    return new;
end
$$;

create trigger board_post_forget before update of rsvp_id on board_post
    for each row execute function board_post_forget();

-- Guests' votes on posts, up or down, one each a post. Never shown, only their sum.
-- Deleted with the guest, and with the post.
create table board_vote (
    post_id uuid not null references board_post on delete cascade,
    rsvp_id uuid not null references rsvp on delete cascade,
    vote    smallint not null check (vote in (-1, 1)),
    primary key (post_id, rsvp_id)
);

create index board_vote_rsvp on board_vote (rsvp_id);
