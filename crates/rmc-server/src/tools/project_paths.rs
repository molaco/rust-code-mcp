//! Compatibility re-export. The canonical home is `crate::mcp::project_paths` (moved 2026-05-21 in Phase A.2).
//! Deleted in Phase C.3 when `tools` lifts to `rmc-server` and consumers resolve via the main `lib.rs` facade.
pub use crate::mcp::project_paths::*;
