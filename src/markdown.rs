//! A little Markdown, for posts on a message board: enough to write as people do in a
//! chat, and nothing that could change the page around it. Everything else is text,
//! escaped, so a post can hold no HTML of its own.
//!
//! - Paragraphs apart by a blank line, and a line break where the writer made one.
//! - Lists, each line starting with `- ` or `* `, or `1. ` and so on for a numbered one.
//! - Quotes, each line starting with `> `.
//! - `**bold**`, `*italic*` or `_italic_`, and `` `code` ``.
//! - Links, as `[text](https://…)` or a bare `https://…`; only http, https and mailto,
//!   marked as the writer's (`ugc`) and not followed. The site's `Referrer-Policy`
//!   already keeps where a guest came from to itself.
//!
//! No headings, images or tables: a post is a few lines among others, and an image would
//! load from another site, which the site's Content Security Policy forbids anyway.

use crate::i18n::escape_html;

/// The post as HTML: block elements, each line's text escaped.
pub fn to_html(text: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let kind = Block::of(line);
        // The block runs while its lines are of its kind; a paragraph till a blank line
        // or another kind of block.
        let mut items = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() && Block::of(lines[i]) == kind {
            items.push(kind.content(lines[i]));
            i += 1;
        }
        match kind {
            Block::Paragraph => {
                out.push_str("<p>");
                join(&items, &mut out);
                out.push_str("</p>");
            }
            Block::Quote => {
                out.push_str("<blockquote><p>");
                join(&items, &mut out);
                out.push_str("</p></blockquote>");
            }
            Block::List | Block::Numbered => {
                let tag = if kind == Block::List { "ul" } else { "ol" };
                out.push_str(&format!("<{tag}>"));
                for item in items {
                    out.push_str("<li>");
                    inline(item, true, &mut out);
                    out.push_str("</li>");
                }
                out.push_str(&format!("</{tag}>"));
            }
        }
    }
    out
}

/// Lines of a paragraph or quote, with a break between each.
fn join(lines: &[&str], out: &mut String) {
    for (n, line) in lines.iter().enumerate() {
        if n > 0 {
            out.push_str("<br>");
        }
        inline(line, true, out);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Paragraph,
    Quote,
    List,
    Numbered,
}

impl Block {
    fn of(line: &str) -> Self {
        let line = line.trim_start();
        if line.starts_with("> ") || line == ">" {
            Self::Quote
        } else if line.starts_with("- ") || line.starts_with("* ") {
            Self::List
        } else if numbered(line).is_some() {
            Self::Numbered
        } else {
            Self::Paragraph
        }
    }

    /// The line without what marks its kind.
    fn content(self, line: &str) -> &str {
        let trimmed = line.trim_start();
        match self {
            Self::Paragraph => line.trim(),
            Self::Quote => trimmed.strip_prefix('>').unwrap_or(trimmed).trim(),
            Self::List => trimmed[2..].trim(),
            Self::Numbered => numbered(trimmed).unwrap_or(trimmed).trim(),
        }
    }
}

/// What follows `1. ` at the start of a line, if it starts so.
fn numbered(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    (1..=3)
        .contains(&digits)
        .then(|| line[digits..].strip_prefix(". "))
        .flatten()
}

/// A line's text, escaped, with its bold, italics, code and, if `links`, links: not in
/// a link's own text, where a link would make one inside another.
fn inline(text: &str, links: bool, out: &mut String) {
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let at_word_start = text.len() == rest.len()
            || !text[..text.len() - rest.len()]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        if let Some(after) = span(rest, at_word_start, links, out) {
            rest = after;
            continue;
        }
        escape_char(c, out);
        rest = &rest[c.len_utf8()..];
    }
}

/// Writes the code, emphasis or link `rest` starts with, if it does, and returns what
/// follows it.
fn span<'a>(rest: &'a str, at_word_start: bool, links: bool, out: &mut String) -> Option<&'a str> {
    if let Some(after) = rest.strip_prefix('`') {
        let end = after.find('`').filter(|&end| end > 0)?;
        out.push_str("<code>");
        out.push_str(&escape_html(&after[..end]));
        out.push_str("</code>");
        return Some(&after[end + 1..]);
    }
    if let Some(after) = rest.strip_prefix("**") {
        let end = closing(after, "**")?;
        out.push_str("<strong>");
        inline(&after[..end], links, out);
        out.push_str("</strong>");
        return Some(&after[end + 2..]);
    }
    for mark in ["*", "_"] {
        // An underscore inside a word, as in snake_case, is just that.
        if mark == "_" && !at_word_start {
            continue;
        }
        if let Some(after) = rest.strip_prefix(mark) {
            let end = closing(after, mark)?;
            out.push_str("<em>");
            inline(&after[..end], links, out);
            out.push_str("</em>");
            return Some(&after[end + 1..]);
        }
    }
    if !links {
        return None;
    }
    if let Some(after) = rest.strip_prefix('[') {
        let close = after.find("](")?;
        let tail = &after[close + 2..];
        let end = tail.find(')')?;
        let url = &tail[..end];
        if close == 0 || !linkable(url) {
            return None;
        }
        link(url, out, |out| inline(&after[..close], false, out));
        return Some(&tail[end + 1..]);
    }
    if at_word_start && (rest.starts_with("https://") || rest.starts_with("http://")) {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        // Punctuation after an address belongs to the sentence.
        let url = rest[..end].trim_end_matches(['.', ',', ':', ';', '!', '?', ')', '\'', '"']);
        if !linkable(url) {
            return None;
        }
        link(url, out, |out| out.push_str(&escape_html(url)));
        return Some(&rest[url.len()..]);
    }
    None
}

