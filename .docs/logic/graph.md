# graph — Detailed Logic

## Module: ast_resolve

### `resolve_call_to_function(sema, call) -> Option<Function>`
**Call graph:** `CallExpr::expr` -> `Semantics::resolve_expr_as_callable` -> `Callable::kind`
**Steps:**
1. Extract the callee expression from the `CallExpr` (returns `None` if absent).
2. Resolve the expression to a `Callable` via `Semantics::resolve_expr_as_callable` (handles turbofish / generic arg lists that `resolve_path` mishandles).
3. Match the callable's kind, returning `Some(Function)` only for `CallableKind::Function`; closures, fn pointers, tuple-struct constructors, and tuple-variant constructors return `None`.

---

## Module: attributes

### `extract_attributes(model, db, _vfs, local_crates, def_to_node)`
**Call graph:** `attach_db` -> `Semantics::new` -> `Crate::root_module` -> `visit_module`
**Steps:**
1. Attach the `RootDatabase` to the current thread-local context via `attach_db`.
2. Construct a `Semantics` wrapper around the database.
3. For each local crate, look up its root `Module`.
4. Recursively walk every reachable module via `visit_module` to populate Item attributes.

### `visit_module(model, sema, def_to_node, krate, module)` (private)
**Call graph:** `Module::declarations` -> `visit_module` (recursion) | `visit_adt` | `visit_assoc_item` -> `Module::impl_defs` -> `Impl::trait_` / `Impl::self_ty` / `Impl::items` -> `set_attrs_for`
**Steps:**
1. Iterate the module's top-level declarations from `module.declarations(db)`.
2. For child `ModuleDef::Module` entries owned by the same crate, recurse into them.
3. For functions/traits/type-aliases/consts/statics, fetch the AST source via `HasSource::source` and call `set_attrs_for`.
4. For ADTs, dispatch to `visit_adt`; for traits, also walk trait `items` via `visit_assoc_item`.
5. After declarations, walk every inherent (non-trait) `Impl` of local ADTs and recursively visit each impl's assoc items.

### `visit_adt(model, sema, def_to_node, adt)` (private)
**Call graph:** `Struct/Union/Enum::source` -> `set_attrs_for` -> `Enum::variants` -> `EnumVariant::source`
**Steps:**
1. Match the `Adt` variant (Struct, Union, Enum) and extract its `AdtId` form.
2. Fetch the source AST and call `set_attrs_for` for the type itself.
3. For enums, additionally iterate every variant and call `set_attrs_for` for each variant's source.

### `visit_assoc_item(model, sema, def_to_node, assoc)` (private)
**Call graph:** `AssocItem::*::source` -> `set_attrs_for`
**Steps:**
1. Match on the `AssocItem` kind (Function / Const / TypeAlias) and convert to its def-id form.
2. Fetch the AST source and pass to `set_attrs_for` with the appropriate `ModuleDefId`.

### `set_attrs_for(model, def_to_node, def_id, node)` (private generic)
**Call graph:** `HasAttrs::attrs` -> `Attr::kind` / `Attr::syntax` -> `HasDocComments::doc_comments`
**Steps:**
1. Look up the Item's `NodeId` via `def_to_node`; bail if absent.
2. Look up the mutable `Node` in `model.nodes`; bail if absent.
3. Skip if the node already has populated attributes (defensive against double-visits).
4. Iterate outer attributes (`attr.kind().is_outer()`), append the trimmed source text to the output Vec.
5. Iterate doc comments, strip `///` or `//!` prefix, split multi-line `/** */` into lines, prepend `"/// "` and push each line.
6. Assign the final Vec to `item_node.attributes`.

---

## Module: bindings

### `extract_bindings(model, db, local_crates, crate_node_for, crate_name_for, module_node_for) -> HashMap<ModuleDefId, NodeId>`
**Call graph:** `crate_def_map` -> `DefMap::modules` -> `process_entry` -> `classify_type_provenance` / `classify_value_provenance`
**Steps:**
1. Seed `def_to_node` with all known local module ids so module-target imports resolve.
2. For each local crate, look up its `DefMap` and crate metadata.
3. For each non-block module, walk both the type-namespace and value-namespace items of its `ItemScope`.
4. Per entry, classify provenance (Declared vs NamedImport vs GlobImport vs ExternCrateImport) and call `process_entry`.
5. After the walk, dedup duplicate type/value bindings on `(from_module, visible_name, target, kind)` keeping the first row.
6. Return the populated `def_to_node` map for downstream passes.

### `process_entry(...)` (private, `#[allow(too_many_arguments)]`)
**Call graph:** `resolve_or_create_target` -> `model.insert_contains` -> `encode_visibility` -> `use_has_explicit_visibility`
**Steps:**
1. Filter out macros, builtin types, and enum variants (modeled separately).
2. Resolve or create the target NodeId via `resolve_or_create_target`.
3. If the binding is `Declared` and the target is an Item parented to this module, push a contains-edge unless one already exists.
4. Encode visibility through `encode_visibility`.
5. If a `UseId` is present, call `use_has_explicit_visibility` to detect explicit `pub` syntax.
6. Push a `Binding` record onto `model.bindings`.

### `resolve_or_create_target(model, db, def_to_node, module_node_for, def_id, from_crate_name, visible_name) -> Option<NodeId>` (private)
**Call graph:** `module_def_owner_module` -> `create_local_item_node` | `stub_qualified_name` -> `NodeId::from_components` -> `model.insert_node`
**Steps:**
1. Return any cached `NodeId` from `def_to_node`.
2. Find the def's owner module; bail if not resolvable.
3. If owner is a local module, call `create_local_item_node` and cache the result.
4. Otherwise, build a stub qualified name, derive an `ExternalSymbol` NodeId, and insert a stub Node when not yet present.

### `create_local_item_node(...)` (private)
**Call graph:** `item_kind_for_def` -> `name_for_def` -> `item_kind_label` -> `NodeId::from_components` -> `model.insert_node`
**Steps:**
1. Map `ModuleDefId` to `ItemKind`; bail if not a recognized item.
2. Resolve the canonical name (falling back to `visible_name`).
3. Compose a qualified name from the parent module's qualified name plus this item's name.
4. Hash `[workspace_hash, "item", crate_name, module_qual, kind_label, name]` to produce a stable `NodeId`.
5. Insert a fully-populated Item Node and return its id.

### `item_kind_for_def(def_id) -> Option<ItemKind>` (private)
**Call graph:** none
**Steps:**
1. Match `ModuleDefId` to its corresponding `ItemKind` (Function / Struct / Enum / Union / Trait / TypeAlias / Const / Static), returning `None` for unhandled variants.

### `item_kind_label(kind) -> &'static str` (private)
**Call graph:** none
**Steps:**
1. Map each `ItemKind` to a stable string component used inside `NodeId` hash inputs.

### `name_for_def(db, def_id) -> Option<String>` (private)
**Call graph:** `Function/Adt/Trait/TypeAlias/Const/Static::name`
**Steps:**
1. Convert the def id to its HIR wrapper and call `name(db)`, returning the canonical name string (or `None` for `Const` without a name).

### `module_def_owner_module(db, def_id) -> Option<ModuleId>` (private)
**Call graph:** various `*Id::module(db)`
**Steps:**
1. For each `ModuleDefId` variant, call its `module(db)` accessor to determine the declaring module.

### `owner_crate_name(db, module_id) -> String` (private)
**Call graph:** `Crate::extra_data` -> `display_name`
**Steps:**
1. Look up the module's crate, then read its `display_name.canonical_name`, falling back to `"unknown_crate"`.

### `module_qualified_path(db, module_id) -> String` (private)
**Call graph:** `owner_crate_name` -> `Module::name` / `DefMap::containing_module`
**Steps:**
1. Resolve the crate name.
2. Walk parent modules collecting names, reverse to root-down order.
3. Join the segments and prepend the crate name.

### `stub_qualified_name(db, def_id, from_crate_name, visible_name) -> String` (private)
**Call graph:** `module_def_owner_module` -> `module_qualified_path` -> `name_for_def`
**Steps:**
1. If the def has an owner module, build `"<module_qual>::<name>"` (preferring the canonical name).
2. Otherwise return `"extern::<from_crate_name>::<visible_name>"`.

### `classify_type_provenance(p) -> (BindingKind, Option<UseId>)` (private)
**Call graph:** none
**Steps:**
1. `None` -> `Declared`; `ImportOrExternCrate::Import` -> `NamedImport`; `Glob` -> `GlobImport`; `ExternCrate` -> `ExternCrateImport` (no UseId).

