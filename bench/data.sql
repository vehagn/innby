-- What `just bench` loads on top of seed/sample.sql: an event fifteen times the size
-- Innby is made for, "Bryllup", with 1000 invitations (a third with a confirmed address,
-- answers in every state) and 10 group links of 50 sign-ups each; 50 more events, for
-- the list of events; and helper, a user in .env.example with no role of their own, as
-- an editor of the wedding.
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
