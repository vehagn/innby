//! HMAC-signed links. Possession of a link is the guest's authorisation, and nothing
//! secret is stored: the server recomputes the tag from the key and the row.

use std::{fmt, str::FromStr};

use anyhow::{Result, anyhow, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

/// Tags are truncated to 128 bits, which keeps links short and is still far beyond guessing.
const TAG_LEN: usize = 16;
const MIN_KEY_LEN: usize = 32;

#[derive(Clone)]
pub struct LinkKey(Hmac<Sha256>);

impl fmt::Debug for LinkKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkKey(..)")
    }
}

impl LinkKey {
    /// Parses a standard base64 key of at least 32 bytes, e.g. from `openssl rand -base64 32`.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let key = STANDARD
            .decode(encoded.trim())
            .map_err(|_| anyhow!("not valid base64"))?;
        ensure!(
            key.len() >= MIN_KEY_LEN,
            "must decode to at least {MIN_KEY_LEN} bytes, got {}",
            key.len()
        );
        Ok(Self(
            Hmac::new_from_slice(&key).expect("HMAC takes keys of any length"),
        ))
    }

    /// A MAC over a purpose and the given fields. Every field is length-prefixed, so
    /// different field splits can never produce the same input.
    fn mac(&self, purpose: &str, fields: &[&[u8]]) -> Hmac<Sha256> {
        let mut mac = self.0.clone();
        for field in std::iter::once(purpose.as_bytes()).chain(fields.iter().copied()) {
            mac.update(&(field.len() as u32).to_be_bytes());
            mac.update(field);
        }
        mac
    }

    fn id_mac(&self, purpose: Purpose, id: Uuid, nonce: i32) -> Hmac<Sha256> {
        let purpose = match purpose {
            Purpose::Rsvp => "rsvp",
            Purpose::Group => "group",
        };
        self.mac(purpose, &[id.as_bytes(), &nonce.to_be_bytes()])
    }

    fn email_mac(&self, id: Uuid, email: &str, expires: i64) -> Hmac<Sha256> {
        self.mac(
            "email",
            &[id.as_bytes(), email.as_bytes(), &expires.to_be_bytes()],
        )
    }

    /// The path segment of a guest's RSVP link or a group's sign-up link: `{id}.{tag}`.
    pub fn link(&self, purpose: Purpose, id: Uuid, nonce: i32) -> String {
        let mac = self.id_mac(purpose, id, nonce);
        format!("{}.{}", encode_id(id), encode_tag(mac))
    }

    /// Checks the link's tag in constant time against the row's current nonce.
    pub fn verify(&self, purpose: Purpose, link: &IdLink, nonce: i32) -> bool {
        self.id_mac(purpose, link.id, nonce)
            .verify_truncated_left(&link.tag)
            .is_ok()
    }

    /// A guest's score token for one game: `{id}.{tag}`, signing the game and the
    /// guest's link nonce, so it saves only that guest's scores in that game, and stops
    /// with their link. It goes to the game, on an origin of its own, in place of the
    /// link, which opens everything else.
    pub fn score_token(&self, id: Uuid, nonce: i32, game: &str) -> String {
        let mac = self.score_mac(id, nonce, game);
        format!("{}.{}", encode_id(id), encode_tag(mac))
    }

    /// Checks a score token in constant time against the guest's current nonce.
    pub fn verify_score(&self, token: &IdLink, nonce: i32, game: &str) -> bool {
        self.score_mac(token.id, nonce, game)
            .verify_truncated_left(&token.tag)
            .is_ok()
    }

    fn score_mac(&self, id: Uuid, nonce: i32, game: &str) -> Hmac<Sha256> {
        self.mac(
            "score",
            &[id.as_bytes(), &nonce.to_be_bytes(), game.as_bytes()],
        )
    }

    /// The path segment of an e-mail confirmation link: `{id}.{expires}.{tag}`. It signs
    /// the address too, so it stops working as soon as the guest changes it.
    pub fn email_link(&self, id: Uuid, email: &str, expires: DateTime<Utc>) -> String {
        let expires = expires.timestamp();
        format!(
            "{}.{expires}.{}",
            encode_id(id),
            encode_tag(self.email_mac(id, email, expires))
        )
    }

    /// Checks the tag against the guest's current address. Expiry is the caller's check.
    pub fn verify_email(&self, link: &EmailLink, email: &str) -> bool {
        self.email_mac(link.id, email, link.expires.timestamp())
            .verify_truncated_left(&link.tag)
            .is_ok()
    }
}

fn encode_id(id: Uuid) -> String {
    URL_SAFE_NO_PAD.encode(id.as_bytes())
}

fn encode_tag(mac: Hmac<Sha256>) -> String {
    URL_SAFE_NO_PAD.encode(&mac.finalize().into_bytes()[..TAG_LEN])
}

fn decode_id(encoded: &str) -> Result<Uuid, ()> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(drop)?;
    Uuid::from_slice(&bytes).map_err(drop)
}

fn decode_tag(encoded: &str) -> Result<[u8; TAG_LEN], ()> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(drop)?;
    bytes.try_into().map_err(drop)
}

/// What an `{id}.{tag}` link opens. Each signs a different purpose, so neither kind of
/// link works as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A guest's RSVP page, `/r/{link}`.
    Rsvp,
    /// A group's sign-up page, `/join/{link}`.
    Group,
}

/// A parsed but not yet verified `{id}.{tag}` link: a guest's RSVP link or a group's
/// sign-up link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdLink {
    pub id: Uuid,
    tag: [u8; TAG_LEN],
}

impl FromStr for IdLink {
    type Err = ();

