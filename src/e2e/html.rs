//! Reads a page the way a browser shows it: its text, its links, and its forms with
//! the labels a person would look for.

use html5gum::{Token, Tokenizer};

#[derive(Debug, Default)]
pub struct Document {
    pub title: String,
    /// Everything a reader sees, with whitespace collapsed.
    pub text: String,
    pub links: Vec<Link>,
    pub forms: Vec<Form>,
    /// Fields outside any form, such as the read-only links the admin shows to copy.
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub href: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub action: Option<String>,
    pub post: bool,
    pub fields: Vec<Field>,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Hidden,
    Checkbox,
    Radio,
    Select,
    Textarea,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub value: String,
    pub checked: bool,
    /// A select's options, as value and text.
    pub options: Vec<(String, String)>,
    /// What a person reads as the field's name: its `<label>`, or its `aria-label`.
    pub label: String,
    id: String,
}

#[derive(Debug, Clone)]
pub struct Button {
    pub name: Option<String>,
    pub value: String,
    pub text: String,
}

/// Where the text being read goes, besides the page's text.
#[derive(Default)]
struct Open {
    title: bool,
    link: Option<Link>,
    form: Option<Form>,
    button: Option<Button>,
    /// A `<label>`: its `for`, its text, and the fields inside it so far.
    label: Option<(Option<String>, String, Vec<Field>)>,
    option: Option<(String, bool, String)>,
    select: Option<Field>,
    textarea: Option<Field>,
}

impl Document {
    pub fn parse(html: &str) -> Self {
        let mut doc = Self::default();
        let mut open = Open::default();
        // Labels with `for`, matched with their fields at the end.
        let mut labels: Vec<(String, String)> = Vec::new();
        for token in Tokenizer::new(html).flatten() {
            match token {
                Token::StartTag(tag) => {
                    if breaks(&tag.name) {
                        doc.text.push(' ');
                    }
                    let attr = |name: &str| {
                        tag.attributes
                            .get(name.as_bytes())
                            .map(|value| string(&value.value))
                    };
                    match string(&tag.name).as_str() {
                        "title" => open.title = true,
                        "a" => {
                            open.link = attr("href").map(|href| Link {
                                href,
                                text: String::new(),
                            });
                        }
                        "form" => {
                            open.form = Some(Form {
                                action: attr("action"),
                                post: attr("method")
                                    .is_some_and(|m| m.eq_ignore_ascii_case("post")),
                                fields: Vec::new(),
                                buttons: Vec::new(),
                            });
                        }
                        "button" => {
                            open.button = Some(Button {
                                name: attr("name"),
                                value: attr("value").unwrap_or_default(),
                                text: String::new(),
                            });
                        }
                        "label" => open.label = Some((attr("for"), String::new(), Vec::new())),
                        "input" | "select" | "textarea" => {
                            let kind = match string(&tag.name).as_str() {
                                "select" => Kind::Select,
                                "textarea" => Kind::Textarea,
                                _ => match attr("type").as_deref() {
                                    Some("hidden") => Kind::Hidden,
                                    Some("checkbox") => Kind::Checkbox,
                                    Some("radio") => Kind::Radio,
                                    _ => Kind::Text,
                                },
                            };
                            let field = Field {
                                name: attr("name").unwrap_or_default(),
                                kind,
                                value: attr("value").unwrap_or_else(|| match kind {
                                    Kind::Checkbox | Kind::Radio => "on".into(),
                                    _ => String::new(),
                                }),
                                checked: attr("checked").is_some(),
                                options: Vec::new(),
                                label: attr("aria-label").unwrap_or_default(),
                                id: attr("id").unwrap_or_default(),
                            };
                            match kind {
                                Kind::Select => open.select = Some(field),
                                Kind::Textarea => open.textarea = Some(field),
                                _ => open.add(&mut doc, field),
                            }
                        }
                        "option" => {
                            open.option = Some((
                                attr("value").unwrap_or_default(),
                                attr("selected").is_some(),
                                String::new(),
                            ));
                        }
                        _ => {}
                    }
                }
                Token::EndTag(tag) => {
                    if breaks(&tag.name) {
                        doc.text.push(' ');
                    }
                    match string(&tag.name).as_str() {
                        "title" => open.title = false,
                        "a" => doc.links.extend(open.link.take().map(Link::tidy)),
                        "form" => doc.forms.extend(open.form.take()),
                        "button" => {
                            if let (Some(mut button), Some(form)) =
                                (open.button.take(), open.form.as_mut())
                            {
                                button.text = collapse(&button.text);
                                form.buttons.push(button);
                            }
                        }
                        "label" => {
                            if let Some((target, text, fields)) = open.label.take() {
                                let text = collapse(&text);
                                if let Some(target) = target {
                                    labels.push((target, text.clone()));
                                }
                                for mut field in fields {
                                    if field.label.is_empty() {
                                        field.label.clone_from(&text);
                                    }
                                    open.place(&mut doc, field);
                                }
                            }
                        }
                        "option" => {
                            if let (Some((value, selected, text)), Some(select)) =
                                (open.option.take(), open.select.as_mut())
                            {
                                let text = collapse(&text);
                                // An option without a value sends its text.
                                let value = if value.is_empty() {
                                    text.clone()
                                } else {
                                    value
                                };
                                if selected || select.options.is_empty() {
                                    select.value.clone_from(&value);
                                }
                                select.options.push((value, text));
                            }
                        }
                        "select" => {
                            if let Some(select) = open.select.take() {
                                open.add(&mut doc, select);
                            }
                        }
                        "textarea" => {
                            if let Some(textarea) = open.textarea.take() {
                                open.add(&mut doc, textarea);
                            }
                        }
                        _ => {}
                    }
                }
                Token::String(text) => {
                    let text = string(&text.value);
                    if let Some(textarea) = open.textarea.as_mut() {
                        textarea.value.push_str(&text);
                        continue;
                    }
                    if open.title {
                        doc.title.push_str(&text);
                        continue;
                    }
                    doc.text.push_str(&text);
                    for target in [
                        open.link.as_mut().map(|link| &mut link.text),
                        open.button.as_mut().map(|button| &mut button.text),
                        open.label.as_mut().map(|label| &mut label.1),
                        open.option.as_mut().map(|option| &mut option.2),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        target.push_str(&text);
                    }
                }
                _ => {}
            }
        }
        doc.title = collapse(&doc.title);
        doc.text = collapse(&doc.text);
        // A `<label for>` names the field with that id, wherever it is.
        let fields = doc
            .fields
            .iter_mut()
            .chain(doc.forms.iter_mut().flat_map(|form| form.fields.iter_mut()));
        for field in fields {
            if let Some((_, text)) = labels.iter().find(|(id, _)| *id == field.id)
                && !field.id.is_empty()
                && field.label.is_empty()
            {
                field.label.clone_from(text);
            }
        }
        doc
    }
}

