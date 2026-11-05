import assert from "node:assert/strict";
import { test } from "node:test";

import { replay } from "../lib/replay.js";
import * as snake from "../snake/logic.js";

test("the same seed gives the same apples", () => {
  assert.deepEqual(snake.start(7).food, snake.start(7).food);
});

test("the snake runs into the wall without a turn", () => {
  // From x 6, heading right, the wall is 14 squares on.
  const state = replay(snake, 1, [], 1000);
  assert.equal(state.over, true);
  assert.equal(state.tick, snake.WIDTH - 6);
});

test("it can't turn straight back, even after another turn in the same tick", () => {
  const state = snake.start(1);
  snake.step(state, ["left"]);
  assert.equal(state.heading, "right");
  snake.step(state, ["up", "left"]);
  assert.equal(state.heading, "up");
  assert.equal(state.over, false);
});

test("an apple scores ten and grows the snake", () => {
  const state = snake.start(1);
  state.food = [7, 10];
  snake.step(state, []);
  assert.deepEqual([state.score, state.snake.length], [10, 4]);
  assert.notDeepEqual(state.food, [7, 10]);
});
