//! Reading and returning process memory.
//!
//! # Why this module exists
//!
//! Dropping a loaded rust-analyzer context frees the objects but does not give
//! the pages back to the kernel. Measured on a live daemon (24-core machine,
//! one large workspace loaded):
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

/// The process's memory split by what backs it, from `/proc/self/smaps_rollup`.
///
/// `rss` alone cannot be decomposed: a page of the binary and a page of salsa
/// memo count the same in it. The split here is the first cut of any portrait —
/// file pages belong to no allocator at all, and swapped anonymous pages belong
/// to the heap while not being resident, so a portrait of a partly swapped
/// daemon that summed to RSS would be summing to the wrong total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ProcessMappings {
    pub rss_kib: u64,
    /// Resident private anonymous memory: heaps, thread stacks, anonymous `mmap`.
    pub anonymous_kib: u64,
    /// Anonymous memory that is currently in swap rather than resident.
    pub swap_kib: u64,
}

impl ProcessMappings {
    /// Resident pages that are not anonymous: the binary and its libraries,
    /// mapped model and index files, shared memory.
    pub fn file_kib(&self) -> u64 {
        self.rss_kib.saturating_sub(self.anonymous_kib)
    }

    /// Everything the process holds, resident or not — the total a portrait
    /// has to add up to.
    pub fn held_kib(&self) -> u64 {
        self.rss_kib + self.swap_kib
    }
}

/// `None` off Linux, and `None` rather than zeros when a field is missing.
pub fn smaps_rollup() -> Option<ProcessMappings> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/smaps_rollup")
            .ok()
            .and_then(|text| parse_smaps_rollup(&text))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(crate) fn parse_smaps_rollup(text: &str) -> Option<ProcessMappings> {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix(':')?;
            value.split_whitespace().next()?.parse::<u64>().ok()
        })
    };
    Some(ProcessMappings {
        rss_kib: field("Rss")?,
        anonymous_kib: field("Anonymous")?,
        swap_kib: field("Swap")?,
    })
}

/// Resident and swapped memory of all mappings sharing one name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MappingUsage {
    /// The mapped file's path, or the kernel's bracketed name (`[heap]`,
    /// `[stack]`); unnamed anonymous mappings are all `[anonymous]`.
    pub name: String,
    pub mappings: usize,
    pub rss_kib: u64,
    pub swap_kib: u64,
}

/// The mappings holding the most memory, grouped by name, largest first.
///
/// Answers what `ProcessMappings` cannot: *which* file is resident (the ONNX
/// model, an LMDB environment, a tantivy segment) and how the anonymous part
/// splits between glibc's main heap and the rest. Empty off Linux.
pub fn largest_mappings(top: usize) -> Vec<MappingUsage> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/smaps")
            .map(|text| parse_smaps(&text, top))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = top;
        Vec::new()
    }
}

/// The header line of one mapping in `/proc/<pid>/smaps`.
struct MappingHeader<'a> {
    start: u64,
    end: u64,
    perms: &'a str,
    /// The mapped file's path or the kernel's bracketed name; `[anonymous]`
    /// for an unnamed mapping.
    name: &'a str,
}

/// `None` for the field lines (`Rss:`, `VmFlags:` …) that follow a header.
fn parse_mapping_header(line: &str) -> Option<MappingHeader<'_>> {
    // A header is `start-end perms offset dev inode [name]`; every field line
    // starts with a `Name:` token instead.
    let mut fields = line.splitn(6, char::is_whitespace);
    let range = fields.next()?;
    if range.ends_with(':') {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    let perms = fields.next()?;
    // The name may itself contain spaces, so it is the rest of the line after
    // the fifth field rather than the sixth token.
    let name = fields
        .nth(3)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("[anonymous]");
    Some(MappingHeader {
        start: u64::from_str_radix(start, 16).ok()?,
        end: u64::from_str_radix(end, 16).ok()?,
        perms,
        name,
    })
}

