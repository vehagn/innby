// The real client and pages must finish failed rounds too, without a browser or a clock.
import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { play } from "../lib/client.js";
import * as balloon from "../balloon/logic.js";
import * as snake from "../snake/logic.js";
import * as tetris from "../tetris/logic.js";

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function observe(promise) {
  const outcome = { status: "pending" };
  promise.then(
    (value) => Object.assign(outcome, { status: "fulfilled", value }),
    (error) => Object.assign(outcome, { status: "rejected", error }),
  );
  return outcome;
}

// A missing settlement should fail an assertion, not leave the test waiting forever.
async function until(predicate, message, limit = 100) {
  for (let i = 0; i < limit; i += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  assert.ok(predicate(), message);
}

async function settled(outcome) {
  await until(() => outcome.status !== "pending", "the promise did not settle");
  return outcome;
}

function globals(t, values) {
  const saved = Object.keys(values).map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  t.after(() => {
    for (const [name, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  });
  for (const [name, value] of Object.entries(values)) {
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  }
}

function harness(t) {
  let now = 0;
  let next = 0;
  const frames = new Map();
  const requests = [];
  const fetch = t.mock.fn((url, options) => {
    const pending = deferred();
    requests.push({
      url, options,
      respond(body, status = 200) {
        pending.resolve({ ok: status >= 200 && status < 300, status, json: async () => body });
      },
      reject: pending.reject,
    });
    return pending.promise;
  });
  const requestAnimationFrame = t.mock.fn((callback) => {
    frames.set(++next, callback);
    return next;
  });
  globals(t, {
    fetch,
    performance: { now: t.mock.fn(() => now) },
    requestAnimationFrame,
    cancelAnimationFrame: t.mock.fn((id) => frames.delete(id)),
  });
  return {
    requests, frames, fetch, requestAnimationFrame,
    frame(milliseconds) {
      now += milliseconds;
      const callbacks = [...frames.values()];
      frames.clear();
      for (const callback of callbacks) callback(now);
    },
  };
}

async function pump(clock, predicate, tick, limit = 10_000, beforeFrame = () => {}) {
  for (let i = 0; i < limit; i += 1) {
    if (predicate()) return;
    assert.ok(clock.frames.size <= 1, "a round must have at most one queued frame");
    if (clock.frames.size) beforeFrame();
    assert.doesNotThrow(() => clock.frame(tick), "frame errors must settle round.done");
    await Promise.resolve();
  }
  assert.ok(predicate(), "the round did not finish within the frame bound");
}

const who = { token: "opaque token/+=", lang: "nb-NO" };

async function begin(clock, logic, draw, guest = who) {
  const outcome = observe(play(logic, guest, draw));
  assert.equal(clock.requests.length, 1);
  assert.equal(clock.requests[0].url, "api/rounds");
  assert.deepEqual(clock.requests[0].options, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: undefined,
  });
  clock.requests[0].respond({ round: "round-1", seed: 7 });
  await settled(outcome);
  assert.equal(outcome.status, "fulfilled");
  return outcome.value;
}

function stopped(clock, round, callbacks = []) {
  assert.equal(clock.frames.size, 0);
  const requests = clock.requests.length;
  const scheduled = clock.requestAnimationFrame.mock.callCount();
  const calls = callbacks.map((callback) => callback.mock.callCount());
  assert.doesNotThrow(() => round?.move("right"));
  for (let i = 0; i < 3; i += 1) clock.frame(1_000);
  assert.equal(clock.frames.size, 0);
  assert.equal(clock.requests.length, requests);
  assert.equal(clock.requestAnimationFrame.mock.callCount(), scheduled);
  assert.deepEqual(callbacks.map((callback) => callback.mock.callCount()), calls);
}

function target(t, values = {}) {
  const listeners = new Map();
  return {
    textContent: "",
    disabled: false,
    ...values,
    listeners,
    addEventListener: t.mock.fn((name, listener) => {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(listener);
    }),
    dispatch(name, event = {}) {
      for (const listener of listeners.get(name) ?? []) listener(event);
    },
  };
}

function pageBrowser(t, game, logic) {
  const keyboard = target(t);
  const paint = Object.fromEntries(
    ["clearRect", "fillRect", "beginPath", "arc", "fill", "moveTo", "lineTo", "stroke"]
      .map((name) => [name, t.mock.fn()]),
  );
  const elements = Object.fromEntries(
    ["score", "level", "lines", "result", "note", "scores"].map((id) => [id, target(t)]),
  );
  elements.start = target(t, { textContent: "Start", blur: t.mock.fn() });
  elements.board = target(t, {
    width: logic.WIDTH, height: logic.HEIGHT,
    getContext: t.mock.fn((kind) => {
      assert.equal(kind, "2d");
      return paint;
    }),
  });
  elements.scores.replaceChildren = t.mock.fn();
  elements.scores.insertRow = t.mock.fn(() => ({
    insertCell: t.mock.fn(() => ({ textContent: "" })),
  }));
  const buttons = logic.MOVES.map((move) => target(t, { dataset: { move } }));
  const history = { replaceState: t.mock.fn() };
  globals(t, {
    location: { hash: `#${new URLSearchParams(who)}`, pathname: `/${game}/`, search: "?test=1" },
    history,
    document: {
      getElementById: t.mock.fn((id) => {
        assert.ok(elements[id], `unexpected element: ${id}`);
        return elements[id];
      }),
      querySelectorAll: t.mock.fn((selector) => {
        assert.equal(selector, "[data-move]");
        return buttons;
      }),
    },
    addEventListener: keyboard.addEventListener,
  });
  return {
    elements, buttons, history,
    key(key) {
      const preventDefault = t.mock.fn();
      keyboard.dispatch("keydown", { key, preventDefault });
      assert.equal(preventDefault.mock.callCount(), 1);
    },
  };
}

describe("the game client and its pages", { concurrency: false }, () => {
  test("a successful round submits the moves, token and language unchanged", async (t) => {
    const clock = harness(t);
    const start = t.mock.fn(snake.start);
    const draw = t.mock.fn();
    const guest = { ...who };
    const round = await begin(clock, { ...snake, start }, draw, guest);
    const outcome = observe(round.done);
    assert.deepEqual(start.mock.calls[0].arguments, [7]);
    assert.equal(draw.mock.callCount(), 1);
    round.move("up");
    round.move("right");
    clock.frame(snake.TICK_MS);
    round.move("down");
    clock.frame(snake.TICK_MS);
    await pump(clock, () => clock.requests.length === 2, snake.TICK_MS);
    const submission = clock.requests[1];
    assert.equal(submission.url, "api/rounds/round-1");
    assert.deepEqual(submission.options, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ moves: [[0, "up"], [0, "right"], [1, "down"]], ...who }),
    });
    assert.deepEqual(guest, who);
    assert.equal(outcome.status, "pending");
    assert.equal(draw.mock.calls.at(-1).arguments[0].over, true);
    const answer = { score: 10, saved: true };
    submission.respond(answer);
    await settled(outcome);
    assert.deepEqual(outcome, { status: "fulfilled", value: { ok: true, status: 200, body: answer } });
    stopped(clock, round, [draw]);
  });

  test("a final network rejection rejects round.done and leaves no frames", async (t) => {
    const clock = harness(t);
    const draw = t.mock.fn();
    const round = await begin(clock, snake, draw);
    const outcome = observe(round.done);
    await pump(clock, () => clock.requests.length === 2, snake.TICK_MS);
    const failure = new Error("score submission unavailable");
    clock.requests[1].reject(failure);
    await settled(outcome);
    assert.deepEqual(outcome, { status: "rejected", error: failure });
    stopped(clock, round, [draw]);
  });

  test("a non-OK final response still resolves to the response object", async (t) => {
    const clock = harness(t);
    const round = await begin(clock, snake, t.mock.fn());
    const outcome = observe(round.done);
    await pump(clock, () => clock.requests.length === 2, snake.TICK_MS);
    const body = { error: "no such round" };
    clock.requests[1].respond(body, 422);
    await settled(outcome);
    assert.deepEqual(outcome, { status: "fulfilled", value: { ok: false, status: 422, body } });
    stopped(clock, round);
  });

  test("a throwing tick rejects round.done without drawing or submitting again", async (t) => {
    const clock = harness(t);
    const failure = new Error("tick failed");
    const step = t.mock.fn((state, moves) => {
      if (state.tick === 1) throw failure;
      snake.step(state, moves);
    });
    const draw = t.mock.fn();
    const round = await begin(clock, { ...snake, step }, draw);
    const outcome = observe(round.done);
    await pump(clock, () => outcome.status !== "pending", snake.TICK_MS, 100);
    assert.deepEqual(outcome, { status: "rejected", error: failure });
    assert.equal(step.mock.callCount(), 2);
    assert.equal(draw.mock.callCount(), 2);
    assert.equal(clock.requests.length, 1);
    stopped(clock, round, [step, draw]);
  });

  for (const when of ["first tick", "game over"]) {
    test(`a drawing failure at ${when} rejects round.done without submitting`, async (t) => {
      const clock = harness(t);
      const failure = new Error("drawing failed");
      const draw = t.mock.fn((state) => {
        if (when === "first tick" ? state.tick > 0 : state.over) throw failure;
      });
      const round = await begin(clock, snake, draw);
      const outcome = observe(round.done);
      await pump(clock, () => outcome.status !== "pending", snake.TICK_MS, 100);
      assert.deepEqual(outcome, { status: "rejected", error: failure });
      assert.equal(clock.requests.length, 1);
      stopped(clock, round, [draw]);
    });
  }

  for (const kind of ["network", "non-OK response", "logic", "initial draw"]) {
    test(`a ${kind} start failure rejects play without scheduling a round`, async (t) => {
      const clock = harness(t);
      const failure = new Error(`${kind} failed`);
      const start = t.mock.fn((seed) => {
        if (kind === "logic") throw failure;
        return snake.start(seed);
      });
      const draw = t.mock.fn(() => {
        if (kind === "initial draw") throw failure;
      });
      const outcome = observe(play({ ...snake, start }, who, draw));
      assert.equal(clock.requests.length, 1);
      if (kind === "network") clock.requests[0].reject(failure);
      else clock.requests[0].respond({ round: "round-1", seed: 7 }, kind === "non-OK response" ? 503 : 200);
      await settled(outcome);
      assert.equal(outcome.status, "rejected");
      if (kind === "non-OK response") assert.match(outcome.error.message, /start a round/);
      else assert.equal(outcome.error, failure);
      assert.equal(start.mock.callCount(), ["network", "non-OK response"].includes(kind) ? 0 : 1);
      assert.equal(draw.mock.callCount(), kind === "initial draw" ? 1 : 0);
      assert.equal(clock.requestAnimationFrame.mock.callCount(), 0);
      stopped(clock, null, [start, draw]);
    });
  }

  for (const [game, logic, key, move] of [
    ["snake", snake, "ArrowRight", "right"],
    ["tetris", tetris, " ", "drop"],
    ["balloon", balloon, " ", "puff"],
  ]) {
    test(`${game}'s page recovers from a rejected submission and completes another round`, async (t) => {
      const clock = harness(t);
      const browser = pageBrowser(t, game, logic);
      await import(`../${game}/page.js?client-test=${game}`);
      const { start, result, note, board } = browser.elements;
      assert.deepEqual(browser.history.replaceState.mock.calls[0].arguments, [null, "", `/${game}/?test=1`]);
      assert.equal(clock.requests.length, 1);
      assert.equal(clock.requests[0].url, `api/scores?lang=${who.lang}`);
      assert.equal(clock.requests[0].options.headers.Authorization, `Bearer ${who.token}`);
      clock.requests[0].respond({ scores: [] });
      await until(() => note.textContent === "No scores yet.", "the page did not load its high scores");
      assert.equal(start.listeners.get("click").length, 1);
      const click = start.listeners.get("click")[0];

      function idleControls() {
        const requests = clock.requests.length;
        assert.doesNotThrow(() => {
          browser.key(key);
          for (const button of browser.buttons) button.dispatch("click");
          board.dispatch("pointerdown");
        });
        assert.equal(clock.frames.size, 0);
        assert.equal(clock.requests.length, requests);
      }

      idleControls();
      for (let attempt = 1; attempt <= 2; attempt += 1) {
        const before = clock.requests.length;
        const completion = observe(click());
        assert.equal(start.disabled, true);
        assert.equal(result.textContent, "");
        assert.equal(clock.requests.length, before + 1);
        assert.equal(clock.requests.at(-1).url, "api/rounds");
        clock.requests.at(-1).respond({ round: `page-${attempt}`, seed: 7 });
        await until(() => clock.frames.size === 1, "the page did not start a round");
        // Let the start handler receive the round before sending controls.
        await Promise.resolve();
        assert.equal(completion.status, "pending");
        assert.doesNotThrow(() => {
          browser.key(key);
          browser.buttons.find((button) => button.dataset.move === move).dispatch("click");
          board.dispatch("pointerdown");
        });
        await pump(clock, () => clock.requests.length === before + 2, logic.TICK_MS, 10_000, () => {
          // Hard drops reach Tetris's natural game over without thousands of redraws.
          if (game === "tetris") browser.key(key);
        });
        const submission = clock.requests.at(-1);
        assert.equal(submission.url, `api/rounds/page-${attempt}`);
        const sent = JSON.parse(submission.options.body);
        assert.equal(sent.token, who.token);
        assert.equal(sent.lang, who.lang);
        assert.ok(sent.moves.length > 0, "the page's controls must reach the real round");
        assert.ok(sent.moves.every(([tick, name]) => Number.isInteger(tick) && logic.MOVES.includes(name)));
        assert.equal(clock.frames.size, 0);
        assert.equal(start.disabled, true);
        assert.equal(completion.status, "pending");
        if (attempt === 1) submission.reject(new Error("score submission unavailable"));
        else submission.respond({ score: 42, saved: true });
        await settled(completion);
        assert.equal(completion.status, "fulfilled", "the page must handle the round's failure itself");
        assert.equal(result.textContent, attempt === 1 ? "score submission unavailable" : "42 points.");
        assert.equal(start.disabled, false);
        assert.equal(start.textContent, "Play again");
        idleControls();
        stopped(clock, null);
      }
    });
  }
});