// Snake's page: draws the board, and plays rounds with ../lib/client.js.
import { controls, guest, play, scores, showScores } from "../lib/client.js";
import * as logic from "./logic.js";

const who = guest();
const board = document.getElementById("board");
const paint = board.getContext("2d");
const cell = board.width / logic.WIDTH;
const button = document.getElementById("start");
const [score, result, note, table] = ["score", "result", "note", "scores"].map((id) =>
  document.getElementById(id),
);

function draw(state) {
  paint.clearRect(0, 0, board.width, board.height);
  paint.fillStyle = "#e4572e";
  const [fx, fy] = state.food ?? [-1, -1];
  paint.beginPath();
  paint.arc((fx + 0.5) * cell, (fy + 0.5) * cell, cell * 0.4, 0, 2 * Math.PI);
  paint.fill();
  state.snake.forEach(([x, y], i) => {
    paint.fillStyle = i === 0 ? "#a7e3b1" : "#4caf75";
    paint.fillRect(x * cell + 1, y * cell + 1, cell - 2, cell - 2);
  });
  score.textContent = state.score;
}

let round = null;
controls(
  { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right", w: "up", s: "down", a: "left", d: "right" },
  (name) => round?.move(name),
);

button.addEventListener("click", async () => {
  button.disabled = true;
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
