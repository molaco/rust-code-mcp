//! The Rust heap, counted where it is allocated.
//!
//! # Why the allocator has to count
//!
//! The process portrait (`semantic::ProcessPortrait`) cuts "heap in use" from
//! `mallinfo2`, which counts *chunks*: every allocation plus glibc's header
//! (8 bytes) and its rounding to 16 with a 32-byte minimum. Every owner on the
//! other side of the subtraction — salsa's `heap_size`, the interners — counts
//! *requested* bytes, the size the program asked for. The difference landed
//! in "unattributed": a heap profile of a loaded analysis (heaptrack, live
//! allocations at exit) found 1032 MiB requested against 1147 MiB of chunks,
//! so 115 of the 215 MiB without an owner were the allocator's own
//! bookkeeping. Nothing outside the allocator can tell those apart, because
//! the overhead depends on the size of each allocation.
//!
//! So the global allocator is wrapped ([`CensusAllocator`]) and keeps two
//! running sums: requested bytes, and — on glibc — the chunk each allocation
//! really occupies (`malloc_usable_size` plus the header). Their difference is
//! the overhead; chunks in use that the census never saw are allocations made
//! by C and C++ code (ONNX Runtime, MIGraphX), which bypass Rust's allocator.
//!
//! # Cost
//!
//! Two relaxed atomic adds per allocation and per free, on counters sharded by
//! thread so that rust-analyzer's parallel cache priming does not serialise on
//! one cache line. The glibc chunk size is one read of the chunk header, done
//! by `malloc_usable_size`.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// Counter shards; a power of two, so a hash's top bits pick one.
const SHARDS: usize = 64;
const SHARD_BITS: u32 = SHARDS.trailing_zeros();

/// One shard of the running sums, alone on its cache line pair (adjacent-line
/// prefetch pulls 128 bytes on x86).
#[repr(align(128))]
pub struct Shard {
    requested: AtomicIsize,
    chunks: AtomicIsize,
}

/// The running sums of one allocator. Shards are added up on read; a free on
/// another thread than its allocation subtracts from another shard, which the
/// total does not notice.
pub struct Census {
    shards: [Shard; SHARDS],
    /// Set by the first allocation through an allocator using this census. A
    /// census nobody allocates through reads as absent, not as an empty heap.
    used: AtomicBool,
}

impl Census {
    pub const fn new() -> Self {
        Self {
            shards: [const {
                Shard {
                    requested: AtomicIsize::new(0),
                    chunks: AtomicIsize::new(0),
                }
            }; SHARDS],
            used: AtomicBool::new(false),
        }
    }

    /// The shard of the calling thread: the address of a thread-local, hashed.
    ///
    /// The thread-local is `const`-initialised and has no destructor, so
    /// reading it never allocates (which would recurse into the allocator) and
    /// stays valid while a thread is being torn down. Thread-local blocks of
    /// different threads sit at the same offset in equally sized stack
    /// mappings, so their low bits agree — a multiplicative hash takes its top
    /// bits from all of them.
    fn shard(&self) -> &Shard {
        thread_local! {
            static ANCHOR: u8 = const { 0 };
        }
        let address = ANCHOR.with(|anchor| std::ptr::from_ref(anchor) as usize);
        let index = (address as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - SHARD_BITS);
        &self.shards[index as usize]
    }

    fn add(&self, requested: isize, chunks: isize) {
        // A load of a line nobody writes after the first allocation is free;
        // storing on every call would put all threads back on one line.
        if !self.used.load(Ordering::Relaxed) {
            self.used.store(true, Ordering::Relaxed);
        }
        let shard = self.shard();
        shard.requested.fetch_add(requested, Ordering::Relaxed);
        shard.chunks.fetch_add(chunks, Ordering::Relaxed);
    }

    /// `None` until something allocated through this census.
    ///
    /// The shards are read one after another while other threads allocate, so
    /// the sums are a moment's estimate, not a snapshot — exact once the
    /// process is quiet.
    pub fn read(&self) -> Option<HeapCensus> {
        if !self.used.load(Ordering::Relaxed) {
            return None;
        }
        let (mut requested, mut chunks) = (0i64, 0i64);
        for shard in &self.shards {
            requested += shard.requested.load(Ordering::Relaxed) as i64;
            chunks += shard.chunks.load(Ordering::Relaxed) as i64;
        }
        Some(HeapCensus {
            requested_bytes: requested,
            chunk_bytes: CHUNKS_MEASURED.then_some(chunks),
        })
    }
}

/// What the Rust side of the heap holds now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct HeapCensus {
    /// Bytes requested by live allocations — the unit `heap_size` counts in.
    pub requested_bytes: i64,
    /// What those allocations occupy in glibc's arenas, headers and rounding
    /// included — the unit `mallinfo2` counts in. `None` under mimalloc, whose
    /// statistics the portrait does not read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_bytes: Option<i64>,
}

/// Whether [`chunk_bytes_of`] measures glibc chunks in this build.
const CHUNKS_MEASURED: bool = cfg!(all(
    not(feature = "mimalloc"),
    target_os = "linux",
    target_env = "gnu"
));

/// The chunk a live glibc allocation occupies: its usable size plus the
/// 8-byte size field in front of it.
///
/// A chunk with an `mmap` of its own carries a 16-byte header instead, so it
/// is under-counted by 8 bytes; those are allocations of 128 KiB and more, few
/// enough that the error stays in the kilobytes.
///
/// # Safety
///
/// `ptr` must be a live allocation made by glibc's `malloc` family — which is
/// what `System` hands out on linux-gnu, for every alignment.
#[cfg(all(not(feature = "mimalloc"), target_os = "linux", target_env = "gnu"))]
unsafe fn chunk_bytes_of(ptr: *mut u8) -> isize {
    // SAFETY: the caller's contract.
    let usable = unsafe { libc::malloc_usable_size(ptr.cast()) };
    (usable + std::mem::size_of::<usize>()) as isize
}

