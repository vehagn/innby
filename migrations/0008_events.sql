-- Events, which the host creates in the admin UI. Everything belongs to an event, and goes
-- with it.
create table event (
    id               uuid primary key default uuidv7(),
    -- In Bokmål; the other languages fall back to it.
    title            text not null check (length(title) <= 200),
    title_nn         text,
    title_en         text,
    title_en_us      text,
    title_zh_hans    text,
    title_zh_hant    text,
    description      text not null default '',
    description_nn   text,
    description_en   text,
    description_en_us text,
    description_zh_hans text,
    description_zh_hant text,
    location         text not null default '',
    starts_at        timestamptz not null,
    -- Dates show in the event's time zone, wherever the guest is.
    time_zone        text not null default 'Europe/Oslo',
    -- Guests can answer until then; null means until the event starts.
    rsvp_deadline    timestamptz
);

alter table invite_group add column event_id uuid not null references event on delete cascade;
create index invite_group_event on invite_group (event_id);

alter table rsvp add column event_id uuid not null references event on delete cascade;
create index rsvp_event on rsvp (event_id);

-- A confirmed address is unique within an event: one guest can be invited to several.
drop index rsvp_confirmed_email_uq;
create unique index rsvp_confirmed_email_uq on rsvp (event_id, email)
    where email_status = 'confirmed';
