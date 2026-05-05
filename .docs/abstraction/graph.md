# graph — Abstract Logic

## Module: ast_resolve
**Purpose:** Resolve AST call expressions to HIR functions in a turbofish-safe way.

1. **Resolve a call expression's callee to a HIR function, filtering out closures, fn pointers, and tuple constructors** -> `resolve_call_to_function()`

---

## Module: attributes
**Purpose:** Walk every local item and attach its outer attributes and doc comments to its graph node.

1. **Attach DB, build Semantics, and recursively visit every reachable module per local crate** -> `extract_attributes()`, `visit_module()`
2. **Dispatch on item kind (ADT, trait, assoc item) and fetch source AST per declaration** -> `visit_adt()`, `visit_assoc_item()`
3. **Read outer attributes and normalize doc comments onto the target node** -> `set_attrs_for()`

---

## Module: bindings
**Purpose:** Populate the Item-level `def_to_node` map and emit Binding records (declarations, named/glob imports, extern-crate imports) per module.

1. **Walk every local crate's DefMap, classifying type/value-namespace entries by provenance** -> `extract_bindings()`, `classify_type_provenance()`, `classify_value_provenance()`
2. **Process one scope entry: filter macros/builtins, resolve target node, emit contains-edge and Binding record** -> `process_entry()`
3. **Resolve a def to a NodeId, creating a local Item node or external-symbol stub as needed** -> `resolve_or_create_target()`, `create_local_item_node()`, `stub_qualified_name()`
4. **Look up def metadata: item kind label, canonical name, owner module, owner crate name, and module path** -> `item_kind_for_def()`, `item_kind_label()`, `name_for_def()`, `module_def_owner_module()`, `owner_crate_name()`, `module_qualified_path()`
5. **Encode HIR visibility (Public / PubCrate / Module-restricted / Private) into the binding record** -> `encode_visibility()`, `use_has_explicit_visibility()`

---

## Module: channel_audit
**Purpose:** Detect and classify channel-constructor call sites (tokio/std/crossbeam/flume) with capacity extraction.

1. **Classify a canonical path as a known channel constructor and parse capacity literals** -> `classify_channel_path()`, `parse_capacity_arg()`, `extract_int_literal()`
2. **Walk every local file's AST, resolve call expressions, classify, and emit findings** -> `channel_audit()`
3. **Resolve enclosing fn and skip cfg(test)-gated sites; resolve workspace-relative paths** -> `resolve_enclosing_function()`, `enclosed_by_cfg_test()`, `item_has_cfg_test()`, `resolve_workspace_relative()`, `canonical_function_path()`

---

## Module: derive_audit
**Purpose:** Find ADT items missing a configured set of required derives.

1. **Run the audit: scan candidate items by crate/kind, join with declared bindings, and flag missing derives** -> `derive_audit()`
2. **Parse `#[derive(...)]` attribute strings into a trait-name set and compute the missing set per item** -> `extract_derives()`, `missing_required_derives()`
3. **Provide the default kind filter (Struct/Enum/Union)** -> `default_kind_filter()`

---

## Module: docs_audit
**Purpose:** Find public items lacking `///` doc comments.

1. **Run the audit: collect candidate items, derive their effective visibility from bindings, and flag undocumented pub items** -> `missing_docs_audit()`, `is_undocumented_pub_item()`
2. **Provide the default documentable-kind filter** -> `default_kind_filter()`

---

## Module: extract
**Purpose:** Top-level orchestration that produces the full `ExtractionModel` from a loaded workspace.

1. **Drive the extraction pipeline: workspace node, crates/modules, then bindings, impls, attributes, signatures, statics, usages** -> `extract()`
2. **Emit Crate and Module nodes with contains-edges per local crate** -> `emit_crate()`
3. **Compute crate display name and module path segments** -> `crate_display_name()`, `module_path_segments()`

---

## Module: fn_body_audit
**Purpose:** Pattern-match function bodies for risky idioms (unwrap, panic macros, unbounded loops, await-while-holding-guard, transmute, self-recursion).

