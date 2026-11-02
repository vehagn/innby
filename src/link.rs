//! HMAC-signed links. Possession of a link is the guest's authorisation, and nothing
//! secret is stored: the server recomputes the tag from the key and the row.

use std::{fmt, str::FromStr};

use anyhow::{Result, anyhow, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
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

    fn rsvp_mac(&self, id: Uuid, nonce: i32) -> Hmac<Sha256> {
        self.mac("rsvp", &[id.as_bytes(), &nonce.to_be_bytes()])
    }

    /// The path segment of a guest's RSVP link: `{id}.{tag}`.
    pub fn rsvp_link(&self, id: Uuid, nonce: i32) -> String {
        let tag = self.rsvp_mac(id, nonce).finalize().into_bytes();
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(id.as_bytes()),
            URL_SAFE_NO_PAD.encode(&tag[..TAG_LEN])
        )
    }

    /// Checks the link's tag in constant time against the row's current nonce.
    pub fn verify_rsvp(&self, link: &RsvpLink, nonce: i32) -> bool {
        self.rsvp_mac(link.id, nonce)
            .verify_truncated_left(&link.tag)
            .is_ok()
    }
}

/// A parsed but not yet verified RSVP link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsvpLink {
    pub id: Uuid,
    tag: [u8; TAG_LEN],
}

impl FromStr for RsvpLink {
    type Err = ();

    fn from_str(link: &str) -> Result<Self, ()> {
        let (id, tag) = link.split_once('.').ok_or(())?;
        let id = URL_SAFE_NO_PAD.decode(id).map_err(drop)?;
        let tag = URL_SAFE_NO_PAD.decode(tag).map_err(drop)?;
        Ok(Self {
            id: Uuid::from_slice(&id).map_err(drop)?,
            tag: tag.try_into().map_err(drop)?,
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
        let link = key().rsvp_link(id(), 0);
        assert_eq!(link.len(), 22 + 1 + 22);
        let parsed: RsvpLink = link.parse().unwrap();
        assert_eq!(parsed.id, id());
        assert!(key().verify_rsvp(&parsed, 0));
    }

    #[test]
    fn bumped_nonce_invalidates_link() {
        let parsed: RsvpLink = key().rsvp_link(id(), 0).parse().unwrap();
        assert!(!key().verify_rsvp(&parsed, 1));
    }

    #[test]
    fn other_key_rejects_link() {
        let other = LinkKey::from_base64(&STANDARD.encode([7u8; 32])).unwrap();
        let parsed: RsvpLink = key().rsvp_link(id(), 0).parse().unwrap();
        assert!(!other.verify_rsvp(&parsed, 0));
    }

    #[test]
    fn tampered_tag_is_rejected() {
        let link = key().rsvp_link(id(), 0);
        let (id_part, tag) = link.split_once('.').unwrap();
        let flipped = if tag.starts_with('A') { 'B' } else { 'A' };
        let tampered = format!("{id_part}.{flipped}{}", &tag[1..]);
        let parsed: RsvpLink = tampered.parse().unwrap();
        assert!(!key().verify_rsvp(&parsed, 0));
    }

    #[test]
    fn malformed_links_do_not_parse() {
        let link = key().rsvp_link(id(), 0);
        for bad in [
            "",
            "nodot",
            "a.b",
            &link[..30],
            &format!("{link}A"),
            "!!.??",
        ] {
            assert!(bad.parse::<RsvpLink>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rejects_short_or_invalid_keys() {
        assert!(LinkKey::from_base64(&STANDARD.encode([1u8; 16])).is_err());
        assert!(LinkKey::from_base64("not base64!").is_err());
    }
}