### `classify_value_provenance(p) -> (BindingKind, Option<UseId>)` (private)
**Call graph:** none
**Steps:**
1. Same shape as type-provenance but on `ImportOrGlob` (no extern-crate variant in the value namespace).

### `use_has_explicit_visibility(db, use_id) -> bool` (private)
**Call graph:** `UseId::lookup` -> `to_node` -> `HasVisibility::visibility`
**Steps:**
1. Look up the `Use` declaration's source AST.
2. Return `true` iff the AST node carries an explicit visibility token.

### `encode_visibility(model, db, _def_map, vis, from_crate, module_node_for) -> BindingVisibility` (private)
**Call graph:** `Crate::display_name` -> `NodeId::from_components`
**Steps:**
1. Map `HirVisibility::Public` to `BindingVisibility::Public`.
2. For `PubCrate`, resolve the crate node id (or fall back to `Private` if not in this workspace).
3. For `Module(restrict_id, _)`, look up the local module NodeId and return `RestrictedTo`, else `Private`.

---

## Module: channel_audit

### `ChannelAuditOpts` (struct)
Plain config struct holding `crate_id_filter` and `skip_test_fns`.

### `ChannelFinding` (struct)
Output record describing one detected channel call site.

### `classify_channel_path(canonical_path) -> Option<(&'static str, bool)>`
**Call graph:** none
**Steps:**
1. Match the canonical path against a hardcoded table of known channel constructors (tokio, std, crossbeam, flume).
2. Return `(kind_label, bounded_flag)` for matched entries; `None` for unknown paths.

### `parse_capacity_arg(arg_text) -> Option<u64>`
**Call graph:** none
**Steps:**
1. Trim whitespace and remove `_` digit separators.
2. Parse the cleaned string as `u64`; return `None` for non-numeric or negative input.

### `channel_capacity_audit(loaded, snap, opts) -> Result<Vec<ChannelFinding>>`
**Call graph:** `attach_db` -> `Semantics::new` -> `crate_def_map` -> `resolve_workspace_relative` -> `sema.parse_guess_edition` -> `resolve_call_to_function` -> `canonical_function_path` -> `classify_channel_path` -> `enclosed_by_cfg_test` -> `extract_int_literal` -> `resolve_enclosing_function`
**Steps:**
1. Attach the database and create a `Semantics`.
2. If a crate filter is set, look it up by NodeId and store the qualified name in a HashSet.
3. For each local crate (filtered), iterate its `DefMap` modules to build a `FileId -> crate_name` map.
4. For each file, parse the AST and walk every `CallExpr` descendant.
5. Skip non-PathExpr callees, then resolve to a HIR `Function` via `resolve_call_to_function`.
6. Compute the canonical function path and classify it; bail on unknown paths.
7. Optionally skip test-cfg'd call sites; for bounded channels, extract the first int-literal argument as capacity.
8. Resolve the enclosing function NodeId via `resolve_enclosing_function`.
9. Emit a `ChannelFinding` and finally sort the result by `(file, span_start)`.

### `canonical_function_path(db, func) -> String` (private)
**Call graph:** `Function::module` -> `Module::path_to_root` -> `Function::name`
**Steps:**
1. Resolve the function's module and crate.
2. Walk module ancestors from crate root down, collecting names.
3. Concatenate `crate_name::seg::seg::fn_name` (with empty-segment guards).

### `extract_int_literal(expr) -> Option<u64>` (private)
**Call graph:** `parse_capacity_arg`
**Steps:**
1. Match on `Expr::Literal` then `LiteralKind::IntNumber`.
2. Read the literal's source text and pass to `parse_capacity_arg`.

### `enclosed_by_cfg_test(node) -> bool` (private)
**Call graph:** `item_has_cfg_test`
**Steps:**
1. Walk ancestor syntax nodes; for each that is an `ast::Item`, check `item_has_cfg_test`.
2. Return `true` on first match, else `false`.

### `item_has_cfg_test(item) -> bool` (private)
**Call graph:** `HasAttrs::attrs`
**Steps:**
1. Iterate the item's outer attributes via the appropriate `HasAttrs` impl.
2. Strip whitespace from each attribute's source text and check for `cfg(test)` / `cfg(any(test` / `cfg(all(test` substrings.

### `resolve_enclosing_function(sema, syntax_root, offset, snap, db) -> (Option<NodeId>, Option<String>)` (private)
**Call graph:** `token_at_offset` -> `Semantics::scope_at_offset` -> `SemanticsScope::containing_function` -> `canonical_function_path` -> `OpenedSnapshot::lookup_by_qualified_name`
**Steps:**
1. Pick a token at `offset` (handling None / Single / Between cases).
2. Compute a scope at that offset using either the token's parent or the file root.
3. Get the containing function via `containing_function`; return `(None, None)` if absent.
4. Build the qualified name and look up its NodeId in the snapshot.

### `resolve_workspace_relative(vfs, file_id, workspace_root) -> Option<String>` (private)
**Call graph:** `Vfs::file_path`
**Steps:**
1. Resolve the absolute path of the file.
2. Strip the workspace-root prefix; return the relative path as a string.

---

## Module: derive_audit

### `AuditOpts`, `DeriveFinding` (structs)
Input/output records for the audit.

### `derive_audit(snap, opts) -> Result<Vec<DeriveFinding>>`
**Call graph:** `read_txn` -> `nodes_by_id.iter` -> `bindings_by_id.iter` -> `missing_required_derives`
**Steps:**
1. Open a read transaction.
2. Linear-scan `nodes_by_id` collecting candidate Item nodes (filtering by crate, kind set, and `::tests::` substring).
3. Walk `bindings_by_id`, keeping `Declared` bindings whose target is in the candidate set, picking the parent-matching row when ambiguous.
4. For each candidate, format visibility (`pub`/`non-pub`) and skip non-pub items if `pub_only`.
5. Call `missing_required_derives` to get current/missing derive sets.
6. Build a sorted `DeriveFinding` and finally sort the output by `(file, span_start)`.

### `extract_derives(attributes) -> HashSet<String>`
**Call graph:** `String::strip_prefix` -> `String::rfind` -> `split(',')` -> `rsplit_once("::")`
**Steps:**
1. Iterate each attribute string and skip those not starting with `#[derive(`.
2. Find the closing `)]` to delimit the inner list.
3. Split by `,`, trim each piece, and strip any path qualifier via `rsplit_once("::")`.
4. Collect non-empty trait names into a set.

### `missing_required_derives(node, kind_filter, pub_only, skip_tests, required) -> Option<(HashSet<String>, HashSet<String>)>`
**Call graph:** `extract_derives`
**Steps:**
1. Reject non-Item nodes, items outside the kind filter, non-pub items (when `pub_only`), and test-module items (when `skip_tests`).
2. Extract the current derive set via `extract_derives`.
3. Compute `required - current` as the missing set.
4. Return `Some((current, missing))` only when missing is non-empty.

### `default_kind_filter() -> HashSet<ItemKind>`
**Call graph:** none
**Steps:**
1. Return `{Struct, Enum, Union}` — the three kinds that accept derive macros.

---

## Module: docs_audit

### `AuditOpts`, `MissingDocsFinding` (structs)
Input/output records.

### `missing_docs_audit(snap, opts) -> Result<Vec<MissingDocsFinding>>`
**Call graph:** `read_txn` -> `nodes_by_id.iter` -> `bindings_by_id.iter` -> `is_undocumented_pub_item`
**Steps:**
1. Open a read transaction.
2. Pass 1: collect candidate Item nodes (kind/crate/test filters).
3. Pass 2: walk `bindings_by_id` to build a target -> `(BindingVisibility, parent_match)` map for the candidates.
4. Pass 3: drop any non-public visibility, populate `node.visibility = "pub"`, then run `is_undocumented_pub_item`.
5. Emit findings and finally sort by `(file, span_start)`.

### `is_undocumented_pub_item(node, kind_filter, skip_tests) -> bool`
**Call graph:** none
**Steps:**
1. Reject non-Item nodes, items outside the kind filter, non-`pub` visibility, and test-module items (when `skip_tests`).
2. Return `true` iff no entry in `node.attributes` starts with `///`.

### `default_kind_filter() -> HashSet<ItemKind>`
**Call graph:** none
**Steps:**
1. Return the standard documentable-kind set: Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, Method.

---

## Module: extract

