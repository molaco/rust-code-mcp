//! `rmc-server` — MCP server cluster.
//!
//! Hosts the three modules that together implement the MCP server surface:
//! `tools` (the rmcp adapter endpoints), `mcp` (the `SyncManager` plus
//! project-path resolver), and `semantic` (the rust-analyzer IDE
//! wrapper used by analysis tools), plus `deep_stack`, the thread every one of
//! them has to borrow before it runs rust-analyzer.

// lancedb 0.29's async stack can push Send inference for the sync-manager
// background task past the default recursion limit when the runtime owner
// spawns it.
#![recursion_limit = "512"]
#![warn(unreachable_pub, dead_code)]

// The test binary of this crate allocates as the daemon does: through the heap
// census (`mcp::heap_census`), which the portrait's tests read, over the
// allocator the build selects. Without the mimalloc arm the feature would switch
// only the release call (`mi_collect` instead of `malloc_trim`) while every
// allocation still went to glibc, and a memory probe run "under mimalloc" would
// measure glibc with a no-op release.
#[cfg(all(test, feature = "mimalloc"))]
#[global_allocator]
static TEST_GLOBAL_ALLOCATOR: mcp::heap_census::CensusAllocator<mimalloc::MiMalloc> =
    mcp::heap_census::CensusAllocator {
        inner: mimalloc::MiMalloc,
        census: &mcp::heap_census::GLOBAL_CENSUS,
    };
#[cfg(all(test, not(feature = "mimalloc")))]
#[global_allocator]
static TEST_GLOBAL_ALLOCATOR: mcp::heap_census::CensusAllocator<std::alloc::System> =
    mcp::heap_census::CensusAllocator {
        inner: std::alloc::System,
        census: &mcp::heap_census::GLOBAL_CENSUS,
    };

pub(crate) mod deep_stack;
pub mod panic_boundary;
pub mod tools;
pub mod mcp;
pub mod semantic;