pub(crate) fn parse_smaps(text: &str, top: usize) -> Vec<MappingUsage> {
    let mut by_name: std::collections::HashMap<String, MappingUsage> = Default::default();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let mut tokens = line.split_whitespace();
        let Some(first) = tokens.next() else { continue };
        if let Some(header) = parse_mapping_header(line) {
            let name = header.name.to_owned();
            by_name
                .entry(name.clone())
                .or_insert_with(|| MappingUsage {
                    name: name.clone(),
                    mappings: 0,
                    rss_kib: 0,
                    swap_kib: 0,
                })
                .mappings += 1;
            current = Some(name);
            continue;
        }
        let Some(name) = &current else { continue };
        let value = || {
            tokens
                .clone()
                .next()
                .and_then(|value| value.parse::<u64>().ok())
        };
        let usage = by_name.get_mut(name).unwrap();
        match first {
            "Rss:" => usage.rss_kib += value().unwrap_or(0),
            "Swap:" => usage.swap_kib += value().unwrap_or(0),
            _ => {}
        }
    }
    let mut usages: Vec<MappingUsage> = by_name.into_values().collect();
    usages.sort_by(|a, b| {
        (b.rss_kib + b.swap_kib)
            .cmp(&(a.rss_kib + a.swap_kib))
            .then_with(|| a.name.cmp(&b.name))
    });
    usages.truncate(top);
    usages
}

/// glibc's `HEAP_MAX_SIZE` on 64-bit targets (`2 * DEFAULT_MMAP_THRESHOLD_MAX`).
///
/// Every heap of a non-main arena is mapped at an address aligned to it —
/// `new_heap` maps twice the size and cuts to the boundary — because `free`
/// finds a chunk's heap by masking the chunk's address. That alignment is what
/// tells an arena heap apart from the other anonymous mappings in `smaps`.
const GLIBC_HEAP_ALIGN: u64 = 64 << 20;

/// How much of glibc's arenas is resident or swapped, read from the kernel.
///
/// `mallinfo2` cannot say: its `arena` and `fordblks` count address space, and
/// pages that `malloc_trim` returned with `MADV_DONTNEED` — or that were never
/// touched — stay in both. On the live daemon (2026-10-21) that was 6.1 GiB of
/// 16.4 GB of arenas, exactly what a trim 22 minutes earlier had returned, and a
/// portrait cut from `mallinfo2` alone read 10.8 GB of "fragmentation" where
/// 4.2 GB was resident, with the line beside it at −6.4 GB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ArenaResidency {
    /// `[heap]` (the main arena) plus the writable anonymous mappings that
    /// start on a heap boundary (every other arena).
    pub mappings: usize,
    /// Their writable size. The cross-check of the recognition: it covers
    /// [`AllocatorUsage::arena_bytes`], or the alignment rule missed heaps.
    pub mapped_kib: u64,
    /// Of that, resident or in swap. `mapped − held` is what trims returned
    /// plus what was never touched.
    pub held_kib: u64,
}

/// `None` off Linux.
pub fn arena_residency() -> Option<ArenaResidency> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/smaps")
            .ok()
            .map(|text| parse_arena_residency(&text))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(crate) fn parse_arena_residency(text: &str) -> ArenaResidency {
    let mut residency = ArenaResidency {
        mappings: 0,
        mapped_kib: 0,
        held_kib: 0,
    };
    // Whether the mapping whose field lines are being read is an arena's.
    let mut in_arena = false;
    for line in text.lines() {
        if let Some(header) = parse_mapping_header(line) {
            in_arena = header.name == "[heap]"
                || (header.name == "[anonymous]"
                    && header.perms.starts_with("rw")
                    && header.start % GLIBC_HEAP_ALIGN == 0);
            if in_arena {
                residency.mappings += 1;
                residency.mapped_kib += (header.end - header.start) / 1024;
            }
            continue;
        }
        if !in_arena {
            continue;
        }
        let mut tokens = line.split_whitespace();
        if let Some("Rss:" | "Swap:") = tokens.next() {
            // The kernel's own format: `Rss:   1234 kB`.
            residency.held_kib += tokens.next().unwrap().parse::<u64>().unwrap();
        }
    }
    residency
}

