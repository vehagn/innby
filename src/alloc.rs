//! The memory allocator: mimalloc, giving freed memory back to the system at once, in
//! ordinary pages rather than huge ones.
//!
//! The images are static musl binaries, and musl's own allocator keeps the memory it
//! frees: a few minutes of ordinary traffic left `innby serve` at 40 MiB and `innby
//! admin` past 100 MiB, over the chart's 64 MiB limit, never to shrink. mimalloc is as
//! fast or faster, and with no delay before it purges, both stay near what's in use
//! (`just bench` measures it; docs/decisions.md has the numbers).
//!
//! mimalloc also asks Linux for transparent huge pages, so touching a little of a 2 MiB
//! stretch charges the pod for all of it: with a full message board on the guest's page,
//! that alone took `innby serve` from 28 MiB to 56 under load, over its budget. The
//! process turns them off for itself before mimalloc has handed out much; the option
//! mimalloc has for it is read as it starts, before `main` can set it.

use libmimalloc_sys::{mi_option_set, mi_option_t};

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// `mi_option_purge_delay` in mimalloc.h, which libmimalloc-sys doesn't name: the option
/// before `mi_option_use_numa_nodes` (16), the same in mimalloc 2 and 3.
const PURGE_DELAY: mi_option_t = 15;

/// Purges freed memory at once rather than after a delay, and keeps to ordinary pages.
/// Called first thing in `main`, before any other thread runs, since mimalloc's options
/// aren't thread-safe.
pub fn configure() {
    // SAFETY: setting an option is a plain write to mimalloc's option table, and no
    // other thread exists yet to race it.
    unsafe { mi_option_set(PURGE_DELAY, 0) };
    // SAFETY: PR_SET_THP_DISABLE takes plain integers and touches no memory of ours. It
    // can only fail on a kernel without transparent huge pages, where there's nothing to
    // turn off.
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0)
    };
}

#[cfg(test)]
mod tests {
    use libmimalloc_sys::{mi_option_get, mi_option_use_numa_nodes};

    use super::*;

    #[test]
    fn the_purge_delay_is_the_option_before_the_numa_nodes() {
        assert_eq!(PURGE_DELAY + 1, mi_option_use_numa_nodes);
        configure();
        // SAFETY: a plain read of mimalloc's option table.
        assert_eq!(unsafe { mi_option_get(PURGE_DELAY) }, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transparent_huge_pages_are_off() {
        configure();
        // SAFETY: PR_GET_THP_DISABLE takes plain integers and touches no memory of ours.
        assert_eq!(
            unsafe { libc::prctl(libc::PR_GET_THP_DISABLE, 0, 0, 0, 0) },
            1
        );
    }
}
