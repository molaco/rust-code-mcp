# Section D — P1.1 Read View / Navigate

## Overview

P1.1 is the **observation half** of the agent loop: the apparatus the agent uses to *see* the workspace before it asks for a write. It sits in M1 alongside P1.2 and P1.3; all three run on the **slow, cold-built** `OpenedSnapshot` and have no dependency on the warm-host writer (P0.2). The read-side is purely a thin composition layer on top of `rmc_graph::graph::query/*` — `lookup_by_qualified_name`, `module_tree`, `who_calls`, `call_graph`, `imports_of`, `exports_of`, `re_export_chain`, `enum_variants`, `crate_edges`, `find_root_module_of`, `node_by_id`, `callees_of`, `referrers_of`, plus the snapshot-internal `span_index()` / `line_to_byte()` — wrapped in a stateful `Navigator` that knows about *scale*, *focus*, and *cost*.

The five verbs (`goto`, `zoom`, `show_body`, `show_callers`, `follow_trail`) compose into one canonical observation type (`ContextView`) and one canonical addressing type (`Location`). The body operator is the **inverse of skeleton**: instead of stripping bodies for cheap surface dumps, it materialises a body span on demand and adds its byte/4 token cost to the view. Cluster scale (P1.3 territory) gets a stub so `Scale::Cluster` and `Location::Cluster(ClusterId)` are wired today and refilled later without re-shaping callers.

## New modules / files

- `crates/rmc-graph/src/graph/view/mod.rs` — public surface: `Location`, `Scale`, `ContextView`, `Navigator`, `NavStep`, `ZoomDir`, `NeighborSlot`, `NeighborKind`, `CallSlice`, `BodySlice`, `MapPane`, `CratePin`, `ModulePin`, `ClusterPin`, `ClusterId`, `ViewError`.
- `crates/rmc-graph/src/graph/view/location.rs` — `Location` enum, `Scale`, `Location::scale()`, `Location::from_qualified(snap, &str)`, `Location::node_id()`, `ClusterId` newtype stub.
- `crates/rmc-graph/src/graph/view/context.rs` — `ContextView`, `MapPane`, `NeighborSlot`, `CallSlice`, `BodySlice`, per-scale assemblers.
- `crates/rmc-graph/src/graph/view/navigate.rs` — `Navigator`, 5 verbs, `NavStep`, `follow_trail`, `ViewError`.
- `crates/rmc-graph/src/graph/view/body.rs` — skeleton-inverse: given `Node` with `(file, span)`, slice file bytes via `OpenedSnapshot::line_to_byte`.
- `crates/rmc-graph/src/graph/view/cost.rs` — `TokenCost`, `estimate_*` helpers, `BUDGET_DEFAULT`.
- `crates/rmc-graph/src/graph/view/cluster_stub.rs` — `ClusterId`, `ClusterPin`, `placeholder_cluster_neighbors()`; P1.3 replaces.
- Optional later: `crates/rmc-server/src/tools/graph/navigate.rs` — MCP handlers `navigate_goto`, `navigate_zoom`, `navigate_show_body`, `navigate_show_callers`, `navigate_follow_trail`.

`graph/mod.rs` gains `pub mod view;` and re-exports `pub use view::{Location, Scale, ContextView, Navigator, NavStep, ZoomDir};`. Placing `view` inside `crate::graph` keeps `span_index` / `line_to_byte` accessible at `pub(crate)` (matches the `codemap` precedent).

## Type definitions

