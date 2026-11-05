import assert from "node:assert/strict";
import { test } from "node:test";

import { replay } from "../lib/replay.js";
import * as tetris from "../tetris/logic.js";

test("pieces pile up until there's no room, with no moves at all", () => {
  const state = replay(tetris, 3, [], 100_000);
  assert.equal(state.over, true);
  assert.equal(state.score, 0);
});

test("a piece falls a row every so many ticks", () => {
  const state = tetris.start(3);
  const y = state.piece.y;
  for (let i = 0; i < tetris.gravity(1); i += 1) tetris.step(state, []);
  assert.equal(state.piece.y, y + 1);
});

test("a hard drop scores two a row, and a full row clears for 100", () => {
  const state = tetris.start(3);
  // The bottom row full but for four squares, and a flat I-piece above the gap.
  const bottom = tetris.HEIGHT - 1;
  state.board[bottom] = state.board[bottom].map((_, x) => (x >= 3 && x <= 6 ? 0 : 1));
  const [o, x] = [false, true];
  state.piece = { kind: 1, x: 3, y: 0, shape: [[o, o, o, o], [x, x, x, x], [o, o, o, o], [o, o, o, o]] };
  tetris.step(state, ["drop"]);
  // From row 1 down to row 19: 18 rows, two points each, and the row.
  assert.equal(state.score, 18 * 2 + 100);
  assert.equal(state.lines, 1);
  assert.ok(state.board[bottom].every((square) => square === 0));
});

test("turning keeps the piece on the board", () => {
  const state = tetris.start(5);
  for (let i = 0; i < 6; i += 1) tetris.step(state, ["left"]);
  tetris.step(state, ["rotate"]);
  for (const [x] of tetris.cells(state.piece)) assert.ok(x >= 0 && x < tetris.WIDTH);
});
