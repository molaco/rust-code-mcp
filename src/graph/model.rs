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
    /// Layer 4: a fn declared inside an inherent `impl T { ... }` block, OR a
    /// fn declared in a `trait T { fn m(); }`. Both share this variant — the
    /// distinction is encoded by `parent_id` pointing at a struct/enum/union
    /// Item vs a trait Item.
    Method,
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
    /// The containing scope of this node. For Workspace = None; for Crate = the
    /// Workspace; for Module = parent Module or Crate; for top-level Items = the
    /// owning Module. As of Layer 4, methods / associated consts / associated
    /// types declared inside an inherent `impl Foo { ... }` block or a
    /// `trait Foo { ... }` declaration carry `parent_id` pointing at the host
    /// type's or trait's Item NodeId — i.e. an Item-typed parent is now valid
    /// for the Method/AssocConst/AssocType item kinds.
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
    /// True iff the source `use` statement carries an explicit `pub` (or
    /// `pub(...)`) visibility modifier in syntax. Distinct from
    /// `visibility`, which is the *resolved* effective visibility (HIR
    /// inherits / normalizes parent-visibility for non-pub `use`s). Used by
    /// `declared_reexports_of` to find every "pub use" the module declares
    /// regardless of whether it's reachable from any specific consumer.
    #[serde(default)]
    pub is_explicit_pub_use: bool,
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
    /// Function-scope attribution. `None` means the reference site is not
    /// inside any function body — e.g. a const initializer, a type alias
    /// bound, an enum variant discriminant. Closures attribute to their
    /// enclosing fn.
    #[serde(default)]
    pub consumer_function: Option<NodeId>,
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
