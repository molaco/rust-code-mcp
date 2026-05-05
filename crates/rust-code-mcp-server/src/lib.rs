//! MCP server wiring for rust-code-mcp.

#![warn(unreachable_pub, dead_code)]

pub mod config;
pub mod mcp;
pub mod monitoring;
pub mod semantic;
pub mod tools;

pub use config::Config;
