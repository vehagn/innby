//! QR codes for the admin pages, drawn as SVG in the event's style: the modules' shape and
//! colour, and an icon or emoji in the middle. A code with something in the middle uses
//! the highest error correction, which reads it with the middle hidden.

use std::fmt::Write;

use qrcode::{Color, EcLevel, QrCode};
use serde::Deserialize;

use super::lang;
use crate::db::qr::{Icon, Shape, Style};

/// The light margin around a code, in modules, as the standard asks.
const QUIET_ZONE: usize = 4;

/// The corners' finder patterns are 7 modules square.
const FINDER: usize = 7;

const HEART: &str = "M12 21s-7.6-4.7-9.6-9.3C.9 8.3 3 4.8 6.6 4.8c2.1 0 3.6 1.2 5.4 3.3 1.8-2.1 3.3-3.3 5.4-3.3 3.6 0 5.7 3.5 4.2 6.9C19.6 16.3 12 21 12 21z";
/// A dot, from the left of its module's middle: a circle as two half arcs, back where it
/// began. The fill closes it.
const DOT: &str = "a.45.45 0 1 0 .9 0a.45.45 0 1 0-.9 0";
const STAR: &str = "M12 2.5l2.9 6.1 6.7.8-5 4.6 1.3 6.6L12 17.3l-5.9 3.3 1.3-6.6-5-4.6 6.7-.8z";

