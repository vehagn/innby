// What a game's page needs, in the browser: the guest's token from the address, rounds
// from the game's server, a steady tick, the moves made, and the high score table. See
// docs/games.md; snake/ and tetris/ use it.
/**
 * The guest's score token and language, from the fragment Innby's link put them in.
 * They're taken out of the address bar, so a bookmark or a link to this page doesn't
 * carry the token. Without one, the game plays without keeping the score.
 */
export function guest() {
  const params = new URLSearchParams(location.hash.slice(1));
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  return { token: params.get("token"), lang: params.get("lang") || "en" };
}

/** Calls the game's own server, under ./api/. */
async function api(path, { method = "GET", body, token } = {}) {
  const response = await fetch(`api/${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: body && JSON.stringify(body),
  });
  return { ok: response.ok, status: response.status, body: await response.json().catch(() => ({})) };
}

/**
 * Plays one round of a game: asks the server for a seed, ticks the logic every
 * `logic.TICK_MS`, and gives the server the moves at the end, which works out the
 * score. `draw(state)` is called after each tick. Returns `{ move, done }`: `move(name)`
 * makes a move on the next tick, and `done` resolves with the server's answer.
 */
export async function play(logic, who, draw) {
  const started = await api("rounds", { method: "POST" });
  if (!started.ok) throw new Error("the game's server didn't start a round");
  const { round, seed } = started.body;
  const state = logic.start(seed);
  const moves = [];
  let queued = [];
  draw(state);

  let finish;
  const done = new Promise((resolve) => (finish = resolve));
  let last = performance.now();
  let owed = 0;
  const frame = (now) => {
    // A hidden tab doesn't play on, and doesn't catch up in a rush either: taking
    // longer than the moves need is fine, taking less isn't.
    owed = Math.min(owed + (now - last), logic.TICK_MS * 2);
    last = now;
    while (owed >= logic.TICK_MS && !state.over) {
      owed -= logic.TICK_MS;
      for (const name of queued) moves.push([state.tick, name]);
      logic.step(state, queued);
      queued = [];
      draw(state);
    }
    if (state.over) {
      api(`rounds/${round}`, {
        method: "POST",
        body: { moves, token: who.token, lang: who.lang },
      }).then(finish);
    } else {
      requestAnimationFrame(frame);
    }
  };
  requestAnimationFrame(frame);
  return { move: (name) => queued.push(name), done };
}

/** The event's high score table, for a guest with a token. */
export async function scores(who) {
  if (!who.token) return null;
  const answer = await api(`scores?lang=${encodeURIComponent(who.lang)}`, { token: who.token });
  return answer.ok ? answer.body : null;
}

/**
 * Shows a high score table in a <table>'s <tbody>, or why there isn't one in `note`.
 */
export function showScores(body, note, table, who) {
  body.replaceChildren();
  if (!who.token) {
    note.textContent = "Open this game from your invitation's page to keep your score.";
    return;
  }
  if (!table || table.scores.length === 0) {
    note.textContent = table ? "No scores yet." : "The high scores couldn't be loaded.";
    return;
  }
  note.textContent = "";
  table.scores.forEach((line, i) => {
    const row = body.insertRow();
    if (line.you) row.className = "you";
    for (const text of [i + 1, line.name + (line.you ? " (you)" : ""), line.shown]) {
      row.insertCell().textContent = text;
    }
  });
}

/**
 * Wires the keyboard, and buttons with `data-move="{name}"` for touch screens, to a
 * round's moves. `keys` maps `KeyboardEvent.key` to a move.
 */
export function controls(keys, move) {
  addEventListener("keydown", (event) => {
    const name = keys[event.key];
    if (name) {
      event.preventDefault();
      move(name);
    }
  });
  for (const button of document.querySelectorAll("[data-move]")) {
    button.addEventListener("click", () => move(button.dataset.move));
  }
}
