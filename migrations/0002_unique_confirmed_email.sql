-- An address is unique once it's confirmed, not before. Until then several RSVPs may hold
-- it, so adding an address never shows whether another guest already has it.
drop index rsvp_email_uq;
create unique index rsvp_confirmed_email_uq on rsvp (email) where email_status = 'confirmed';