/// `data` as an SVG QR code in `style`. Without a size, it fills what the page gives it.
pub fn svg(data: &str, style: &Style, pixels: Option<u32>) -> String {
    let level = match style.icon {
        Icon::None => EcLevel::M,
        _ => EcLevel::H,
    };
    let Ok(code) = QrCode::with_error_correction_level(data.as_bytes(), level) else {
        return String::new();
    };
    let n = code.width();
    let dark = |x: usize, y: usize| code[(x, y)] == Color::Dark;
    // The middle, in modules: about a quarter of the code, odd so it sits on the centre
    // module, and well within what the error correction makes up for.
    let hole = match style.icon {
        Icon::None => 0,
        _ => (n / 4) | 1,
    };
    let hole_at = (n - hole) / 2;
    let in_hole = |x: usize, y: usize| {
        let range = hole_at..hole_at + hole;
        range.contains(&x) && range.contains(&y)
    };
    let in_finder = |x: usize, y: usize| {
        (x < FINDER || x >= n - FINDER) && y < FINDER || x < FINDER && y >= n - FINDER
    };

    let size = n + 2 * QUIET_ZONE;
    let mut svg = String::new();
    let dimensions = pixels.map_or(String::new(), |px| {
        format!(r#" width="{px}" height="{px}""#)
    });
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}"{dimensions} aria-hidden="true"><rect width="{size}" height="{size}" fill="#fff"/><g transform="translate({QUIET_ZONE} {QUIET_ZONE})" fill="{color}"><path d=""##,
        color = style.color,
    );
    let drawn = |x: usize, y: usize| dark(x, y) && !in_finder(x, y) && !in_hole(x, y);
    for y in 0..n {
        let mut x = 0;
        // The last dot drawn in this row: a dot's two arcs end where they began, so the
        // next can be placed from it, in fewer characters.
        let mut last_dot = None;
        while x < n {
            if !drawn(x, y) {
                x += 1;
                continue;
            }
            // Squares in a row are one rectangle; the other shapes go one by one.
            if style.shape == Shape::Square {
                let run = (x..n).take_while(|&x| drawn(x, y)).count();
                let _ = write!(svg, "M{x} {y}h{run}v1h-{run}z");
                x += run;
                continue;
            }
            if style.shape == Shape::Dots {
                match last_dot {
                    Some(last) => {
                        let _ = write!(svg, "m{} 0", x - last);
                    }
                    None => {
                        let _ = write!(svg, "M{x}.05 {y}.5");
                    }
                }
                svg.push_str(DOT);
                last_dot = Some(x);
                x += 1;
                continue;
            }
            let neighbour = |dx: isize, dy: isize| {
                let (x, y) = (x.checked_add_signed(dx), y.checked_add_signed(dy));
                matches!((x, y), (Some(x), Some(y)) if x < n && y < n && dark(x, y))
            };
            rounded_module(&mut svg, x, y, neighbour);
            x += 1;
        }
    }
    svg.push_str(r#""/><path fill-rule="evenodd" d=""#);
    for (x, y) in [(0, 0), (n - FINDER, 0), (0, n - FINDER)] {
        finder(&mut svg, style.shape, x, y);
    }
    svg.push_str(r#""/>"#);
    if hole > 0 {
        centre(&mut svg, style, hole_at, hole);
    }
    svg.push_str("</g></svg>");
    svg
}

/// One rounded module, added to a path, as short as the path syntax allows: a code holds
/// a thousand modules, and the guest and group pages draw one inline. Each corner is
/// round, half a module across, unless a dark neighbour joins it on either side; a
/// straight side between is drawn only where it has length.
fn rounded_module(svg: &mut String, x: usize, y: usize, dark: impl Fn(isize, isize) -> bool) {
    let round = |dx, dy| !(dark(dx, 0) || dark(0, dy));
    let (tl, tr, br, bl) = (round(-1, -1), round(1, -1), round(1, 1), round(-1, 1));
    if !(tl || tr || br || bl) {
        let _ = write!(svg, "M{x} {y}h1v1h-1z");
        return;
    }
    // A side's length: a whole module, less half for each round corner at its ends.
    let side = |a: bool, b: bool| match u8::from(a) + u8::from(b) {
        0 => Some("1"),
        1 => Some(".5"),
        _ => None,
    };
    let _ = write!(svg, "M{x}{} {y}", if tl { ".5" } else { "" });
    // Clockwise from the top left: each side, then the round corner after it.
    for (side, line, corner, arc) in [
        (side(tl, tr), "h", tr, "a.5.5 0 0 1 .5.5"),
        (side(tr, br), "v", br, "a.5.5 0 0 1-.5.5"),
        (side(br, bl), "h-", bl, "a.5.5 0 0 1-.5-.5"),
        (side(bl, tl), "v-", tl, "a.5.5 0 0 1 .5-.5"),
    ] {
        if let Some(length) = side {
            let _ = write!(svg, "{line}{length}");
        }
        if corner {
            svg.push_str(arc);
        }
    }
    svg.push('z');
}

/// A finder pattern: a ring of 7 by 7 modules around a 3 by 3 square.
fn finder(svg: &mut String, shape: Shape, x: usize, y: usize) {
    let (outer, inner, centre) = match shape {
        Shape::Square => (0.0, 0.0, 0.0),
        Shape::Rounded => (2.0, 1.2, 0.9),
        Shape::Dots => (2.5, 1.6, 1.5),
    };
    let (x, y) = (x as f32, y as f32);
    rounded_rect(svg, x, y, 7.0, outer);
    rounded_rect(svg, x + 1.0, y + 1.0, 5.0, inner);
    rounded_rect(svg, x + 2.0, y + 2.0, 3.0, centre);
}

/// A square with rounded corners, added to a path.
fn rounded_rect(svg: &mut String, x: f32, y: f32, side: f32, r: f32) {
    let straight = side - 2.0 * r;
    let _ = write!(
        svg,
        "M{} {y}h{straight}a{r} {r} 0 0 1 {r} {r}v{straight}a{r} {r} 0 0 1 -{r} {r}h-{straight}a{r} {r} 0 0 1 -{r} -{r}v-{straight}a{r} {r} 0 0 1 {r} -{r}z",
        x + r,
    );
}

/// The white square in the middle, and the icon or emoji on it.
fn centre(svg: &mut String, style: &Style, at: usize, side: usize) {
    let (at, side) = (at as f32, side as f32);
    let _ = write!(
        svg,
        r##"<rect x="{at}" y="{at}" width="{side}" height="{side}" rx="{r}" fill="#fff"/>"##,
        r = side / 4.0,
    );
    // White around the icon keeps it clear of the code.
    let padding = 0.75;
    let inner = side - 2.0 * padding;
    let middle = at + side / 2.0;
    match style.icon {
        Icon::None => {}
        Icon::Heart | Icon::Star => {
            let path = if style.icon == Icon::Heart {
                HEART
            } else {
                STAR
            };
            let _ = write!(
                svg,
                r#"<path transform="translate({} {}) scale({})" d="{path}"/>"#,
                at + padding,
                at + padding,
                inner / 24.0,
            );
        }
        Icon::Emoji => {
            let _ = write!(
                svg,
                r#"<text x="{middle}" y="{middle}" font-size="{}" text-anchor="middle" dominant-baseline="central" font-family="Apple Color Emoji, Segoe UI Emoji, Noto Color Emoji, sans-serif">{}</text>"#,
                inner * 0.9,
                escape(&style.emoji),
            );
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The style form on the admin index page.
#[derive(Deserialize)]
pub struct StyleForm {
    shape: Shape,
    color: String,
    icon: Icon,
    #[serde(default)]
    emoji: String,
}

impl StyleForm {
    pub fn validate(self) -> Result<Style, String> {
        check(Style {
            shape: self.shape,
            color: self.color,
            icon: self.icon,
            emoji: self.emoji,
        })
    }
}

/// A style as typed or carried from another event, checked: a colour phones can scan,
/// and an emoji if one goes in the middle. What's wrong is in the page's language.
pub fn check(style: Style) -> Result<Style, String> {
    let text = lang::text();
    let color = style.color.trim().to_ascii_lowercase();
    if !is_dark_enough(&color) {
        return Err(text.error_qr_color.clone());
    }
    let emoji = style.emoji.trim().to_owned();
    let emoji_ok = is_emoji(&emoji);
    if style.icon == Icon::Emoji && !emoji_ok {
        return Err(text.error_qr_emoji.clone());
    }
    Ok(Style {
        shape: style.shape,
        color,
        icon: style.icon,
        // Kept for next time, if it would do.
        emoji: if emoji_ok { emoji } else { String::new() },
    })
}

/// Whether a `#rrggbb` colour has a contrast of at least 4.5:1 on white, as phones need
/// to tell the modules apart.
fn is_dark_enough(color: &str) -> bool {
    // Six hex digits and nothing else: `from_str_radix` would also take a sign, as in
    // `#+12345`, which the database then refuses.
    let Some(hex) = color
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return false;
    };
    let Ok(rgb) = u32::from_str_radix(hex, 16) else {
        return false;
    };
    let channel = |shift: u32| {
        let value = f64::from((rgb >> shift) & 0xff) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
    1.05 / (luminance + 0.05) >= 4.5
}

/// Whether the text is one emoji, or near enough: short, and not plain letters.
fn is_emoji(text: &str) -> bool {
    let length = text.chars().count();
    (1..=8).contains(&length)
        && !text.is_ascii()
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(shape: Shape, icon: Icon) -> Style {
        Style {
            shape,
            color: "#00695c".into(),
            icon,
            emoji: "🎉".into(),
        }
    }

    #[test]
    fn draws_every_shape_and_icon() {
        for shape in Shape::ALL {
            for icon in Icon::ALL {
                let svg = svg("https://fest.example.com/r/abc", &style(shape, icon), None);
                assert!(svg.starts_with("<svg"), "{shape:?} {icon:?}");
                assert!(svg.ends_with("</svg>"));
                assert!(svg.contains(r##"fill="#00695c""##));
                assert_eq!(
                    svg.contains("🎉"),
                    icon == Icon::Emoji,
                    "{shape:?} {icon:?}"
                );
                assert_eq!(svg.contains("<rect x="), icon != Icon::None);
            }
        }
        let sized = svg("x", &style(Shape::Square, Icon::None), Some(512));
        assert!(sized.contains(r#"width="512" height="512""#));
    }

    #[test]
    fn draws_each_module_as_shortly_as_it_can() {
        // Alone, a rounded module is a circle of four half-module corners.
        let mut path = String::new();
        rounded_module(&mut path, 3, 4, |_, _| false);
        assert_eq!(
            path,
            "M3.5 4a.5.5 0 0 1 .5.5a.5.5 0 0 1-.5.5a.5.5 0 0 1-.5-.5a.5.5 0 0 1 .5-.5z"
        );
        // Joined on every side, a square.
        path.clear();
        rounded_module(&mut path, 3, 4, |_, _| true);
        assert_eq!(path, "M3 4h1v1h-1z");
        // Joined on the left only: round on the right, straight on the left.
        path.clear();
        rounded_module(&mut path, 3, 4, |dx, dy| (dx, dy) == (-1, 0));
        assert_eq!(path, "M3 4h.5a.5.5 0 0 1 .5.5a.5.5 0 0 1-.5.5h-.5v-1z");

        // Squares in a row are one rectangle: far fewer moves than dark modules.
        let square = svg(
            "https://fest.example.com/r/abc",
            &style(Shape::Square, Icon::None),
            None,
        );
        let modules = QrCode::new(b"https://fest.example.com/r/abc")
            .unwrap()
            .to_colors();
        let dark = modules.iter().filter(|c| **c == Color::Dark).count();
        assert!(square.matches('M').count() < dark / 2, "{square}");
        // Dots after the first in a row are placed from the one before.
        let dots = svg(
            "https://fest.example.com/r/abc",
            &style(Shape::Dots, Icon::None),
            None,
        );
        assert!(dots.contains(&format!("{DOT}m1 0{DOT}")), "{dots}");
    }

    #[test]
    fn escapes_the_emoji() {
        let mut style = style(Shape::Dots, Icon::Emoji);
        style.emoji = "<&>".into();
        let svg = svg("x", &style, None);
        assert!(svg.contains("&lt;&amp;&gt;"));
    }

    #[test]
    fn checks_colours_and_emoji() {
        assert!(is_dark_enough("#000000"));
        assert!(is_dark_enough("#00695c"));
        assert!(is_dark_enough("#00796b"));
        assert!(!is_dark_enough("#4db6ac"));
        assert!(!is_dark_enough("#ffffff"));
        assert!(!is_dark_enough("teal"));
        assert!(!is_dark_enough("#00695"));
        assert!(!is_dark_enough("#+12345"));
        assert!(!is_dark_enough("#-12345"));
        assert!(!is_dark_enough("#00695g"));

        assert!(is_emoji("🎉"));
        assert!(is_emoji("❤️"));
        assert!(is_emoji("👨‍👩‍👧"));
        assert!(!is_emoji(""));
        assert!(!is_emoji("AB"));
        assert!(!is_emoji("🎉 🎉"));
        assert!(!is_emoji("🎉🎉🎉🎉🎉🎉🎉🎉🎉"));
    }
}
