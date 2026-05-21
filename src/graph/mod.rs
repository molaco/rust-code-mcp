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

pub(crate) use embedding_cache::ensure_embeddings_for;
pub use ids::{BindingId, NodeId};
pub use loader::{LoadedWorkspace, load};
pub(crate) use math::cosine;
pub use model::{
    Binding, BindingVisibility, FunctionSignature, ItemKind, Namespace, Node, NodeKind, Usage,
};
pub(crate) use model::EmbeddingRecord;
pub use query::model::{
    CallGraphNode, CrateDeadPub, CrateEdge, CrateMetric, DeadPubFinding, EnrichedCallSite,
    ForbiddenDependencyRule, ForbiddenDependencyViolation, FunctionFilter, FunctionWithSignature,
    ItemWithAttribute, ModuleDependency, ModuleDependencySymbol, ModuleTreeNode, OverlapScope,
    OverlapsReport, PubTypeAliasMasqueradingAsReexport, ReExportChain, RecursiveCallersCount,
    SelfKindFilter, UsageSummaryRow, WorkspaceStats,
};
pub use snapshot::{BuildOptions, OpenedSnapshot, build_and_persist, open_current};
pub use storage::{GraphEnvOptions, GraphPaths};
