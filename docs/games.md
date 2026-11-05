# Minigames

A host can offer minigames on an event, each with a high score table: points, or time
taken. Guests play from their own page, and their best score in each game shows there,
in the game, and on the event's Games tab in the admin UI.

Innby doesn't contain the games. A game is any web page, served from an origin of its
own, that talks to Innby's scores API. [`games/reaction/`](../games/reaction/index.html)
is an example: one HTML file with nothing to build.

## Setting games up

1. Serve the games on an origin other than the site's, one game per path:
   `https://games.example.com/reaction/`, `https://games.example.com/quiz/`. A static
   file server or a bucket does; Innby needn't reach it.
2. List them for Innby:

   ```sh
   INNBY_GAMES_URL=https://games.example.com
   INNBY_GAMES="reaction:time:Reaction; quiz:points:Quick quiz"
   ```

   Each game is `id:points|time:Name`, apart by `;` or lines. The id is its path under
   `INNBY_GAMES_URL` and how scores name it: letters a to z, digits and `-`. `points`
   means the most wins; `time` is milliseconds taken, and the fewest wins. The name is
   what the pages show, in every language. In the chart, put both under `extraConfig`.
3. On an event's **Games** tab, tick the games it offers. Turning one off deletes its
   scores.

### Why an origin of their own

A game is JavaScript, and JavaScript on the site's own origin can do whatever the
guest's browser can there: read the guest's pages, their personal link with them, and
answer or delete their reply. So Innby refuses an `INNBY_GAMES_URL` on the site's origin,
and a game gets a **score token** instead of the link. It saves and reads that guest's
scores in that one game, nothing else, and stops working when the guest gets a new link.

A subdomain (`games.example.com` beside `fest.example.com`) or another port will do: the
origin is what counts.

## Writing a game

### Starting

The guest's page links to `{INNBY_GAMES_URL}/{id}/`, with three values in the fragment:

```
https://games.example.com/reaction/#innby=https://fest.example.com&token=AZJ3….k2Pq…&lang=en
```

| Value | |
|---|---|
| `innby` | The site's origin, where the API is |
| `token` | The guest's score token for this game |
| `lang` | The guest's language: `nb`, `nn`, `en`, `en-us`, `zh-hans` or `zh-hant` |

Browsers never send the fragment to a server, so the token doesn't reach the games'
server or its logs. Read it, then take it out of the address bar with
`history.replaceState`, so a bookmark or a link to the page doesn't carry it.

Without a token, as when someone opens the game directly, let them play, and say that
their score is kept only when they play from their invitation's page.

### The API

Both calls take the token as `Authorization: Bearer {token}`, and `?lang=` for the names
of those who aren't on the guest list. Only `INNBY_GAMES_URL`'s origin may call them.

**`GET {innby}/api/games/{id}/scores`**: the event's high score table.

**`POST {innby}/api/games/{id}/scores`** with `{"score": 1234}` (`Content-Type:
application/json`): keeps the score if it's the guest's best, and answers with the table.
A score is a whole number from 0: points, or milliseconds for a game of time. Send one a
round; a client may send 600 an hour.

Both answer:

```json
{
  "game": "reaction",
  "kind": "time",
  "scores": [
    { "name": "Ola N.", "score": 213, "shown": "0.21 s", "you": false },
    { "name": "Kari Nordmann", "score": 248, "shown": "0.24 s", "you": true }
  ],
  "yours": 248
}
```

| Field | |
|---|---|
| `scores` | The ten best, the best first; of two equal, the earlier |
| `name` | As on the guest list: full, or first name and initial, as each guest chose; *A guest*, in their language, for those not on it |
| `shown` | The score as Innby's pages show it: points, or seconds to a hundredth |
| `you` | Whether it's the guest's own |
| `yours` | The guest's best, whether or not it's in the ten; `null` before their first |

| Status | |
|---|---|
| 401 | No token, or one that isn't valid for this game: malformed, forged, for a guest who got a new link or was deleted |
| 404 | No such game, or the guest's event doesn't offer it |
| 422 | A score that isn't a whole number from 0 to 10¹² |
| 429 | Too many scores from one address |

### Things to know

- **Scores can be faked.** Anyone with a token can send any score. It's a party, so
  Innby doesn't try to stop it; a game that wants to can keep its rounds honest itself.
- **Names are personal data.** Show them, but don't keep or send them anywhere.
- **Load nothing from other sites**, so guests' browsers don't tell anyone else who's
  playing.

## How Innby keeps scores

Only each guest's best score in each game is kept, in `game_score`, beside the games an
event offers in `event_game` (`migrations/0001_schema.sql`). A score goes with its guest,
when the event stops offering the game, and with the event, 30 days after it starts. A
guest's export includes their scores, and the privacy notice says all this. The code is
in `src/web/games.rs` (the guest's page and the API), `src/admin/games.rs` (the Games
tab), `src/db/games.rs` and `src/config/games.rs`.
