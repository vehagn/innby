-- When an event ends, if the host says: shown beside its start, and the end of the entry
-- in a guest's calendar. Null for an event with no set end.
alter table event
    add column ends_at timestamptz,
    add constraint event_ends_after_it_starts check (ends_at > starts_at);
