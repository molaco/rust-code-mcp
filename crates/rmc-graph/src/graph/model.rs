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
    /// A variant of an enum (`enum E { A, B(u32), C { x: i32 } }` → three
    /// EnumVariant items). `parent_id` points at the enum's Item NodeId, NOT
    /// the enclosing module. Visibility is inherited from the enum (always
    /// `None` here — same shape as `Method`).
    EnumVariant,
}

impl ItemKind {
    /// Returns `true` for variants that can be invoked: `Function`, `Method`, `AssocFunction`.
    pub fn is_callable(self) -> bool {
        matches!(self, Self::Function | Self::Method | Self::AssocFunction)
    }

    /// Returns `true` for variants that name a type: `Struct`, `Enum`, `Union`, `Trait`, `TypeAlias`.
    pub fn is_type(self) -> bool {
        matches!(
            self,
            Self::Struct | Self::Enum | Self::Union | Self::Trait | Self::TypeAlias
        )
    }
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
    /// v8: outer attributes and doc-comment lines attached to this item, in
    /// source order. Each element is the trimmed source text of one attribute
    /// (e.g. `"#[derive(Debug, Clone)]"`, `"#[must_use]"`,
    /// `"#[non_exhaustive]"`) or one doc-comment line formatted as
    /// `"/// doc text"` (one entry per line — multi-line doc-comments are
    /// surfaced as multiple entries so substring queries can match a single
    /// line). Inner attributes (`#![...]`) are not collected — they apply to
    /// the enclosing module/file, not the item itself. Empty for items with
    /// no attributes, or for items whose AST source could not be resolved
    /// (e.g. macro-generated impls).
    #[serde(default)]
    pub attributes: Vec<String>,
    /// Cargo target kind for crate nodes: `lib`, `bin`, `example`, `test`,
    /// `bench`, `build`, or `unknown`. `None` for non-crate nodes and for
    /// older snapshots that predate target-kind extraction.
    #[serde(default)]
    pub crate_target_kind: Option<String>,
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

/// v9 (Phase 5) — per-function signature record.
///
/// One `FunctionSignature` per local function (free fn, inherent assoc fn,
/// trait declaration fn). Type strings come from RA's `HirDisplay` rendered
/// against the function's owning crate as `DisplayTarget`; anonymous
/// lifetimes (`'_`) are suppressed by default — named lifetimes ('a, 'static)
/// render verbatim. Stored on `ExtractionModel.signatures` and persisted into
/// the `signatures_by_target` LMDB sub-DB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    #[serde(default)]
    pub is_async: bool,
    #[serde(default)]
    pub self_param: Option<SelfKind>,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub return_type: String,
    #[serde(default)]
    pub generics: Vec<GenericBound>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelfKind {
    Owned,
    Ref,
    RefMut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    /// Empty string when RA returned no name (RA's `Param.name(db)` is
    /// `Option<Name>` — falls back to empty here so callers can substitute
    /// `_` or `arg{idx}` as they see fit).
    pub name: String,
    /// `HirDisplay` output for the parameter type, anonymous lifetimes
    /// suppressed.
    pub ty: String,
    /// `true` for `&T` or `&mut T`, `false` for owned types.
    pub by_ref: bool,
    /// `true` for `&mut T`. Owned `mut x` (a binding-mode marker) is *not*
    /// reflected here; this field tracks reference mutability only.
    pub mutability: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericBound {
    /// Type-parameter name as written, e.g. `"T"` or `"K"`.
    pub name: String,
    /// Each entry is a Trait name (e.g. `["Send", "Sync"]`).
    /// NOTE (RA caveat): `TypeParam::trait_bounds` does *not* include
    /// where-clause bounds added by methods after the parameter is
    /// introduced — see the FIXME on `TypeParam::trait_bounds` in
    /// `ra_ap_hir`. Treat the list as a *partial* view of the bounds.
    pub bounds: Vec<String>,
}

/// v10 (Phase 7 Path B) — per-Static metadata.
///
/// One `StaticMetadata` per local `static` item. `type_string` is RA's
/// `HirDisplay` of the static's declared type, rendered against the static's
/// owning crate as `DisplayTarget`. `is_mut` is `true` iff the source uses
/// `static mut FOO: ...` (carries `StaticFlags::MUTABLE`). Stored on
/// `ExtractionModel.statics` and persisted into the
/// `static_metadata_by_target` LMDB sub-DB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticMetadata {
    #[serde(default)]
    pub type_string: String,
    #[serde(default)]
    pub is_mut: bool,
}

/// v11 — per-Item embedding cache record for `semantic_overlaps`.
///
/// Persisted into the `embeddings_by_target` sub-DB lazily by the
/// `semantic_overlaps` MCP tool — `build_hypergraph` does NOT populate it
/// (this is purely query-time written, not part of `ExtractionModel`).
///
/// `content_hash` is `SHA-256(source_bytes)` truncated to 16 bytes; mismatch
/// invalidates the entry (the item's source changed since the cache was
/// written). `embedder_version` pins the embedding model + dimension so cache
/// entries from a different model are detected and refreshed. `vector` length
/// depends on the active embedder backend (default 1024 for
/// Qwen3-Embedding-0.6B).
///
/// `f32` `PartialEq` is intentionally not derived for `Eq` — the vector is
/// floating-point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub content_hash: [u8; 16],
    pub vector: Vec<f32>,
    pub embedder_version: String,
    pub generated_at_unix: u64,
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
    /// v9: one entry per local function whose signature was extractable
    /// (free fns, inherent assoc fns, trait declaration fns; not impl-trait
    /// method bodies). Persisted to `signatures_by_target`.
    pub signatures: Vec<(NodeId, FunctionSignature)>,
    /// v10 (Phase 7 Path B): one entry per local `static` item with the HIR
    /// type stringified via `HirDisplay` and the `mut` flag. Persisted to
    /// `static_metadata_by_target`.
    pub statics: Vec<(NodeId, StaticMetadata)>,
}

impl ExtractionModel {
    pub fn insert_node(&mut self, node: Node) {
        self.nodes.entry(node.id).or_insert(node);
    }

    pub fn insert_contains(&mut self, parent: NodeId, child: NodeId) {
        self.contains.push((parent, child));
    }
}
