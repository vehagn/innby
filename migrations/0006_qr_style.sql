-- How the QR codes on the admin pages look. One style for the whole event, so printed
-- invitations and posters match. It holds no guest data.
create type qr_shape as enum ('square', 'rounded', 'dots');
create type qr_icon as enum ('none', 'heart', 'star', 'emoji');

create table qr_style (
    -- Always true: the table has one row.
    id    boolean primary key default true check (id),
    shape qr_shape not null default 'rounded',
    -- The dark modules' colour, as #rrggbb. The light ones are always white.
    color text not null default '#00695c' check (color ~ '^#[0-9a-f]{6}$'),
    icon  qr_icon not null default 'none',
    -- The emoji in the middle, when icon is 'emoji'.
    emoji text not null default '' check (length(emoji) <= 8)
);
insert into qr_style default values;
