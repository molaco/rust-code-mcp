//! Legacy server tools facade.

pub mod analysis_tools {
    pub use rust_code_mcp_server::tools::analysis_tools::*;
}

pub mod clear_cache_tool {
    pub use rust_code_mcp_server::tools::clear_cache_tool::*;
}

pub mod graph_tools {
    pub use rust_code_mcp_server::tools::graph_tools::*;
}

pub mod health_tool {
    pub use rust_code_mcp_server::tools::health_tool::*;
}

pub mod index_tool {
    pub use rust_code_mcp_server::tools::index_tool::*;
}

pub mod indexing_tools {
    pub use rust_code_mcp_server::tools::indexing_tools::*;
}

pub mod project_paths {
    pub use rust_code_mcp_server::tools::project_paths::*;
}

pub mod query_tools {
    pub use rust_code_mcp_server::tools::query_tools::*;
}

pub mod search_tool {
    pub use rust_code_mcp_server::tools::search_tool::*;
}

pub mod search_tool_router {
    pub use rust_code_mcp_server::tools::search_tool_router::*;
}
