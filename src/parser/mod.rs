//! Legacy parser facade.

pub use rust_code_mcp_syntax::{
    CallGraph, Import, ParseResult, Range, RustParser, Symbol, SymbolKind, TypeReference,
    Visibility, extract_imports, extract_imports_from_ast, get_external_dependencies,
};

pub mod call_graph {
    pub use rust_code_mcp_syntax::call_graph::*;
}

pub mod imports {
    pub use rust_code_mcp_syntax::imports::*;
}

pub mod type_references {
    pub use rust_code_mcp_syntax::type_references::*;
}
