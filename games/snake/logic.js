// Snake's rules: eat the apples, don't hit the walls or yourself. Pure and seeded, so
// the browser plays with it and the server replays the same round to score it; see
// ../lib/replay.js. Ten points an apple.
import { random } from "../lib/random.js";

export const WIDTH = 20;
export const HEIGHT = 20;
/** A tick, and the snake moves a square. */
export const TICK_MS = 120;
export const MOVES = ["up", "down", "left", "right"];

const STEPS = { up: [0, -1], down: [0, 1], left: [-1, 0], right: [1, 0] };
const BACK = { up: "down", down: "up", left: "right", right: "left" };

export function start(seed) {
  const state = {
    rand: random(seed),
    tick: 0,
    score: 0,
    over: false,
    // The head first.
    snake: [
      [6, 10],
      [5, 10],
      [4, 10],
    ],
    heading: "right",
    food: null,
  };
  placeFood(state);
  return state;
}

export function step(state, moves) {
  // The last turn this tick counts, but never straight back the way the snake came,
  // even after another turn in the same tick.
  const moved = state.heading;
  for (const move of moves) {
    if (STEPS[move] && move !== BACK[moved]) state.heading = move;
  }
  const [dx, dy] = STEPS[state.heading];
  const [x, y] = [state.snake[0][0] + dx, state.snake[0][1] + dy];
  const eats = state.food[0] === x && state.food[1] === y;
  // The tail moves away this tick unless the snake grows, so the head may take its place.
  const body = eats ? state.snake : state.snake.slice(0, -1);
  state.tick += 1;
  if (x < 0 || y < 0 || x >= WIDTH || y >= HEIGHT || body.some(([bx, by]) => bx === x && by === y)) {
    state.over = true;
    return;
  }
  state.snake = [[x, y], ...body];
  if (eats) {
    state.score += 10;
    placeFood(state);
  }
}

/** An apple on a free square, or the end, when the snake fills the board. */
function placeFood(state) {
  const free = [];
  for (let y = 0; y < HEIGHT; y += 1) {
    for (let x = 0; x < WIDTH; x += 1) {
      if (!state.snake.some(([sx, sy]) => sx === x && sy === y)) free.push([x, y]);
    }
  }
  if (free.length === 0) {
    state.over = true;
    return;
  }
  state.food = free[state.rand.below(free.length)];
}
