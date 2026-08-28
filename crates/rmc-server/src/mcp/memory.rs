//! Reading and returning process memory.
//!
//! # Why this module exists
//!
//! Dropping a loaded rust-analyzer context frees the objects but does not give
//! the pages back to the kernel. Measured on a live daemon (24-core machine,
//! one `bur/rust_app` workspace loaded):
//!
//! ```text
//! before                       5291 MB RSS
//! clear_runtime semantic_only  5291 MB RSS   <- one project dropped, zero bytes returned
//! clear_runtime all            4922 MB RSS   <- only the search cache gave anything back
//! ```
//!
//! With an *empty* runtime — no projects, no cache entries — the process still
//! held 4.9 GB: 3.4 GB spread over 323 anonymous regions (glibc's per-thread
//! arenas, ~64 MB each) plus 1.3 GB in the main heap. rust-analyzer's salsa
//! database allocates millions of small objects from a rayon pool, which
//! fragments those arenas; `free()` returns the chunks to the arena and the
//! arena keeps them.
//!
//! So a "clear" that reports success while RSS does not move is telling the
//! truth about its own bookkeeping and lying about the thing the operator
//! actually wanted. [`release_free_memory`] is the missing half, and
//! [`rss_kib`] is what makes the result checkable rather than believed.

/// Resident set size of this process, in KiB.
///
/// `None` on platforms without `/proc` — the caller reports "unknown" rather
/// than inventing a number, because a fabricated zero would read as "we
/// released everything".
pub fn rss_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| parse_proc_status_rss_kib(&status))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(crate) fn parse_proc_status_rss_kib(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

/// Memory the kernel estimates is available for a new workload, in KiB.
///
/// `MemAvailable`, not `MemFree`: page cache and reclaimable slab are free for
/// the asking, and `MemFree` on a machine that has been up for a day reads as
/// an emergency when there is none.
///
/// This is the one number about the *machine* rather than the process. Without
/// it a daemon comfortably below its own RSS limit keeps gigabytes of analysis
/// cached while a `cargo` run next to it goes to swap — the caches belong to
/// whoever needs the memory more, and a build needs it more than a warm cache.
///
/// `None` on platforms without `/proc`, for the same reason as [`rss_kib`]: an
/// invented number here would read as "plenty of memory".
pub fn mem_available_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|meminfo| parse_proc_meminfo_available_kib(&meminfo))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(crate) fn parse_proc_meminfo_available_kib(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let value = line.strip_prefix("MemAvailable:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

/// mimalloc's own "give it back" entry point.
///
/// Declared here rather than imported: `libmimalloc-sys` binds only the
/// allocation calls (`mi_malloc`, `mi_free`, …) and stops there, while the
/// symbol itself is part of every mimalloc build. Depending on that crate is
/// still what puts the library on the link line — the extern block only names a
/// function it already contains.
#[cfg(feature = "mimalloc")]
unsafe extern "C" {
    fn mi_collect(force: bool);
}

/// Anchor that keeps `libmimalloc-sys` on the link line.
///
/// A bare `extern` block names a symbol without telling the linker where it
/// lives. `libmimalloc-sys` ships the archive that defines `mi_collect`, but a
/// dependency nothing references is dropped before linking, and the build then
/// fails with `undefined symbol: mi_collect` — while the very same code links
/// fine from the binary crate, which references mimalloc through its
/// `#[global_allocator]`. Referring to one bound function here is what makes
/// the archive present in both cases.
#[cfg(feature = "mimalloc")]
const _MIMALLOC_LINK_ANCHOR: unsafe extern "C" fn(usize) -> *mut std::ffi::c_void =
    libmimalloc_sys::mi_malloc;

/// Ask the allocator to return free memory to the kernel.
///
/// Returns whether a release was actually attempted: `false` means this build
/// has no way to do it, which the caller must not present as "nothing needed
/// releasing".
///
/// # Which allocator gets asked
///
/// Whichever one is actually linked. With the `mimalloc` feature the global
/// allocator is mimalloc, and trimming glibc's arenas would be a no-op against
/// the wrong heap — the exact shape of bug that makes a knob look broken. So
/// the feature switches the call, not just the allocator.
///
/// # What it can and cannot do
///
/// `malloc_trim` returns pages that are *entirely* free. A fragmented arena —
/// one live 64-byte object holding a 64 MB region — stays resident, and no
/// amount of trimming changes that. Treat a small drop as evidence of
/// fragmentation, not as a failed call: that is precisely why the caller
/// measures RSS on both sides instead of assuming.
pub fn release_free_memory() -> bool {
    #[cfg(feature = "mimalloc")]
    {
        // `true` = force: also return memory from thread-local heaps of threads
        // that are still alive, which is the whole point here — the rayon pool
        // that loaded the workspace is still parked, holding its heaps.
        //
        // SAFETY: `mi_collect` takes no pointers and is documented as callable
        // from any thread at any time.
        unsafe { mi_collect(true) };
        true
    }
    #[cfg(all(not(feature = "mimalloc"), target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` takes a pad in bytes and is safe to call at any
        // time from any thread; it only walks the allocator's own free lists.
        unsafe { libc::malloc_trim(0) };
        true
    }
    #[cfg(not(any(feature = "mimalloc", all(target_os = "linux", target_env = "gnu"))))]
    {
        false
    }
}

/// What a release attempt did, in numbers the caller can print.
///
/// `before`/`after` are `None` where RSS is unreadable; `released_kib` is then
/// `None` too rather than `0`, keeping "we could not tell" distinct from "we
/// freed nothing".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MemoryRelease {
    pub attempted: bool,
    pub rss_kib_before: Option<u64>,
    pub rss_kib_after: Option<u64>,
    pub released_kib: Option<u64>,
}

