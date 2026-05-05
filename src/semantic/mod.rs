//! Semantic code analysis using rust-analyzer

use std::sync::{LazyLock, Mutex};

pub use rust_code_mcp_ra_analysis::{Location, SemanticService};

/// Global semantic service instance (Mutex because AnalysisHost is not Sync)
pub static SEMANTIC: LazyLock<Mutex<SemanticService>> = LazyLock::new(|| {
    Mutex::new(SemanticService::new())
});

pub mod loader {
    pub use rust_code_mcp_ra_analysis::loader::*;
}

pub mod position {
    pub use rust_code_mcp_ra_analysis::position::*;
}
