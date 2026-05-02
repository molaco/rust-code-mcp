//! Persisted workspace hypergraph.
//!
//! Layered as: loader → extraction model → extraction passes → persistence
//! → read path → MCP tools. Each layer is built and tested in isolation.

pub mod bindings;
pub mod extract;
pub mod ids;
pub mod impls;
pub mod loader;
pub mod model;
pub mod queries;
pub mod snapshot;
pub mod storage;
pub mod usages;

pub use extract::extract;
pub use ids::{BindingId, NodeId, UsageId, workspace_hash};
pub use loader::{LoadedWorkspace, load};
pub use model::{
    Binding, BindingKind, BindingVisibility, ExtractionModel, ItemKind, Namespace, Node, NodeKind,
    Usage, UsageCategory,
};
pub use queries::{
    CommonFnName, CrateDeadPub, CrateEdge, DeadPubFinding, EdgeSymbol, ModuleShadow,
    ModuleTreeNode, NodeKindCounts, OverlapsReport, TypeCollision, TypeLocation, UsageSummaryRow,
    VisibilityCounts, WithinCrateDuplicate, WorkspaceStats,
};
pub use snapshot::{
    BuildOptions, BuildResult, OpenedSnapshot, build_and_persist, open_current, open_specific,
};
pub use storage::{
    GraphDatabases, GraphEnvOptions, GraphManifest, GraphPaths, SCHEMA_VERSION,
    compute_fingerprint,
};
