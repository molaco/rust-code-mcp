# Section H — P1.5c modify_signature + P1.5d extract/inline + P1.5e module ops

## Overview

This slice completes the structural CRUD surface that P1.5a/b opened, by adding the nine remaining verbs: `modify_signature` (P1.5c), `extract_function` / `extract_trait` / `inline` (P1.5d), and `split_module` / `merge_modules` / `create_module` / `move_module` / `lift_to_crate` / `lower_to_module` (P1.5e). All ride the apply==rebuild engine and Checkpoint envelope built in Section G. P1.4 simulate is layered on top by reusing `Crud::compute_effects()` and skipping `persist()`.

**M2b work** (CRUD expansion after M3 proves the loop on `modify_body` alone). **P1.5c carries the highest correctness risk in the entire CRUD surface**: when a parameter is added to a fn signature, every callsite suddenly under-supplies arguments. The question of what to put in their place — refuse, silently fill with `Default::default()`, or insert `todo!()` — is a real semantic call. We pick **`todo!()` as the default policy** (encoded in `CallsiteFill::Todo`): keeps the workspace compiling against the type system, makes intent visible to downstream grep / cargo-check, avoids the silent-change failure mode of `Default`. The other tricky item is **`lift_to_crate`** which mutates workspace `Cargo.toml` + a member `Cargo.toml` — D2 classifies Cargo edits as **full-rebuild** class, so `lift_to_crate` and `lower_to_module` are flagged as **high-cost** verbs.

## New modules / files

All in `crates/rmc-crud/src/`:

- `crates/rmc-crud/src/modify_signature.rs` — P1.5c (sig rewrite + callsite synthesis).
- `crates/rmc-crud/src/extract_function.rs` — P1.5d extract code range → new fn + call.
- `crates/rmc-crud/src/extract_trait.rs` — P1.5d hoist inherent-impl methods into new trait + `impl Trait for T`.
- `crates/rmc-crud/src/inline.rs` — P1.5d inverse of extract_function.
- `crates/rmc-crud/src/split_module.rs` — P1.5e partition module items into N sibling modules.
- `crates/rmc-crud/src/merge_modules.rs` — P1.5e fold N modules into one.
- `crates/rmc-crud/src/create_module.rs` — P1.5e add empty/item-seeded child module.
- `crates/rmc-crud/src/move_module.rs` — P1.5e relocate/rename whole module file + update `use` paths.
- `crates/rmc-crud/src/lift_to_crate.rs` — P1.5e promote module to new workspace crate.
- `crates/rmc-crud/src/lower_to_module.rs` — P1.5e inverse: fold small workspace crate back.
- `crates/rmc-crud/src/callsite_fill.rs` — `CallsiteFill` enum + `CallsiteCtx`.
- `crates/rmc-crud/src/cargo_surgery.rs` — `toml_edit`-based reads/writes of `Cargo.toml`; format-preserving.
- `crates/rmc-crud/src/syn_ast.rs` — common helpers for `syn`/`ra_ap_syntax` parse + **byte-range location only** (signature span, arg-list span, impl-block item ranges). **No `prettyplease::unparse`** — replacement text is string-built and spliced via `source_edit::splice_bytes` per E5 (see Canonical Reconciliation §R4).
- `crates/rmc-crud/src/name_resolution.rs` — thin wrapper around RA's `Semantics` for capture analysis in `extract_function`.

`crates/rmc-crud/src/lib.rs` re-exports new verbs; `facade.rs` gains nine methods; `edit.rs` gains the `Cargo` variant + `is_full_rebuild()`; `error.rs` gains new variants.

New deps in `crates/rmc-crud/Cargo.toml` (E5: analysis-only — no `printing`,
no `prettyplease`, no `quote`/`proc-macro2` codegen; build replacement
strings by hand and splice):
```
syn = { version = "2", features = ["full", "parsing", "extra-traits", "visit", "visit-mut"] }
toml_edit = "0.22"
cargo_metadata = { workspace = true }
ra_ap_hir = "0.0.330"
ra_ap_ide_db = { workspace = true }
ra_ap_syntax = { workspace = true }
ra_ap_vfs = "0.0.330"
rmc-graph = { path = "../rmc-graph" }
rmc-server = { path = "../rmc-server" }
```

