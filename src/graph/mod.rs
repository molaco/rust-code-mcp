//! Persisted workspace hypergraph.
//!
//! Layered as: loader → extraction model → extraction passes → persistence
//! → read path → MCP tools. Each layer is built and tested in isolation.

pub mod ast_resolve;
pub(in crate::graph) mod audit_util;
pub mod attributes;
pub mod bindings;
pub mod channel_audit;
pub mod codemap;
pub mod derive_audit;
pub mod docs_audit;
mod embedding_cache;
pub mod extract;
pub mod fn_body_audit;
pub mod hir_trim;
pub mod ids;
pub mod impls;
pub(crate) mod labels;
pub mod loader;
mod math;
pub mod model;
mod query;
pub mod recursion_check;
pub mod signatures;
pub mod snapshot;
pub mod statics;
pub mod storage;
#[cfg(test)]
pub(crate) mod test_support;
pub mod unsafe_audit;
pub mod usages;

pub use extract::extract;
pub(crate) use embedding_cache::ensure_embeddings_for;
pub use ids::{BindingId, NodeId, UsageId, workspace_hash};
pub use loader::{LoadedWorkspace, load};
pub(crate) use math::cosine;
pub use model::{
    Binding, BindingKind, BindingVisibility, EmbeddingRecord, ExtractionModel, FunctionSignature,
    GenericBound, ItemKind, Namespace, Node, NodeKind, Param, SelfKind, StaticMetadata, Usage,
    UsageCategory,
};
pub use query::model::*;
pub use query::audits::classify_metadata;
pub use snapshot::{
    BuildOptions, BuildResult, OpenedSnapshot, build_and_persist, open_current, open_specific,
};
pub use unsafe_audit::UnsafeFinding;
pub use storage::{
    GraphDatabases, GraphEnvOptions, GraphManifest, GraphPaths, SCHEMA_VERSION,
    compute_fingerprint,
};
