//! Persisted workspace hypergraph.
//!
//! Layered as: loader → extraction model → extraction passes → persistence
//! → read path → MCP tools. Each layer is built and tested in isolation.

#![warn(unreachable_pub, dead_code)]

mod ast_resolve;
mod attributes;
mod bindings;
pub mod channel_audit;
pub mod derive_audit;
pub mod docs_audit;
mod extract;
pub mod fn_body_audit;
mod hir_trim;
mod ids;
mod impls;
pub mod loader;
mod model;
mod queries;
pub mod recursion_check;
mod signatures;
mod snapshot;
mod statics;
mod storage;
pub mod unsafe_audit;
mod usages;

pub use extract::extract;
pub use ids::{BindingId, NodeId, UsageId, workspace_hash};
pub use loader::{LoadedWorkspace, load};
pub use model::{
    Binding, BindingKind, BindingVisibility, EmbeddingRecord, ExtractionModel, FunctionSignature,
    GenericBound, ItemKind, Namespace, Node, NodeKind, Param, SelfKind, StaticMetadata, Usage,
    UsageCategory,
};
pub use queries::{
    CallGraphNode, CommonFnName, CrateDeadPub, CrateEdge, CrateMetric, DeadPubFinding, EdgeSymbol,
    EnrichedCallSite, ForbiddenDependencyRule, ForbiddenDependencyViolation, FunctionFilter,
    FunctionWithSignature, ItemWithAttribute, ModuleShadow, ModuleTreeNode, MutStaticFinding,
    NodeKindCounts, OverlapsReport, PubTypeAliasMasqueradingAsReexport, ReExportChain,
    ReExportLink, RecursiveCallersCount, SelfKindFilter, TypeCollision, TypeLocation,
    UsageSummaryRow, VisibilityCounts, WithinCrateDuplicate, WorkspaceStats,
};
pub use snapshot::{
    BuildOptions, BuildResult, GraphRoTxn, OpenedSnapshot, build_and_persist, open_current,
    open_specific,
};
pub use unsafe_audit::UnsafeFinding;
pub use storage::{
    GraphDatabases, GraphEnvOptions, GraphManifest, GraphPaths, SCHEMA_VERSION,
    compute_fingerprint,
};

#[cfg(test)]
pub(crate) fn test_workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("graph crate should live under crates/rust-code-mcp-graph")
        .to_path_buf()
}