## Type definitions

### callsite_fill.rs

```rust
pub enum CallsiteFill {
    /// DEFAULT: `todo!("filled in by modify_signature: <param>")`. Compiles,
    /// runtime-panics if reached, easy to grep.
    Todo,
    /// `Default::default()` if param type implements Default (best-effort
    /// scan; falls back to Todo if unknown).
    Default,
    /// Refuse the op entirely → `EditError::SignatureSynthesisRefused`.
    Refuse,
    /// Caller-supplied builder; receives (callsite, new_param), returns string spliced verbatim.
    ClosureBuilder(Box<dyn Fn(&CallsiteCtx) -> String + Send + Sync>),
}

impl Default for CallsiteFill { fn default() -> Self { CallsiteFill::Todo } }

pub struct CallsiteCtx<'a> {
    pub fn_id: NodeId,
    pub added_param: &'a Param,
    pub call_site_file: &'a str,
    pub call_site_byte: u32,
    pub caller_fn: Option<NodeId>,
}
```

### modify_signature.rs

```rust
pub struct SignatureChange {
    pub target: NodeId,
    pub new_sig: FunctionSignature,     // ENTIRE new sig, not a delta
    pub callsite_fill: CallsiteFill,    // default Todo
}

pub(crate) struct SignatureDelta {
    pub added:    Vec<(usize, Param)>,
    pub removed:  Vec<usize>,
    pub renamed:  Vec<(usize, String)>,
    pub retyped:  Vec<(usize, String)>,
    pub reordered: Option<Vec<usize>>,
    pub self_changed: bool,
    pub return_changed: bool,
    pub generics_changed: bool,
    pub async_changed: bool,
}
```

### extract_function.rs / extract_trait.rs / inline.rs

```rust
pub struct ExtractFunctionOp {
    pub source_fn: NodeId,
    pub byte_range: (u32, u32),               // inside source_fn's file
    pub new_fn_name: String,
    pub captured_locals: Vec<String>,         // hint; empty = auto-detect
    pub new_fn_visibility: BindingVisibility,
}

pub struct ExtractTraitOp {
    pub source_struct: NodeId,
    pub method_subset: Vec<NodeId>,
    pub trait_name: String,
    pub trait_visibility: BindingVisibility,
    pub place_trait_inline: bool,
}

pub struct InlineOp { pub target_fn: NodeId, pub policy: InlinePolicy }
pub enum InlinePolicy { InlineAll, InlineSites(Vec<UsageId>) }
```

### split_module / merge_modules / create_module / move_module

```rust
pub struct SplitModuleOp { pub source_module: NodeId, pub splits: Vec<ModuleSplit> }
pub struct ModuleSplit {
    pub new_name: String,
    pub items: Vec<NodeId>,
    pub keep_reexport: bool,
}
pub struct MergeModulesOp { pub sources: Vec<NodeId>, pub dest: NodeId }
pub struct CreateModuleOp {
    pub parent: NodeId,
    pub name: String,
    pub initial_items: Vec<NodeId>,
    pub use_mod_rs: bool,
}
pub struct MoveModuleOp {
    pub source_module: NodeId,
    pub new_parent: NodeId,
    pub new_name: Option<String>,
}
```

### lift_to_crate / lower_to_module

```rust
pub struct LiftToCrateOp {
    pub source_module: NodeId,
    pub new_crate_name: String,    // kebab-case
    pub edition: String,            // "2021" / "2024"
    pub keep_facade: bool,
}

pub struct LowerToModuleOp {
    pub source_crate: NodeId,
    pub dest_parent_module: NodeId,
    pub new_module_name: Option<String>,
}
```

### Crud methods