```rust
// view/location.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClusterId(pub [u8; 16]);  // P1.3 replaces

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scale { Crate, Module, Cluster, Item, Body }

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Location {
    Workspace,
    Crate(NodeId),
    Module(NodeId),
    Cluster(ClusterId),
    Item(NodeId),
    Body { item: NodeId, file: String, span: (u32, u32) },
}

impl Location {
    pub fn scale(&self) -> Scale {
        match self {
            Location::Workspace => Scale::Crate,
            Location::Crate(_) => Scale::Crate,
            Location::Module(_) => Scale::Module,
            Location::Cluster(_) => Scale::Cluster,
            Location::Item(_) => Scale::Item,
            Location::Body { .. } => Scale::Body,
        }
    }
    pub fn node_id(&self) -> Option<NodeId> {
        match self {
            Location::Crate(id) | Location::Module(id) | Location::Item(id) => Some(*id),
            Location::Body { item, .. } => Some(*item),
            _ => None,
        }
    }
    pub fn from_qualified(snap: &OpenedSnapshot, q: &str) -> Result<Self, ViewError> {
        let (id, node) = snap.lookup_by_qualified_name(q)?
            .ok_or_else(|| ViewError::Unresolved(q.to_string()))?;
        Ok(match node.kind {
            NodeKind::Workspace => Location::Workspace,
            NodeKind::Crate => Location::Crate(id),
            NodeKind::Module => Location::Module(id),
            NodeKind::Item => Location::Item(id),
            NodeKind::ExternalSymbol => return Err(ViewError::ExternalSymbol(q.to_string())),
        })
    }
}
```

```rust
// view/context.rs

#[derive(Debug, Clone, Serialize)]
pub struct ContextView {
    pub focus: Location,
    pub scale: Scale,
    pub map_pane: MapPane,
    pub focal_node: Option<NodePin>,
    pub neighbors: Vec<NeighborSlot>,
    pub callgraph: Option<CallSlice>,
    pub exports: Vec<EnrichedBinding>,
    pub body: Option<BodySlice>,
    pub token_cost: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodePin {
    pub id: NodeId,
    pub qualified_name: String,
    pub display_name: String,
    pub kind: &'static str,
    pub item_kind: Option<String>,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub attributes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapPane {
    pub crates: Vec<CratePin>,
    pub modules: Vec<ModulePin>,
    pub clusters: Vec<ClusterPin>,
    pub current_path: Vec<NodeId>,
}

#[derive(Debug, Clone, Serialize)] pub struct CratePin { pub id: NodeId, pub name: String, pub efferent: u32, pub afferent: u32 }
#[derive(Debug, Clone, Serialize)] pub struct ModulePin { pub id: NodeId, pub qualified_name: String, pub display_name: String, pub depth: u8, pub child_count: u32 }
#[derive(Debug, Clone, Serialize)] pub struct ClusterPin { pub id: ClusterId, pub label: String }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NeighborKind { Sibling, Parent, Child, Import, Reexport, EnumVariant, Cluster }

#[derive(Debug, Clone, Serialize)]
pub struct NeighborSlot { pub label: String, pub loc: Location, pub kind: NeighborKind, pub item_kind: Option<String> }

#[derive(Debug, Clone, Serialize)]
pub struct CallSlice {
    pub callers: Vec<EnrichedCallSite>,
    pub callees: Vec<EnrichedCallSite>,
    pub callers_tree: Option<CallGraphNode>,
    pub callees_tree: Option<CallGraphNode>,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodySlice { pub file: String, pub start: u32, pub end: u32, pub line_start: u32, pub line_end: u32, pub text: String }
```

