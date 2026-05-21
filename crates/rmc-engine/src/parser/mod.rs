//! Rust code parsing with rust-analyzer (ra_ap_syntax)
//!
//! Extracts symbols (functions, structs, traits, etc.) from Rust source files
//! and builds a call graph for understanding code relationships.

pub mod call_graph;
pub mod imports;
pub mod type_references;

mod rust_parser;
mod types;

pub use rust_parser::RustParser;
pub use types::{ParseResult, Range, Symbol, SymbolKind, Visibility};

// Re-export internal helper so sibling submodules (e.g. `type_references`) can
// continue to reach it via `super::line_of_offset` after the split.
pub(in crate::parser) use rust_parser::line_of_offset;
