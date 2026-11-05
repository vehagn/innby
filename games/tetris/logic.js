// Tetris's rules: pieces fall, full rows clear, and the game ends when a new piece has
// no room. Pure and seeded, so the browser plays with it and the server replays the same
// round to score it; see ../lib/replay.js.
//
// Scoring: 100, 300, 500 or 800 for one to four rows at once, times the level; a point a
// row for a soft drop, two for a hard one. Every ten rows is a level, and pieces fall
// faster.
import { random } from "../lib/random.js";

export const WIDTH = 10;
export const HEIGHT = 20;
/** A tick. Pieces fall a row every few, fewer as the level rises. */
export const TICK_MS = 50;
export const MOVES = ["left", "right", "rotate", "down", "drop"];

// Each piece as rows of its square, `#` for a block. Its number, from 1, colours it.
const PIECES = [
  ["....", "####", "....", "...."], // I
  ["##", "##"], // O
  [".#.", "###", "..."], // T
  [".##", "##.", "..."], // S
  ["##.", ".##", "..."], // Z
  ["#..", "###", "..."], // J
  ["..#", "###", "..."], // L
].map((rows) => rows.map((row) => [...row].map((c) => c === "#")));

const LINES = [0, 100, 300, 500, 800];

export function start(seed) {
  const state = {
    rand: random(seed),
    tick: 0,
    score: 0,
    over: false,
    lines: 0,
    level: 1,
    // Rows from the top; 0 is empty, else the piece's number.
    board: Array.from({ length: HEIGHT }, () => Array(WIDTH).fill(0)),
    bag: [],
    piece: null,
    // Ticks since the piece last fell a row.
    falling: 0,
  };
  spawn(state);
  return state;
}

/** Ticks between rows falling: 16 at level 1, down to 2. */
export function gravity(level) {
  return Math.max(2, 18 - 2 * level);
}

export function step(state, moves) {
  for (const move of moves) {
    if (state.over) break;
    const { piece } = state;
    if (move === "left") tryMove(state, { ...piece, x: piece.x - 1 });
    else if (move === "right") tryMove(state, { ...piece, x: piece.x + 1 });
    else if (move === "rotate") rotate(state);
    else if (move === "down" && tryMove(state, { ...piece, y: piece.y + 1 })) {
      state.score += 1;
      state.falling = 0;
    } else if (move === "drop") {
      while (tryMove(state, { ...state.piece, y: state.piece.y + 1 })) state.score += 2;
      lock(state);
    }
  }
  state.tick += 1;
  if (state.over) return;
  state.falling += 1;
  if (state.falling >= gravity(state.level)) {
    state.falling = 0;
    if (!tryMove(state, { ...state.piece, y: state.piece.y + 1 })) lock(state);
  }
}

/** The board's squares a piece covers. */
export function cells(piece) {
  const out = [];
  piece.shape.forEach((row, dy) =>
    row.forEach((block, dx) => block && out.push([piece.x + dx, piece.y + dy])),
  );
  return out;
}

function fits(state, piece) {
  return cells(piece).every(
    ([x, y]) => x >= 0 && x < WIDTH && y >= 0 && y < HEIGHT && state.board[y][x] === 0,
  );
}

function tryMove(state, piece) {
  if (!fits(state, piece)) return false;
  state.piece = piece;
  return true;
}

/** A quarter turn clockwise, nudged a square aside if it doesn't fit where it is. */
function rotate(state) {
  const { shape } = state.piece;
  const size = shape.length;
  const turned = shape.map((row, y) => row.map((_, x) => shape[size - 1 - x][y]));
  for (const nudge of [0, -1, 1, -2, 2]) {
    if (tryMove(state, { ...state.piece, shape: turned, x: state.piece.x + nudge })) return;
  }
}

function lock(state) {
  for (const [x, y] of cells(state.piece)) state.board[y][x] = state.piece.kind;
  const kept = state.board.filter((row) => row.some((square) => square === 0));
  const cleared = HEIGHT - kept.length;
  state.board = [...Array.from({ length: cleared }, () => Array(WIDTH).fill(0)), ...kept];
  state.score += LINES[cleared] * state.level;
  state.lines += cleared;
  state.level = 1 + Math.floor(state.lines / 10);
  state.falling = 0;
  spawn(state);
}

/** The next piece from the bag, a shuffled set of all seven, at the top. */
function spawn(state) {
  if (state.bag.length === 0) {
    state.bag = [0, 1, 2, 3, 4, 5, 6];
    for (let i = state.bag.length - 1; i > 0; i -= 1) {
      const j = state.rand.below(i + 1);
      [state.bag[i], state.bag[j]] = [state.bag[j], state.bag[i]];
    }
  }
  const kind = state.bag.pop();
  const shape = PIECES[kind];
  const piece = { kind: kind + 1, shape, x: Math.floor((WIDTH - shape.length) / 2), y: 0 };
  if (!fits(state, piece)) {
    state.over = true;
    return;
  }
  state.piece = piece;
}
