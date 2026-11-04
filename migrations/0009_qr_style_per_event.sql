-- Each event has its own QR code style, which its guests' and groups' codes are drawn in,
-- so each party's printed invitations and posters can look its own. Events that exist
-- keep the style every code had until now.
alter table event
    add column qr_shape qr_shape not null default 'rounded',
    -- The dark modules' colour, as #rrggbb. The light ones are always white.
    add column qr_color text not null default '#00695c' check (qr_color ~ '^#[0-9a-f]{6}$'),
    add column qr_icon  qr_icon not null default 'none',
    -- The emoji in the middle, when qr_icon is 'emoji'.
    add column qr_emoji text not null default '' check (length(qr_emoji) <= 8);

update event
set qr_shape = style.shape, qr_color = style.color, qr_icon = style.icon, qr_emoji = style.emoji
from qr_style style;

drop table qr_style;