```rust
// view/navigate.rs

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)] pub enum ZoomDir { In, Out }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NavStep {
    Goto(Location), ZoomIn, ZoomOut, ShowBody, ShowCallers(u32),
}

#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("qualified name did not resolve: {0}")] Unresolved(String),
    #[error("qualified name is an external symbol: {0}")] ExternalSymbol(String),
    #[error("cannot zoom {dir:?} from {from:?}")] NoZoom { from: Scale, dir: ZoomDir },
    #[error("show_body requires Item or Body scale (got {0:?})")] BodyAtWrongScale(Scale),
    #[error("focus item has no body (file/span missing)")] BodyMissing,
    #[error("cluster scale is a stub until P1.3")] ClusterStub,
    #[error("view too large: {tokens} tokens > budget {budget}")] ViewTooLarge { tokens: usize, budget: usize },
    #[error(transparent)] Anyhow(#[from] anyhow::Error),
    #[error(transparent)] Io(#[from] std::io::Error),
}

pub struct Navigator<'a> {
    pub snap: &'a OpenedSnapshot,
    pub host: Option<&'a crate::graph::host::WorkspaceHost>,  // P0.2 placeholder
    pub budget: usize,
}

impl<'a> Navigator<'a> {
    pub fn new(snap: &'a OpenedSnapshot) -> Self { ... }
    pub fn with_budget(self, b: usize) -> Self { ... }
    pub fn goto(&self, loc: Location) -> Result<ContextView, ViewError>;
    pub fn zoom(&self, view: &ContextView, dir: ZoomDir) -> Result<ContextView, ViewError>;
    pub fn show_body(&self, view: &ContextView) -> Result<ContextView, ViewError>;
    pub fn show_callers(&self, view: &ContextView, depth: u32) -> Result<ContextView, ViewError>;
    pub fn follow_trail(&self, start: Location, steps: &[NavStep]) -> Result<ContextView, ViewError>;
}
```

## Step-by-step implementation

1. **`Location` + qualified-name parser.** WHERE: `view/location.rs`. Implement `Location::from_qualified` as a wrapper over `OpenedSnapshot::lookup_by_qualified_name`; pattern-match `Node.kind`. Implement `Location::scale()`, `node_id()`, `Location::parent(&self, snap) -> Option<Location>` via `Node.parent_id`. DEPENDS: `OpenedSnapshot::lookup_by_qualified_name`, `node_by_id`. VERIFY: `goto_qualified_resolves`.

2. **Scale ladder + `zoom`.** WHERE: `view/navigate.rs::Navigator::zoom`. **ZoomIn:** `Workspace → Crate(first by sorted crate_edges); Crate → Module(find_root_module_of); Module → Item(first child via children_by_parent); Item → Body{item, file, span}`; `Body` → `NoZoom`. **ZoomOut:** `Body → Item; Item → Module(parent_id); Module → Crate(if parent is crate); Crate → Workspace; Workspace → NoZoom`. Cluster scale today errors `ClusterStub` unless P1.3 is wired. DEPENDS: `Node.parent_id`, `find_root_module_of`, `crate_edges`. VERIFY: `zoom_in_out_idempotent`.

3. **MapPane assembly.** WHERE: `view/context.rs::build_map_pane`. **Crates rim (always):** run `snap.crate_edges()` once + `snap.crate_dependency_metric()`; resolve crate NodeIds via `lookup_by_qualified_name`. **Module tree (at Module/Item/Body scale):** walk `parent_id` up to crate; call `snap.module_tree(&crate_qualified, Some(N))` (default N=2); flatten DFS into `ModulePin`s; re-resolve each via `lookup_by_qualified_name`. **Current path:** walk `parent_id` from focus up to workspace; `[crate_root_module, ..., focus]`. **Clusters:** empty stub. DEPENDS: `crate_edges`, `crate_dependency_metric`, `module_tree`, `find_root_module_of`, `node_by_id`. VERIFY: `mappane_includes_path`.

4. **Neighbor enumeration.** WHERE: `view/context.rs::collect_neighbors`. Per scale:
   - **Crate:** edges from `crate_edges()` filtered to `name`; add root module as `Child`.
   - **Module:** `children_by_parent` via `dbs.children_by_parent.get_duplicates(rtxn, mid.as_bytes())` (same pattern as `build_module_tree` at `query/modules.rs:134-145`); each child → `node_by_id` → `NeighborSlot { kind: Child }`. Parent: `Node.parent_id`. Imports: `snap.imports_of(mid)` enriched via `snap.enrich_bindings`.
   - **Item:** siblings via `parent_id` then `children_by_parent` excluding self. Reexports: `snap.re_export_chain(iid)`. Enum variants: if `item_kind == Some(Enum)`, `snap.enum_variants(iid)`.
   - **Body:** same as Item.
   - **Cluster:** stub returns `vec![]` and tracing warn.
   VERIFY: unit test on `imports_of` against a known module.

