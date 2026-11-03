-- The host's logins, kept as LLDAP keeps them. A login gets a JWT that lasts a day and a
-- refresh token that gets new JWTs for a month. Neither table holds personal data.

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