### `extract(loaded) -> ExtractionModel`
**Call graph:** `workspace_hash` -> `NodeId::from_components` -> `emit_crate` -> `extract_bindings` -> `extract_impl_items` -> `extract_attributes` -> `extract_signatures` -> `extract_statics` -> `extract_usages`
**Steps:**
1. Optionally enable timing via the `EXTRACT_TIMING` env var.
2. Compute the workspace hash and root-Workspace NodeId; insert a Workspace Node.
3. For each local crate, call `emit_crate` to populate Crate and Module nodes.
4. Run `extract_bindings` to produce the initial `def_to_node` map and Binding records.
5. Run `extract_impl_items` to emit Method / AssocConst / AssocType / EnumVariant nodes and extend `def_to_node`.
6. Run `extract_attributes`, `extract_signatures`, `extract_statics`, and `extract_usages` in sequence.
7. Optionally print per-phase timings; return the populated model.

### `emit_crate(...)` (private)
**Call graph:** `crate_display_name` -> `NodeId::from_components` -> `crate_def_map` -> `module_path_segments` -> `model.insert_node` -> `model.insert_contains`
**Steps:**
1. Build the crate's NodeId from `[workspace_hash, "crate", crate_name]`.
2. Insert a Crate node with `parent = workspace_id` and a contains-edge.
3. Iterate every non-block module in the def map.
4. Compute the module's path segments and qualified name.
5. Hash the NodeId, register in `module_node_for`, and determine `parent_id` (crate root, parent module, or fallback to crate).
6. Insert the Module node with appropriate `display_name` and a contains-edge to its parent.

### `crate_display_name(db, krate) -> String` (private)
**Call graph:** `Crate::display_name`
**Steps:**
1. Read `krate.display_name(db).canonical_name()`; fall back to `"unknown_crate"`.

### `module_path_segments(db, def_map, module_id) -> Vec<String>` (private)
**Call graph:** `Module::name` -> `DefMap::containing_module`
**Steps:**
1. Walk parent modules from `module_id` upward, collecting names.
2. Reverse to produce root-down order.

---

## Module: fn_body_audit

### `ALL_PATTERNS` (const)
Static slice of the eight valid pattern names.

### `FnBodyAuditOpts`, `FnBodyFinding`, `RawFinding` (structs)
Input options, output findings, and intermediate per-pattern hits.

### `parse_pattern_filter(input) -> Result<HashSet<&'static str>, String>`
**Call graph:** none
**Steps:**
1. If `None` or empty Vec, return all valid patterns.
2. Otherwise, validate each input name against `ALL_PATTERNS`, erroring on unknown names.

### `match_unwrap(body) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `MethodCallExpr::cast`
**Steps:**
1. Walk descendants; for each `MethodCallExpr` whose `name_ref` is `unwrap`, record a `RawFinding`.

### `match_expect(body) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `MethodCallExpr::cast`
**Steps:**
1. Same shape as `match_unwrap` but matching `expect`.

### `match_panic_macros(body) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `MacroCall::cast` -> `MacroCall::path` -> `Path::segment` -> `Segment::name_ref`
**Steps:**
1. Walk descendants; for each `MacroCall` whose final segment name is one of `panic`/`unreachable`/`todo`/`unimplemented`, record a `RawFinding`.

### `match_unwrap_unchecked(body) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `MethodCallExpr::cast`
**Steps:**
1. Same shape as `match_unwrap` but matching either `unwrap_unchecked` or `unwrap_err_unchecked`.

### `match_unbounded_loop(body) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `LoopExpr::cast` -> `loop_body` -> `body.descendants`
**Steps:**
1. For each `loop { ... }` expression, walk the body looking for any `BreakExpr`, `ReturnExpr`, or `TryExpr`.
2. If none exist, record the loop's text range as a finding.

### `match_await_in_guard_scope(body) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `AwaitExpr::cast` -> ancestor walk for `BlockExpr` -> `BlockExpr::statements` -> `LetStmt::initializer` / `ty`
**Steps:**
1. For each `AwaitExpr`, walk ancestors to find the enclosing block.
2. Iterate let-statements that end before the await offset.
3. Concatenate initializer + type-annotation source text.
4. Match against guard-type needles (`MutexGuard`, `RwLockReadGuard`, `.lock()`, etc.); if found, record the await.

### `match_transmute(body, sema, db) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `CallExpr::cast` -> `resolve_call_to_function` -> `canonical_function_path`
**Steps:**
1. For each path-callee `CallExpr`, resolve via `resolve_call_to_function`.
2. Compute the canonical path; if equal to `std::mem::transmute` or `core::mem::transmute`, record a finding.

### `match_self_recursion(body, self_qualified_name, sema, db) -> Vec<RawFinding>`
**Call graph:** `body.descendants` -> `CallExpr::cast` / `MethodCallExpr::cast` -> `resolve_call_to_function` / `resolve_method_call` -> `canonical_function_path`
**Steps:**
1. For each call/method-call expression, resolve to a HIR `Function`.
2. If the canonical name matches the enclosing fn's qualified name, record a finding.

### `fn_body_audit(loaded, snap, opts) -> Result<Vec<FnBodyFinding>>`
**Call graph:** `attach_db` -> `Semantics::new` -> `crate_def_map` -> `resolve_workspace_relative` -> `sema.parse_guess_edition` -> `enclosing_fn_for_body_offset` -> all `match_*` -> `build_context`
**Steps:**
1. Attach DB, create `Semantics`, and compute crate filter set if given.
2. Collect every local-crate file id.
3. For each file, read text (cached), parse AST, then walk all `Fn` nodes with bodies.
4. Skip test-cfg'd fns when requested.
5. Resolve enclosing function via `enclosing_fn_for_body_offset`.
6. Run each enabled pattern matcher; for self-recursion, supply the enclosing fn's qualified name.
7. Build context strings via `build_context` and emit `FnBodyFinding` records.
8. Sort by `(file, span_start, pattern)`.

### `build_context(file_text, start, end) -> String` (private)
**Call graph:** `str::rfind('\n')` / `str::find('\n')`
**Steps:**
1. Find the previous and next line boundaries around `[start, end)`.
2. Slice the file text to include the surrounding line and one line before/after, trimmed.

### `canonical_function_path(db, func) -> String` (private)
**Call graph:** identical to channel_audit's version
**Steps:** see `channel_audit::canonical_function_path`.

### `enclosing_fn_for_body_offset(sema, syntax_root, offset, snap, db) -> (Option<NodeId>, Option<String>)` (private)
**Call graph:** `token_at_offset` -> `scope_at_offset` -> `containing_function` -> `canonical_function_path` -> `OpenedSnapshot::lookup_by_qualified_name`
**Steps:** see `channel_audit::resolve_enclosing_function` (identical algorithm).

### `enclosed_by_cfg_test`, `item_has_cfg_test`, `resolve_workspace_relative` (private)
Identical to the channel_audit variants.

---

## Module: hir_trim

### `trim_hir_display(s) -> String`
**Call graph:** `String::find` -> `String::replace_range` -> `strip_build_hasher_default` -> `strip_lazy_lock_init_fn`
**Steps:**
1. Iteratively replace `, Global>` with `>` (handles nested `Vec<Vec<T, Global>, Global>`).
2. Iteratively replace `, RandomState>` with `>` to clean up `HashMap` defaults.
3. Apply `strip_build_hasher_default` to remove `, BuildHasherDefault<...>>`.
4. Apply `strip_lazy_lock_init_fn` to remove redundant `LazyLock<X, fn() -> X>` second arg.
5. Defensive: log a `tracing::trace!` if angle brackets are unbalanced.

### `strip_build_hasher_default(s) -> String` (private)
**Call graph:** byte-level state machine
**Steps:**
1. Scan for `, BuildHasherDefault<` occurrences.
2. Walk forward tracking angle-bracket depth (skipping `->` arrow tokens).
3. When the matching `>` lands immediately before another `>`, drop the `, BuildHasherDefault<...>` chunk; otherwise emit verbatim.

### `strip_lazy_lock_init_fn(s) -> String` (private)
**Call graph:** byte-level state machine
**Steps:**
1. Find each `LazyLock<` and walk the inner type, tracking the depth-1 comma position.
2. Compare `lhs` text to `rhs` and only strip if `rhs == "fn() -> <lhs>"`.
3. Otherwise, emit the original generic args verbatim.

---

## Module: ids

### `NodeId`, `BindingId`, `UsageId` (structs)
Newtype wrappers around `[u8; 32]` SHA-256 digests, with bespoke serde for byte-array round-tripping.

