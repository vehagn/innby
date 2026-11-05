-- Sample data for trying Innby out and for working on it: three events, with guests in
-- every state. Load it with `just db seed`, which runs it in one transaction.
--
-- It's made up: names from no one in particular, and addresses at example.com and
-- example.org, which can't receive mail. Dates are relative to today, so the sample
-- stays current:
--
--   Julebord     in five weeks; answers close a week before. Six languages, three
--                group links, and guests in every state: coming, not coming, no
--                answer, with and without an e-mail address, each way of being listed.
--   Sommerfest   in half a year, just begun: one group link, a few invitations.
--   Bursdag      ten days ago, so answers are closed; the janitor deletes it 30 days
--                after it started.
--
-- Julebord offers the example minigames, with some scores. Ola, a user in .env.example
-- with no role of their own, helps run Sommerfest and may see who's coming to Julebord.
--
-- The ids are fixed, so loading it again replaces the sample (and nothing else) and
-- every guest keeps the same link. Links are signed from the ids with the
-- INNBY_LINK_SECRET; open them from the admin UI.

-- An evening in Oslo, some days from today.
create function pg_temp.oslo(days int, time_of_day time) returns timestamptz
language sql as $$ select (current_date + days + time_of_day) at time zone 'Europe/Oslo' $$;

delete from event where id in (
    '00000000-5eed-7000-8000-000000000001',
    '00000000-5eed-7000-8000-000000000002',
    '00000000-5eed-7000-8000-000000000003'
);

insert into event (id, title, description, location, starts_at, time_zone, rsvp_deadline)
values (
    '00000000-5eed-7000-8000-000000000001',
    '{"nb": "Julebord", "nn": "Julebord", "en": "Christmas party", "en-us": "Holiday party",
      "zh-hans": "圣诞晚会", "zh-hant": "聖誕晚會"}',
    '{"nb": "Velkommen til årets julebord!\n\nVi starter med gløgg klokka sju, og middagen serveres halv åtte. Pent antrekk.",
      "nn": "Velkomen til årets julebord!\n\nVi startar med gløgg klokka sju, og middagen blir servert halv åtte. Pent antrekk.",
      "en": "Welcome to this year''s Christmas party!\n\nMulled wine at seven, and dinner at half past. Dress smartly.",
      "en-us": "Welcome to this year''s holiday party!\n\nMulled wine at seven, and dinner at seven thirty. Dress up.",
      "zh-hans": "欢迎参加今年的圣诞晚会！\n\n七点供应热红酒，七点半开席。请着正装。",
      "zh-hant": "歡迎參加今年的聖誕晚會！\n\n七點供應熱紅酒，七點半開席。請著正裝。"}',
    'Storgata 1, 0155 Oslo', pg_temp.oslo(35, '19:00'), 'Europe/Oslo', pg_temp.oslo(28, '23:59')
), (
    '00000000-5eed-7000-8000-000000000002',
    '{"nb": "Sommerfest", "en": "Summer party"}',
    '{"nb": "Grill, bading og lange kvelder. Ta med badetøy!",
      "en": "Barbecue, swimming and long evenings. Bring your swimwear!"}',
    'Huk, Bygdøy', pg_temp.oslo(182, '15:00'), 'Europe/Oslo', null
), (
    '00000000-5eed-7000-8000-000000000003',
    '{"nb": "Bursdag", "en": "Birthday party"}',
    '{"nb": "Kake og kaffe, og leker for de minste.",
      "en": "Cake and coffee, and games for the little ones."}',
    '', pg_temp.oslo(-10, '18:00'), 'Europe/Oslo', null
);

-- Each event's QR codes look their own.
update event set qr_shape = 'dots', qr_color = '#b71c1c', qr_icon = 'star'
where id = '00000000-5eed-7000-8000-000000000001';
update event set qr_shape = 'rounded', qr_color = '#1565c0', qr_icon = 'emoji', qr_emoji = '☀️'
where id = '00000000-5eed-7000-8000-000000000002';

insert into invite_group (id, event_id, name, max_party_size, created_at) values
    ('00000000-5eed-7000-8000-000000000101', '00000000-5eed-7000-8000-000000000001', 'Klatreklubben', 2, now() - interval '20 days'),
    ('00000000-5eed-7000-8000-000000000102', '00000000-5eed-7000-8000-000000000001', 'Naboer i Storgata', 3, now() - interval '20 days'),
    ('00000000-5eed-7000-8000-000000000103', '00000000-5eed-7000-8000-000000000001', 'Kolleger', 1, now() - interval '19 days'),
    ('00000000-5eed-7000-8000-000000000201', '00000000-5eed-7000-8000-000000000002', 'Venner', 2, now() - interval '1 day'),
    ('00000000-5eed-7000-8000-000000000301', '00000000-5eed-7000-8000-000000000003', 'Familien', 4, now() - interval '40 days');