#[cfg(not(all(not(feature = "mimalloc"), target_os = "linux", target_env = "gnu")))]
unsafe fn chunk_bytes_of(_ptr: *mut u8) -> isize {
    0
}

/// Forwards every call to `inner` and keeps `census` current.
pub struct CensusAllocator<A: 'static> {
    pub inner: A,
    pub census: &'static Census,
}

// SAFETY: every call goes to `inner` unchanged; the census only observes the
// sizes, and reads a chunk header only while the allocation is live (after
// it was made, before it is freed).
unsafe impl<A: GlobalAlloc> GlobalAlloc for CensusAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc(layout) };
        if !ptr.is_null() {
            self.census
                .add(layout.size() as isize, unsafe { chunk_bytes_of(ptr) });
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc_zeroed(layout) };
        if !ptr.is_null() {
            self.census
                .add(layout.size() as isize, unsafe { chunk_bytes_of(ptr) });
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let chunk = unsafe { chunk_bytes_of(ptr) };
        unsafe { self.inner.dealloc(ptr, layout) };
        self.census.add(-(layout.size() as isize), -chunk);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Read before the call: on success `ptr` is no longer an allocation.
        let old_chunk = unsafe { chunk_bytes_of(ptr) };
        let new = unsafe { self.inner.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            self.census.add(
                new_size as isize - layout.size() as isize,
                unsafe { chunk_bytes_of(new) } - old_chunk,
            );
        }
        new
    }
}

/// The census of the process's global allocator, which the binary (and this
/// crate's test binary) installs as
/// `CensusAllocator { inner: <System or MiMalloc>, census: &GLOBAL_CENSUS }`.
pub static GLOBAL_CENSUS: Census = Census::new();

/// The Rust heap now; `None` in a binary whose global allocator does not
/// count into [`GLOBAL_CENSUS`].
pub fn heap_census() -> Option<HeapCensus> {
    GLOBAL_CENSUS.read()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::System;

    /// A census of its own, driven by direct calls: the global one moves with
    /// every test running beside this one, this one only with what it does.
    static PRIVATE: Census = Census::new();
    static COUNTING: CensusAllocator<System> = CensusAllocator {
        inner: System,
        census: &PRIVATE,
    };

    /// Requested bytes follow allocations and frees exactly, and on glibc the
    /// chunks carry the allocator's header and rounding on top of them.
    ///
    /// 25 bytes is the size the overhead is largest for: 25 + 8 rounds up to a
    /// 48-byte chunk. A census that counts chunks as requested bytes (or the
    /// other way round) fails the overhead check; one that skips a free fails
    /// the return to zero.
    #[test]
    fn the_census_counts_requests_and_chunks_and_returns_to_zero() {
        assert_eq!(PRIVATE.read(), None, "nothing allocated through it yet");
        let layout = Layout::from_size_align(25, 1).unwrap();
        let n = 1000;
        let ptrs: Vec<*mut u8> = (0..n).map(|_| unsafe { COUNTING.alloc(layout) }).collect();
        let held = PRIVATE.read().expect("allocated through it");
        assert_eq!(held.requested_bytes, 25 * n as i64);
        if CHUNKS_MEASURED {
            assert_eq!(
                held.chunk_bytes,
                Some(48 * n as i64),
                "a 25-byte request occupies a 48-byte glibc chunk"
            );
        }

        // Growing in place or moving, the census follows the new size.
        let grown: Vec<*mut u8> = ptrs
            .into_iter()
            .map(|ptr| unsafe { COUNTING.realloc(ptr, layout, 200) })
            .collect();
        let held = PRIVATE.read().unwrap();
        assert_eq!(held.requested_bytes, 200 * n as i64);
        if CHUNKS_MEASURED {
            // 200 + 8 is 208, a multiple of 16 — but a realloc that grows in
            // place absorbs the free neighbour and splits it off only if the
            // rest is a whole chunk (32 bytes or more), so a chunk can keep up
            // to 16 bytes beyond that. The census reads the chunk, not the
            // formula; the bounds say it does.
            let chunks = held.chunk_bytes.unwrap();
            assert!(
                (208 * n as i64..=224 * n as i64).contains(&chunks),
                "{n} chunks of 200 requested bytes occupy {chunks} bytes"
            );
        }

        let grown_layout = Layout::from_size_align(200, 1).unwrap();
        for ptr in grown {
            unsafe { COUNTING.dealloc(ptr, grown_layout) };
        }
        let held = PRIVATE.read().unwrap();
        assert_eq!(
            held.requested_bytes, 0,
            "every free subtracts what its allocation added"
        );
        assert_eq!(held.chunk_bytes, CHUNKS_MEASURED.then_some(0));
    }

    /// This crate's test binary counts through the global census — the
    /// portrait's tests depend on it, and so does the claim that the binary's
    /// installation (the same type, the same static) works.
    #[test]
    fn the_test_binary_allocates_through_the_global_census() {
        let census = heap_census().expect("lib.rs installs the census for tests");
        assert!(census.requested_bytes > 0);
        if CHUNKS_MEASURED {
            assert!(census.chunk_bytes.unwrap() > census.requested_bytes);
        }
    }
}
