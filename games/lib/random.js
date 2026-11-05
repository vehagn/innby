// A seeded random number generator (mulberry32), the same in the browser and on the
// server, so a round replays exactly from its seed and the player's moves.
export function random(seed) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  // A whole number from 0 up to, but not including, n.
  next.below = (n) => Math.floor(next() * n);
  return next;
}