### `BindingId::from_components(parts) -> Self`
**Call graph:** `Sha256::new` -> `Sha256::update` -> `Sha256::finalize`
**Steps:**
1. Initialize a SHA-256 hasher.
2. Update with each part's bytes, separated by a NUL byte to prevent prefix collisions.
3. Finalize and copy the 32-byte digest into the newtype.

### `BindingId::to_hex(&self) -> String`
**Call graph:** `hex_encode`
**Steps:**
1. Format the inner bytes as a 64-char lowercase hex string.

### `BindingId::as_bytes(&self) -> &[u8; 32]`
**Call graph:** none
**Steps:**
1. Return a borrow of the inner array.

### `BindingId::fmt` (`Debug` impl)
**Call graph:** `to_hex`
**Steps:**
1. Render `BindingId(<first-12-hex-chars>…)`.

### `UsageId::from_components`, `to_hex`, `as_bytes`, `fmt`
Identical shape to `BindingId`.

### `NodeId::from_components`, `to_hex`, `as_bytes`, `fmt`
Identical shape to `BindingId`.

### `workspace_hash(workspace_root) -> String`
**Call graph:** `Sha256::new` -> `Sha256::update` -> `hex_encode`
**Steps:**
1. SHA-256 hash the workspace root path's UTF-8 bytes.
2. Render as a hex string.

### `hex_encode(bytes) -> String` (private)
**Call graph:** none
**Steps:**
1. Map each byte to two lowercase hex chars and concatenate.

### `serde_bytes_32::serialize` / `deserialize` (private module)
Custom serde adapter that round-trips `[u8; 32]` through `serde_bytes`.

---

## Module: impls

### `extract_impl_items(model, db, vfs, local_crates, crate_node_for, crate_name_for, def_to_node)`
**Call graph:** `attach_db` -> `Semantics::new` -> `Impl::all_in_crate` -> `Impl::trait_` / `self_ty` / `items` -> `emit_assoc_item` -> `Trait::items` -> `Enum::variants` -> `emit_enum_variant`
**Steps:**
1. Snapshot the existing ADT and Trait `def_to_node` mappings (avoids borrow conflict on mutation).
2. Attach the db, create Semantics, and iterate each local crate.
3. For each inherent impl (trait-impl skipped), resolve the host ADT's NodeId and walk every assoc item via `emit_assoc_item`.
4. For each local trait, walk its `trait.items(db)` to emit declaration items via `emit_assoc_item`.
5. For each local enum, iterate variants and emit Item nodes via `emit_enum_variant`.

### `emit_assoc_item(...)` (private)
**Call graph:** `AssocItem::*::name` -> `Definition::try_to_nav` -> `resolve_workspace_relative` -> `NodeId::from_components` -> `model.insert_node` -> `model.insert_contains`
**Steps:**
1. Match on assoc kind to extract `(ItemKind, Definition, ModuleDefId, name)`; skip anonymous consts.
2. Resolve the navigation target via `def.try_to_nav`; bail if absent.
3. Resolve the file path relative to the workspace; bail if outside the workspace.
4. Compute byte offsets and a kind label; hash a NodeId scoped by `[workspace_hash, kind_label, crate, file, byte_offset, name]`.
5. Insert a Method/AssocConst/AssocType Item node with `parent = host` and a contains-edge.
6. Register in `def_to_node` (entry-or-insert) so the usages pass picks up the def.

### `emit_enum_variant(...)` (private)
**Call graph:** `Definition::EnumVariant::try_to_nav` -> `resolve_workspace_relative` -> `NodeId::from_components` -> `model.insert_node` -> `model.insert_contains`
**Steps:**
1. Read the variant's name and resolve its nav target.
2. Build a qualified name `<enum_qual>::<variant_name>` and a NodeId scoped by `[workspace_hash, "enum_variant", crate, file, byte_offset, name]`.
3. Insert an `EnumVariant` Item node parented to the enum and add a contains-edge.
4. Register `ModuleDefId::EnumVariantId(variant_id)` -> NodeId in `def_to_node`.

### `resolve_workspace_relative(vfs, file_id, workspace_root) -> Option<String>` (private)
Identical to other modules' helpers.

---

## Module: loader

### `LoadedWorkspace` (struct)
Holds `workspace_root`, `RootDatabase`, `Vfs`, and the filtered `local_crates` Vec.

### `load(directory) -> Result<LoadedWorkspace>`
**Call graph:** `Path::canonicalize` -> `load_workspace_at` -> `filter_local_crates`
**Steps:**
1. Canonicalize the input directory (returns Err with context on failure).
2. Build a `CargoConfig` (`sysroot=Discover`, `no_deps=false`, `features=All`, `all_targets`, `set_test`).
3. Build a `LoadCargoConfig` with `prefill_caches=true` and parallel DefMap construction.
4. Call `ra_ap_load_cargo::load_workspace_at` to populate db + vfs.
5. Filter to local crates via `filter_local_crates`.
6. Return the assembled `LoadedWorkspace`.

### `filter_local_crates(db) -> Vec<Crate>` (private)
**Call graph:** `Crate::all` -> `Crate::origin`
**Steps:**
1. Enumerate all crates and retain those where `origin(db).is_local()`.

---

## Module: model

### Type definitions
- `NodeKind` enum: `Workspace | Crate | Module | Item | ExternalSymbol`.
- `ItemKind` enum: `Function | Struct | Enum | Union | Trait | TypeAlias | Const | Static | AssocFunction | AssocConst | AssocType | Method | EnumVariant`.
- `Namespace` enum: `Type | Value`.
- `BindingKind` enum: `Declared | NamedImport | GlobImport | ExternCrateImport`.
- `BindingVisibility` enum: `Public | Crate(NodeId) | RestrictedTo(NodeId) | Private`.
- `Node` struct: id, kind, names, parent, kind metadata, file/span, visibility, attributes.
- `Binding` struct: from_module, namespace, name, target, kind, visibility, is_explicit_pub_use.
- `UsageCategory` enum, `Usage` struct.
- `FunctionSignature`, `SelfKind`, `Param`, `GenericBound` (Phase 5 signatures).
- `StaticMetadata` (Phase 7 statics), `EmbeddingRecord` (semantic_overlaps cache).
- `ExtractionModel` struct: nodes, bindings, usages, contains, signatures, statics.

### `ExtractionModel::insert_node(&mut self, node: Node)`
**Call graph:** `BTreeMap::entry`
**Steps:**
1. Insert the node by id only if no entry exists yet (`or_insert`).

### `ExtractionModel::insert_contains(&mut self, parent: NodeId, child: NodeId)`
**Call graph:** none
**Steps:**
1. Push `(parent, child)` onto the contains Vec.

---

## Module: mod (graph::mod)

Module file declares the `pub mod` set and re-exports the public API surface (`extract`, ids, loader, model, queries, snapshot, storage, unsafe_audit). No functions or impls of its own.

---

## Module: queries

### Type definitions
Many `pub struct` records — `DeadPubFinding`, `CrateDeadPub`, `CrateEdge`, `EdgeSymbol`, `ForbiddenDependencyRule`, `ForbiddenDependencyViolation`, `OverlapsReport`, `TypeCollision`, `TypeLocation`, `ModuleShadow`, `WithinCrateDuplicate`, `CommonFnName`, `EnrichedCallSite`, `CallGraphNode`, `RecursiveCallersCount`, `UsageSummaryRow`, `ModuleTreeNode`, `WorkspaceStats`, `NodeKindCounts`, `VisibilityCounts`, `ItemWithAttribute`, `FunctionFilter`, `SelfKindFilter`, `FunctionWithSignature`, `PubTypeAliasMasqueradingAsReexport`, `ReExportLink`, `ReExportChain`, `CrateMetric`, `MutStaticFinding`. Plus consts `MAX_REEXPORT_HOPS` and `MUT_STATIC_PATTERNS`.

### `classify_metadata(meta) -> Vec<&'static str>`
**Call graph:** none
**Steps:**
1. Iterate `MUT_STATIC_PATTERNS`.
2. For the `static mut` row, push if `meta.is_mut`.
3. For other rows, push if `meta.type_string` contains the needle substring.

### `OpenedSnapshot::lookup_by_qualified_name(&self, name) -> Result<Option<(NodeId, Node)>>`
**Call graph:** `lookup_by_qualified_name_inner`
**Steps:**
1. Delegate to the recursive helper with `MAX_REEXPORT_HOPS` budget.