impl MemoryRelease {
    /// Not attempted, and honest about it.
    pub fn skipped() -> Self {
        let rss = rss_kib();
        Self {
            attempted: false,
            rss_kib_before: rss,
            rss_kib_after: rss,
            released_kib: None,
        }
    }
}

/// Run `release_free_memory` around an RSS measurement.
///
/// Saturating subtraction: RSS can legitimately *rise* across the call when
/// another thread allocates meanwhile, and a wrapped huge number would be worse
/// than reporting zero.
pub fn release_and_measure() -> MemoryRelease {
    let before = rss_kib();
    let attempted = release_free_memory();
    let after = rss_kib();
    let released = match (before, after) {
        (Some(before), Some(after)) => Some(before.saturating_sub(after)),
        _ => None,
    };
    MemoryRelease {
        attempted,
        rss_kib_before: before,
        rss_kib_after: after,
        released_kib: released,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_status_parser_reads_rss() {
        let status = "Name:\ttest\nVmRSS:\t  12345 kB\n";

        assert_eq!(parse_proc_status_rss_kib(status), Some(12345));
    }

    #[test]
    fn proc_status_parser_reports_absence_rather_than_zero() {
        assert_eq!(parse_proc_status_rss_kib("Name:\ttest\n"), None);
    }

    #[test]
    fn meminfo_parser_reads_available() {
        let meminfo = "MemTotal:       64000000 kB\n\
                       MemFree:         1000000 kB\n\
                       MemAvailable:   35000000 kB\n";

        assert_eq!(parse_proc_meminfo_available_kib(meminfo), Some(35_000_000));
    }

    /// `MemFree` must not be mistaken for it: the two differ by the whole page
    /// cache, and reading the wrong one would starve the daemon on a healthy
    /// machine.
    #[test]
    fn meminfo_parser_does_not_settle_for_memfree() {
        let meminfo = "MemTotal:       64000000 kB\nMemFree:         1000000 kB\n";

        assert_eq!(parse_proc_meminfo_available_kib(meminfo), None);
    }

    /// On Linux the reading has to exist, or the floor silently never applies.
    #[cfg(target_os = "linux")]
    #[test]
    fn available_memory_is_readable_on_this_platform() {
        assert!(
            mem_available_kib().is_some_and(|kib| kib > 0),
            "MemAvailable must be readable, otherwise the machine-wide floor is dead code"
        );
    }

    /// The measurement must survive RSS growing during the call rather than
    /// wrapping into a nonsense "released 18 exabytes".
    #[test]
    fn release_report_never_wraps() {
        let release = release_and_measure();

        if let Some(released) = release.released_kib {
            let before = release.rss_kib_before.expect("before present with delta");
            assert!(
                released <= before,
                "released {released} KiB cannot exceed the {before} KiB we started with"
            );
        }
    }

    /// On the platforms we actually ship to, "nothing to release" must not be
    /// indistinguishable from "this build cannot release".
    #[cfg(any(feature = "mimalloc", all(target_os = "linux", target_env = "gnu")))]
    #[test]
    fn release_is_available_on_supported_builds() {
        assert!(
            release_and_measure().attempted,
            "glibc and mimalloc builds must both have a working release path"
        );
    }
}
