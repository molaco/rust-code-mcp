//! In-memory model for the workspace hypergraph.
//!
//! `ExtractionModel` is the output of the extraction passes. It is the
//! single source of truth that the persistence layer (Layer 4) serializes
//! into heed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ids::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    Workspace,
    Crate,
    Module,
    Item,
    ExternalSymbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind {
    Function,
    Struct,
    Enum,
    Union,
    Trait,
    TypeAlias,
    Const,
    Static,
    AssocFunction,
    AssocConst,
    AssocType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Namespace {
    Type,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BindingKind {
    Declared,
    NamedImport,
    GlobImport,
    ExternCrateImport,
}

/// Visibility carried on a `Binding`, in a form that lets export queries
/// answer "is this visible from consumer module C?" without re-walking HIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BindingVisibility {
    Public,
    Crate(NodeId),
    /// Restricted to the module subtree rooted at this node.
    RestrictedTo(NodeId),
    /// Restriction does not resolve to any local node — treated as not exported.
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub display_name: String,
    pub qualified_name: String,
    pub crate_id: Option<NodeId>,
    pub parent_id: Option<NodeId>,
    pub item_kind: Option<ItemKind>,
    pub file: Option<String>, // workspace-relative
    pub span: Option<(u32, u32)>,
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub from_module: NodeId,
    pub namespace: Namespace,
    pub visible_name: String,
    pub target: NodeId,
    pub kind: BindingKind,
    pub visibility: BindingVisibility,
}

/// Reference category for a `Usage`. Mirrors `ra_ap_ide_db::search::ReferenceCategory`,
/// reduced to the cases we care about; `Import` references are filtered out at
/// extraction time (they're already modeled as `Binding`s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UsageCategory {
    Read,
    Write,
    Test,
    Other,
}

/// A non-import reference to an Item. One record per concrete reference site,
/// so `who_uses(target)` can return file:range tuples and so `dead_pub` can
/// distinguish a never-referenced item from one only referenced inside its
/// own crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub target: NodeId,
    pub consumer_module: NodeId,
    pub file: String, // workspace-relative
    pub start: u32,   // byte offset
    pub end: u32,
    pub category: UsageCategory,
}

#[derive(Debug, Clone)]
pub struct ExtractionModel {
    pub workspace_root: PathBuf,
    pub workspace_hash: String,
    pub workspace_id: NodeId,
    pub nodes: BTreeMap<NodeId, Node>,
    pub bindings: Vec<Binding>,
    pub usages: Vec<Usage>,
    /// (parent, child) — workspace→crate, crate→root_module, module→child_module, module→item.
    pub contains: Vec<(NodeId, NodeId)>,
}

impl ExtractionModel {
    pub fn insert_node(&mut self, node: Node) {
        self.nodes.entry(node.id).or_insert(node);
    }

    pub fn insert_contains(&mut self, parent: NodeId, child: NodeId) {
        self.contains.push((parent, child));
    }
}