### `OpenedSnapshot::lookup_by_qualified_name_inner(&self, name, hops_remaining)` (private)
**Call graph:** `read_txn` -> `nodes_by_id.iter` -> `bindings_for_from_module`
**Steps:**
1. Phase 1: linear-scan `nodes_by_id` for an exact `qualified_name` match; return on hit.
2. Phase 2: split `name` on the rightmost `::`; recursively resolve the prefix as a re-export facade.
3. Once the prefix module is found, walk its bindings looking for a non-Declared binding whose `visible_name == leaf` and follow its target.
4. Hops-remaining decrements bound the recursion.

### `OpenedSnapshot::node_by_id(&self, rtxn, id) -> Result<Option<Node>>`
**Call graph:** `nodes_by_id.get`
**Steps:**
1. Single-key LMDB lookup by NodeId bytes.

### `OpenedSnapshot::find_root_module_of(&self, crate_id) -> Result<Option<NodeId>>`
**Call graph:** `read_txn` -> `nodes_by_id.get` / `iter`
**Steps:**
1. Verify `crate_id` resolves to a `NodeKind::Crate` node.
2. Linear-scan `nodes_by_id` for a Module whose `parent_id == crate_id` and whose `qualified_name` equals the crate's qualified name.

### `OpenedSnapshot::imports_of(&self, module) -> Result<Vec<Binding>>`
**Call graph:** `bindings_for_from_module`
**Steps:**
1. Iterate bindings whose `from_module == module`, retaining only those with `kind != Declared`.

### `OpenedSnapshot::exports_of(&self, module, consumer) -> Result<Vec<Binding>>`
**Call graph:** `module_ancestors` -> `node_by_id` -> `bindings_for_from_module` -> `is_visible_from`
**Steps:**
1. Build the consumer's module-ancestor set (for `RestrictedTo` checks).
2. Look up the consumer's owning crate.
3. Iterate the module's bindings, filter via `is_visible_from`.

### `OpenedSnapshot::reexports_of(&self, module, consumer) -> Result<Vec<Binding>>`
**Call graph:** `exports_of`
**Steps:**
1. Get the full export set, then drop `Declared` rows.

### `OpenedSnapshot::declared_reexports_of(&self, module) -> Result<Vec<Binding>>`
**Call graph:** `bindings_for_from_module`
**Steps:**
1. Walk the module's bindings, retaining only `kind != Declared && is_explicit_pub_use`.

### `OpenedSnapshot::who_imports(&self, target) -> Result<Vec<Binding>>`
**Call graph:** `bindings_for_target`
**Steps:**
1. Walk bindings whose target is `target`, retaining `kind != Declared`.

### `OpenedSnapshot::usages_of(&self, target) -> Result<Vec<Usage>>`
**Call graph:** `usages_for_target`
**Steps:**
1. Iterate usages indexed by target and collect.

### `OpenedSnapshot::usages_in(&self, consumer_module) -> Result<Vec<Usage>>`
**Call graph:** `usages_for_consumer`
**Steps:**
1. Iterate usages indexed by consumer module and collect.

### `OpenedSnapshot::who_calls(&self, target_fn) -> Result<Vec<EnrichedCallSite>>`
**Call graph:** `usages_for_target` -> `nodes_by_id.get` -> `usage_category_label`
**Steps:**
1. Look up the callee's qualified name.
2. Collect every Usage whose `consumer_function.is_some()`.
3. For each, resolve the caller's qualified name and emit an `EnrichedCallSite` with the file:byte-range and category label.

### `OpenedSnapshot::calls_from(&self, caller_fn) -> Result<Vec<EnrichedCallSite>>`
**Call graph:** `usages_for_consumer_function` -> `nodes_by_id.get` -> `usage_category_label`
**Steps:**
1. Look up the caller's qualified name.
2. Collect every Usage emitted from this caller's body.
3. For each, resolve the callee's qualified name and emit an `EnrichedCallSite`.

### `OpenedSnapshot::call_graph(&self, root_fn, depth) -> Result<CallGraphNode>`
**Call graph:** `call_graph_rec`
**Steps:**
1. Initialize an empty visited set.
2. Recursively expand outgoing call edges via `call_graph_rec`.

### `OpenedSnapshot::call_graph_rec(&self, fn_id, depth, visited)` (private)
**Call graph:** `nodes_by_id.get` -> `usages_for_consumer_function`
**Steps:**
1. Look up the fn's qualified name and crate name.
2. If `fn_id` is already in `visited`, return a `truncated_at_cycle = true` leaf.
3. Otherwise, collect distinct callee NodeIds from `usages_for_consumer_function`.
4. If `depth == 0` and there are callees, return `truncated_at_depth = true`.
5. Recurse into each callee with `depth - 1`.

### `OpenedSnapshot::callers_in_crate(&self, target, crate_qualified) -> Result<Vec<EnrichedCallSite>>`
**Call graph:** `usages_for_target` -> `nodes_by_id.get`
**Steps:**
1. Look up the callee's qualified name.
2. Iterate usages targeting `target`; for each whose caller fn lives in a crate matching `crate_qualified`, emit an `EnrichedCallSite`.

### `OpenedSnapshot::recursive_callers_count(&self, target, depth) -> Result<RecursiveCallersCount>`
**Call graph:** `usages_for_target`
**Steps:**
1. Resolve the target's qualified name; short-circuit on `depth == 0`.
2. Compute hop-1 callers via `usages_for_target`, building the initial frontier and visited set.
3. BFS up to `depth` hops, expanding each frontier node's incoming usages.
4. Detect truncation via a peek pass on the final frontier.
5. Return counts and depth_reached.

### `OpenedSnapshot::who_uses_summary(&self, target) -> Result<Vec<UsageSummaryRow>>`
**Call graph:** `usages_for_target` -> `usage_category_label` -> `nodes_by_id.get`
**Steps:**
1. Aggregate usages by `consumer_module`: total count + per-category map.
2. For each consumer module, resolve qualified name and crate qualified name.
3. Sort by `total_count DESC`, ties by qualified name.

### `OpenedSnapshot::enum_variants(&self, enum_id) -> Result<Vec<Node>>`
**Call graph:** `children_by_parent.get_duplicates` -> `nodes_by_id.get`
**Steps:**
1. Walk the enum's children via DUP_SORT.
2. Filter to `item_kind == EnumVariant`.
3. Sort by `(file, span_start)` (declaration order).

### `OpenedSnapshot::item_attributes(&self, target) -> Result<Vec<String>>`
**Call graph:** `nodes_by_id.get`
**Steps:**
1. Look up the node and return its `attributes` field (empty Vec on miss).

### `OpenedSnapshot::items_with_attribute(&self, crate_id, attr_pattern) -> Result<Vec<ItemWithAttribute>>`
**Call graph:** `nodes_by_id.iter` -> `match_attribute`
**Steps:**
1. Short-circuit empty patterns to an empty result.
2. Linear-scan Item nodes scoped to `crate_id`.
3. For each, find the first attribute that anchor-matches `attr_pattern` (using `match_attribute`), emitting a row with `match_location ∈ {"attr","doc"}`.
4. Sort by qualified name.

### `OpenedSnapshot::function_signature(&self, target) -> Result<Option<FunctionSignature>>`
**Call graph:** `signatures_by_target.get`
**Steps:**
1. Direct LMDB lookup by NodeId.

### `OpenedSnapshot::static_metadata(&self, target) -> Result<Option<StaticMetadata>>`
**Call graph:** `static_metadata_by_target.get`
**Steps:**
1. Direct LMDB lookup by NodeId.

### `OpenedSnapshot::mut_static_audit(&self) -> Result<Vec<MutStaticFinding>>`
**Call graph:** `nodes_by_id.iter` -> `static_metadata_by_target.get` -> `classify_metadata`
**Steps:**
1. Collect every Item node with `item_kind == Static`.
2. For each, fetch its `StaticMetadata` and classify.
3. Emit one finding per matched pattern; sort by `(qualified_name, matched_pattern)`.

### `OpenedSnapshot::functions_with_filter(&self, crate_id, filter) -> Result<Vec<FunctionWithSignature>>`
**Call graph:** `signatures_by_target.iter` -> `nodes_by_id.get` -> `filter_matches`
**Steps:**
1. Linear-scan `signatures_by_target`, joining each row with its node.
2. Skip nodes outside `crate_id`.
3. Apply `filter_matches` predicate.
4. Sort by qualified name.

