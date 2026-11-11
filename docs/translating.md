# Translating Innby

Every text Innby shows, on the public site, in the admin UI and in its mails, is in
[`locales/`](../locales/), one file per language:

| File | Language |
|---|---|
| `en.toml` | English, the base: plain British English, which has every text |
| `en-gb.toml` | English (Traditional): British, a touch formal |
| `en-us.toml` | English (Simplified): American, short |
| `nb.toml` | Norwegian Bokmål |
| `nn.toml` | Norwegian Nynorsk |
| `zh-hans.toml` | Simplified Chinese |
| `zh-hant.toml` | Traditional Chinese |

Each is a module: it holds the texts it has, and the rest come from the languages it
falls back to. The two English variants hold only what they say differently.

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
  are; the tests check every language has the same ones as English.
- **Punctuation** that differs between scripts, like "：" for ": ", is part of the text.
- **Dates** use `date_format`, and a time alone (an event's end on the day it starts)
  `time_format`, both in [chrono's notation](https://docs.rs/chrono/latest/chrono/format/strftime/).
  `when_until` puts a start and an end together.

## Fallbacks

A file doesn't need every text. One it leaves out comes from its `fallback` languages, in
order, and from theirs in turn:

| Language | Falls back to |
|---|---|
| Nynorsk | Bokmål, then English |
| Traditional Chinese | Simplified Chinese, then English |
| Bokmål, Simplified Chinese, English (Traditional), English (Simplified) | English |
| English | nothing: it's the base |

So a variant like English (Simplified) only needs the texts it says differently, and a
new text can go in `en.toml` first and be translated later. Texts the host writes, such
as an event's title, fall back the same way.

`just test` (or Innby itself, at start) refuses a text English doesn't have, which
catches typos in key names, and a language that falls back to itself.

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

Exactly one language, the base, falls back to nothing, and so has every text and a
privacy notice: English.
