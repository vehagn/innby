// Tetris's page: draws the board, and plays rounds with ../lib/client.js.
import { controls, guest, play, scores, showScores } from "../lib/client.js";
import * as logic from "./logic.js";

const COLOURS = ["", "#4dd0e1", "#ffd54f", "#ba68c8", "#81c784", "#e57373", "#64b5f6", "#ffb74d"];

const who = guest();
const board = document.getElementById("board");
const paint = board.getContext("2d");
const cell = board.width / logic.WIDTH;
const button = document.getElementById("start");
const [score, level, lines, result, note, table] = [
  "score", "level", "lines", "result", "note", "scores",
].map((id) => document.getElementById(id));

function square(x, y, kind) {
  paint.fillStyle = COLOURS[kind];
  paint.fillRect(x * cell + 1, y * cell + 1, cell - 2, cell - 2);
}

function draw(state) {
  paint.clearRect(0, 0, board.width, board.height);
  state.board.forEach((row, y) => row.forEach((kind, x) => kind && square(x, y, kind)));
  if (state.piece && !state.over) {
    for (const [x, y] of logic.cells(state.piece)) square(x, y, state.piece.kind);
  }
  score.textContent = state.score;
  level.textContent = state.level;
  lines.textContent = state.lines;
}

let round = null;
controls(
  { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "rotate", ArrowDown: "down", " ": "drop" },
  (name) => round?.move(name),
);

button.addEventListener("click", async () => {
  button.disabled = true;
  button.blur();
  result.textContent = "";
  try {
    round = await play(logic, who, draw);
    const answer = (await round.done).body;
    result.textContent = answer.error
      ? `That round didn't count: ${answer.error}.`
      : `${answer.score} points${answer.saved ? "" : ", not kept"}.`;
    if (answer.scores) showScores(table, note, answer, who);
  } catch (error) {
    result.textContent = error.message;
  }
  round = null;
  button.disabled = false;
  button.textContent = "Play again";
});

draw(logic.start(0));
scores(who).then((answer) => showScores(table, note, answer, who));