```rust
impl Crud {
    pub fn modify_signature(&mut self, op: SignatureChange) -> Result<EditOutcome, EditError>;
    pub fn extract_function(&mut self, op: ExtractFunctionOp) -> Result<EditOutcome, EditError>;
    pub fn extract_trait(&mut self, op: ExtractTraitOp) -> Result<EditOutcome, EditError>;
    pub fn inline(&mut self, op: InlineOp) -> Result<EditOutcome, EditError>;
    pub fn split_module(&mut self, op: SplitModuleOp) -> Result<EditOutcome, EditError>;
    pub fn merge_modules(&mut self, op: MergeModulesOp) -> Result<EditOutcome, EditError>;
    pub fn create_module(&mut self, op: CreateModuleOp) -> Result<EditOutcome, EditError>;
    pub fn move_module(&mut self, op: MoveModuleOp) -> Result<EditOutcome, EditError>;
    pub fn lift_to_crate(&mut self, op: LiftToCrateOp) -> Result<EditOutcome, EditError>;
    pub fn lower_to_module(&mut self, op: LowerToModuleOp) -> Result<EditOutcome, EditError>;
}
```

### New `EditError` variants

```rust
SignatureSynthesisRefused { fn_id: NodeId, callsite_count: usize },
CargoTomlConflict { crate_name: String, reason: String },
ExtractFunctionScopeCapture { unresolved: Vec<String> },
ExtractTraitMethodsNotInherent { stray: Vec<NodeId> },
ItemsNotInModule { module: NodeId, stray: Vec<NodeId> },
ModuleTreeConflict { parent: NodeId, name: String, reason: String },
InlineRecursiveFn { fn_id: NodeId },
```

### EditOutcome extension

```rust
pub struct EditOutcome {
    pub file_edits: Vec<FileEdit>,
    pub file_moves: Vec<FileMove>,
    pub cargo_edits: Vec<CargoEdit>,    // NEW; usually empty
    pub class: EditClass,
    pub affected_items: Vec<NodeId>,
    pub checkpoint: Checkpoint,
}

pub struct CargoEdit {
    pub manifest_path: PathBuf,
    pub new_contents: String,            // toml_edit-rendered, format-preserved
}

pub enum EditClass {
    Body, SigOrVis, ItemAddRemove, ModuleTree, Macro,
    Cargo,                               // → COLD REBUILD
}
impl EditClass {
    pub fn is_full_rebuild(self) -> bool { matches!(self, Self::Cargo | Self::Macro) }
}
```

## Step-by-step implementation

Each verb's skeleton:
```
1. take Checkpoint
2. resolve/validate → EditError on bad input
3. compute_effects()  → FileEdits + FileMoves + CargoEdits + EditClass
4. apply_to_disk()
5. host.apply_edits()
6. on error → Checkpoint::restore() → bubble EditError
7. else → return EditOutcome
```

### P1.5c — `modify_signature`

**Step 1 — Resolve + validate.** Require `item_kind ∈ {Function, Method, AssocFunction}`. Fetch current sig via `snap.function_signature(op.target)?`. VERIFY: `modify_sig_rejects_non_fn`.

**Step 2 — Diff old vs new sig.** `SignatureDelta`: pair by position, refine by name where positions don't match. Same-name+different-ty → `retyped`. New name not in old → `added`. Old name not in new → `removed`. Same set different order → `reordered = Some(permutation)`. VERIFY: `diff_detects_add_remove_rename_reorder`.

**Step 3 — Rewrite the function declaration.** Read file, `syn::parse_file(&src)?`. Locate `ItemFn`/`ImplItemFn`/`TraitItemFn` by span (via `visit_mut::VisitMut`). Replace its `syn::Signature` with translated `FunctionSignature`:
- `is_async` → `sig.asyncness`.
- `self_param` → `sig.inputs.first_mut()` set to `FnArg::Receiver`.
- `params` → `FnArg::Typed(PatType { pat: Ident, ty: syn::parse_str(&p.ty)?, .. })`.
- `return_type` → `syn::parse_str(&format!("-> {}", new_sig.return_type))?`.
- `generics` → `syn::parse_str(&render_generics(&new_sig.generics))?`.

