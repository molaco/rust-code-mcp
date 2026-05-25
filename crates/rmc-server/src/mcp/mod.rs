//! MCP (Model Context Protocol) integration
//!
//! This module provides background synchronization and server management
//! for the rust-code-mcp service.

pub mod project_paths;
pub mod search_cache;
pub mod sync;
pub mod workspace_locks;

pub use search_cache::*;
pub use sync::*;
pub use workspace_locks::*;
