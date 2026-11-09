-- Hosts who log in with a password, made by an admin on the admin UI's Users tab, beside
-- those in the config (INNBY_ADMIN_USER, INNBY_ADMIN_USERS), whose names they can't take.
-- Unlike the other login tables, this holds personal data, a host's user name, kept until
-- an admin deletes them.
create table admin_user (
    name          text primary key check (name ~ '^[A-Za-z0-9._@-]{1,100}$'),
    -- Their role for every event; null for none, so they see only the events that give
    -- them one.
    role          text check (role in ('admin', 'editor', 'viewer')),
    -- argon2, as `innby hash-password` makes it.
    password_hash text not null check (password_hash like '$argon2%'),
    -- Bumped by a new password. Sessions carry it, so a new password logs the user out
    -- everywhere.
    version       int not null default 0,
    created_at    timestamptz not null default now()
);