1. **Validate and parse the user's pattern filter against the known pattern set** -> `parse_pattern_filter()`, `ALL_PATTERNS`
2. **Per-pattern matchers that walk a fn body's syntax to record raw findings** -> `match_unwrap()`, `match_expect()`, `match_panic_macros()`, `match_unwrap_unchecked()`, `match_unbounded_loop()`, `match_await_in_guard_scope()`, `match_transmute()`, `match_self_recursion()`
3. **Drive the audit: parse files, walk fn bodies, run enabled matchers, attach context strings** -> `fn_body_audit()`, `build_context()`
4. **Resolve enclosing fn and helpers shared with channel_audit** -> `enclosing_fn_for_body_offset()`, `canonical_function_path()`, `enclosed_by_cfg_test()`, `item_has_cfg_test()`, `resolve_workspace_relative()`

---

## Module: hir_trim
**Purpose:** Clean noisy default type parameters out of HIR-display strings (e.g., `, Global>`, `RandomState`, `BuildHasherDefault`, `LazyLock` init fn).

1. **Apply the full chain of trims to a HIR-display string** -> `trim_hir_display()`
2. **Strip `BuildHasherDefault` and redundant `LazyLock` init-fn type args via depth-tracking scans** -> `strip_build_hasher_default()`, `strip_lazy_lock_init_fn()`

---

## Module: ids
**Purpose:** Stable SHA-256-based identifiers for nodes, bindings, and usages, plus a workspace-root hash.

1. **Build IDs by hashing a NUL-separated component list and render hex / debug forms** -> `BindingId::from_components()`, `UsageId::from_components()`, `NodeId::from_components()`, `BindingId::to_hex()`, `BindingId::as_bytes()`, `BindingId::fmt()`, `UsageId::to_hex()`, `UsageId::as_bytes()`, `UsageId::fmt()`, `NodeId::to_hex()`, `NodeId::as_bytes()`, `NodeId::fmt()`
2. **Hash a workspace root path into a stable hex digest** -> `workspace_hash()`, `hex_encode()`
3. **Round-trip 32-byte arrays through serde** -> `serde_bytes_32::serialize()`, `serde_bytes_32::deserialize()`

---

## Module: impls
**Purpose:** Emit Method, AssocConst, AssocType, and EnumVariant Item nodes from inherent impls, traits, and enums.

1. **Walk inherent impls, traits, and enums per local crate; emit assoc items and variants** -> `extract_impl_items()`
2. **Emit one assoc-item node with kind label, file/span, and contains-edge to the host** -> `emit_assoc_item()`
3. **Emit one enum-variant Item node parented to the enum** -> `emit_enum_variant()`
4. **Resolve workspace-relative file paths** -> `resolve_workspace_relative()`

---

## Module: loader
**Purpose:** Load a Cargo workspace into a populated `RootDatabase` + `Vfs` and filter to local crates.

1. **Canonicalize the directory and run `ra_ap_load_cargo` with the standard config** -> `load()`
2. **Filter the resulting crate list to those with local origin** -> `filter_local_crates()`

---

## Module: model
**Purpose:** Define the in-memory ExtractionModel and its node/binding/usage/signature/static record types.

1. **Type definitions for every node kind, item kind, binding kind, visibility, namespace, and metadata struct** -> `NodeKind`, `ItemKind`, `Namespace`, `BindingKind`, `BindingVisibility`, `Node`, `Binding`, `UsageCategory`, `Usage`, `FunctionSignature`, `SelfKind`, `Param`, `GenericBound`, `StaticMetadata`, `EmbeddingRecord`, `ExtractionModel`
2. **Insert nodes idempotently and append contains-edges** -> `ExtractionModel::insert_node()`, `ExtractionModel::insert_contains()`

---

## Module: mod (graph::mod)
**Purpose:** Module file declaring the `pub mod` set and re-exporting the graph crate's public API surface.

1. **Re-export `extract`, `ids`, `loader`, `model`, `queries`, `snapshot`, `storage`, `unsafe_audit` etc.** -> module declarations only

---

## Module: queries
**Purpose:** Read-side query layer over the persisted snapshot — name lookup, imports/exports, dead-pub, call graph, audits, overlaps, module tree, workspace stats.

