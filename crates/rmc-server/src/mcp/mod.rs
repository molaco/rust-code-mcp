//! MCP (Model Context Protocol) integration
//!
//! This module provides background synchronization and server management
//! for the rust-code-mcp service.

pub mod defaults;
pub mod project_paths;
pub mod runtime;
pub mod search_cache;
pub mod sync;
pub mod workspace_locks;

pub use defaults::*;
pub use runtime::*;
pub use search_cache::*;
pub use sync::*;
pub use workspace_locks::*;
