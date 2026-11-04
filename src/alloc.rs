//! The memory allocator: mimalloc, giving freed memory back to the system at once.
//!
//! The images are static musl binaries, and musl's own allocator keeps the memory it
//! frees: a few minutes of ordinary traffic left `innby serve` at 40 MiB and `innby
//! admin` past 100 MiB, over the chart's 64 MiB limit, never to shrink. mimalloc is as
//! fast or faster, and with no delay before it purges, both stay near what's in use
//! (`just bench` measures it; docs/decisions.md has the numbers).

use libmimalloc_sys::{mi_option_set, mi_option_t};

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// `mi_option_purge_delay` in mimalloc.h, which libmimalloc-sys doesn't name: the option
/// before `mi_option_use_numa_nodes` (16), the same in mimalloc 2 and 3.
const PURGE_DELAY: mi_option_t = 15;

/// Purges freed memory at once rather than after a delay. Called first thing in `main`,
/// before any other thread runs, since mimalloc's options aren't thread-safe.
pub fn purge_at_once() {
    // SAFETY: setting an option is a plain write to mimalloc's option table, and no
    // other thread exists yet to race it.
    unsafe { mi_option_set(PURGE_DELAY, 0) };
}

#[cfg(test)]
mod tests {
    use libmimalloc_sys::{mi_option_get, mi_option_use_numa_nodes};

    use super::*;

    #[test]
    fn the_purge_delay_is_the_option_before_the_numa_nodes() {
        assert_eq!(PURGE_DELAY + 1, mi_option_use_numa_nodes);
        purge_at_once();
        // SAFETY: a plain read of mimalloc's option table.
        assert_eq!(unsafe { mi_option_get(PURGE_DELAY) }, 0);
    }
}