1. **Type definitions for every query result row and audit record** -> `DeadPubFinding`, `CrateDeadPub`, `CrateEdge`, `EdgeSymbol`, `ForbiddenDependencyRule`, `ForbiddenDependencyViolation`, `OverlapsReport`, `TypeCollision`, `TypeLocation`, `ModuleShadow`, `WithinCrateDuplicate`, `CommonFnName`, `EnrichedCallSite`, `CallGraphNode`, `RecursiveCallersCount`, `UsageSummaryRow`, `ModuleTreeNode`, `WorkspaceStats`, `NodeKindCounts`, `VisibilityCounts`, `ItemWithAttribute`, `FunctionFilter`, `SelfKindFilter`, `FunctionWithSignature`, `PubTypeAliasMasqueradingAsReexport`, `ReExportLink`, `ReExportChain`, `CrateMetric`, `MutStaticFinding`
2. **Resolve a qualified name to a node, following re-export hops up to a budget** -> `OpenedSnapshot::lookup_by_qualified_name()`, `OpenedSnapshot::lookup_by_qualified_name_inner()`
3. **Direct LMDB lookups for nodes, signatures, statics, attributes, root modules** -> `OpenedSnapshot::node_by_id()`, `OpenedSnapshot::find_root_module_of()`, `OpenedSnapshot::function_signature()`, `OpenedSnapshot::static_metadata()`, `OpenedSnapshot::item_attributes()`
4. **Module-scoped import/export queries with visibility filtering** -> `OpenedSnapshot::imports_of()`, `OpenedSnapshot::exports_of()`, `OpenedSnapshot::reexports_of()`, `OpenedSnapshot::declared_reexports_of()`
5. **Reverse-import and usage queries** -> `OpenedSnapshot::who_imports()`, `OpenedSnapshot::usages_of()`, `OpenedSnapshot::usages_in()`, `OpenedSnapshot::who_uses_summary()`
6. **Call-graph queries (forward, backward, recursive, scoped)** -> `OpenedSnapshot::who_calls()`, `OpenedSnapshot::calls_from()`, `OpenedSnapshot::call_graph()`, `OpenedSnapshot::call_graph_rec()`, `OpenedSnapshot::callers_in_crate()`, `OpenedSnapshot::recursive_callers_count()`
7. **Workspace audits: dead-pub, mut-static, missing-docs, attribute search, function filtering, pub-use-as-type-alias** -> `OpenedSnapshot::dead_pub_in_crate()`, `OpenedSnapshot::dead_pub_report()`, `OpenedSnapshot::mut_static_audit()`, `classify_metadata()`, `OpenedSnapshot::items_with_attribute()`, `OpenedSnapshot::functions_with_filter()`, `OpenedSnapshot::pub_use_pub_type_audit()`
8. **Cross-crate edges and metrics** -> `OpenedSnapshot::crate_edges()`, `OpenedSnapshot::forbidden_dependency_check()`, `OpenedSnapshot::crate_dependency_metric()`
9. **Re-export chain BFS and overlaps/collision report** -> `OpenedSnapshot::re_export_chain()`, `OpenedSnapshot::overlaps()`
10. **Module-tree dump and workspace counters** -> `OpenedSnapshot::module_tree()`, `OpenedSnapshot::build_module_tree()`, `OpenedSnapshot::workspace_stats()`
11. **Enum variants and unsafe-audit dispatch** -> `OpenedSnapshot::enum_variants()`, `OpenedSnapshot::unsafe_audit()`
12. **Internal DUP_SORT iterator and label/format helpers** -> `bindings_for_from_module()`, `bindings_for_target()`, `usages_for_target()`, `usages_for_consumer()`, `usages_for_consumer_function()`, `module_ancestors()`, `label_node_kind()`, `label_item_kind()`, `label_binding_kind()`, `glob_match()`, `usage_category_label()`, `match_attribute()`, `format_binding_visibility()`, `filter_matches()`, `is_visible_from()`

---

## Module: recursion_check
**Purpose:** Detect recursion cycles among workspace functions via DFS over the persisted call graph.

