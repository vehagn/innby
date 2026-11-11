//! Icons: line drawings in the text's colour, as inline SVG, hidden from screen readers,
//! which read the words beside them. Both sites' tab bars use them.

/// A tab in a tab bar, the admin's or a guest's, as the templates draw it.
pub struct TabLink {
    pub href: String,
    pub label: &'static str,
    /// Inline SVG, hidden from screen readers, which read the label.
    pub icon: &'static str,
    pub current: bool,
}

/// An icon's drawing in the SVG every icon shares.
macro_rules! svg {
    ($drawing:literal) => {
        concat!(
            r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true">"#,
            $drawing,
            "</svg>"
        )
    };
}

/// A calendar page.
pub const CALENDAR: &str =
    svg!(r#"<rect x="4" y="5" width="16" height="15" rx="2"/><path d="M4 10h16M8 3v4M16 3v4"/>"#);

/// A calendar page with a plus.
pub const CALENDAR_PLUS: &str = svg!(
    r#"<rect x="4" y="5" width="16" height="15" rx="2"/><path d="M4 10h16M8 3v4M16 3v4M12 13v5M9.5 15.5h5"/>"#
);

/// One person.
pub const PERSON: &str =
    svg!(r#"<circle cx="12" cy="8" r="4"/><path d="M4 21c0-4.4 3.6-8 8-8s8 3.6 8 8"/>"#);

/// One person with a tick: a guest who answered.
pub const PERSON_TICK: &str = svg!(
    r#"<circle cx="9" cy="8" r="3.5"/><path d="M3 20c0-3.3 2.7-6 6-6 1.6 0 3 .6 4.1 1.6M15 18l2 2 4-4"/>"#
);

/// An envelope.
pub const ENVELOPE: &str =
    svg!(r#"<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>"#);

/// Three people: a group.
pub const PEOPLE: &str = svg!(
    r#"<circle cx="12" cy="8" r="3"/><circle cx="5" cy="10" r="2"/><circle cx="19" cy="10" r="2"/><path d="M6.5 20c0-3 2.5-5.5 5.5-5.5s5.5 2.5 5.5 5.5M2 18c0-1.9 1.1-3.4 2.7-3.9M22 18c0-1.9-1.1-3.4-2.7-3.9"/>"#
);

/// A QR code.
pub const QR_CODE: &str = svg!(
    r#"<rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><path d="M14 14h2v2M20 14v.01M14 20h.01M17 17v3h3"/>"#
);

/// A game controller.
/// A speech bubble, for the message board.
pub const SPEECH: &str =
    svg!(r#"<path d="M4 5.5h16v10H10l-4.5 3.5v-3.5H4z"/><path d="M8 9.5h8M8 12.5h5"/>"#);
pub const GAMEPAD: &str = svg!(
    r#"<rect x="2.5" y="7" width="19" height="11" rx="5.5"/><path d="M7.5 11v3M6 12.5h3M15.5 12h.01M18 14h.01"/>"#
);

/// A pencil.
pub const PENCIL: &str = svg!(r#"<path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4"/>"#);

/// A globe, as the language menu has.
pub const GLOBE: &str = svg!(
    r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3"/>"#
);

/// A key.
pub const KEY: &str =
    svg!(r#"<circle cx="8" cy="15" r="4"/><path d="M10.8 12.2L20 3M16 7l3 3M14 9l2 2"/>"#);
