-- Sign-up links the host makes, one per group of people, e.g. "Climbing club". Anyone
-- with a group's link can sign up with a name and gets their own RSVP link back.
create table invite_group (
    id             uuid primary key default uuidv7(),
    name           text not null check (length(name) between 1 and 100),
    -- How many people each sign-up may cover.
    max_party_size int not null default 2 check (max_party_size between 1 and 20),
    -- Bumped to revoke the group's link; part of the HMAC input.
    link_nonce     int not null default 0,
    created_at     timestamptz not null default now()
);

-- The group link an open RSVP signed up through. A group with sign-ups can't be deleted.
alter table rsvp add column group_id uuid references invite_group;
create index rsvp_group on rsvp (group_id) where group_id is not null;

-- How the guest's name shows in the guest list that every link shows. Null until the
-- guest chooses: their first name and an initial before they answer, and their full name
-- once they're coming. Declined guests are never listed.
create type name_display as enum ('full', 'partial', 'hidden');
alter table rsvp add column name_display name_display;
