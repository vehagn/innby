// Balloon's rules are Rust (src/lib.rs), built to balloon.wasm (`just games` builds it).
// This wraps them in what ../lib/replay.js expects of any game's logic, `start` and
// `step`, so the page and the server run the same WebAssembly, and so the same rules.
const source = new URL("./balloon.wasm", import.meta.url);
const bytes =
  typeof window === "undefined"
    ? await (await import("node:fs/promises")).readFile(source)
    : await (await fetch(source)).arrayBuffer();
const module = await WebAssembly.compile(bytes);
const board = new WebAssembly.Instance(module).exports;

export const TICK_MS = board.tick_ms();
export const MOVES = ["puff"];
/** The board, in pixels, for drawing. */
export const WIDTH = board.width();
export const HEIGHT = board.height();
export const BALLOON_X = board.balloon_x();
export const RADIUS = board.radius();
export const CLOUD_WIDTH = board.cloud_width();
export const GAP = board.gap();

/** A round, in an instance of its own: the server replays several at once. */
export function start(seed) {
  const round = new WebAssembly.Instance(module).exports;
  round.start(seed >>> 0);
  const state = { round };
  read(state);
  return state;
}

export function step(state, moves) {
  state.round.step(moves.includes("puff") ? 1 : 0);
  read(state);
}

function read(state) {
  const { round } = state;
  state.tick = round.tick();
  state.score = round.score();
  state.over = round.over() === 1;
  state.y = round.balloon_y();
  state.clouds = Array.from({ length: round.clouds() }, (_, i) => ({
    x: round.cloud_x(i),
    gap: round.cloud_gap(i),
  }));
}