    fn from_str(link: &str) -> Result<Self, ()> {
        let (id, tag) = link.split_once('.').ok_or(())?;
        Ok(Self {
            id: decode_id(id)?,
            tag: decode_tag(tag)?,
        })
    }
}

/// A parsed but not yet verified e-mail confirmation link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailLink {
    pub id: Uuid,
    pub expires: DateTime<Utc>,
    tag: [u8; TAG_LEN],
}

impl FromStr for EmailLink {
    type Err = ();

    fn from_str(link: &str) -> Result<Self, ()> {
        let mut parts = link.split('.');
        let (Some(id), Some(expires), Some(tag), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(());
        };
        Ok(Self {
            id: decode_id(id)?,
            expires: DateTime::from_timestamp(expires.parse().map_err(drop)?, 0).ok_or(())?,
            tag: decode_tag(tag)?,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// base64 of "0123456789abcdef0123456789abcdef".
    pub const KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

    fn key() -> LinkKey {
        LinkKey::from_base64(KEY).unwrap()
    }

    fn id() -> Uuid {
        "0192f0c1-7a3e-7b4c-8d2e-0123456789ab".parse().unwrap()
    }

    #[test]
    fn link_round_trips() {
        let link = key().link(Purpose::Rsvp, id(), 0);
        assert_eq!(link.len(), 22 + 1 + 22);
        let parsed: IdLink = link.parse().unwrap();
        assert_eq!(parsed.id, id());
        assert!(key().verify(Purpose::Rsvp, &parsed, 0));
    }

    #[test]
    fn group_and_rsvp_links_are_not_interchangeable() {
        let group: IdLink = key().link(Purpose::Group, id(), 0).parse().unwrap();
        assert!(key().verify(Purpose::Group, &group, 0));
        assert!(!key().verify(Purpose::Group, &group, 1));
        assert!(!key().verify(Purpose::Rsvp, &group, 0));
        let rsvp: IdLink = key().link(Purpose::Rsvp, id(), 0).parse().unwrap();
        assert!(!key().verify(Purpose::Group, &rsvp, 0));
    }

    #[test]
    fn a_score_token_saves_one_guests_scores_in_one_game() {
        let token: IdLink = key().score_token(id(), 0, "quiz").parse().unwrap();
        assert!(key().verify_score(&token, 0, "quiz"));
        assert!(!key().verify_score(&token, 0, "pinball"));
        // A new link for the guest stops it.
        assert!(!key().verify_score(&token, 1, "quiz"));
        // It isn't their link, nor is their link a token.
        assert!(!key().verify(Purpose::Rsvp, &token, 0));
        let rsvp: IdLink = key().link(Purpose::Rsvp, id(), 0).parse().unwrap();
        assert!(!key().verify_score(&rsvp, 0, "quiz"));
    }

    #[test]
    fn bumped_nonce_invalidates_link() {
        let parsed: IdLink = key().link(Purpose::Rsvp, id(), 0).parse().unwrap();
        assert!(!key().verify(Purpose::Rsvp, &parsed, 1));
    }

    #[test]
    fn other_key_rejects_link() {
        let other = LinkKey::from_base64(&STANDARD.encode([7u8; 32])).unwrap();
        let parsed: IdLink = key().link(Purpose::Rsvp, id(), 0).parse().unwrap();
        assert!(!other.verify(Purpose::Rsvp, &parsed, 0));
    }

    #[test]
    fn tampered_tag_is_rejected() {
        let link = key().link(Purpose::Rsvp, id(), 0);
        let (id_part, tag) = link.split_once('.').unwrap();
        let flipped = if tag.starts_with('A') { 'B' } else { 'A' };
        let tampered = format!("{id_part}.{flipped}{}", &tag[1..]);
        let parsed: IdLink = tampered.parse().unwrap();
        assert!(!key().verify(Purpose::Rsvp, &parsed, 0));
    }

    #[test]
    fn malformed_links_do_not_parse() {
        let link = key().link(Purpose::Rsvp, id(), 0);
        for bad in [
            "",
            "nodot",
            "a.b",
            &link[..30],
            &format!("{link}A"),
            "!!.??",
        ] {
            assert!(bad.parse::<IdLink>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn email_link_is_bound_to_address_and_expiry() {
        let expires = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let link = key().email_link(id(), "kari@example.com", expires);
        let parsed: EmailLink = link.parse().unwrap();
        assert_eq!((parsed.id, parsed.expires), (id(), expires));
        assert!(key().verify_email(&parsed, "kari@example.com"));
        assert!(!key().verify_email(&parsed, "ola@example.com"));

        let later = link.replace("1800000000", "1900000000");
        assert!(!key().verify_email(&later.parse().unwrap(), "kari@example.com"));
        // An RSVP tag over the same id is no e-mail tag.
        let rsvp_tag = key().link(Purpose::Rsvp, id(), 0);
        let (_, rsvp_tag) = rsvp_tag.split_once('.').unwrap();
        let (prefix, _) = link.rsplit_once('.').unwrap();
        let forged: EmailLink = format!("{prefix}.{rsvp_tag}").parse().unwrap();
        assert!(!key().verify_email(&forged, "kari@example.com"));
    }

    #[test]
    fn malformed_email_links_do_not_parse() {
        let expires = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let link = key().email_link(id(), "kari@example.com", expires);
        for bad in [
            "",
            "a.b",
            "a.b.c",
            &format!("{link}.x"),
            &link.replace("1800000000", "soon"),
        ] {
            assert!(bad.parse::<EmailLink>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rejects_short_or_invalid_keys() {
        assert!(LinkKey::from_base64(&STANDARD.encode([1u8; 16])).is_err());
        assert!(LinkKey::from_base64("not base64!").is_err());
    }
}