/// A character as HTML text, as `escape_html` writes it, without a string for each.
fn escape_char(c: char, out: &mut String) {
    match c {
        '&' => out.push_str("&#38;"),
        '<' => out.push_str("&#60;"),
        '>' => out.push_str("&#62;"),
        '"' => out.push_str("&#34;"),
        '\'' => out.push_str("&#39;"),
        c => out.push(c),
    }
}

/// Where emphasis that opened just before `text` closes: the next `mark` with something
/// other than a space on either side.
fn closing(text: &str, mark: &str) -> Option<usize> {
    if text.starts_with(char::is_whitespace) {
        return None;
    }
    text.match_indices(mark)
        .map(|(at, _)| at)
        .find(|&at| at > 0 && !text[..at].ends_with(char::is_whitespace))
}

/// Whether a link may go there: the web or an e-mail address, and nothing a browser would
/// run or that would break out of the attribute.
fn linkable(url: &str) -> bool {
    let scheme = ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| url.len() > scheme.len() && url.starts_with(scheme));
    scheme
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"'))
}

fn link(url: &str, out: &mut String, text: impl FnOnce(&mut String)) {
    out.push_str(r#"<a href=""#);
    out.push_str(&escape_html(url));
    out.push_str(r#"" rel="nofollow ugc noopener">"#);
    text(out);
    out.push_str("</a>");
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn escapes_everything_that_isnt_markdown() {
        assert_eq!(
            to_html(r#"<script>alert("hi")</script> & <b>"#),
            "<p>&#60;script&#62;alert(&#34;hi&#34;)&#60;/script&#62; &#38; &#60;b&#62;</p>"
        );
    }

    #[test]
    fn writes_emphasis_code_and_links() {
        assert_eq!(
            to_html("**Bold** and *italic*, _too_, a `<tag>` and snake_case_name"),
            "<p><strong>Bold</strong> and <em>italic</em>, <em>too</em>, a \
             <code>&#60;tag&#62;</code> and snake_case_name</p>"
        );
        assert_eq!(
            to_html("See [the map](https://example.com/?a=1&b=2) or https://example.org."),
            "<p>See <a href=\"https://example.com/?a=1&#38;b=2\" rel=\"nofollow ugc noopener\">\
             the map</a> or <a href=\"https://example.org\" rel=\"nofollow ugc noopener\">\
             https://example.org</a>.</p>"
        );
        // A star on its own, or with spaces inside, is a star.
        assert_eq!(
            to_html("2 * 3 = 6 and * no *"),
            "<p>2 * 3 = 6 and * no *</p>"
        );
    }

    #[test]
    fn links_only_to_the_web_and_mail() {
        for unsafe_link in [
            "[x](javascript:alert(1))",
            "[x](data:text/html,hi)",
            "[x](https://example.com\"onclick=\"x)",
            "[x](https://)",
        ] {
            let html = to_html(unsafe_link);
            assert!(!html.contains("<a "), "{unsafe_link}: {html}");
        }
        assert!(to_html("[mail](mailto:kari@example.com)").contains("href=\"mailto:"));
        // Not one inside another.
        let html = to_html("[[a](https://a.example)](https://b.example)");
        let nested = html
            .split("</a>")
            .any(|link| link.matches("<a ").count() > 1);
        assert!(!nested, "{html}");
    }

    #[test]
    fn writes_paragraphs_lists_and_quotes() {
        let text =
            "Hi all,\nsee you!\n\n- cake\n- coffee\n\n1. first\n2. second\n\n> she said\n> so";
        assert_eq!(
            to_html(text),
            "<p>Hi all,<br>see you!</p><ul><li>cake</li><li>coffee</li></ul>\
             <ol><li>first</li><li>second</li></ol><blockquote><p>she said<br>so</p></blockquote>"
        );
    }
}
