//! Legacy graph facade.

pub use rust_code_mcp_graph::*;

pub mod ast_resolve {
    pub use rust_code_mcp_graph::ast_resolve::*;
}

pub mod attributes {
    pub use rust_code_mcp_graph::attributes::*;
}

pub mod bindings {
    pub use rust_code_mcp_graph::bindings::*;
}

pub mod channel_audit {
    pub use rust_code_mcp_graph::channel_audit::*;
}

pub mod derive_audit {
    pub use rust_code_mcp_graph::derive_audit::*;
}

pub mod docs_audit {
    pub use rust_code_mcp_graph::docs_audit::*;
}

pub mod extract {
    pub use rust_code_mcp_graph::extract::*;
}

pub mod fn_body_audit {
    pub use rust_code_mcp_graph::fn_body_audit::*;
}

pub mod hir_trim {
    pub use rust_code_mcp_graph::hir_trim::*;
}

pub mod ids {
    pub use rust_code_mcp_graph::ids::*;
}

pub mod impls {
    pub use rust_code_mcp_graph::impls::*;
}

pub mod loader {
    pub use rust_code_mcp_graph::loader::*;
}

pub mod model {
    pub use rust_code_mcp_graph::model::*;
}

pub mod queries {
    pub use rust_code_mcp_graph::queries::*;
}

pub mod recursion_check {
    pub use rust_code_mcp_graph::recursion_check::*;
}

pub mod signatures {
    pub use rust_code_mcp_graph::signatures::*;
}

pub mod snapshot {
    pub use rust_code_mcp_graph::snapshot::*;
}

pub mod statics {
    pub use rust_code_mcp_graph::statics::*;
}

pub mod storage {
    pub use rust_code_mcp_graph::storage::*;
}

pub mod unsafe_audit {
    pub use rust_code_mcp_graph::unsafe_audit::*;
}

pub mod usages {
    pub use rust_code_mcp_graph::usages::*;
}
