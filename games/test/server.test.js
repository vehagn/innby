import assert from "node:assert/strict";
import { test } from "node:test";

import { random } from "../lib/random.js";
import { Rounds } from "../lib/server.js";
import * as balloon from "../balloon/logic.js";
import * as snake from "../snake/logic.js";
import * as tetris from "../tetris/logic.js";

/** Plays a round as a browser does, with random moves, noting them. */
function play(logic, seed) {
  const pick = random(99);
  const state = logic.start(seed);
  const moves = [];
  while (!state.over) {
    const now = pick() < 0.2 ? [logic.MOVES[pick.below(logic.MOVES.length)]] : [];
    for (const name of now) moves.push([state.tick, name]);
    logic.step(state, now);
  }
  return { state, moves };
}

for (const logic of [snake, tetris, balloon]) {
  test(`${logic.TICK_MS} ms ticks: the server scores a round as the browser played it`, () => {
    const rounds = new Rounds();
    const { round, seed } = rounds.start(0);
    const { state, moves } = play(logic, seed);
    const took = state.tick * logic.TICK_MS;
    assert.deepEqual(rounds.finish(logic, round, moves, took), { score: state.score });
    // Once only.
    assert.match(rounds.finish(logic, round, moves, took).error, /no such round/);
  });

  test(`${logic.TICK_MS} ms ticks: a round can't be faster than its moves`, () => {
    const rounds = new Rounds();
    const { round, seed } = rounds.start(0);
    const { state, moves } = play(logic, seed);
    // Well under the time its ticks take, beyond the slack.
    const result = rounds.finish(logic, round, moves, state.tick * logic.TICK_MS - 5_000);
    if (state.tick * logic.TICK_MS > 5_000) assert.match(result.error, /faster/);
  });
}

test("moves must be in order, and known", () => {
  const rounds = new Rounds();
  for (const moves of [[[2, "up"], [1, "up"]], [[0, "jump"]], "up", [[0.5, "up"]]]) {
    const { round } = rounds.start(0);
    assert.match(rounds.finish(snake, round, moves, 60_000).error, /moves are/);
  }
});