### `OpenedSnapshot::pub_use_pub_type_audit(&self, crate_id) -> Result<Vec<PubTypeAliasMasqueradingAsReexport>>`
**Call graph:** `nodes_by_id.iter` -> `bindings_for_from_module`
**Steps:**
1. Collect every `TypeAlias` Item in the crate.
2. For each alias, walk its parent module's bindings.
3. Flag bindings that are `is_explicit_pub_use`, share the alias's display name, and target a different node.
4. Sort by alias qualified name.

### `OpenedSnapshot::re_export_chain(&self, target) -> Result<ReExportChain>`
**Call graph:** `bindings_for_target` -> `nodes_by_id.get`
**Steps:**
1. Resolve the canonical qualified name.
2. BFS frontier seeded at `(target, depth=1)` with cycle detection on `(from_module, visible_name)`.
3. For each non-Declared `is_explicit_pub_use` binding, emit a `ReExportLink` and queue the re-exporting module as a downstream target (capped by `MAX_REEXPORT_HOPS`).
4. Sort links by `(depth, module_qualified, visible_name)`.

### `OpenedSnapshot::crate_dependency_metric(&self) -> Result<Vec<CrateMetric>>`
**Call graph:** `crate_edges` -> `nodes_by_id.iter`
**Steps:**
1. Run `crate_edges` to collect cross-crate edges.
2. Linear-scan nodes to build per-crate item counters (total, traits, pub type aliases).
3. Build distinct producer/consumer sets per crate from the edges.
4. Compute `instability = Ce/(Ce+Ca)` and `abstractness = (traits+pub_aliases)/total_items`, NaN-guarded.
5. Sort by crate name.

### `OpenedSnapshot::dead_pub_in_crate(&self, crate_id) -> Result<Vec<DeadPubFinding>>`
**Call graph:** `nodes_by_id.iter` -> `bindings_for_target` -> `usages_for_target`
**Steps:**
1. Collect every Item node in `crate_id`.
2. For each, find its `Declared` binding and require `BindingVisibility::Public`.
3. Skip if any non-Declared importer lives in another crate.
4. Skip if any usage's consumer module lives in another crate.
5. Emit a `DeadPubFinding` and sort by qualified name.

### `OpenedSnapshot::dead_pub_report(&self) -> Result<Vec<CrateDeadPub>>`
**Call graph:** `nodes_by_id.iter` -> `dead_pub_in_crate`
**Steps:**
1. Collect every Crate node id and qualified name; sort by name.
2. Run `dead_pub_in_crate` per crate, packaging the results into `CrateDeadPub` rows.

### `OpenedSnapshot::crate_edges(&self) -> Result<Vec<CrateEdge>>`
**Call graph:** `nodes_by_id.iter` -> `bindings_by_id.iter` -> `usages_by_id.iter` -> `label_node_kind` -> `label_binding_kind`
**Steps:**
1. Build node->crate, crate->name, node->qualified_name, node->kind label maps.
2. Aggregate import counts by `(consumer_crate, producer_crate, target, binding_kind)` skipping intra-crate.
3. Aggregate usage counts by `(consumer_crate, producer_crate, target)`.
4. Merge per-edge per-symbol records (collapsing rows for the same target).
5. Sort each edge's symbols by total ref count desc, then sort edges by `(consumer, producer)`.

### `OpenedSnapshot::forbidden_dependency_check(&self, rules) -> Result<Vec<ForbiddenDependencyViolation>>`
**Call graph:** `crate_edges` -> `glob_match`
**Steps:**
1. Compute `crate_edges`.
2. For each edge × rule pair, test consumer/producer/except glob matches.
3. Emit a violation with the highest-ref-count symbol as `sample_symbol`.
4. Sort by `(rule_index, consumer, producer)`.

### `OpenedSnapshot::unsafe_audit(&self, loaded) -> Result<Vec<UnsafeFinding>>`
**Call graph:** `unsafe_audit::unsafe_audit_impl`
**Steps:**
1. Delegate to the implementation in `super::unsafe_audit`.

### `OpenedSnapshot::overlaps(&self) -> Result<OverlapsReport>`
**Call graph:** `nodes_by_id.iter` -> `label_item_kind`
**Steps:**
1. First pass: build crate-id -> display_name map and crate name set.
2. Second pass: for each node, detect module-shadows-crate-name, group type-kind items by display name, and tally fn-name spread.
3. Filter type collisions to ≥2 distinct crates; emit sorted `TypeCollision` rows.
4. Filter within-crate duplicates to ≥2 entries.
5. Filter common fn names to ≥4 distinct crates.
6. Sort each section deterministically.

### `OpenedSnapshot::module_tree(&self, crate_name, depth) -> Result<ModuleTreeNode>`
**Call graph:** `nodes_by_id.iter` -> `bindings_by_id.iter` -> `format_binding_visibility` -> `build_module_tree`
**Steps:**
1. Find the `Crate` node with matching qualified name.
2. Pre-build a per-Item visibility map by joining the crate's items with their `Declared` bindings.
3. Format each `BindingVisibility` to a string via `format_binding_visibility`.
4. Recursively build the tree via `build_module_tree`.

### `OpenedSnapshot::build_module_tree(&self, rtxn, node_id, depth_limit, cur_depth, item_visibility)` (private)
**Call graph:** `nodes_by_id.get` -> `children_by_parent.get_duplicates` -> recursion -> `label_node_kind` / `label_item_kind`
**Steps:**
1. Look up the current node.
2. Unless depth_limit reached, walk children (collecting ids first to drop the iterator borrow), recurse, and sort by display name.
3. For Item nodes, fetch visibility from the precomputed map; otherwise use the node's own `visibility` field.

### `OpenedSnapshot::workspace_stats(&self) -> Result<WorkspaceStats>`
**Call graph:** `nodes_by_id.iter` -> `bindings_by_id.iter` -> `label_item_kind` -> `label_binding_kind`
**Steps:**
1. Iterate nodes, incrementing kind counters and per-ItemKind tallies.
2. Iterate bindings, incrementing per-BindingKind tallies and (for Declared rows) per-visibility counts.
3. Mirror `private` -> `pub_self` for compatibility.
4. Compute `pub_crate_share = pub_crate / (pub_ + pub_crate)`, NaN-guarded.

### Helper iterators (private)
- `bindings_for_from_module`: DUP_SORT iterator that resolves each `BindingId` to a full `Binding`.
- `bindings_for_target`: same shape, keyed by target NodeId.
- `usages_for_target`: DUP_SORT iterator resolving `UsageId` to `Usage`.
- `usages_for_consumer`: same, keyed by consumer module.
- `usages_for_consumer_function`: same, keyed by caller fn NodeId.
- `module_ancestors`: walk `parent_id` upward into a `HashSet<NodeId>` with cycle guard.

### Free helpers (private)
- `label_node_kind(node) -> String`: stringify NodeKind, including `Item.<kind>`.
- `label_item_kind(k) -> &'static str`: stringify ItemKind.
- `label_binding_kind(k) -> &'static str`: stringify BindingKind.
- `glob_match(pattern, text) -> bool`: linear glob matcher with `*` wildcards (anchored start, anchored end, between via substring search).
- `usage_category_label(c) -> &'static str`: stringify UsageCategory.
- `match_attribute(attr, pat) -> Option<&'static str>`: anchored prefix match — returns `"attr"` when `pat` is a prefix of `attr`, `"doc"` when it's a prefix of the body of a `///` doc comment.
- `format_binding_visibility(rtxn, snap, vis) -> String`: render BindingVisibility as `"pub"`, `"pub(crate)"`, `"pub(in path)"`, or `"pub(self)"`.
- `filter_matches(filter, sig) -> bool`: predicate combining `is_async`/`min_param_count`/`has_param_type`/`returns_type_pattern`/`self_kind` filters.
- `is_visible_from(vis, consumer_crate, consumer_ancestry) -> bool`: visibility check used by `exports_of`.

---

## Module: recursion_check

### `RecursionOpts`, `RecursionCycleInternal` (structs), `HARD_CAP_CYCLE_LENGTH`, `DEFAULT_CYCLE_LENGTH` (consts)

### `clamp_cycle_length(requested) -> usize`
**Call graph:** none
**Steps:**
1. Default to `DEFAULT_CYCLE_LENGTH` if `None`.
2. Clamp into `[1, HARD_CAP_CYCLE_LENGTH]`.

