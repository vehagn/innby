//! Filling in a text's `{placeholders}`, and the plural of a count.

use std::fmt::Display;

/// `one` if `n` is 1, else `other`, with `{n}` filled in.
pub fn count(n: impl Display, one: &str, other: &str) -> String {
    let n = n.to_string();
    fill(if n == "1" { one } else { other }, &[("n", &n)])
}

/// `count`, with the number as `<strong>n</strong>` and all of it escaped. For
/// templates, with `|safe`.
pub fn bold_count(n: impl Display, one: &str, other: &str) -> String {
    let n = n.to_string();
    bold(if n == "1" { one } else { other }, "n", &n)
}

/// `text` with its one `{key}` filled in. For templates, which escape the result.
pub fn filled(text: &str, key: &str, value: impl Display) -> String {
    fill(text, &[(key, &value)])
}

/// `text` with its one `{key}` filled in as `<strong>value</strong>`, all of it escaped.
/// For templates, with `|safe`.
pub fn bold(text: &str, key: &str, value: &str) -> String {
    let placeholder = format!("{{{key}}}");
    let (before, after) = text.split_once(&placeholder).unwrap_or((text, ""));
    format!(
        "{}<strong>{}</strong>{}",
        escape_html(before),
        escape_html(value),
        escape_html(after)
    )
}

/// `text` with the characters HTML gives a meaning escaped, for a value put into markup by hand.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&#38;")
        .replace('<', "&#60;")
        .replace('>', "&#62;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

/// `text` with each `{key}` replaced by its value, in one pass, so a value that contains
/// braces (a guest's name, say) is never filled in itself. Unknown keys stay as they are.
pub fn fill(text: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let known = after.split_once('}').and_then(|(key, tail)| {
            let (_, value) = values.iter().find(|(name, _)| *name == key)?;
            Some((value, tail))
        });
        match known {
            Some((value, tail)) => {
                filled.push_str(&value.to_string());
                rest = tail;
            }
            None => {
                filled.push('{');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bolds_a_value_and_escapes_everything() {
        assert_eq!(
            bold("Hi <{name}> & co", "name", "<b>Kari</b>"),
            "Hi &#60;<strong>&#60;b&#62;Kari&#60;/b&#62;</strong>&#62; &#38; co"
        );
        assert_eq!(filled("{name}，你好！", "name", "王小明"), "王小明，你好！");
    }

    #[test]
    fn fills_in_each_placeholder_once() {
        let name = "{url}";
        let filled = fill(
            "Hi, {name}! {url} {unknown}",
            &[("name", &name), ("url", &"x")],
        );
        assert_eq!(filled, "Hi, {url}! x {unknown}");
        assert_eq!(fill("{", &[]), "{");
    }
}
