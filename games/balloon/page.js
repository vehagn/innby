// Balloon's page: draws the board, and plays rounds with ../lib/client.js.
import { controls, guest, play, scores, showScores } from "../lib/client.js";
import * as logic from "./logic.js";

const who = guest();
const board = document.getElementById("board");
const paint = board.getContext("2d");
const button = document.getElementById("start");
const [score, result, note, table] = ["score", "result", "note", "scores"].map((id) =>
  document.getElementById(id),
);

function draw(state) {
  paint.fillStyle = "#2b4c6f";
  paint.fillRect(0, 0, logic.WIDTH, logic.HEIGHT);
  paint.fillStyle = "#dfe7ee";
  for (const cloud of state.clouds) {
    const top = cloud.gap - logic.GAP / 2;
    paint.fillRect(cloud.x, 0, logic.CLOUD_WIDTH, top);
    paint.fillRect(cloud.x, top + logic.GAP, logic.CLOUD_WIDTH, logic.HEIGHT - top - logic.GAP);
  }
  paint.strokeStyle = "#f2c14e";
  paint.beginPath();
  paint.moveTo(logic.BALLOON_X, state.y + logic.RADIUS);
  paint.lineTo(logic.BALLOON_X - 4, state.y + logic.RADIUS * 2);
  paint.stroke();
  paint.fillStyle = "#e4572e";
  paint.beginPath();
  paint.arc(logic.BALLOON_X, state.y, logic.RADIUS, 0, 2 * Math.PI);
  paint.fill();
  score.textContent = state.score;
}

let round = null;
const puff = () => round?.move("puff");
controls({ " ": "puff", ArrowUp: "puff", w: "puff" }, () => puff());
board.addEventListener("pointerdown", puff);

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