Re-render: `let new_src = prettyplease::unparse(&file);`. One `FileEdit { path, new_contents: new_src }`. VERIFY: `modify_sig_rewrites_decl_only`.

**Step 4 — Find all callsites of OLD sig.** `let sites = snap.who_calls(op.target)?;` + `snap.usages_of(op.target)?` for non-fn-body refs. Union is the rewrite set. Tag with body-call vs const-ref (Default-substitution only valid in body context). VERIFY: `modify_sig_collects_all_callsites`.

**Step 5 — Rewrite each callsite.** Group by file; descending byte-offset order. For each file: parse `syn::File`; for each site (visit_mut to find topmost `ExprCall`/`ExprMethodCall` containing the offset); manipulate `call.args`:
- **Reorder:** permute `args` per `perm`.
- **Remove:** `args.remove(i)` for each `removed` (descending).
- **Add:** for each `(j, new_param)`, build `syn::Expr` per `callsite_fill`:
  - `Todo` → `syn::parse_str::<syn::Expr>(&format!(r#"todo!("filled in by modify_signature: {}")"#, new_param.name))?`.
  - `Default` → small allowlist `{i*, u*, f*, bool, String, Vec<_>, Option<_>, HashMap<_,_>}` or `: Default` bound in `new_sig.generics`; else fall back to `Todo`.
  - `Refuse` → `EditError::SignatureSynthesisRefused { fn_id, callsite_count }`.
  - `ClosureBuilder(f)` → `syn::parse_str(&f(&ctx))?`.

Insertion order: removals/reorder first, then insertions in increasing index. Re-render per touched file; emit `FileEdit`. VERIFY: `modify_sig_add_param_inserts_todo`, `_remove_param_drops_arg`, `_reorder_perm_correct`.

**Step 6 — Classify + apply.** `EditClass::SigOrVis` (D2 expands to editing crate + reverse-deps). `Crud::take_checkpoint()` → `Crud::apply_file_edits(edits, class)` → host writes + LMDB patch + return `EditOutcome`. On error → `Checkpoint::restore()`.

### P1.5d — `extract_function`

**Step 7 — Parse + locate range.** Resolve `source_fn` (is_callable). Open file, `syn::parse_file(&src)?`, walk to `ItemFn` matching span; find contiguous statement sub-slice covering `op.byte_range`. Fail if crosses statement boundary → `EditError::InvalidByteRange`. VERIFY: `extract_fn_rejects_mid_statement`.

**Step 8 — Capture analysis via RA.** Need `Semantics`; warm host lives behind `WorkspaceHost::semantics()`. Compute `TextRange` from `op.byte_range` via line index; `let scope = sema.scope_at_offset(file_id, range.start())?;`. Collect every `syn::Ident` inside slice (via `syn::visit::Visit`); filter to those resolving (`scope.process_all_names`) to `ScopeDef::Local(Local)`. For each captured local: `Local::ty(db)` → `Type::display(db).to_string()` → param type. Decide `&T` / `&mut T` / `T` from `Local::is_mut(db)` + whether lifted code mutates (re-walk: `=` LHS, `&mut`, method call on `&mut self`). Non-local free idents (paths, use-imports, macro names) left alone. Sanity-check against `op.captured_locals` if non-empty; mismatch → `EditError::ExtractFunctionScopeCapture { unresolved }`. VERIFY: `extract_fn_captures_locals_with_correct_mut`.

**Step 9 — Synthesize + splice new fn.** Build `syn::ItemFn`: visibility, ident, inputs from captured locals as `&[mut] <ty>`, output from tail expression type if any. Insert `file.items.insert(idx + 1, ItemFn(...))`. Replace `byte_range` with `let _ = new_fn_name(&mut captured_a, captured_b, ...);` (or just call if `()` return, or `let r = ...` if tail-expr). Re-render. VERIFY: `extract_fn_emits_callable_new_fn`.