-- name_display is null until a guest chooses; group sign-ups (those with a group_id)
-- are attending from the start.
insert into rsvp (
    id, event_id, group_id, name, response, party_size, max_party_size, lang,
    email, email_status, email_consent_at, privacy_version, responded_at, name_display,
    created_at
) values
    -- Julebord: invitations.
    ('00000000-5eed-7000-8000-000000001001', '00000000-5eed-7000-8000-000000000001', null,
     'Kari & Ola Nordmann', 'attending', 2, 2, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '12 days', 'full', now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001002', '00000000-5eed-7000-8000-000000000001', null,
     'Per Hansen', 'pending', 1, 1, 'nb', null, 'none', null,
     null, null, null, now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001003', '00000000-5eed-7000-8000-000000000001', null,
     'Ingrid Larsen', 'declined', 1, 2, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '9 days', null, now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001004', '00000000-5eed-7000-8000-000000000001', null,
     'Familien Berg', 'attending', 3, 4, 'nn', null, 'none', null,
     '2026-11-04', now() - interval '8 days', 'partial', now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001005', '00000000-5eed-7000-8000-000000000001', null,
     'Sofie Haugen', 'pending', 1, 2, 'nb', 'sofie@example.com', 'unconfirmed', now() - interval '2 days',
     '2026-11-04', null, null, now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001006', '00000000-5eed-7000-8000-000000000001', null,
     'Lars Johansen', 'attending', 1, 1, 'en', 'lars@example.org', 'confirmed', now() - interval '6 days',
     '2026-11-04', now() - interval '6 days', 'full', now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001007', '00000000-5eed-7000-8000-000000000001', null,
     '李伟', 'attending', 2, 2, 'zh-hans', null, 'none', null,
     '2026-11-04', now() - interval '5 days', 'full', now() - interval '21 days'),
    ('00000000-5eed-7000-8000-000000001008', '00000000-5eed-7000-8000-000000000001', null,
     'Emma Smith', 'pending', 1, 2, 'en-us', null, 'none', null,
     null, null, null, now() - interval '14 days'),
    ('00000000-5eed-7000-8000-000000001009', '00000000-5eed-7000-8000-000000000001', null,
     'Nils Øyen', 'attending', 1, 1, 'nn', null, 'none', null,
     '2026-11-04', now() - interval '3 days', 'hidden', now() - interval '14 days'),
    -- Julebord: sign-ups through group links.
    ('00000000-5eed-7000-8000-000000001101', '00000000-5eed-7000-8000-000000000001', '00000000-5eed-7000-8000-000000000101',
     'Mari Bakke', 'attending', 2, 2, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '11 days', 'full', now() - interval '11 days'),
    ('00000000-5eed-7000-8000-000000001102', '00000000-5eed-7000-8000-000000000001', '00000000-5eed-7000-8000-000000000101',
     'Jonas Lie', 'attending', 1, 2, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '10 days', 'partial', now() - interval '10 days'),
    ('00000000-5eed-7000-8000-000000001103', '00000000-5eed-7000-8000-000000000001', '00000000-5eed-7000-8000-000000000102',
     'Anne-Lise Dahl', 'attending', 3, 3, 'nb', 'anne-lise@example.com', 'confirmed', now() - interval '7 days',
     '2026-11-04', now() - interval '7 days', 'full', now() - interval '7 days'),
    ('00000000-5eed-7000-8000-000000001104', '00000000-5eed-7000-8000-000000000001', '00000000-5eed-7000-8000-000000000102',
     'Tor Eriksen', 'declined', 1, 3, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '2 days', null, now() - interval '7 days'),
    -- Sommerfest: just invited.
    ('00000000-5eed-7000-8000-000000002001', '00000000-5eed-7000-8000-000000000002', null,
     'Kari & Ola Nordmann', 'pending', 1, 2, 'nb', null, 'none', null,
     null, null, null, now() - interval '1 day'),
    ('00000000-5eed-7000-8000-000000002002', '00000000-5eed-7000-8000-000000000002', null,
     'Per Hansen', 'attending', 1, 1, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '6 hours', 'partial', now() - interval '1 day'),
    ('00000000-5eed-7000-8000-000000002003', '00000000-5eed-7000-8000-000000000002', null,
     'Hanne Moe', 'pending', 1, 2, 'en', null, 'none', null,
     null, null, null, now() - interval '1 day'),
    -- Bursdag: over, answers closed.
    ('00000000-5eed-7000-8000-000000003001', '00000000-5eed-7000-8000-000000000003', null,
     'Mormor og morfar', 'attending', 2, 2, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '30 days', 'full', now() - interval '40 days'),
    ('00000000-5eed-7000-8000-000000003002', '00000000-5eed-7000-8000-000000000003', null,
     'Onkel Geir', 'declined', 1, 1, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '25 days', null, now() - interval '40 days'),
    ('00000000-5eed-7000-8000-000000003003', '00000000-5eed-7000-8000-000000000003', null,
     'Silje Nilsen', 'pending', 1, 2, 'nb', null, 'none', null,
     null, null, null, now() - interval '40 days'),
    ('00000000-5eed-7000-8000-000000003101', '00000000-5eed-7000-8000-000000000003', '00000000-5eed-7000-8000-000000000301',
     'Tante Randi', 'attending', 3, 4, 'nb', null, 'none', null,
     '2026-11-04', now() - interval '20 days', 'partial', now() - interval '20 days');

-- Roles for one event, by the name the host logs in with.
insert into event_access (event_id, user_name, role) values
    ('00000000-5eed-7000-8000-000000000001', 'ola', 'viewer'),
    ('00000000-5eed-7000-8000-000000000002', 'ola', 'editor');

-- Julebord offers the example games in games/, which .env.example lists, and some of
-- its guests have played Reaction: time in milliseconds, the least wins.
insert into event_game (event_id, game) values
    ('00000000-5eed-7000-8000-000000000001', 'reaction'),
    ('00000000-5eed-7000-8000-000000000001', 'snake'),
    ('00000000-5eed-7000-8000-000000000001', 'tetris');
insert into game_score (rsvp_id, game, score, scored_at) values
    ('00000000-5eed-7000-8000-000000001001', 'reaction', 287, now() - interval '3 days'),
    ('00000000-5eed-7000-8000-000000001006', 'reaction', 241, now() - interval '2 days'),
    ('00000000-5eed-7000-8000-000000001101', 'reaction', 312, now() - interval '1 day'),
    ('00000000-5eed-7000-8000-000000001009', 'reaction', 265, now() - interval '5 hours');