/// What the allocator says about its heap: how much is handed out and how
/// much it holds free.
///
/// The free part is the number nothing else can give. The heap's live objects
/// are what the rest of a memory report tries to attribute; the free part is
/// memory nobody owns any more and the allocator has not returned — glibc
/// fragmentation, measured instead of inferred from RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AllocatorUsage {
    pub allocator: &'static str,
    /// Taken from the system for the arenas (`brk` and arena `mmap`s).
    pub arena_bytes: u64,
    /// Chunks large enough to get an `mmap` of their own; always in use.
    pub mmapped_bytes: u64,
    /// Allocated and not yet freed, `mmapped_bytes` included. Chunk headers
    /// are part of it, so it exceeds the sum of the requested sizes.
    pub in_use_bytes: u64,
    /// Freed into the arenas and kept there.
    pub free_bytes: u64,
}

/// `None` where the linked allocator cannot report this.
///
/// With the `mimalloc` feature the heap is mimalloc's and `mallinfo2` would
/// describe glibc's nearly empty one — a plausible-looking wrong answer, so the
/// mimalloc build reports nothing instead; mimalloc's in-use total needs its
/// statistics build, which is not the one linked.
pub fn allocator_usage() -> Option<AllocatorUsage> {
    #[cfg(all(not(feature = "mimalloc"), target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `mallinfo2` takes no arguments and only reads the allocator's
        // own bookkeeping under its arena locks.
        let info = unsafe { libc::mallinfo2() };
        let arena = info.arena as u64;
        let mmapped = info.hblkhd as u64;
        Some(AllocatorUsage {
            allocator: "glibc",
            arena_bytes: arena,
            mmapped_bytes: mmapped,
            in_use_bytes: info.uordblks as u64 + mmapped,
            free_bytes: info.fordblks as u64,
        })
    }
    #[cfg(not(all(not(feature = "mimalloc"), target_os = "linux", target_env = "gnu")))]
    {
        None
    }
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
        // `true` = force: free the retired pages and purge the arenas now
        // instead of after `purge_delay`. It does NOT reach other threads'
        // heaps: mimalloc v3 collects only the calling thread's (`theap.c`,
        // `mi_collect`: "cannot really collect process wide"). On a workspace
        // of ~49 000 files (`allocator_floor_after_load_and_work`, 2026-10-22) this call
        // returned 0 bytes after the load and after the work, in every run.
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

/// Release the shared GPU embedding sessions unused for `max_idle`; the
/// daemon's watchdog calls it every tick. Returns how many went. The binary
/// reaches the engine only through this crate.
pub fn release_idle_gpu_sessions(max_idle: std::time::Duration) -> usize {
    rmc_engine::embeddings::release_idle_gpu_sessions(max_idle)
}

