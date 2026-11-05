//! Balloon's rules: a balloon sinks, a puff of air lifts it, and clouds drift past with a
//! gap in each to steer it through: a point a gap. Whole numbers only, and every random
//! number from the round's seed, so a round replays exactly, in the browser or on the
//! server (`wasm.rs` is how they call it).
//!
//! Distances are in sixteenths of a pixel of a 320 × 240 board, so the balloon moves
//! smoothly without fractions.

mod wasm;

/// A pixel, in the units positions are kept in.
pub const PX: i32 = 16;
pub const WIDTH: i32 = 320 * PX;
pub const HEIGHT: i32 = 240 * PX;
/// A tick, in milliseconds: the board moves on this often.
pub const TICK_MS: u32 = 33;

/// Where the balloon floats across, and its radius.
pub const BALLOON_X: i32 = 80 * PX;
pub const RADIUS: i32 = 10 * PX;
/// How hard it sinks a tick, and how fast a puff sends it up.
const GRAVITY: i32 = 6;
const PUFF: i32 = -70;
/// How fast it may fall.
const MOST_FALL: i32 = 120;

/// A cloud's width, the gap in it, and how far it drifts a tick.
pub const CLOUD_WIDTH: i32 = 40 * PX;
pub const GAP: i32 = 90 * PX;
const DRIFT: i32 = 2 * PX;
/// Ticks between clouds.
const SPACING: u32 = 90;

/// A cloud: its left edge, and the middle of its gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cloud {
    pub x: i32,
    pub gap: i32,
    /// Whether the balloon is past it, and has its point.
    pub passed: bool,
}

#[derive(Debug, Clone)]
pub struct Game {
    random: Random,
    pub tick: u32,
    pub score: u32,
    pub over: bool,
    pub y: i32,
    velocity: i32,
    pub clouds: Vec<Cloud>,
}

impl Game {
    pub fn new(seed: u32) -> Self {
        let mut game = Self {
            random: Random(seed),
            tick: 0,
            score: 0,
            over: false,
            y: HEIGHT / 2,
            velocity: 0,
            clouds: Vec::new(),
        };
        game.add_cloud();
        game
    }

    /// Moves the game on a tick; `puff` if the player puffed during it.
    pub fn step(&mut self, puff: bool) {
        if self.over {
            return;
        }
        self.tick += 1;
        self.velocity = if puff {
            PUFF
        } else {
            (self.velocity + GRAVITY).min(MOST_FALL)
        };
        self.y += self.velocity;
        for cloud in &mut self.clouds {
            cloud.x -= DRIFT;
            if !cloud.passed && cloud.x + CLOUD_WIDTH < BALLOON_X - RADIUS {
                cloud.passed = true;
                self.score += 1;
            }
        }
        self.clouds.retain(|cloud| cloud.x + CLOUD_WIDTH > 0);
        if self.tick.is_multiple_of(SPACING) {
            self.add_cloud();
        }
        self.over = self.y - RADIUS < 0 || self.y + RADIUS > HEIGHT || self.hits_cloud();
    }

    /// Whether the balloon, as the square around it, touches a cloud outside its gap.
    fn hits_cloud(&self) -> bool {
        self.clouds.iter().any(|cloud| {
            let across = BALLOON_X + RADIUS > cloud.x && BALLOON_X - RADIUS < cloud.x + CLOUD_WIDTH;
            let in_gap =
                self.y - RADIUS > cloud.gap - GAP / 2 && self.y + RADIUS < cloud.gap + GAP / 2;
            across && !in_gap
        })
    }

    /// A cloud at the right edge, its gap somewhere a balloon can reach.
    fn add_cloud(&mut self) {
        let lowest = GAP / 2 + 20 * PX;
        let highest = HEIGHT - GAP / 2 - 20 * PX;
        let gap = lowest + self.random.below((highest - lowest) as u32) as i32;
        self.clouds.push(Cloud {
            x: WIDTH,
            gap,
            passed: false,
        });
    }
}

/// mulberry32: small, and the same everywhere.
#[derive(Debug, Clone)]
struct Random(u32);

impl Random {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }

    /// A whole number from 0 up to, but not including, n.
    fn below(&mut self, n: u32) -> u32 {
        ((u64::from(self.next()) * u64::from(n)) >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(seed: u32, puff: impl Fn(&Game) -> bool) -> Game {
        let mut game = Game::new(seed);
        while !game.over && game.tick < 100_000 {
            let now = puff(&game);
            game.step(now);
        }
        game
    }

    #[test]
    fn without_a_puff_it_sinks_to_the_ground() {
        let game = play(1, |_| false);
        assert!(game.over);
        assert!(game.y + RADIUS > HEIGHT);
        assert_eq!(game.score, 0);
    }

    #[test]
    fn a_puff_lifts_it() {
        let mut game = Game::new(1);
        let y = game.y;
        game.step(true);
        assert!(game.y < y);
    }

    #[test]
    fn the_same_seed_gives_the_same_clouds() {
        assert_eq!(Game::new(7).clouds, Game::new(7).clouds);
        assert_ne!(Game::new(7).clouds, Game::new(8).clouds);
    }

    #[test]
    fn steering_through_the_gaps_scores() {
        // Puff whenever it's below the next gap's middle.
        let game = play(3, |game| {
            let next = game.clouds.iter().find(|cloud| !cloud.passed);
            next.is_some_and(|cloud| game.y > cloud.gap)
        });
        assert!(game.score >= 5, "{}", game.score);
    }

    #[test]
    fn every_gap_is_within_reach() {
        let mut random = Random(5);
        for _ in 0..1000 {
            let mut game = Game::new(random.next());
            for _ in 0..SPACING * 3 {
                game.step(false);
                game.y = HEIGHT / 2;
                game.over = false;
            }
            for cloud in &game.clouds {
                assert!(cloud.gap - GAP / 2 > 0 && cloud.gap + GAP / 2 < HEIGHT);
            }
        }
    }
}