### `recursion_check(snap, opts) -> Result<Vec<RecursionCycleInternal>>`
**Call graph:** `signatures_by_target.iter` -> `nodes_by_id.get` -> `usages_by_consumer_function.get_duplicates` -> `usages_by_id.get` -> `find_cycles_from` -> `canonicalize_cycle`
**Steps:**
1. Build the function set from `signatures_by_target`, capturing qualified names and the in-scope subset (per crate filter).
2. For each fn, build an adjacency list of distinct callees from `usages_by_consumer_function`.
3. Drop the read txn (so the closure passed to `find_cycles_from` is independent).
4. For each fn, enumerate cycles via DFS bounded by `max_cycle_length`.
5. Canonicalize each cycle and dedup against a set.
6. Filter cycles by crate-scope (any node in scope) when a filter is set.
7. Sort by `(cycle_length, first-fn-qualified-name)`.

### `find_cycles_from<F>(start, max_depth, outgoing_edges) -> Vec<Vec<NodeId>>`
**Call graph:** `dfs`
**Steps:**
1. Bail on `max_depth == 0`.
2. Initialize `path = [start]` and `on_path = {start}`.
3. Run DFS via `dfs`.

### `dfs<F>(start, outgoing, max_depth, path, on_path, out)` (private)
**Call graph:** recursion
**Steps:**
1. Bail if path exceeds `max_depth`.
2. For each outgoing neighbor of `path.last()`:
   - If `next == start`, push the current path as a cycle.
   - If `next` is already on the path, skip.
   - If at depth limit, skip.
   - Otherwise extend the path and recurse, popping on return.

### `canonicalize_cycle(cycle) -> Vec<NodeId>`
**Call graph:** `Vec::rotate_left`
**Steps:**
1. Locate the index of the lexicographically smallest NodeId.
2. Rotate the cycle so that id appears first.

### `enclosing_fn_qualified_names(snap, cycle) -> Result<Vec<String>>`
**Call graph:** `read_txn` -> `nodes_by_id.get`
**Steps:**
1. For each NodeId in the cycle, look up its qualified name.

---

## Module: signatures

### `extract_signatures(model, db, _vfs, def_to_node)`
**Call graph:** `attach_db` -> `Function::from` -> `Function::krate` -> `Crate::to_display_target` -> `build_signature`
**Steps:**
1. Attach the db.
2. For each `(def_id, node_id)` whose def is a `FunctionId`, instantiate `Function`.
3. Cache the per-crate `DisplayTarget`.
4. Call `build_signature`; on success, push `(node_id, FunctionSignature)` onto `model.signatures`.

### `build_signature(db, dt, func) -> Option<FunctionSignature>` (private)
**Call graph:** `Function::is_async` -> `Function::self_param` -> `Access` -> `Function::params_without_self` -> `Param::ty` -> `Type::as_reference` -> `trim_hir_display` -> `Function::ret_type` -> `GenericDef::type_or_const_params` -> `TypeOrConstParam::as_type_param` -> `TypeParam::trait_bounds`
**Steps:**
1. Read `is_async`.
2. Map `self_param.access(db)` to `SelfKind` (Owned/Ref/RefMut).
3. For each non-self param, build a `Param` with name, trimmed HirDisplay type string, by_ref/mutability flags.
4. Compute the return type via `func.ret_type(db).display(db, dt)` and pass through `trim_hir_display`.
5. For each generic type param (skipping implicit ones), collect declaration-site trait bounds.
6. Return the assembled `FunctionSignature`.

---

## Module: snapshot

### `BuildOptions` (struct), `Default` impl, `BuildResult` (struct)

### `build_and_persist(directory, options) -> Result<BuildResult>`
**Call graph:** `loader::load` -> `GraphPaths::for_workspace*` -> `compute_fingerprint` -> `graph_id_for` -> `read_manifest` -> `EnvOpenOptions::open` -> `extract::extract` -> `write_model` -> `write_manifest` -> `publish_current` -> `now_unix`
**Steps:**
1. Optionally enable timing.
2. Load the workspace via `loader::load`.
3. Build the `GraphPaths` (with optional override), ensure dirs.
4. Compute the workspace fingerprint and graph id.
5. If a matching manifest exists and `force_rebuild` is false, return a reused `BuildResult`.
6. Clear/create the snapshot directory.
7. Open the heed env, run extraction.
8. Call `write_model` to persist all sub-DBs.
9. Build and write the manifest, publish via atomic `CURRENT` rename.
10. Return the new `BuildResult`.

### `persist_loaded(loaded, options) -> Result<BuildResult>`
**Call graph:** `GraphPaths::for_workspace*` -> `compute_fingerprint` -> `graph_id_for` -> `EnvOpenOptions::open` -> `extract::extract` -> `write_model` -> `write_manifest` -> `publish_current` -> `now_unix`
**Steps:**
1. Same as `build_and_persist` but skips `loader::load` and the reuse short-circuit (always rewrites).

### `write_model(env, _env_opts, model, workspace_hash, fingerprint, graph_id) -> Result<(u64, u64, u64)>` (private)
**Call graph:** `Env::write_txn` -> `GraphDatabases::create` -> `Database::put` -> `binding_id_for` -> `usage_id_for`
**Steps:**
1. Open a write txn; instantiate sub-dbs via `GraphDatabases::create`.
2. Write every `Node` keyed by NodeId.
3. Write every `Binding`: primary record + `bindings_by_from_module` + `bindings_by_target` index entries.
4. Write `(parent, child)` edges into `children_by_parent`.
5. Write every `Usage`: primary + per-target/per-consumer/per-consumer-function indexes.
6. Write `signatures_by_target` and `static_metadata_by_target` rows.
7. Write meta rows (workspace_hash, fingerprint, graph_id, schema_version, counts).
8. Commit; return `(node_count, binding_count, usage_count)`.

### `binding_id_for(binding) -> BindingId`
**Call graph:** `BindingId::from_components`
**Steps:**
1. Build a 4-tuple `[from_module_hex, namespace_label("T"|"V"), visible_name, target_hex]`.
2. SHA-256 it via `BindingId::from_components`.

### `usage_id_for(u) -> UsageId`
**Call graph:** `UsageId::from_components`
**Steps:**
1. Build a 6-tuple `[target_hex, consumer_module_hex, file, start, end, category_label]`.
2. SHA-256 it via `UsageId::from_components`.

### `publish_current(paths, graph_id)` (private)
**Call graph:** `fs::write` -> `fs::rename`
**Steps:**
1. Write the graph id to a temp file inside `root_dir`.
2. Atomically rename to `CURRENT`.

### `now_unix() -> Result<u64>` (private)
**Call graph:** `SystemTime::now` -> `duration_since`
**Steps:**
1. Compute seconds since `UNIX_EPOCH`.

### `OpenedSnapshot` (struct)

### `OpenedSnapshot::read_txn(&self) -> Result<GraphRoTxn<'_>>`
**Call graph:** `Env::read_txn`
**Steps:** open a read transaction.

### `OpenedSnapshot::write_txn(&self) -> Result<GraphRwTxn<'_>>`
**Call graph:** `Env::write_txn`
**Steps:** open a write transaction.

### `OpenedSnapshot::node(&self, txn, id) -> Result<Option<Node>>`
**Call graph:** `nodes_by_id.get`
**Steps:** single-key LMDB lookup.

### `open_current(paths, env) -> Result<Option<OpenedSnapshot>>`
**Call graph:** `fs::read_to_string` -> `open_specific`
**Steps:**
1. Bail if no `CURRENT` pointer exists.
2. Read the graph id from the pointer.
3. Delegate to `open_specific`.

### `open_specific(paths, graph_id, env_opts) -> Result<Option<OpenedSnapshot>>`
**Call graph:** `read_manifest_compatible` -> `EnvOpenOptions::open` -> `Env::read_txn` -> `GraphDatabases::open` -> `RoTxn::commit`
**Steps:**
1. Bail (`Ok(None)`) on missing manifest or schema mismatch.
2. Verify `data.mdb` exists; otherwise raise `bail!`.
3. Open the heed env.
4. Open all sub-databases inside a read txn and commit it (registering dbi handles).
5. Return the assembled `OpenedSnapshot`.

---

## Module: statics

### `extract_statics(model, db, _vfs, def_to_node)`
**Call graph:** `attach_db` -> `Static::from` -> `Static::krate` -> `Crate::to_display_target` -> `Static::ty` -> `Type::display` -> `trim_hir_display` -> `Static::is_mut`
**Steps:**
1. Attach the db.
2. For each `(def_id, node_id)` whose def is a `StaticId`, instantiate `Static`.
3. Cache the per-crate `DisplayTarget`.
4. Stringify the static's type via `HirDisplay`, trim std defaults via `trim_hir_display`.
5. Skip if the resulting type string is empty.
6. Push `(node_id, StaticMetadata { type_string, is_mut })` onto `model.statics`.

