//! Persisted workspace hypergraph.
//!
//! Layered as: loader → extraction model → extraction passes → persistence
//! → read path → MCP tools. Each layer is built and tested in isolation.

mod ast_resolve;
pub(in crate::graph) mod audit_util;
mod attributes;
mod bindings;
mod channel_audit;
pub mod codemap;
mod derive_audit;
mod docs_audit;
#[cfg(feature = "semantic-embeddings")]
mod embedding_cache;
mod extract;
mod fn_body_audit;
mod hir_trim;
pub mod ids;
mod impls;
mod labels;
mod loader;
mod math;
pub mod model;
mod query;
mod recursion_check;
mod signatures;
mod skeleton;
pub mod snapshot;
mod statics;
mod storage;
#[cfg(test)]
pub(crate) mod test_support;
mod unsafe_audit;
mod usages;

pub use ids::{BindingId, NodeId};
pub use extract::extract as extract_workspace_model;
pub use labels::{item_kind_display_label, item_kind_short_label};
pub use loader::{LoadedWorkspace, load};
pub use model::{
    Binding, BindingVisibility, ExtractionModel, FunctionSignature, ItemKind, Namespace, Node,
    NodeKind, Usage,
};
#[cfg(feature = "semantic-embeddings")]
pub(crate) use model::EmbeddingRecord;
pub use query::audits::{
    ChannelCapacityAuditOptions, DeriveAuditOptions, FnBodyAuditOptions, GraphAuditError,
    MissingDocsAuditOptions, RecursionCheckOptions, run_channel_capacity_audit,
    run_derive_audit, run_fn_body_audit, run_missing_docs_audit, run_mut_static_audit,
    run_recursion_check, run_unsafe_audit,
};
pub use query::model::{
    CallGraphNode, ChannelCapacityFinding, CrateDeadPub, CrateEdge, CrateMetric, CrateTypeItem,
    DeadPubFinding, DeriveAuditFinding, EnrichedBinding, EnrichedCallSite, EnrichedCrateDeadPub,
    EnrichedDeadPub, EnrichedUsage, FnBodyAuditFinding, FnBodyAuditOutput,
    ForbiddenDependencyRule, ForbiddenDependencyViolation, FunctionFilter, FunctionWithSignature,
    ItemWithAttribute, MissingDocsAuditFinding, ModuleDependency, ModuleDependencySymbol,
    ModuleTreeNode, MutStaticAuditFinding, OverlapScope, OverlapsReport,
    PubTypeAliasMasqueradingAsReexport, ReExportChain, RecursionCheckOutput, RecursionCycle,
    RecursiveCallersCount, SelfKindFilter, SemanticOverlapScope, SemanticOverlapsOutput,
    SimilarityCluster, SimilarityItem, SimilarityPair, UnsafeAuditFinding, UsageSummaryRow,
    WorkspaceStats,
};
#[cfg(feature = "semantic-embeddings")]
pub use query::similarity::{
    GraphSimilarityError, SemanticOverlapOptions, run_semantic_overlaps,
};
pub use skeleton::{
    SkeletonDiagnostic, SkeletonFile, SkeletonOptions, SkeletonOutput,
    render_crate_skeletons,
};
pub use snapshot::{
    BuildOptions, GraphSnapshotCleanupEntry, GraphSnapshotCleanupOptions,
    GraphSnapshotCleanupReport, OpenedSnapshot, build_and_persist,
    clear_all_workspace_snapshots, clear_workspace_snapshots, open_current,
    open_current_for_workspace,
};
pub use storage::{GraphEnvOptions, GraphPaths};
