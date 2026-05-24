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
pub mod labels;
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

pub use ids::{BindingId, NodeId};
pub use loader::{LoadedWorkspace, load};
pub use model::{
    Binding, BindingVisibility, FunctionSignature, ItemKind, Namespace, Node, NodeKind, Usage,
};
pub(crate) use model::EmbeddingRecord;
pub use query::audits::{
    ChannelCapacityAuditOptions, FnBodyAuditOptions, GraphAuditError, RecursionCheckOptions,
    run_channel_capacity_audit, run_fn_body_audit, run_mut_static_audit, run_recursion_check,
    run_unsafe_audit,
};
pub use query::model::{
    CallGraphNode, ChannelCapacityFinding, CrateDeadPub, CrateEdge, CrateMetric, DeadPubFinding,
    EnrichedBinding, EnrichedCallSite, EnrichedCrateDeadPub, EnrichedDeadPub, EnrichedUsage,
    FnBodyAuditFinding, FnBodyAuditOutput, ForbiddenDependencyRule, ForbiddenDependencyViolation,
    FunctionFilter, FunctionWithSignature, ItemWithAttribute, ModuleDependency,
    ModuleDependencySymbol, ModuleTreeNode, MutStaticAuditFinding, OverlapScope, OverlapsReport,
    PubTypeAliasMasqueradingAsReexport, ReExportChain, RecursionCheckOutput, RecursionCycle,
    RecursiveCallersCount, SelfKindFilter, SemanticOverlapScope, SemanticOverlapsOutput,
    SimilarityCluster, SimilarityItem, SimilarityPair, UnsafeAuditFinding, UsageSummaryRow,
    WorkspaceStats,
};
pub use query::similarity::{
    GraphSimilarityError, SemanticOverlapOptions, run_semantic_overlaps,
};
pub use snapshot::{
    BuildOptions, GraphSnapshotCleanupEntry, GraphSnapshotCleanupOptions,
    GraphSnapshotCleanupReport, OpenedSnapshot, build_and_persist,
    clear_all_workspace_snapshots, clear_workspace_snapshots, open_current,
    open_current_for_workspace,
};
pub use storage::{GraphEnvOptions, GraphPaths};
