// Plays a round from its seed and the moves the player made, as `[tick, move]` pairs in
// tick order: what the browser did as it went, and what the server does again to work
// out the score. A game's logic is a module with `start(seed)`, giving a state, and
// `step(state, moves)`, moving it on one tick; the state has `tick`, `score` and `over`.
export function replay(logic, seed, inputs, maxTicks) {
  const state = logic.start(seed);
  let next = 0;
  while (!state.over && state.tick < maxTicks) {
    const moves = [];
    while (next < inputs.length && inputs[next][0] === state.tick) {
      moves.push(inputs[next][1]);
      next += 1;
    }
    logic.step(state, moves);
  }
  return state;
}