5. **`goto` assembling ContextView.** WHERE: `view/navigate.rs::Navigator::goto`. Sequence: scale → `build_map_pane` → `build_focal_node` (populating `signature` via `function_signature(iid)`, `attributes` via `item_attributes(iid)`) → `collect_neighbors` → `exports = if module { snap.exports_of(focus_mid, focus_mid).and_then(enrich_bindings).unwrap_or_default() } else { vec![] }` → `cost::estimate(...)` → if `> budget` → `ViewTooLarge`. VERIFY: `goto_qualified_resolves`.

6. **`show_body` (skeleton-inverse).** WHERE: `view/body.rs::materialise_body`. Pull `file = node.file.clone().ok_or(BodyMissing)?`, `(start, end) = node.span.ok_or(BodyMissing)?`. Get line-to-byte via `OpenedSnapshot::line_to_byte(file)`. Convert byte offsets to line via `partition_point(|&off| off <= start)`. `text = String::from_utf8_lossy(&bytes[start..end]).into_owned()`. In `Navigator::show_body`: focus must be `Item` or `Body`. Update `view` clone with `focus = Body { item, file, span }`, `scale = Body`, `body = Some(...)`, `token_cost += body_tokens`. If `> budget` → `ViewTooLarge`. VERIFY: `show_body_token_growth`.

7. **`show_callers`.** WHERE: `view/navigate.rs::Navigator::show_callers`. For `Item(iid)`: `callers = snap.who_calls(iid)?; callees = snap.calls_from(iid)?;`. If `depth > 1`: `callees_tree = Some(snap.call_graph(iid, depth)?)`; callers_tree via reverse BFS using `snap.referrers_of(target)` iteratively, synthesise into `CallGraphNode`-shaped tree. Update clone's `callgraph`. VERIFY: `show_callers_matches_who_calls`.

8. **`follow_trail`.** Pure interpreter:
   ```rust
   let mut view = self.goto(start)?;
   for step in steps {
       view = match step {
           NavStep::Goto(loc) => self.goto(loc.clone())?,
           NavStep::ZoomIn => self.zoom(&view, ZoomDir::In)?,
           NavStep::ZoomOut => self.zoom(&view, ZoomDir::Out)?,
           NavStep::ShowBody => self.show_body(&view)?,
           NavStep::ShowCallers(d) => self.show_callers(&view, *d)?,
       };
   }
   Ok(view)
   ```
   Every step re-checks budget; trail can fail mid-way with `ViewTooLarge`. VERIFY: `follow_trail_replays`.

9. **Token cost estimator.** WHERE: `view/cost.rs`. Coefficients (conservative `bytes/4` baseline for Claude tokenizers):
   - `FOCAL_NODE_BASE = 60`, `SIGNATURE_TOK = 40`, `ATTRIBUTE_TOK = 10` per attr.
   - `NEIGHBOR_SLOT_TOK = 12`, `MAP_CRATE_PIN_TOK = 8`, `MAP_MODULE_PIN_TOK = 14`.
   - `EXPORT_BINDING_TOK = 20`, `CALL_SITE_TOK = 25`, `BODY_TOK = body.text.len().div_ceil(4)`.
   - `CALLGRAPH_NODE_TOK = 18` per node recursively.
   ```rust
   pub fn estimate(focal: &Option<NodePin>, neighbors: &[NeighborSlot], map: &MapPane,
                   body: Option<&BodySlice>, calls: Option<&CallSlice>) -> usize { ... }
   pub fn body_tokens(body: &BodySlice) -> usize { body.text.len().div_ceil(4) }
   pub const BUDGET_DEFAULT: usize = 8_000;
   ```

