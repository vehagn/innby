# Translating Innby

Every text Innby shows, on the public site, in the admin UI and in its mails, is in
[`locales/`](../locales/), one file per language:

| File | Language |
|---|---|
| `nb.toml` | Norwegian Bokmål, which has every text |
| `nn.toml` | Norwegian Nynorsk |
| `en.toml` | British English: "traditional", a touch formal |
| `en-us.toml` | American English: "simplified", short and plain |
| `zh-hans.toml` | Simplified Chinese |
| `zh-hant.toml` | Traditional Chinese |

The privacy notice is a document rather than a set of texts, so it's an HTML template
per language in [`templates/privacy/`](../templates/privacy/).

## A language file

Each file starts with what the language is:

```toml
language.name = "Nynorsk"           # its own name, on the language menu
language.tag = "nn"                 # for <html lang>
language.badge = "NN"               # its mark in the language menu
language.locale = "nn_NO"           # for day and month names
language.serves = ["nn"]            # the browser languages it answers
language.fallback = ["nb"]          # where texts this file leaves out come from
```

Then come the texts, with the admin UI's under `[admin]` and the mails under `[mail]`.

- **Placeholders** such as `{name}` mark where a value goes. Keep them exactly as they
  are; the tests check every language has the same ones as Bokmål.
- **Punctuation** that differs between scripts, like "：" for ": ", is part of the text.
- **Dates** use `date_format`, in [chrono's notation](https://docs.rs/chrono/latest/chrono/format/strftime/).

## Fallbacks

A file doesn't need every text. One it leaves out comes from its `fallback` languages, in
order, and from theirs in turn:

| Language | Falls back to |
|---|---|
| American English | British English, then Bokmål |
| Traditional Chinese | Simplified Chinese, British English, Bokmål |
| Simplified Chinese | British English, Bokmål |
| British English, Nynorsk | Bokmål |

So a variant like American English only needs the texts it says differently, and a new
text can go in `nb.toml` first and be translated later. Texts the host writes, such as an
event's title, fall back the same way.

`just test` (or Innby itself, at start) refuses a text Bokmål doesn't have, which catches
typos in key names, and a language that falls back to itself.

## A new language

Most of a language lives in its file, but a few places in the code list the languages:

1. Add its file in `locales/`, with its `language` table.
2. Add it to `Lang` in [`src/i18n/mod.rs`](../src/i18n/mod.rs): a variant (with the
   code as its `serde` and `sqlx` name), its code, and its file.
3. Add a migration that adds it to the database's `lang` type, since a guest's language
   is stored for their mail. An event's title and description are kept as maps by
   language code, so they need no new columns.
4. Add a field for it in `Texts` ([`src/i18n/texts.rs`](../src/i18n/texts.rs)), for the
   event title and description in that language. The admin's event form then offers it.
5. Add its privacy notice in `templates/privacy/`, and a line for it in
   `templates/privacy.html`.

The compiler points out any `match` on `Lang` you've missed.
