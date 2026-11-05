# Minigames

A host can offer minigames on an event, each with a high score table: points, or time
taken. Guests play from their own page, and their best score in each game shows there,
in the game, and on the event's Games tab in the admin UI.

Innby doesn't contain the games. A game is any web page, served from an origin of its
own, that talks to Innby's scores API. There are two kinds:

- **A game in the browser** sends its scores from the guest's browser. Anyone can make
  one up with a request, so it suits a game where that doesn't matter.
  [`games/reaction/`](../games/reaction/index.html) is one: a single HTML file.
- **A game with a server of its own** has that server work the score out, and send it
  to Innby's internal API with the game's key. The browser never says what it scored.
  [`games/snake/`](../games/snake/) and [`games/tetris/`](../games/tetris/) are two,
  meant to be copied: the browser plays with the game's rules, and the server replays
  the moves with the same rules to score them. [`games/balloon/`](../games/balloon/) is
  a third, with its rules in Rust, built to WebAssembly that both run.

[`games/serve.js`](../games/serve.js) serves the examples, with Node.js and no packages,
and `just up` runs it on :8082.

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
   what the pages show, in every language. For a game with a server of its own, put
   `server:` before the name: `snake:points:server:Snake`. In the chart, put both under
   `extraConfig`, and see [the server's own section](#a-game-with-a-server-of-its-own).
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

## A game with a server of its own

The browser can't be trusted with a score, since whoever holds the guest's token can
send any. So the game's server works the score out itself, and is the only one that may
send it.

### How the examples do it

A round of Snake or Tetris goes:

1. The page asks its server for a round, `POST api/rounds`, and gets an id and a random
   seed. The server notes when.
2. The page plays with the game's rules, `logic.js`, and that seed, a tick at a time,
   and notes each move the player makes, as `[tick, move]`.
3. At the end it sends the moves, `POST api/rounds/{id}` with `{moves, token, lang}`.
4. The server replays them from the seed with the same `logic.js`, which gives the score,
   and sends it to Innby with the guest's token and the game's key. It answers with
   Innby's table.

The rules are one file that both the browser and the server run, so they can't drift
apart. The server refuses a round that's unknown, finished already, more than two hours
old, not over when the moves run out, or that took less real time than its ticks need.
That last means a program could still play, but only as fast as a person.

### Writing one

Copy `games/snake/` and change three files:

| File | |
|---|---|
| `logic.js` | The rules, with no side effects: `start(seed)` returns a state with `tick`, `score` and `over`; `step(state, moves)` moves it on one tick; `TICK_MS` is how long a tick is, and `MOVES` names the moves there are. Take every random number from `random(seed)` in `../lib/random.js`, so a replay comes out the same. |
| `page.js` | Draws the state, and wires the keys and buttons: `play`, `controls`, `scores` and `showScores` from `../lib/client.js` do the rest. |
| `index.html` | The page, with buttons marked `data-move="{move}"` for touch screens. |

`games/serve.js` finds a game by its `logic.js` and serves its API; add tests beside
those in `games/test/`, which `just test` runs with `node --test`. List the game in
`INNBY_GAMES` with `server:`, and give its server the key.

### Writing one in Rust

[`games/balloon/`](../games/balloon/) has its rules in Rust, built to WebAssembly, which
the page and the server both run, so the rules are still one thing. Copy it and change:

| File | |
|---|---|
| `src/lib.rs` | The rules, in whole numbers, with every random number from the seed. Its own crate and workspace, with tests beside the rules. |
| `src/wasm.rs` | The rules as plain WebAssembly exports: `start(seed)`, `step(moves)`, `tick`, `score` and `over`, and whatever the page needs to draw. No bindings or generated code. |
| `logic.js` | Loads the `.wasm` and wraps it in `start` and `step`, as `../lib/replay.js` expects; a round gets an instance of its own. |
| `page.js`, `index.html` | As for a game in JavaScript. |

`just games` builds every `games/*/Cargo.toml` for `wasm32-unknown-unknown` and puts
`{id}.wasm` beside its page (it isn't committed); `just up` and `just test` run it first,
and `just test` and `just lint` cover the crates too. Any language that compiles to
WebAssembly can do the same.

### The game's key

`innby game-key {id}` prints it. It's derived from `INNBY_LINK_KEY` and the game's id,
so Innby keeps nothing new, and changing the link key changes every game's key. The
game's server needs it, and nothing else does: keep it in a Secret, as
`INNBY_GAME_KEY_{ID}` for `games/serve.js` (`INNBY_GAME_KEY_SNAKE`).

### Innby's internal API

`innby serve` listens for games' servers on a port of its own, `INNBY_INTERNAL_LISTEN`
(`0.0.0.0:8083`), which no route leads to, when a game in `INNBY_GAMES` has a server.

**`GET /internal/games/{id}/scores`** and **`POST`** with `{"score": 1234}` work as the
browser's calls do, with the same answers, but take:

| Header | |
|---|---|
| `Authorization: Bearer {key}` | The game's key, which says which game is speaking |
| `Innby-Token: {token}` | The guest's score token, from the page, which says for whom |

A wrong or missing key is a 401, as is a token that isn't valid for the game. A browser
sending a score for such a game to the public API gets a 403.

### In Kubernetes

Run the games' server in the release's namespace, with labels of its own, and tell the
chart:

```yaml
games:
  internal:
    enabled: true
    podLabels: { app.kubernetes.io/name: innby-games }
extraConfig:
  INNBY_GAMES_URL: https://games.example.com
  INNBY_GAMES: "snake:points:server:Snake; tetris:points:server:Tetris"
```

The site's pods then open 8083 behind the `{release}-internal` Service, and the chart's
network policies, plain or Cilium, let only pods with those labels in. The games' server
reaches Innby at `http://{release}-internal:8083` (`INNBY_INTERNAL_URL`), and its own
hostname, `games.example.com`, is a route of its own to it, so it serves the pages too.

## How Innby keeps scores

Only each guest's best score in each game is kept, in `game_score`, beside the games an
event offers in `event_game` (`migrations/0001_schema.sql`). A score goes with its guest,
when the event stops offering the game, and with the event, 30 days after it starts. A
guest's export includes their scores, and the privacy notice says all this. The code is
in `src/web/games.rs` (the guest's page and the API), `src/admin/games.rs` (the Games
tab), `src/db/games.rs` and `src/config/games.rs`.