---

## Module: storage

### `SCHEMA_VERSION` (const, currently 11)
### `CURRENT_POINTER_FILENAME`, `SNAPSHOTS_DIRNAME`, `MANIFEST_FILENAME` (consts)
### `GraphEnvOptions` (struct), `Default` impl

### `GraphEnvOptions::to_open_options(self) -> EnvOpenOptions<WithoutTls>`
**Call graph:** `EnvOpenOptions::new` -> `read_txn_without_tls` -> `map_size` / `max_dbs` / `max_readers`
**Steps:**
1. Build a `WithoutTls` env opens-options struct with the configured map_size / max_dbs / max_readers.

### `GraphPaths` (struct)

### `GraphPaths::for_workspace(workspace_root) -> Self`
**Call graph:** `default_data_dir` -> `for_workspace_in`
**Steps:**
1. Look up the OS-default data dir and forward to `for_workspace_in`.

### `GraphPaths::for_workspace_in(base_dir, workspace_root) -> Self`
**Call graph:** `super::ids::workspace_hash`
**Steps:**
1. Compute the workspace hash.
2. Build `root_dir`, `current_pointer_path`, and `snapshots_dir` paths.

### `GraphPaths::snapshot_dir(&self, graph_id) -> PathBuf`
**Call graph:** none
**Steps:** join `snapshots_dir / graph_id`.

### `GraphPaths::manifest_path(&self, graph_id) -> PathBuf`
**Call graph:** `snapshot_dir`
**Steps:** join `snapshot_dir / "manifest.json"`.

### `GraphPaths::ensure_dirs(&self) -> std::io::Result<()>`
**Call graph:** `fs::create_dir_all`
**Steps:** create `snapshots_dir` (parents implicit).

### `default_data_dir() -> PathBuf`
**Call graph:** `ProjectDirs::from`
**Steps:**
1. Resolve the platform's data dir under `dev/rust-code-mcp/search/graphs`.
2. Fall back to a relative `.rust-code-mcp/graphs` if `ProjectDirs` is unavailable.

### `compute_fingerprint(workspace_root) -> Result<String>`
**Call graph:** `WalkDir::new` -> `fs::read` -> `Sha256::new/update/finalize`
**Steps:**
1. Walk every file under `workspace_root` excluding `target/` and `.git/`.
2. For each `.rs` / `Cargo.toml` / `Cargo.lock` file, hash its contents.
3. Sort by relative path, then hash `(rel_path, NUL, file_digest, NUL)` of every entry into a single SHA-256.
4. Return the 64-char hex digest.

### `graph_id_for(workspace_hash, fingerprint) -> String`
**Call graph:** `Sha256::new/update/finalize`
**Steps:**
1. Hash `[workspace_hash, NUL, fingerprint, NUL, SCHEMA_VERSION_le_bytes]`.
2. Return the first 16 bytes as a 32-char hex string.

### `GraphDatabases` (struct)
Holds `Database` handles for `meta_by_key`, `nodes_by_id`, `bindings_*`, `children_by_parent`, `usages_*`, `signatures_by_target`, `static_metadata_by_target`, `embeddings_by_target`.

### `GraphDatabases::create(env, wtxn) -> Result<Self>`
**Call graph:** `open_or_create_str_bytes` -> `open_or_create_bytes_bincode` -> `open_or_create_bytes_bytes`
**Steps:**
1. Create each sub-database with the configured key/value types and DUP_SORT flag where applicable.

### `GraphDatabases::open(env, rtxn) -> Result<Option<Self>>`
**Call graph:** `Env::open_database`
**Steps:**
1. Try to open `meta_by_key`; return `None` if absent.
2. Open every other sub-db, contextualizing missing-db errors.

### `open_or_create_str_bytes(env, wtxn, name, dup_sort) -> Result<Database<Str, Bytes>>` (private)
**Call graph:** `Env::database_options` -> `DatabaseFlags::DUP_SORT` -> `create`
**Steps:**
1. Configure types and optional DUP_SORT flag, then create.

### `open_or_create_bytes_bincode<T>(env, wtxn, name, dup_sort)` (private generic)
**Call graph:** same shape as `open_or_create_str_bytes`
**Steps:**
1. Same shape, parameterized over the bincode value type.

### `open_or_create_bytes_bytes(env, wtxn, name, dup_sort)` (private)
**Call graph:** same shape
**Steps:**
1. Same shape with `Bytes` for both key and value.

### `GraphManifest` (struct, serde)

### `write_manifest(path, manifest) -> Result<()>`
**Call graph:** `serde_json::to_string_pretty` -> `fs::write`
**Steps:**
1. Pretty-print the manifest as JSON and write atomically.

### `read_manifest(path) -> Result<GraphManifest>`
**Call graph:** `fs::read` -> `serde_json::from_slice`
**Steps:**
1. Read and parse the manifest JSON.
2. Return an error if `schema_version` mismatches.

### `read_manifest_compatible(path) -> Result<Option<GraphManifest>>`
**Call graph:** `fs::read` -> `serde_json::from_slice` -> `tracing::warn!`
**Steps:**
1. Read and parse like `read_manifest`.
2. Return `Ok(None)` (with a warn log) on schema mismatch instead of erroring.

### `_binding_id_marker(_)` (private dead-code marker)
No-op function used only to keep the `BindingId` import alive.

---

## Module: unsafe_audit

### `UnsafeFinding` (struct)

### `unsafe_audit_impl(loaded, snap) -> Result<Vec<UnsafeFinding>>`
**Call graph:** `attach_db` -> `Semantics::new` -> `crate_def_map` -> `resolve_workspace_relative` -> `fs::read_to_string` -> `sema.parse_guess_edition` -> `BlockExpr::cast` -> `unsafe_token` -> `has_safety_comment_in_preceding_lines` -> `token_at_offset` -> `scope_at_offset` -> `containing_function` -> `OpenedSnapshot::lookup_by_qualified_name`
**Steps:**
1. Attach the db and create `Semantics`.
2. Collect every `FileId` reachable from local crates' modules.
3. For each file, read text and parse AST.
4. For each `BlockExpr` with an `unsafe_token`, compute span, line count, and SAFETY-comment heuristic.
5. Resolve enclosing fn via `scope_at_offset` + `containing_function`; build the qualified name and look up its NodeId.
6. Push an `UnsafeFinding`.
7. Sort by `(file, span_start)`.

### `has_safety_comment_in_preceding_lines(text, unsafe_offset) -> bool` (pub(crate))
**Call graph:** `str::rfind('\n')` -> `str::split('\n')`
**Steps:**
1. Locate the start of the line containing `unsafe`.
2. Take the preceding text up to that line.
3. Examine up to the last 5 lines.
4. Return `true` iff any contains the literal `SAFETY`.

### `resolve_workspace_relative(...)` (private)
Identical helper used elsewhere.

---

## Module: usages

### `extract_usages(model, db, vfs, def_to_node, module_node_for)`
**Call graph:** `attach_db` -> `Semantics::new` -> `ModuleDef::from` -> `Definition::*` -> `Definition::try_to_nav` -> `resolve_workspace_relative` -> `Definition::usages` -> `Sema::parse` -> `ReferenceCategory::contains` -> `Semantics::scope_at_offset` -> `SemanticsScope::module` / `containing_function` -> `classify_category`
**Steps:**
1. Attach the db and create `Semantics`.
2. For each `(def_id, target_node_id)`, retain only Item nodes; convert to `Definition`.
3. Backfill the Item's `file` and `span` from the canonical declaration's nav target.
4. Call `def.usages(&sema).all()` to iterate every reference.
5. For each per-file batch, resolve the file path; skip non-workspace files.
6. For each reference, drop IMPORT-category refs.
7. Resolve `consumer_module` via the file-root scope.
8. Resolve `consumer_function` by re-scoping at the token's parent and asking for `containing_function`.
9. Emit a `Usage` row with target / consumer module / file / span / category / consumer_function.

### `classify_category(c) -> UsageCategory` (private)
**Call graph:** `ReferenceCategory::contains`
**Steps:**
1. Map bitflags by precedence: Write > Read > Test > Other.

### `resolve_workspace_relative(...)` (private)
Identical helper.
