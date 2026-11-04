-- Events, which the host creates in the admin UI. Before this, the one event came from the
-- INNBY_EVENT_* settings; everything belongs to an event now, and goes with it.
create table event (
    id               uuid primary key default uuidv7(),
    -- In Bokmål; the other languages fall back to it, as the settings did.
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

-- Data from before holds one event. It gets a placeholder, without a title, which
-- `innby migrate` fills in from the INNBY_EVENT_* settings; the host can also edit it.
insert into event (title, starts_at)
select '', now() + interval '1 year'
where exists (select from rsvp) or exists (select from invite_group);

alter table invite_group add column event_id uuid references event on delete cascade;
update invite_group set event_id = (select id from event);
alter table invite_group alter column event_id set not null;
create index invite_group_event on invite_group (event_id);

alter table rsvp add column event_id uuid references event on delete cascade;
update rsvp set event_id = (select id from event);
alter table rsvp alter column event_id set not null;
create index rsvp_event on rsvp (event_id);

-- A confirmed address is unique within an event: one guest can be invited to several.
drop index rsvp_confirmed_email_uq;
create unique index rsvp_confirmed_email_uq on rsvp (event_id, email)
    where email_status = 'confirmed';