1. **Clamp the user's max-cycle-length into the hard-cap range** -> `clamp_cycle_length()`
2. **Build adjacency lists from usages, run DFS from each fn, canonicalize and dedup cycles** -> `recursion_check()`, `find_cycles_from()`, `dfs()`, `canonicalize_cycle()`
3. **Resolve cycle node-ids back into qualified-name lists** -> `enclosing_fn_qualified_names()`

---

## Module: signatures
**Purpose:** Build per-function structured signatures (async flag, self-kind, params, return type, generic bounds).

1. **Iterate FunctionId defs, cache crate display targets, build and store one signature per fn** -> `extract_signatures()`
2. **Assemble a `FunctionSignature` from HIR queries with type-string trimming** -> `build_signature()`

---

## Module: snapshot
**Purpose:** Persist an `ExtractionModel` to a content-addressed LMDB graph and open existing snapshots for read.

1. **Build/persist a graph from a workspace directory, with reuse short-circuit on fingerprint match** -> `build_and_persist()`
2. **Persist an already-loaded workspace, always rewriting the snapshot** -> `persist_loaded()`
3. **Write all sub-DB rows (nodes, bindings, contains, usages, signatures, statics, meta) inside one txn** -> `write_model()`
4. **Hash a binding/usage into a stable id** -> `binding_id_for()`, `usage_id_for()`
5. **Atomically publish a new graph id via the `CURRENT` pointer** -> `publish_current()`, `now_unix()`
6. **Open a stored snapshot via the `CURRENT` pointer or by explicit graph id** -> `open_current()`, `open_specific()`
7. **Provide read/write transactions and direct node lookups** -> `OpenedSnapshot::read_txn()`, `OpenedSnapshot::write_txn()`, `OpenedSnapshot::node()`

---

## Module: statics
**Purpose:** Capture each `static`'s type string and `is_mut` flag for downstream audits.

1. **Iterate StaticId defs, render trimmed type strings, and store metadata records** -> `extract_statics()`

---

## Module: storage
**Purpose:** Filesystem layout, env options, fingerprinting, manifest IO, and LMDB sub-DB creation/opening.

1. **Constants and tunable env options for the heed environment** -> `SCHEMA_VERSION`, `CURRENT_POINTER_FILENAME`, `SNAPSHOTS_DIRNAME`, `MANIFEST_FILENAME`, `GraphEnvOptions`, `GraphEnvOptions::to_open_options()`
2. **Resolve workspace-keyed paths for the snapshot tree** -> `GraphPaths::for_workspace()`, `GraphPaths::for_workspace_in()`, `GraphPaths::snapshot_dir()`, `GraphPaths::manifest_path()`, `GraphPaths::ensure_dirs()`, `default_data_dir()`
3. **Compute content fingerprints and graph ids** -> `compute_fingerprint()`, `graph_id_for()`
4. **Create or open the suite of typed LMDB sub-databases** -> `GraphDatabases::create()`, `GraphDatabases::open()`, `open_or_create_str_bytes()`, `open_or_create_bytes_bincode()`, `open_or_create_bytes_bytes()`
5. **Read/write the JSON manifest with strict and permissive variants** -> `write_manifest()`, `read_manifest()`, `read_manifest_compatible()`
6. **Internal dead-code marker keeping `BindingId` import alive** -> `_binding_id_marker()`

---

## Module: unsafe_audit
**Purpose:** Find every `unsafe { }` block, attribute span/line metadata, SAFETY-comment heuristic, and enclosing fn.

1. **Walk every local file's `BlockExpr`, detect unsafe tokens, resolve enclosing fn, emit findings** -> `unsafe_audit_impl()`
2. **Heuristically detect SAFETY comments in the preceding lines** -> `has_safety_comment_in_preceding_lines()`
3. **Resolve workspace-relative file paths** -> `resolve_workspace_relative()`

---

## Module: usages
**Purpose:** Emit a Usage row for every reference to every Item, classified by category and attributed to a consumer module/function.

1. **Iterate Item defs, collect `Definition::usages` references, and emit per-reference Usage records** -> `extract_usages()`
2. **Map `ReferenceCategory` bitflags into a single `UsageCategory` by precedence** -> `classify_category()`
3. **Resolve workspace-relative file paths** -> `resolve_workspace_relative()`
