//! Compatibility facade — param schemas live in `crate::tools::params`,
//! the MCP tool router lives in `crate::tools::router`.

pub use crate::tools::params::*;
pub use crate::tools::router::SearchToolRouter as SearchTool;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_tool_backward_compat() {
        let _tool = SearchTool::new();
        assert!(true);
    }
}
