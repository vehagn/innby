// What a game's server needs, in plain Node with no packages: rounds, working out the
// score by replaying them, and passing it to Innby's internal API. See docs/games.md.
//
// A round goes:
//   1. POST {game}/api/rounds: the server makes a seed, and notes when.
//   2. The browser plays with the game's logic and that seed, noting every move.
//   3. POST {game}/api/rounds/{round} with {moves, token, lang}: the server replays the
//      moves from the seed with the same logic, which gives the score, and sends it to
//      Innby with the guest's token and the game's key. A round can be finished once.
//
// The browser never says what it scored, so it can't make a score up. It could still
// send moves a program chose, but only in real time: a round whose moves need longer
// than it took is refused.
import { randomInt, randomUUID } from "node:crypto";

import { replay } from "./replay.js";

/** How long a round may last before it can't be finished. */
const ROUND_LIFETIME_MS = 2 * 60 * 60 * 1000;
/** Rounds kept at once; the oldest go first. */
const MOST_ROUNDS = 10_000;
/** Room for the network and timers, when checking a round took as long as its moves. */
const SLACK_MS = 2_000;
/** The most moves a round may send. */
const MOST_MOVES = 50_000;

export class Rounds {
  #rounds = new Map();

  /** A new round: its id, and the seed the browser plays it with. */
  start(now = Date.now()) {
    const round = randomUUID();
    const seed = randomInt(2 ** 32);
    this.#rounds.set(round, { seed, startedAt: now });
    if (this.#rounds.size > MOST_ROUNDS) {
      this.#rounds.delete(this.#rounds.keys().next().value);
    }
    return { round, seed };
  }

  /**
   * The round's score, from its moves, or why it doesn't count. Takes the round, so
   * it can't be finished twice.
   */
  finish(logic, round, moves, now = Date.now()) {
    const started = this.#rounds.get(round);
    this.#rounds.delete(round);
    if (!started || now - started.startedAt > ROUND_LIFETIME_MS) {
      return { error: "no such round, or it's too old" };
    }
    if (!validMoves(logic, moves)) {
      return { error: "moves are [tick, move] pairs, in order" };
    }
    // As many ticks as the round had time for: no more.
    const maxTicks = Math.floor((now - started.startedAt + SLACK_MS) / logic.TICK_MS);
    const state = replay(logic, started.seed, moves, maxTicks);
    if (!state.over) {
      return { error: "the round isn't over, or was played faster than it could be" };
    }
    return { score: state.score };
  }
}

function validMoves(logic, moves) {
  if (!Array.isArray(moves) || moves.length > MOST_MOVES) return false;
  let last = 0;
  for (const move of moves) {
    if (!Array.isArray(move) || move.length !== 2) return false;
    const [tick, name] = move;
    if (!Number.isInteger(tick) || tick < last || !logic.MOVES.includes(name)) return false;
    last = tick;
  }
  return true;
}

/**
 * Innby's internal API, for one game: where it is (INNBY_INTERNAL_URL) and the game's
 * key (from `innby game-key {id}`).
 */
export class Innby {
  constructor(url, game, key) {
    this.url = url.replace(/\/$/, "");
    this.game = game;
    this.key = key;
  }

  /** Sends a score, and answers Innby's status and high score table. */
  send(token, score, lang) {
    return this.#call("POST", token, lang, { score });
  }

  /** The guest's event's high score table. */
  table(token, lang) {
    return this.#call("GET", token, lang);
  }

  async #call(method, token, lang, body) {
    const query = new URLSearchParams({ lang: lang || "en" });
    const response = await fetch(
      `${this.url}/internal/games/${this.game}/scores?${query}`,
      {
        method,
        headers: {
          Authorization: `Bearer ${this.key}`,
          "Innby-Token": token,
          "Content-Type": "application/json",
        },
        body: body && JSON.stringify(body),
      },
    );
    const text = await response.text();
    return { status: response.status, body: response.ok ? JSON.parse(text) : { error: text } };
  }
}

/**
 * The API a game's page calls, under `{game}/api/`: rounds, and the table. Answers a
 * request it handles, and returns false for one it doesn't.
 */
export function gameApi(logic, innby) {
  const rounds = new Rounds();
  return async (request, response, path) => {
    const send = (status, body) => {
      response.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" });
      response.end(JSON.stringify(body));
    };
    if (request.method === "POST" && path === "rounds") {
      send(200, rounds.start());
      return true;
    }
    const finishing = path.match(/^rounds\/([0-9a-f-]{36})$/);
    if (request.method === "POST" && finishing) {
      const body = await readJson(request);
      const result = rounds.finish(logic, finishing[1], body?.moves);
      if (result.error) return send(422, result), true;
      // Without a token, as when someone opens the game by itself, the score isn't kept.
      if (!body.token || !innby) return send(200, { score: result.score, saved: false }), true;
      const kept = await innby.send(body.token, result.score, body.lang);
      send(kept.status, { score: result.score, saved: kept.status === 200, ...kept.body });
      return true;
    }
    if (request.method === "GET" && path === "scores") {
      const token = request.headers.authorization?.replace(/^Bearer /, "");
      if (!token || !innby) return send(401, { error: "no token" }), true;
      const lang = new URL(request.url, "http://game").searchParams.get("lang");
      const table = await innby.table(token, lang);
      send(table.status, table.body);
      return true;
    }
    return false;
  };
}

/** A request's JSON body, up to 1 MiB, or null. */
async function readJson(request) {
  let size = 0;
  const chunks = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 1024 * 1024) return null;
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return null;
  }
}