**Step 10 — Classify + apply.** `EditClass::ItemAddRemove`. New fn private by default → no reverse-dep impact. VERIFY: `extract_fn_full_round_trip`.

### P1.5d — `extract_trait`

**Step 11 — Validate method subset.** Require `parent.item_kind ∈ {Struct, Enum, Union}`. For each method: `parent_id == op.source_struct` and `item_kind == Some(Method)`. Stray → `ExtractTraitMethodsNotInherent`. Locate inherent `ItemImpl` via `syn` (trait_ is None, self_ty resolves to struct).

**Step 12 — Emit trait + impl.** Build `syn::ItemTrait { vis, ident, items: method_subset.map(|m| TraitItem::Fn(TraitItemFn { sig, default: None })) }` (signature only, no body). Build `syn::ItemImpl { trait_: Some(TypePath(trait_name)), self_ty: struct_path, items: ImplItem::Fn(ImplItemFn { sig, block: lifted body }) }`. Remove method nodes from inherent impl; prepend new trait + impl in file (or new `<mod>/<trait_snake>.rs` if `place_trait_inline == false`). VERIFY: `extract_trait_moves_methods_preserving_bodies`.

**Step 13 — Classify + apply.** `EditClass::ItemAddRemove` if private; `SigOrVis` if pub (changes reverse-dep import resolution).

### P1.5d — `inline`

**Step 14 — Fetch body + callsites.** Resolve target_fn (callable). Locate `ItemFn`/`ImplItemFn`; capture `block: syn::Block` + `sig.inputs` (param names). Reject if any `&mut` ref with conditionally-evaluated param read; reject recursive: `snap.recursive_callers_count(target_fn, 1)?.callers > 0 && body_calls_itself` → `InlineRecursiveFn`. VERIFY: `inline_rejects_recursive`.

**Step 15 — Determine callsite set.** `InlineAll` → `snap.who_calls + usages_of(call-shaped)`. `InlineSites(usage_ids)` → load each via `usages_by_id`.

**Step 16 — Per-callsite substitution with arg-lifting (no double-eval).** Descending byte order per file. For each callsite, build:
```
{
    let __arg_0 = <expr_0>;
    let __arg_1 = <expr_1>;
    ...
    <body_with_param_names_replaced_by___arg_n>
}
```
Param substitution via `visit_mut`: any `syn::Path` with single segment `param_n` → `Ident::new("__arg_n", ...)`. Self handling: method calls get `__arg_self = <receiver>`; receiver was `&self`/`&mut self` → prepend `&` or `&mut`. Replace callsite expression with this block. VERIFY: `inline_substitutes_args_no_double_eval`, `_method_call_self_handling`.

**Step 17 — Delete fn if InlineAll.** After splice, count remaining usages; for safety `delete = (policy == InlineAll)`. Remove `ItemFn` from file. EditClass: `ItemAddRemove` if deleting, else `Body`. VERIFY: `inline_all_deletes_fn_when_no_remaining_callers`.

### P1.5e — `create_module`

**Step 18 — Validate.** Parent must be Module or Crate. Name regex `^[a-z_][a-z0-9_]*$`. Parent file: for Module = `parent.file`; for Crate = root module's file. Decide new path: `<dir>/<name>.rs` or `<dir>/<name>/mod.rs` per `use_mod_rs`. Conflict → `ModuleTreeConflict`.

**Step 19 — Emit files.** `FileMove { from: None, to: new_path, contents: "// new module\n" }`. Append `pub mod <name>;` (or `mod <name>;`) to parent file via `syn::parse_file` + `file.items.push(ItemMod { ... })` + `prettyplease`.