impl Open {
    /// A field that's complete: it waits for the label around it, if any.
    fn add(&mut self, doc: &mut Document, field: Field) {
        match self.label.as_mut() {
            Some((None, _, fields)) => fields.push(field),
            _ => self.place(doc, field),
        }
    }

    fn place(&mut self, doc: &mut Document, field: Field) {
        match self.form.as_mut() {
            Some(form) => form.fields.push(field),
            None => doc.fields.push(field),
        }
    }
}

impl Link {
    fn tidy(mut self) -> Self {
        self.text = collapse(&self.text);
        self
    }
}

impl Form {
    /// The form's data as a browser sends it, pressing `button`.
    pub fn encode(&self, button: &Button) -> String {
        let mut pairs: Vec<(&str, &str)> = self
            .fields
            .iter()
            .filter(|field| !field.name.is_empty())
            .filter(|field| !matches!(field.kind, Kind::Checkbox | Kind::Radio) || field.checked)
            .map(|field| (field.name.as_str(), field.value.as_str()))
            .collect();
        if let Some(name) = &button.name {
            pairs.push((name, &button.value));
        }
        pairs
            .iter()
            .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// `application/x-www-form-urlencoded`, as browsers write it.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.replace("\r\n", "\n").replace('\n', "\r\n").bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Whether a tag starts a new line or box, so the text either side of it doesn't run
/// together. Inline tags such as `<strong>` and `<a>` don't.
fn breaks(tag: &[u8]) -> bool {
    !matches!(
        tag,
        b"a" | b"b" | b"strong" | b"em" | b"i" | b"span" | b"time" | b"code" | b"small"
    )
}

fn string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_links_and_labelled_fields() {
        let doc = Document::parse(
            r#"<title>Guests · Innby</title><p>Hello <b>Kari</b> &amp; Ola.</p><p>Bye</p>
            <a href="/r/x">See who's coming</a>
            <input readonly value="https://x/r/1" aria-label="Link for Kari">
            <form method="post" action="/r/x">
              <label for="size">How many?</label>
              <select id="size" name="party_size"><option value="1">1 guest</option>
                <option value="2" selected>2 guests</option></select>
              <label class="checkbox"><input type="checkbox" name="consent" value="yes"> Yes, please</label>
              <textarea name="names">Kari
Ola</textarea>
              <button name="response" value="attending">I shall attend</button>
            </form>"#,
        );
        assert_eq!(doc.title, "Guests · Innby");
        assert!(doc.text.contains("Hello Kari & Ola. Bye"), "{}", doc.text);
        assert_eq!(doc.links[0].text, "See who's coming");
        assert_eq!(doc.fields[0].label, "Link for Kari");
        let form = &doc.forms[0];
        let labels: Vec<_> = form.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["How many?", "Yes, please", ""]);
        assert_eq!(form.fields[0].value, "2");
        assert_eq!(
            form.encode(&form.buttons[0]),
            "party_size=2&names=Kari%0D%0AOla&response=attending"
        );
    }
}
