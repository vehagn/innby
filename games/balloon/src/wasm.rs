//! The rules as WebAssembly exports, for `logic.js`: a round per instance, so the server
//! replays each in an instance of its own. Plain numbers in and out, so it needs no
//! bindings or generated code.

use std::sync::Mutex;

use crate::{BALLOON_X, CLOUD_WIDTH, GAP, Game, HEIGHT, PX, RADIUS, TICK_MS, WIDTH};

static GAME: Mutex<Option<Game>> = Mutex::new(None);

fn with<T>(read: impl FnOnce(&Game) -> T) -> T {
    read(GAME.lock().unwrap().as_ref().expect("start comes first"))
}

#[unsafe(no_mangle)]
pub extern "C" fn start(seed: u32) {
    *GAME.lock().unwrap() = Some(Game::new(seed));
}

/// A tick; `puff` is 1 if the player puffed during it.
#[unsafe(no_mangle)]
pub extern "C" fn step(puff: u32) {
    GAME.lock()
        .unwrap()
        .as_mut()
        .expect("start comes first")
        .step(puff == 1);
}

#[unsafe(no_mangle)]
pub extern "C" fn tick() -> u32 {
    with(|game| game.tick)
}

#[unsafe(no_mangle)]
pub extern "C" fn score() -> u32 {
    with(|game| game.score)
}

#[unsafe(no_mangle)]
pub extern "C" fn over() -> u32 {
    with(|game| u32::from(game.over))
}

// For drawing, in pixels.

#[unsafe(no_mangle)]
pub extern "C" fn balloon_y() -> i32 {
    with(|game| game.y / PX)
}

#[unsafe(no_mangle)]
pub extern "C" fn clouds() -> u32 {
    with(|game| game.clouds.len() as u32)
}

#[unsafe(no_mangle)]
pub extern "C" fn cloud_x(i: u32) -> i32 {
    with(|game| game.clouds[i as usize].x / PX)
}

#[unsafe(no_mangle)]
pub extern "C" fn cloud_gap(i: u32) -> i32 {
    with(|game| game.clouds[i as usize].gap / PX)
}

// The board's constants, so the page needn't repeat them.

#[unsafe(no_mangle)]
pub extern "C" fn tick_ms() -> u32 {
    TICK_MS
}

#[unsafe(no_mangle)]
pub extern "C" fn width() -> i32 {
    WIDTH / PX
}

#[unsafe(no_mangle)]
pub extern "C" fn height() -> i32 {
    HEIGHT / PX
}

#[unsafe(no_mangle)]
pub extern "C" fn balloon_x() -> i32 {
    BALLOON_X / PX
}

#[unsafe(no_mangle)]
pub extern "C" fn radius() -> i32 {
    RADIUS / PX
}

#[unsafe(no_mangle)]
pub extern "C" fn cloud_width() -> i32 {
    CLOUD_WIDTH / PX
}

#[unsafe(no_mangle)]
pub extern "C" fn gap() -> i32 {
    GAP / PX
}