/// The VRAM budget the next GPU session would be checked against, for the
/// daemon's startup line.
pub fn gpu_vram_budget() -> Result<rmc_engine::embeddings::VramBudget, String> {
    rmc_engine::embeddings::gpu_ledger_report().broker.budget
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
    fn smaps_rollup_parser_reads_the_three_fields() {
        let text = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\n\
                    Rss:            10300000 kB\n\
                    Pss_Anon:        9000000 kB\n\
                    Anonymous:       9100000 kB\n\
                    Swap:            1200000 kB\n";

        let mappings = parse_smaps_rollup(text).expect("all fields present");
        assert_eq!(mappings.rss_kib, 10_300_000);
        assert_eq!(mappings.anonymous_kib, 9_100_000);
        assert_eq!(mappings.swap_kib, 1_200_000);
        assert_eq!(mappings.file_kib(), 1_200_000);
        assert_eq!(mappings.held_kib(), 11_500_000);
        // `Pss_Anon:` must not be read as `Anonymous:` or vice versa, and a
        // missing field is absence, not zero.
        assert_eq!(
            parse_smaps_rollup("Rss: 1 kB\nPss_Anon: 1 kB\nSwap: 0 kB\n"),
            None
        );
    }

    /// Mappings group by name, names with spaces survive, unnamed ones fold
    /// into `[anonymous]`, and only `Rss`/`Swap` are summed.
    #[test]
    fn smaps_parser_groups_by_name() {
        let text = "\
7f00-7f10 r--p 00000000 08:01 42 /models/qwen 3.onnx
Size:               4096 kB
Rss:                3000 kB
Swap:                  0 kB
7f10-7f20 rw-p 00000000 00:00 0
Rss:                 500 kB
Swap:                700 kB
7f20-7f30 rw-p 00000000 00:00 0
Rss:                 100 kB
Swap:                 50 kB
7f30-7f40 rw-p 00000000 00:00 0 [heap]
Rss:                  10 kB
Swap:                  0 kB
VmFlags: rd wr mr mw me ac
";
        let usages = parse_smaps(text, 10);

        assert_eq!(
            usages,
            vec![
                MappingUsage {
                    name: "/models/qwen 3.onnx".into(),
                    mappings: 1,
                    rss_kib: 3000,
                    swap_kib: 0
                },
                MappingUsage {
                    name: "[anonymous]".into(),
                    mappings: 2,
                    rss_kib: 600,
                    swap_kib: 750
                },
                MappingUsage {
                    name: "[heap]".into(),
                    mappings: 1,
                    rss_kib: 10,
                    swap_kib: 0
                },
            ]
        );
        assert_eq!(parse_smaps(text, 1).len(), 1);
    }

    /// Arena heaps are `[heap]` and the writable anonymous mappings on a
    /// 64 MiB boundary; a thread stack, a file, an unaligned anonymous mapping
    /// and a heap's `PROT_NONE` reserve are not.
    #[test]
    fn arena_residency_counts_only_the_allocators_mappings() {
        let text = "\
7f0004000000-7f0008000000 rw-p 00000000 00:00 0
Size:              65536 kB
Rss:               40000 kB
Swap:               1000 kB
7f0008000000-7f000a000000 rw-p 00000000 00:00 0
Rss:               20000 kB
Swap:                  0 kB
7f000a000000-7f000c000000 ---p 00000000 00:00 0
Rss:                   0 kB
Swap:                  0 kB
7f000c001000-7f000c801000 rw-p 00000000 00:00 0
Rss:                 100 kB
Swap:                  0 kB
7f0010000000-7f0010100000 rw-p 00000000 08:01 42 /data/index.lmdb
Rss:                 900 kB
Swap:                  0 kB
55d000000000-55d000400000 rw-p 00000000 00:00 0                          [heap]
Rss:                3000 kB
Swap:                 10 kB
VmFlags: rd wr mr mw me ac
";
        assert_eq!(
            parse_arena_residency(text),
            ArenaResidency {
                mappings: 3,
                mapped_kib: 65536 + 32768 + 4096,
                held_kib: 41000 + 20000 + 3010,
            }
        );
    }

    /// On the build that ships (glibc, no mimalloc) the allocator reports, and
    /// what it reports is consistent with itself.
    #[cfg(all(not(feature = "mimalloc"), target_os = "linux", target_env = "gnu"))]
    #[test]
    fn glibc_reports_its_heap() {
        let held: Vec<Vec<u8>> = (0..64).map(|_| vec![1u8; 4096]).collect();
        let usage = allocator_usage().expect("glibc build reports mallinfo2");

        assert!(usage.in_use_bytes >= 64 * 4096, "{usage:?}");
        assert!(
            usage.in_use_bytes <= usage.arena_bytes + usage.mmapped_bytes,
            "{usage:?}"
        );
        drop(held);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_has_a_portrait() {
        let mappings = smaps_rollup().expect("smaps_rollup readable on linux");
        assert!(
            mappings.anonymous_kib > 0 && mappings.file_kib() > 0,
            "{mappings:?}"
        );
        let largest = largest_mappings(5);
        assert!(!largest.is_empty() && largest.len() <= 5);
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
