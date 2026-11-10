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

The privacy notice is a document rather than a set of texts, so it's an HTML file per
language in [`templates/privacy/`](../templates/privacy/), with `{controller_name}`,
`{controller_email}`, `{version}`, `{keep_after_event_days}` and `{keep_sent_mail_days}`
filled in.

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
- **Dates** use `date_format`, and a time alone (an event's end on the day it starts)
  `time_format`, both in [chrono's notation](https://docs.rs/chrono/latest/chrono/format/strftime/).
  `when_until` puts a start and an end together.

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

A language is its file, and nothing in the code lists them:

1. Add its file in `locales/`, named for its code (`de.toml` is `?lang=de`), with its
   `language` table and as many texts as you have. Its `fallback` gives the rest.
2. Optionally, add its privacy notice as `templates/privacy/{code}.html`. Without one,
   the language shows its nearest fallback's.
3. Build. [`build.rs`](../build.rs) finds the files and compiles them in; the language
   menu, the event form, event files and the database then take the new language.

The database keeps a language as its code, so a new one needs no schema change, and a
code whose file has since gone reads as the base language. The menu lists the base
language first, then the others by their own names.

Exactly one language, the base, falls back to nothing: it has every text, a privacy
notice, and is the site's language when the browser asks for none it has. Today that's
Bokmål.