**Step 20 — Move initial items.** Cut from current file, paste into new module file as part of same edit batch (NodeIds for new module don't exist until re-extract). `EditClass::ModuleTree`. Apply. VERIFY: `create_module_with_initial_items_round_trip`.

### P1.5e — `split_module`

**Step 21 — Validate.** Union of `splits[*].items` is subset of source_module's current items (via `children_by_parent`). Stray → `ItemsNotInModule`. Names unique, not colliding with existing children of `source_module.parent_id`.

**Step 22 — Per-split create + move.** For each `ModuleSplit`: in-process `create_module` logic with `parent = source_module.parent_id`, `name`, `items`, `use_mod_rs = false`. If `keep_reexport`: append `pub use <new_name>::*;` to source file.

**Step 23 — Cleanup re-exports.** Walk source_module file; prune `pub use <child>::X` lines pointing to moved items (unless `keep_reexport`). `EditClass::ModuleTree`. Apply. VERIFY: `split_module_three_ways_items_partitioned`.

### P1.5e — `merge_modules`

**Step 24 — Validate.** All `sources` + `dest` share `parent_id`. Item-name collisions → `ModuleTreeConflict`.

**Step 25 — Move items into dest.** For each source: parse `<source>.rs`, take `file.items`, paste into dest file. Rewrite import-cycles inside merged module.

**Step 26 — Delete source files + `mod` decls.** `FileMove { from: source_file, to: None }`; remove `mod <source>;` from parent.

**Step 27 — Workspace-wide `use` rewrite.** For every workspace file: `use <parent>::<source_name>::X` → `use <parent>::<dest_name>::X`. Via `SemanticService`-style mechanism + `syn`-based prefix substitution. `EditClass::ModuleTree`. Apply. VERIFY: `merge_modules_collapses_two_into_one`.

### P1.5e — `move_module`

**Step 28 — Validate.** source_module is Module (not Crate, not root module). new_parent is Module or Crate. If same parent + no rename → noop. Cycle check via `module_tree` descendant walk. Name collision in new_parent's children → `ModuleTreeConflict`.

**Step 29 — Compute file move.** Old path: `source_module.file`. New path: under `<dir of new_parent's file>/<new_name>.rs` (preserve mod.rs style). `FileMove { from: old, to: new }` plus directory moves if children exist.

**Step 30 — Update `mod` declarations.** Remove `mod <old>;` from old parent file; add `mod <new>;` to new parent file (via `syn` walk).

**Step 31 — Rewrite all `use` paths workspace-wide.** For each `.rs` (Merkle-filtered to those importing the moved module): parse, walk `UseTree`, prefix-replace `<old_qualified>` → `<new_qualified>`. `EditClass::ModuleTree`. Apply. VERIFY: `move_module_updates_all_uses`.

### P1.5e — `lift_to_crate` (Cargo surgery — FULL REBUILD)

**Step 32 — Validate.** source_module.crate_id exists; not lifting root module. new_crate_name kebab-case + not in workspace members (read root `Cargo.toml` via `toml_edit`). edition ∈ {"2021", "2024"}.

**Step 33 — Generate new crate skeleton.** New dir `crates/<new_crate_name>/`. `Cargo.toml`:
```toml
[package]
name = "<new_crate_name>"
version = "0.1.0"
edition = "<op.edition>"

[dependencies]
```
`src/lib.rs` = source_module's current file contents (verbatim). If source_module is dir-style (`mod.rs`), recursively copy subtree. Emit `CargoEdit` + `FileMove` per copied file.

**Step 34 — Compute + inject deps.** `cargo metadata --format-version 1 --no-deps` via `Command`. Walk lifted files; collect `use <name>::...` for each `<name>` that's a workspace or registry crate. Look up version in source crate's `Cargo.toml`; path-dep → `<name> = { path = "../<name>" }`; registry → copy version as-is. Apply via `toml_edit::DocumentMut::insert("dependencies", ...)`.

**Step 35 — Update workspace `Cargo.toml`.** `members.push(format!("crates/{}", new_crate_name))`. If broadly usable, add to `[workspace.dependencies]`. Emit `CargoEdit`.

**Step 36 — Update source crate's `Cargo.toml`.** Add `<new_crate_name> = { workspace = true }` (or path-form) to `[dependencies]`. Emit `CargoEdit`.

**Step 37 — Rewrite import paths workspace-wide.** Replace `<src_crate>::<source_module_path>::X` → `<new_crate_name>::X`. If `keep_facade`: replace source module file contents with `pub use <new_crate_name>::*;` so existing internal callers continue to work.

**Step 38 — Classify + apply (slow path).** `EditClass::Cargo` → `is_full_rebuild() == true`. `Crud::apply_file_edits` routes to cold-rebuild path: write all files, close warm host, delete working LMDB, re-run `build_and_persist`. Checkpoint records jj op id + copy of pre-edit Cargo manifests (LMDB undo log doesn't cover them). VERIFY: `lift_to_crate_full_rebuild_succeeds`, `_workspace_compiles_after`.

### P1.5e — `lower_to_module` (inverse — also FULL REBUILD)

**Step 39 — Validate.** source_crate is workspace lib. dest_parent_module in different crate. If `!keep_facade`, at most one path-dep consumer.

**Step 40 — Copy code in.** Read `crates/<src>/src/lib.rs` → new module body. Recursively walk subtree → reproduce under `<dest_parent>/<new_module_name>/`.

**Step 41 — Update consumer manifests.** For every crate depending on `<src>` (per cargo metadata): remove dep; replace `<src>::X` → `<dest_crate>::<dest_path>::<new_module_name>::X`.

**Step 42 — Remove from workspace.** Root `Cargo.toml`: remove `crates/<src>` from members; remove from `[workspace.dependencies]`. `FileMove` deleting `crates/<src>/` directory.

**Step 43 — Apply (slow).** Same cold rebuild as Step 38. VERIFY: `lower_to_module_round_trip`.

## Tests

(`crates/rmc-crud/tests/`)

**Per-verb behavioral:**
- `modify_sig_add_param_inserts_todo` — add `x: u32` as 1st param to fn with 3 callers; every callsite has `todo!("filled in by modify_signature: x")` at position 0.
- `modify_sig_remove_param_drops_arg` — remove `y` from `fn f(x, y, z)`; `f(1, 2, 3)` becomes `f(1, 3)`.
- `modify_sig_rename_param_no_callsite_change`.
- `modify_sig_reorder_perm_correct`.
- `modify_sig_retype_param_no_callsite_change` (cargo check may fail; that's the agent's problem).
- `modify_sig_refuse_policy_errors` — `Refuse` with add-param → `SignatureSynthesisRefused`; no files modified, checkpoint reverted.
- `modify_sig_default_fallback_to_todo_for_unknown_type`.
- `modify_sig_closure_builder_called_per_site`.
- `modify_sig_method_call_handling`.
- `modify_sig_const_initializer_ref`.
- `extract_fn_captures_locals` — block referencing `i: i32` and `s: &str` → new fn `fn new_fn(i: i32, s: &str)`.
- `extract_fn_captures_mut_borrow` — `*counter += 1` → `counter: &mut i32`.
- `extract_fn_handles_tail_expression`.
- `extract_fn_rejects_mid_statement`.
- `extract_fn_uncaptured_outer_ident_ignored`.
- `extract_fn_round_trips_compile`.
- `extract_trait_moves_methods` — `impl Foo { fn a, fn b, fn c }`; extract `[a, c]` into `trait Bar`; inherent has only `b`, new `impl Bar for Foo { fn a, fn c }`.
- `extract_trait_emits_trait_with_correct_visibility`.
- `extract_trait_separate_file`.
- `extract_trait_rejects_methods_from_other_impls`.
- `inline_substitutes_args` — `f(a + b)` for `fn f(x) { dbg!(x); dbg!(x); }` → `{ let __arg_0 = a + b; dbg!(__arg_0); dbg!(__arg_0); }`.
- `inline_method_call_self_handling`.
- `inline_all_deletes_fn_when_no_remaining_callers`.
- `inline_sites_subset_preserves_fn`.
- `inline_rejects_recursive`.
- `create_module_empty`.
- `create_module_with_initial_items_round_trip`.
- `create_module_mod_rs_style`.
- `create_module_name_collision_errors`.
- `split_module_three_ways`.
- `split_module_keep_reexport_preserves_external_use`.
- `merge_modules_collapses_two_into_one`.
- `merge_modules_name_collision_errors`.
- `move_module_updates_all_uses`.
- `move_module_rejects_cycle`.
- `lift_to_crate_full_rebuild`.
- `lift_to_crate_dep_inference` — module uses `serde::Serialize`; new crate's `Cargo.toml` has `serde = "..."`.
- `lift_to_crate_keep_facade_preserves_external_callers`.
- `lift_to_crate_rejects_duplicate_name`.
- `lower_to_module_inverse_of_lift`.

**Differential / property tests:**
- `differential_apply_vs_cold<Verb>` — for every verb + fixture: apply via warm host; rebuild cold from post-apply source; LMDB byte-equal on reward-bearing fields.
- `checkpoint_restore_roundtrip<Verb>` — take, apply, restore; source byte-equal, edit_seq reverted, queries equal.

**Failure-mode tests:**
- `modify_sig_partial_failure_reverts_atomically` — inject write error mid-apply; restore leaves no half-written files.
- `lift_to_crate_cargo_failure_reverts` — corrupt workspace `Cargo.toml` post-lift; restore reinstates prior manifest.

## Open decisions / risks

- **`CallsiteFill` default = `Todo`.** Picked over `Default` (silent semantic change) and `Refuse` (too aggressive). `Todo` keeps workspace type-checking, panics at runtime if reached, greppable.
- **`syn` 2 + `prettyplease`, not RA's `TextEdit`.** RA's TextEdit perfect for single-symbol edits; verbs here do structural mutation (whole sigs, blocks, ItemFns). Downside: prettyplease re-formats whole files — accepted (rustfmt normalizes anyway).
- **Cargo.toml lib: `toml_edit`.** `DocumentMut` preserves comments + ordering. `cargo_toml` round-trips lossily.
- **Capture analysis depends on warm RA host.** If unavailable → `EditError::HostUnavailable`. P0.2 is critical path anyway.
- **`inline` always lifts each arg.** Even if param used once — small cost, large correctness win (no double-eval, no precedence surprises).
- **Multi-file atomicity = Checkpoint's job.** Every verb calls `take_checkpoint()` BEFORE first write; on ANY failure → `restore()`. D4 covers source + graph + RA host; `EditClass::Cargo` extends to capture pre-edit Cargo manifest contents.
- **`lift_to_crate` / `lower_to_module` are HIGH-COST.** Unambiguous `EditClass::Cargo` → full cold rebuild → tens of seconds. Episode runner exposes cost in action-space metadata; agent's RL signal accounts for it. Alternative: gate behind `declare_done`. Current: emit via regular Crud API, mark `EditClass::Cargo`, let runner decide.
- **`modify_signature` cannot eliminate every false positive in callsite detection.** RA-unresolvable calls (`dyn Trait` over external trait, generic `F: Fn(..)`) won't appear in `who_calls`. Silently break post-edit. **Mitigation:** pair every `modify_signature` with cargo gate (P1.7) — fail → checkpoint reverts.
- **`extract_trait` self-call subtleties.** `impl Trait for Foo { fn a(...) { self.b(...) } }` works if `b` stays on Foo. Trait default method shadowing may compile-but-mean-something-subtly-different. Cargo gate catches compile-break subset.
- **`split_module` re-exports.** Default `keep_reexport = false` requires rewriting external users. Partial `splits` (not covering every item) leaves unsplit items in source_module (no error). Underspecified by design.
- **Determinism.** Every verb's source rewriting is deterministic: `syn::parse_file` deterministic; `prettyplease::unparse` deterministic; `toml_edit` round-trips deterministically; `who_calls` results iterated in LMDB-sorted order (P0.1 guarantee).


---

