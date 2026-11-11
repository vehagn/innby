-- What `just bench` loads on top of seed/sample.sql: an event fifteen times the size
-- Innby is made for, "Bryllup", with 1000 invitations (a third with a confirmed address,
-- answers in every state) and 10 group links of 50 sign-ups each, and an open message
-- board of 500 posts, 150 threads and their replies, with votes, some below zero; 50 more
-- events, for the list of events; and helper, a user in .env.example with no role of
-- their own, as an editor of the wedding.
insert into event (id, lang, title, description, location, starts_at, rsvp_deadline)
values ('00000000-be4c-7000-8000-000000000001', 'nb',
        '{"nb": "Bryllup", "en": "Wedding"}',
        '{"nb": "Vi gifter oss!", "en": "We''re getting married!"}',
        'Oslo', now() + interval '60 days', null);

insert into invite_group (id, event_id, name, max_party_size)
select ('00000000-be4c-7000-8000-' || lpad(g::text, 12, '0'))::uuid,
       '00000000-be4c-7000-8000-000000000001', 'Gruppe ' || g, 2
from generate_series(1001, 1010) g;

insert into rsvp (event_id, name, response, party_size, max_party_size, name_display,
                  email, email_status, email_consent_at, privacy_version, responded_at, lang)
select '00000000-be4c-7000-8000-000000000001', 'Gjest ' || i,
       (array['pending', 'attending', 'declined'])[1 + i % 3]::rsvp_response,
       1, 2, (array['full', 'partial', 'hidden'])[1 + i % 3]::name_display,
       case when i % 3 = 0 then 'gjest' || i || '@example.com' end,
       case when i % 3 = 0 then 'confirmed' else 'none' end::email_status,
       case when i % 3 = 0 then now() end,
       '2026-11-04', now(), 'nb'
from generate_series(1, 1000) i;

insert into rsvp (event_id, group_id, name, response, party_size, max_party_size,
                  name_display, privacy_version, responded_at, lang)
select '00000000-be4c-7000-8000-000000000001',
       ('00000000-be4c-7000-8000-' || lpad((1001 + i % 10)::text, 12, '0'))::uuid,
       'Påmeldt ' || i, 'attending', 1 + i % 2, 2, 'partial', '2026-11-04', now(), 'nb'
from generate_series(1, 500) i;

insert into event (lang, title, starts_at)
select 'nb', jsonb_build_object('nb', 'Fest ' || i), now() + make_interval(days => i)
from generate_series(1, 50) i;

insert into event_access (event_id, user_name, role)
values ('00000000-be4c-7000-8000-000000000001', 'helper', 'editor');

-- The board, as full as it may be (db::board::MAX_POSTS): 400 posts in 150 threads, each
-- by one of the wedding's guests in turn, in a little Markdown, a few votes each and
-- every fifth vote down, which folds some away; and the biggest a page can be, a full
-- thread (MAX_THREAD) of the longest posts (MAX_BODY) in characters of three bytes each,
-- replying each to the last, so it nests as deep as it goes, voted to the top so it's
-- the first page on its own.
update event set board = 'open' where id = '00000000-be4c-7000-8000-000000000001';

create function pg_temp.post(series text, i int) returns uuid
language sql as $$ select ('00000000-be4c-7000-' || series || '-' || lpad(i::text, 12, '0'))::uuid $$;

with guests as (
    select id, row_number() over (order by id) as n
    from rsvp where event_id = '00000000-be4c-7000-8000-000000000001'
)
insert into board_post (id, event_id, parent_id, thread_id, rsvp_id, body, created_at)
select pg_temp.post('8001', i), '00000000-be4c-7000-8000-000000000001',
       case when i > 150 then pg_temp.post('8001', i - 150) end,
       case when i > 150 then pg_temp.post('8001', 1 + (i - 1) % 150) end,
       guests.id,
       'Innlegg ' || i || E': **gleder** meg, se [kartet](https://example.com/' || i || E').\n\n- buss 17:30\n- `taxi` etterpå',
       now() - make_interval(mins => 500 - i)
from generate_series(1, 400) i
join guests on guests.n = 1 + (i * 37) % 1500;

with guests as (
    select id, row_number() over (order by id) as n
    from rsvp where event_id = '00000000-be4c-7000-8000-000000000001'
)
insert into board_post (id, event_id, parent_id, thread_id, rsvp_id, body, created_at)
select pg_temp.post('8002', j), '00000000-be4c-7000-8000-000000000001',
       case when j > 1 then pg_temp.post('8002', j - 1) end,
       case when j > 1 then pg_temp.post('8002', 1) end,
       guests.id, repeat('宴', 2000), now() - make_interval(mins => 100 - j)
from generate_series(1, 100) j
join guests on guests.n = 1 + (j * 41) % 1500;

with guests as (
    select id, row_number() over (order by id) as n
    from rsvp where event_id = '00000000-be4c-7000-8000-000000000001'
)
insert into board_vote (post_id, rsvp_id, vote)
select pg_temp.post('8001', i), guests.id, case when (i + k) % 5 = 0 then -1 else 1 end
from generate_series(1, 400) i
cross join generate_series(1, 10) k
join guests on guests.n = 1 + (i * 13 + k * 101) % 1500
where k <= 1 + i % 10
union all
select pg_temp.post('8002', 1), guests.id, 1
from guests where n <= 20
on conflict do nothing;
