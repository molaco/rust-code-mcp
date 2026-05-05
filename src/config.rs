//! Legacy configuration facade.

pub mod errors;
pub mod indexer;

pub use errors::{Error, ErrorContextExt, Result};
pub use indexer::{IndexerConfig, IndexerCoreConfig, TantivyConfig};
pub use rust_code_mcp_server::Config;