10. **Optional MCP handlers.** WHERE: `crates/rmc-server/src/tools/graph/navigate.rs`. Five tools (`navigate_goto`, `_zoom`, `_show_body`, `_show_callers`, `_follow_trail`) mirroring the `who_calls` pattern at `tools/graph/core.rs`. Params files in `tools/params/`. `navigate_follow_trail` accepts `start: NavigateGotoParams` + `steps: Vec<NavStepJson>` (externally-tagged serde enum). Gated by `#[cfg(feature = "navigate")]`.

11. **Serde round-trip.** All view types derive `Serialize`; address types (`Location`, `Scale`, `NavStep`, `ZoomDir`, `ClusterId`) also `Deserialize`. `#[serde(rename_all = "snake_case")]` on enums. `Location` externally tagged. `NodeId` already serde. `#[serde(skip)]` `callgraph`/`body` when None. VERIFY: round-trip test.

12. **Body hide/show round-trip.** VERIFY: `body_round_trip`.

13. **Wire the module.** `graph/mod.rs`:
    ```rust
    pub mod view;
    pub use view::{Location, Scale, ContextView, Navigator, NavStep, ZoomDir,
                   NeighborSlot, NeighborKind, CallSlice, BodySlice, MapPane,
                   ClusterId, ViewError};
    ```

## Tests

(`crates/rmc-graph/src/graph/view/tests.rs`, reusing `test_support::shared_snapshot()`)

- **`goto_qualified_resolves`** — `Location::from_qualified(snap, "rmc_graph::graph::snapshot::open_current")`; assert `scale == Item`, `focal_node.signature.is_some()`.
- **`zoom_in_out_idempotent`**.
- **`show_body_token_growth`** — `cost2 - cost1 ≈ body.text.len() / 4 ± 16`.
- **`show_callers_matches_who_calls`** — pick `lookup_by_qualified_name_inner`; assert `slice.callers.len() == snap.who_calls(iid)?.len()`.
- **`follow_trail_replays`** — manual chain vs `follow_trail` produce same final view.
- **`mappane_includes_path`** — `current_path.first()` is crate root, `last() == iid`.
- **`view_too_large_refused`** — `with_budget(10)` on a large module → `ViewTooLarge`.
- **`external_symbol_rejected`** — `Location::from_qualified(snap, "std::sync::Arc")` → `ExternalSymbol`.
- **`body_round_trip`** — show_body then zoom_out returns Item with `body: None`.
- **`serde_json_round_trip`**.
- **`zoom_at_floor_errors`** — `Body{..}` + `ZoomIn` → `NoZoom`.

## Open decisions / risks

- **Cluster stub.** `Location::Cluster(ClusterId)` and `MapPane.clusters` wired today; `cluster_stub.rs` returns `ClusterStub` when explicitly asked. P1.3 swaps `cluster_stub.rs` for the real assembler without changing `ContextView` or callers.
- **Cost calibration.** `bytes/4` is conservative; log `(actual_tokenized, estimated)` pairs during M3 and recalibrate.
- **JSON vs compact text.** Ship JSON; P1.8 adds `render_textual(&ContextView) -> String` adapter.
- **File-text caching.** Re-read on each `show_body` (μs). With P0.2 host, `Navigator.host` field consults latest live text first.
- **MCP handler placement.** Thin layer — `Navigator` composes existing `OpenedSnapshot` queries; don't duplicate.
- **`exports_of` consumer.** Pass `consumer = focus_module` ("what this module exposes internally").
- **`show_callers` on Module.** Optional fanout cap 50; off by default.
- **`follow_trail` loops.** No detection; budget is the safety net.
- **`Location::Workspace` semantics.** `goto(Workspace)` → Crate-scale view, `focus = Workspace`, all crates in `MapPane.crates`, empty modules/neighbors, no focal_node.
- **`span_index` / `line_to_byte` visibility.** Both `pub(crate)` on `OpenedSnapshot`. Place `view` inside `crate::graph` (codemap precedent).
- **`ViewTooLarge` policy.** Fires after full assembly (so cost is accurate). Add cheap `estimate_lower_bound(loc, snap)` pre-flight later as perf TODO.


---

