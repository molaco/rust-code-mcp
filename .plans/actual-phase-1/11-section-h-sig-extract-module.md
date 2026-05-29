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
- `crates/rmc-crud/src/syn_ast.rs` — common helpers for `syn`/`ra_ap_syntax` parse used **for ANALYSIS only** (signature span, arg-list span, impl-block item ranges, captured-local resolution). These helpers return byte ranges (`(u32, u32)`) into the original source; they **never** render or unparse an AST. **No `prettyplease`, no `syn` `printing`/`quote`/`proc-macro2` codegen** — replacement text is string-built from each op's input fields and spliced via `source_edit::splice_bytes` per E5 (see Canonical Reconciliation §R4, errata E5). `source_edit` (the byte-splice helper module from Section G) is reused here.
- `crates/rmc-crud/src/name_resolution.rs` — thin wrapper around RA's `Semantics` for capture analysis in `extract_function`.

`crates/rmc-crud/src/lib.rs` re-exports new verbs; `facade.rs` gains nine methods; `edit.rs` gains the `EditClass::CargoManifest` variant + `is_full_rebuild()`; `error.rs` gains new variants.

New deps in `crates/rmc-crud/Cargo.toml` (E5: analysis-only — `syn` is
used purely to *locate byte ranges*, so the codegen-side features are
deliberately omitted: no `printing`, no `quote`, no `proc-macro2`, and
**no `prettyplease` dependency at all**. Replacement text is built by hand
and spliced):
```
# `parsing` + `visit` only — NOT `printing` (no AST→source rendering, per E5).
syn = { version = "2", features = ["full", "parsing", "extra-traits", "visit"] }
toml_edit = "0.22"
cargo_metadata = { workspace = true }
ra_ap_hir = "0.0.330"
ra_ap_ide_db = { workspace = true }
ra_ap_syntax = { workspace = true }
ra_ap_vfs = "0.0.330"
rmc-graph = { path = "../rmc-graph" }
rmc-server = { path = "../rmc-server" }
```
`visit-mut` is intentionally dropped: we no longer mutate the AST (the old
`visit_mut` rewrite path is gone), only walk it immutably to find spans.

## Type definitions

### callsite_fill.rs

```rust
// DERIVE IMPACT (§8): the `ClosureBuilder(Box<dyn Fn ...>)` variant makes the
// enum *un-derivable* for `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash` — a boxed
// closure implements none of them. This is an accepted trade-off for the
// escape-hatch variant. A hand-written `Debug` impl prints the active variant
// name (`"ClosureBuilder(<fn>)"` for the closure) so enclosing types that need
// `Debug` are not blocked; `Clone`/`PartialEq` remain unavailable. Callers that
// need a cloneable/comparable fill should use `Todo`/`Default`/`Refuse`.
#[non_exhaustive]
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

// Hand-written so the closure variant doesn't block `Debug` on enclosing types.
impl std::fmt::Debug for CallsiteFill { /* match → variant name */ }

impl Default for CallsiteFill { fn default() -> Self { CallsiteFill::Todo } }

#[derive(Debug, Clone, Copy)]
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
// Note: cannot derive `Debug`/`Clone` because of `callsite_fill: CallsiteFill`
// (the `ClosureBuilder` variant — see derive-impact note above). A hand-written
// `Debug` delegates to `CallsiteFill`'s manual `Debug`.
pub struct SignatureChange {
    pub target: NodeId,
    pub new_sig: FunctionSignature,     // ENTIRE new sig, not a delta — see DD-3 invariant below
    pub callsite_fill: CallsiteFill,    // default Todo
}

#[derive(Debug, Clone)]
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

> **LIMITATION / INVARIANT (DD-3).** `SignatureChange` deliberately takes the
> *whole new signature* (`new_sig: FunctionSignature`), not an explicit ordered
> edit-list. The input type is **not** being restructured — a whole-sig input is
> the ergonomic choice for callers (and for the RL agent, which emits a target
> signature, not a diff script). The cost is that Step 2 must *infer* the
> `SignatureDelta` by comparing old vs new, and **rename-vs-(remove+add) is
> heuristic and can be ambiguous.** Documented heuristic, applied in this order:
> 1. **Position-pair pass.** Walk old/new params by index. Same name + same type
>    at index `i` → unchanged. Same name, different type → `retyped(i)`.
> 2. **Name-set reconciliation** for positions that didn't pair: a param whose
>    *name* exists in old but at a different index, with all other names stable →
>    `reordered`. A name present in new but absent from old → candidate `added`;
>    a name present in old but absent from new → candidate `removed`.
> 3. **Rename inference (the ambiguous step).** When exactly one name was
>    "removed" and exactly one "added" *at the same position with the same type*,
>    it is classified as `renamed(i, new_name)` rather than `removed(i)` +
>    `added(i, _)`. With multiple simultaneous add+remove at the same position,
>    OR a same-position name change *with* a type change, the heuristic cannot
>    distinguish rename from remove+add and **conservatively chooses remove+add**
>    (which triggers `callsite_fill` for the "added" param). This may insert a
>    `todo!()` where a pure rename was intended — callers wanting an unambiguous
>    rename should use the dedicated rename path / keep the param type stable.
> The cargo gate (P1.7) is the backstop: a mis-inferred delta surfaces as a
> compile error and reverts via Checkpoint.

### extract_function.rs / extract_trait.rs / inline.rs

```rust
#[derive(Debug, Clone)]
pub struct ExtractFunctionOp {
    pub source_fn: NodeId,
    pub byte_range: (u32, u32),               // inside source_fn's file
    pub new_fn_name: String,
    pub captured_locals: Vec<String>,         // hint; empty = auto-detect
    pub new_fn_visibility: BindingVisibility,
}

#[derive(Debug, Clone)]
pub struct ExtractTraitOp {
    pub source_struct: NodeId,
    pub method_subset: Vec<NodeId>,
    pub trait_name: String,
    pub trait_visibility: BindingVisibility,
    pub place_trait_inline: bool,
}

#[derive(Debug, Clone)]
pub struct InlineOp { pub target_fn: NodeId, pub policy: InlinePolicy }

#[derive(Debug, Clone)]
pub enum InlinePolicy { InlineAll, InlineSites(Vec<UsageId>) }
```

### split_module / merge_modules / create_module / move_module

```rust
#[derive(Debug, Clone)]
pub struct SplitModuleOp { pub source_module: NodeId, pub splits: Vec<ModuleSplit> }
#[derive(Debug, Clone)]
pub struct ModuleSplit {
    pub new_name: String,
    pub items: Vec<NodeId>,
    pub keep_reexport: bool,
}
#[derive(Debug, Clone)]
pub struct MergeModulesOp { pub sources: Vec<NodeId>, pub dest: NodeId }
#[derive(Debug, Clone)]
pub struct CreateModuleOp {
    pub parent: NodeId,
    pub name: String,
    pub initial_items: Vec<NodeId>,
    pub use_mod_rs: bool,
}
#[derive(Debug, Clone)]
pub struct MoveModuleOp {
    pub source_module: NodeId,
    pub new_parent: NodeId,
    pub new_name: Option<String>,
}
```

### lift_to_crate / lower_to_module

```rust
#[derive(Debug, Clone)]
pub struct LiftToCrateOp {
    pub source_module: NodeId,
    pub new_crate_name: String,    // kebab-case
    pub edition: String,            // "2021" / "2024"
    pub keep_facade: bool,
}

#[derive(Debug, Clone)]
pub struct LowerToModuleOp {
    pub source_crate: NodeId,
    pub dest_parent_module: NodeId,
    pub new_module_name: Option<String>,
}
```

### Crud methods

Each verb is `#[must_use]` on its `EditOutcome` (carries the live `Checkpoint`).
Every public verb carries an `# Errors` doc section naming its dominant failure
modes (validation refusal, `SynParse` on a malformed target file, `Io` on the
read/write, `HostUnavailable` when the warm RA host is down, and — for the two
Cargo verbs — `CargoMetadata`/`CargoTomlConflict`). Sketch:

```rust
impl Crud {
    /// Rewrite a function/method signature and patch every detectable callsite.
    ///
    /// # Errors
    /// `TargetNotFound`/`WrongItemKind` (bad target), `SynParse` (unparseable
    /// source), `SignatureSynthesisRefused` (added param + `CallsiteFill::Refuse`),
    /// `Io` (read/write), `HostUnavailable`, `LineIndex` (offset→line mapping).
    pub fn modify_signature(&mut self, op: SignatureChange) -> Result<EditOutcome, EditError>;
    /// Extract a statement range into a new private function + call.
    ///
    /// # Errors
    /// `InvalidByteRange`, `ExtractFunctionScopeCapture`, `SynParse`, `Io`,
    /// `HostUnavailable`, `LineIndex`.
    pub fn extract_function(&mut self, op: ExtractFunctionOp) -> Result<EditOutcome, EditError>;
    /// Hoist a subset of inherent methods into a new trait + `impl`.
    ///
    /// # Errors
    /// `ExtractTraitMethodsNotInherent`, `SynParse`, `Io`.
    pub fn extract_trait(&mut self, op: ExtractTraitOp) -> Result<EditOutcome, EditError>;
    /// Inline a function into its callsites (with per-arg let-binding).
    ///
    /// # Errors
    /// `InlineRecursiveFn`, `SynParse`, `Io`, `HostUnavailable`.
    pub fn inline(&mut self, op: InlineOp) -> Result<EditOutcome, EditError>;
    /// Partition a module's items into N sibling modules.
    ///
    /// # Errors
    /// `ItemsNotInModule`, `ModuleTreeConflict`, `SynParse`, `Io`.
    pub fn split_module(&mut self, op: SplitModuleOp) -> Result<EditOutcome, EditError>;
    /// Fold N sibling modules into one.
    ///
    /// # Errors
    /// `ModuleTreeConflict`, `SynParse`, `Io`.
    pub fn merge_modules(&mut self, op: MergeModulesOp) -> Result<EditOutcome, EditError>;
    /// Create a child module, optionally seeded with moved items.
    ///
    /// # Errors
    /// `ModuleTreeConflict`, `SynParse`, `Io`.
    pub fn create_module(&mut self, op: CreateModuleOp) -> Result<EditOutcome, EditError>;
    /// Relocate/rename a module file and rewrite `use` paths workspace-wide.
    ///
    /// # Errors
    /// `ModuleTreeConflict`, `SynParse`, `Io`.
    pub fn move_module(&mut self, op: MoveModuleOp) -> Result<EditOutcome, EditError>;
    /// Promote a module to a new workspace crate (FULL REBUILD).
    ///
    /// # Errors
    /// `CargoTomlConflict`, `CargoMetadata` (running `cargo metadata`),
    /// `SynParse`, `Io`, `HostUnavailable`.
    pub fn lift_to_crate(&mut self, op: LiftToCrateOp) -> Result<EditOutcome, EditError>;
    /// Fold a small workspace crate back into a module (FULL REBUILD).
    ///
    /// # Errors
    /// `CargoTomlConflict`, `CargoMetadata`, `SynParse`, `Io`.
    pub fn lower_to_module(&mut self, op: LowerToModuleOp) -> Result<EditOutcome, EditError>;
}
```

### New `EditError` variants

`EditError` is the crate's single `thiserror` enum (Section G already owns
`TargetNotFound`, `WrongItemKind`, `InvalidByteRange`, `Io { path, op, source }`,
the host-apply/restore variants, etc.). It is `#[non_exhaustive]`. **Not a god
enum:** variants are domain-scoped (validation, synthesis-refusal, parse, IO,
host, cargo) and the count is bounded by the closed verb set. If a future verb
family needs many private failure modes, scope them into a per-op error that
`EditError` wraps with `#[source]` rather than flattening more leaf variants in.
`anyhow` is **not** used anywhere in `rmc-crud` (library crate) — only the
`rmc-spikes`/`rmc-rl` binaries use `anyhow`.

The verbs in this section add domain variants plus the wrapper variants the
bare `?` calls (`syn::parse_file`, `syn::parse_str`, line-index lookups,
`std::fs`, `cargo metadata`/`Command`, warm-host access) require:

```rust
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum EditError {
    // --- existing (Section G), elided ---

    // --- domain (this section) ---
    #[error("signature synthesis refused for {fn_id:?}: {callsite_count} callsite(s) need a fill")]
    SignatureSynthesisRefused { fn_id: NodeId, callsite_count: usize },
    #[error("Cargo.toml conflict for crate `{crate_name}`: {reason}")]
    CargoTomlConflict { crate_name: String, reason: String },
    #[error("extract_function: unresolved scope captures: {unresolved:?}")]
    ExtractFunctionScopeCapture { unresolved: Vec<String> },
    #[error("extract_trait: methods are not inherent on the target: {stray:?}")]
    ExtractTraitMethodsNotInherent { stray: Vec<NodeId> },
    #[error("items not in module {module:?}: {stray:?}")]
    ItemsNotInModule { module: NodeId, stray: Vec<NodeId> },
    #[error("module-tree conflict under {parent:?} for `{name}`: {reason}")]
    ModuleTreeConflict { parent: NodeId, name: String, reason: String },
    #[error("cannot inline recursive fn {fn_id:?}")]
    InlineRecursiveFn { fn_id: NodeId },

    // --- wrapper variants for the `?` calls (source-preserving) ---
    /// `syn`/`ra_ap_syntax` failed to parse the target source (analysis only).
    #[error("failed to parse `{path}`")]
    SynParse {
        path: PathBuf,
        #[source]
        source: syn::Error,
    },
    /// Filesystem read/write (note: Section G's `Io { path, op, source }` is
    /// reused where a path+op is known; this `#[from]` arm catches the rest).
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// `cargo metadata` invocation or JSON decode failed (lift/lower only).
    #[error("cargo metadata failed")]
    CargoMetadata {
        #[source]
        source: cargo_metadata::Error,
    },
    /// Byte-offset → line/column mapping failed (offset past EOF / not on a
    /// char boundary) when feeding RA `Semantics`.
    #[error("line-index lookup failed for offset {offset} in `{path}`")]
    LineIndex { path: PathBuf, offset: u32 },
    /// The warm rust-analyzer host needed for capture analysis is unavailable.
    #[error("warm rust-analyzer host unavailable")]
    HostUnavailable,
}
```

`Command`-based `cargo metadata` is invoked through the typed `cargo_metadata`
crate (`MetadataCommand`), not a raw `Command` whose `Output` is string-parsed;
its `Result` maps to `EditError::CargoMetadata`. Any direct `Command` (none
strictly required here) would map its `io::Error` through the `Io` arm and its
non-zero exit through `CargoTomlConflict`/`CargoMetadata` as appropriate.

### EditOutcome extension

```rust
#[derive(Debug)]                          // not Clone: Checkpoint is move-only
#[must_use = "EditOutcome carries a live Checkpoint that must be committed or restored"]
pub struct EditOutcome {
    pub file_edits: Vec<FileEdit>,
    pub file_moves: Vec<FileMove>,
    pub cargo_edits: Vec<CargoEdit>,    // NEW; usually empty
    pub class: EditClass,
    pub affected_items: Vec<NodeId>,
    pub checkpoint: Checkpoint,
}

#[derive(Debug, Clone)]
pub struct CargoEdit {
    pub manifest_path: PathBuf,
    pub new_contents: String,            // toml_edit-rendered, format-preserved
}

// Canonical variant names per Canonical Reconciliation / global conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditClass {
    BodyOnly, SignatureOrVis, ItemAddRemove, ModuleTree, Macro,
    CargoManifest,                       // lift_to_crate / lower_to_module → COLD REBUILD
}
impl EditClass {
    #[must_use]
    pub fn is_full_rebuild(self) -> bool { matches!(self, Self::CargoManifest | Self::Macro) }
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

**Step 2 — Diff old vs new sig.** Infer `SignatureDelta` per the **DD-3 heuristic** (see the LIMITATION note under the type defs): position-pair pass, then name-set reconciliation, then the ambiguous rename-vs-(remove+add) inference (single same-position same-type swap → `renamed`; otherwise conservative `removed`+`added`). Same-name+different-ty → `retyped`. New name not in old → `added`. Old name not in new → `removed`. Same set different order → `reordered = Some(permutation)`. VERIFY: `diff_detects_add_remove_rename_reorder`.

**Step 3 — Rewrite the function declaration (locate span → build string → splice).** Read file; `syn::parse_file(&src).map_err(|source| EditError::SynParse { path, source })?` for **analysis only**. Walk (immutable `syn::visit::Visit`) to the `ItemFn`/`ImplItemFn`/`TraitItemFn` whose span matches `op.target`, and read off the **byte range of its `syn::Signature`** — from the start of the first signature token (`const`/`async`/`fn`, accounting for the `fn` keyword) to just before the body's opening `{` (or the trailing `;` for a trait fn). Call this `(sig_start, sig_end)` (a `syn_ast` helper, `fn_signature_byte_range`). Do **not** mutate the AST.

Build the replacement signature text by hand from `new_sig: FunctionSignature` (pure string formatter `render_signature`, no AST unparse):
- `is_async` → emit `async ` prefix.
- `self_param` → emit `&self` / `&mut self` / `self` as the first input.
- `params` → `format!("{}: {}", p.name, p.ty)`, comma-joined.
- `return_type` → emit ` -> {}` when non-unit.
- `generics` / where-clause → `render_generics(&new_sig.generics)`.

Splice: `let new_text = source_edit::splice_bytes(&src, sig_start as usize, sig_end as usize, &render_signature(&op.new_sig));`. The body and everything outside `(sig_start, sig_end)` is preserved verbatim. One `FileEdit { path, new_text, edit_class: EditClass::SignatureOrVis }`. VERIFY: `modify_sig_rewrites_decl_only`.

**Ident hygiene (synthesized names).** Where this section synthesizes identifiers (callsite `todo!()` text in Step 5, the `__arg_n`/`__arg_self` let-bindings in Step 16), names must not collide with locals already in scope at the splice site. Scheme: collect the set of identifiers in scope (RA `scope.process_all_names` when the host is warm, else a `syn`-walk of the enclosing item's idents); pick the prefix `__arg` and append the parameter index, then if `__arg_{n}` is already present append a numeric disambiguator (`__arg_{n}_{k}`, incrementing `k`) until unused. The same procedure produces `__arg_self`/`__arg_self_{k}`. The chosen names are recorded so all references within one splice agree.

**Step 4 — Find all callsites of OLD sig.** `let sites = snap.who_calls(op.target)?;` + `snap.usages_of(op.target)?` for non-fn-body refs. Union is the rewrite set. Tag with body-call vs const-ref (Default-substitution only valid in body context). VERIFY: `modify_sig_collects_all_callsites`.

**Step 5 — Rewrite each callsite (locate arg-list span → rebuild arg string → splice).** Group by file; **descending byte-offset order** so earlier splices don't shift later ranges. For each file: parse `syn::File` for analysis; for each site walk (immutable `syn::visit::Visit`) to the topmost `ExprCall`/`ExprMethodCall` whose span contains the offset, and read off the **byte range of the arg list** — the span just inside the parens, `(args_start, args_end)` (a `syn_ast` helper). Also capture each existing argument's source text by its sub-span (so existing args are preserved verbatim, including formatting/comments). Build the *new* comma-separated argument string in memory from those captured slices plus synthesized fills:
- **Reorder:** emit the captured arg slices in `perm` order.
- **Remove:** drop the captured slice at each `removed` index.
- **Add:** for each `(j, new_param)`, produce the fill **text** per `callsite_fill`:
  - `Todo` → `format!(r#"todo!("filled in by modify_signature: {}")"#, new_param.name)` (string, no parse).
  - `Default` → if `new_param.ty` is in the small allowlist `{i*, u*, f*, bool, String, Vec<_>, Option<_>, HashMap<_,_>}` (or carries a `: Default` bound in `new_sig.generics`) emit `Default::default()`; else fall back to the `Todo` text.
  - `Refuse` → return `EditError::SignatureSynthesisRefused { fn_id, callsite_count }` (no file touched; Checkpoint reverts).
  - `ClosureBuilder(f)` → `f(&ctx)` (the returned string is spliced verbatim).

Assemble the final arg list (removals/reorder applied first, then insertions at increasing index), join with `", "`, and `source_edit::splice_bytes(&src, args_start, args_end, &new_args)`. One `FileEdit` per touched file (accumulate splices in descending order before emitting). VERIFY: `modify_sig_add_param_inserts_todo`, `_remove_param_drops_arg`, `_reorder_perm_correct`.

**Step 6 — Classify + apply.** `EditClass::SignatureOrVis` (D2 expands to editing crate + reverse-deps). `Crud::take_checkpoint()` → `Crud::apply_file_edits(edits, class)` → host writes + LMDB patch + return `EditOutcome`. On error → `Checkpoint::restore()`.

### P1.5d — `extract_function`

**Step 7 — Parse + locate range.** Resolve `source_fn` (is_callable). Open file (`std::fs::read_to_string` → `EditError::Io`), `syn::parse_file(&src).map_err(|source| EditError::SynParse { path, source })?` for analysis, walk (immutable `Visit`) to `ItemFn` matching span; find the contiguous statement sub-slice whose byte span covers `op.byte_range` and record that exact `(stmt_start, stmt_end)` range (this is what gets spliced in Step 9). Fail if `op.byte_range` crosses a statement boundary → `EditError::InvalidByteRange`. VERIFY: `extract_fn_rejects_mid_statement`.

**Step 8 — Capture analysis via RA.** Need `Semantics`; warm host lives behind `WorkspaceHost::semantics()` → if down, `EditError::HostUnavailable`. Compute `TextRange` from `op.byte_range` via the file's line index (offset past EOF / not on a char boundary → `EditError::LineIndex { path, offset }`); `let scope = sema.scope_at_offset(file_id, range.start());`. Collect every `syn::Ident` inside the slice (immutable `syn::visit::Visit`); filter to those resolving (`scope.process_all_names`) to `ScopeDef::Local(Local)`. For each captured local: `Local::ty(db)` → `Type::display(db).to_string()` → param type. Decide `&T` / `&mut T` / `T` from `Local::is_mut(db)` + whether the lifted code mutates (re-walk: `=` LHS, `&mut`, method call on `&mut self`). Non-local free idents (paths, use-imports, macro names) left alone. Sanity-check against `op.captured_locals` if non-empty; mismatch → `EditError::ExtractFunctionScopeCapture { unresolved }`. VERIFY: `extract_fn_captures_locals_with_correct_mut`.

**Step 9 — Synthesize + splice new fn (build strings → two splices).** Build the **new fn source text** by hand (no AST construction, no unparse): `format!("{vis}fn {name}({params}){ret} {{\n{body}\n}}", ...)` where `params` is the captured locals rendered as `&[mut] <ty>`, `ret` is `-> <ty>` derived from the tail-expression type if any (else empty), and `body` is the lifted statement slice (verbatim source from `(stmt_start, stmt_end)`). Build the **call replacement text**: `let _ = new_fn_name(&mut captured_a, captured_b, ...);` (or a bare call for `()` return, or `let r = ...;` if there is a tail expr). Apply two `source_edit` splices to the original source, **highest offset first**: (1) insert `"\n\n" + new_fn_text` immediately after the enclosing item's byte-end; (2) `splice_bytes` the `(stmt_start, stmt_end)` range with the call replacement. Emit one `FileEdit`. VERIFY: `extract_fn_emits_callable_new_fn`.

**Step 10 — Classify + apply.** `EditClass::ItemAddRemove`. New fn private by default → no reverse-dep impact. VERIFY: `extract_fn_full_round_trip`.

### P1.5d — `extract_trait`

**Step 11 — Validate method subset.** Require `parent.item_kind ∈ {Struct, Enum, Union}`. For each method: `parent_id == op.source_struct` and `item_kind == Some(Method)`. Stray → `ExtractTraitMethodsNotInherent`. Locate the inherent `ItemImpl` via `syn` analysis (`trait_` is `None`, `self_ty` resolves to the struct) and record the byte ranges of each subset method (`ImplItemFn` span: `(method_start, method_end)`) plus the signature sub-span and body sub-span of each.

**Step 12 — Emit trait + impl (build strings → splice).** Build the **trait text** by hand: `format!("{vis}trait {trait_name} {{\n{sigs}\n}}")` where `sigs` is each subset method's *signature* sub-span (verbatim source) followed by `;` (signature only, no body). Build the **impl text**: `format!("impl {trait_name} for {self_ty} {{\n{methods}\n}}")` where `methods` is each subset method's full `(method_start, method_end)` source verbatim (signature + body preserved exactly). Apply `source_edit` splices, highest offset first: delete each method's byte range from the inherent impl (`source_edit::delete_byte_range`), then insert `"\n\n" + trait_text + "\n\n" + impl_text` either at the top of the file (`place_trait_inline == true`) or as a new file `FileMove { from: None, to: <mod>/<trait_snake>.rs, contents: ... }` with a `mod`/`pub use` wired into the parent (when `place_trait_inline == false`). Preserving the method source verbatim guarantees bodies are byte-identical. VERIFY: `extract_trait_moves_methods_preserving_bodies`.

**Step 13 — Classify + apply.** `EditClass::ItemAddRemove` if private; `EditClass::SignatureOrVis` if pub (changes reverse-dep import resolution).

### P1.5d — `inline`

**Step 14 — Fetch body + callsites.** Resolve target_fn (callable). Locate `ItemFn`/`ImplItemFn` via `syn` analysis; record the **byte range of the body block** (the inner source between the braces) and the param names from `sig.inputs`. Read the body source verbatim from that range (no AST capture). Reject if any `&mut` ref with conditionally-evaluated param read; reject recursive: `snap.recursive_callers_count(target_fn, 1)?.callers > 0 && body_calls_itself` → `InlineRecursiveFn` (the `?` on the snapshot query maps host failure to its existing variant; parse failure → `SynParse`). VERIFY: `inline_rejects_recursive`.

**Step 15 — Determine callsite set.** `InlineAll` → `snap.who_calls + usages_of(call-shaped)`. `InlineSites(usage_ids)` → load each via `usages_by_id`.

**Step 16 — Per-callsite substitution with arg-lifting (no double-eval).** Descending byte order per file. For each callsite, locate the call expression's span and its argument sub-spans via `syn` analysis (read each `<expr_n>` verbatim from source). Synthesize hygienic binding names per the **ident hygiene scheme** (Step 3): the base is `__arg_{n}` / `__arg_self`, disambiguated against the idents in scope at the callsite so they cannot shadow real locals. Build the replacement **block text** by hand:
```
{
    let __arg_0 = <expr_0>;
    let __arg_1 = <expr_1>;
    ...
    <body_with_param_names_replaced_by___arg_n>
}
```
The body comes from the verbatim body-range source captured in Step 14; param→`__arg_n` substitution is done by locating each param-name identifier's sub-span (single-segment path matching `param_n`) **in the body source via `syn` analysis** and `source_edit`-splicing the hygienic name in (highest offset first, on the body string), then wrapping with the `let` prelude. Self handling: method calls get `__arg_self = <receiver>`; if the receiver was `&self`/`&mut self`, prepend `&`/`&mut`. Splice the assembled block over the callsite expression's byte range. VERIFY: `inline_substitutes_args_no_double_eval`, `_method_call_self_handling`.

**Step 17 — Delete fn if InlineAll.** After splice, count remaining usages; for safety `delete = (policy == InlineAll)`. Delete the fn by `source_edit::delete_byte_range` over the `ItemFn`'s span (locate via `syn`), then `source_edit::collapse_blank_lines` at the deletion point. EditClass: `EditClass::ItemAddRemove` if deleting, else `EditClass::BodyOnly`. VERIFY: `inline_all_deletes_fn_when_no_remaining_callers`.

### P1.5e — `create_module`

**Step 18 — Validate.** Parent must be Module or Crate. Name regex `^[a-z_][a-z0-9_]*$`. Parent file: for Module = `parent.file`; for Crate = root module's file. Decide new path: `<dir>/<name>.rs` or `<dir>/<name>/mod.rs` per `use_mod_rs`. Conflict → `ModuleTreeConflict`.

**Step 19 — Emit files.** `FileMove { from: None, to: new_path, contents: "// new module\n" }`. Append the module declaration to the parent file by hand: parse the parent with `syn::parse_file` for analysis to find the byte offset of the last top-level item's end (insertion point), then `source_edit::insert_at_byte_offset(&parent_src, insert_at, &format!("\n{}mod {};\n", vis_prefix, name))` where `vis_prefix` is `"pub "` or `""`. No AST mutation, no unparse.

**Step 20 — Move initial items.** Cut from current file, paste into new module file as part of same edit batch (NodeIds for new module don't exist until re-extract). `EditClass::ModuleTree`. Apply. VERIFY: `create_module_with_initial_items_round_trip`.

### P1.5e — `split_module`

**Step 21 — Validate.** Union of `splits[*].items` is subset of source_module's current items (via `children_by_parent`). Stray → `ItemsNotInModule`. Names unique, not colliding with existing children of `source_module.parent_id`.

**Step 22 — Per-split create + move.** For each `ModuleSplit`: in-process `create_module` logic with `parent = source_module.parent_id`, `name`, `items`, `use_mod_rs = false`. If `keep_reexport`: append `pub use <new_name>::*;` to source file.

**Step 23 — Cleanup re-exports.** Walk source_module file; prune `pub use <child>::X` lines pointing to moved items (unless `keep_reexport`). `EditClass::ModuleTree`. Apply. VERIFY: `split_module_three_ways_items_partitioned`.

### P1.5e — `merge_modules`

**Step 24 — Validate.** All `sources` + `dest` share `parent_id`. Item-name collisions → `ModuleTreeConflict`.

**Step 25 — Move items into dest.** For each source: parse `<source>.rs` for analysis, read each top-level item's byte span, and concatenate those verbatim source slices into the dest file (`source_edit::insert_at_byte_offset` at dest's item-list end). No AST nodes are re-rendered. Rewrite import-cycles inside the merged module by span-located `use`-prefix splices (same mechanism as Step 27).

**Step 26 — Delete source files + `mod` decls.** `FileMove { from: source_file, to: None }`; remove `mod <source>;` from the parent by locating its `ItemMod` span (`syn` analysis) and `source_edit::delete_byte_range` + `collapse_blank_lines`.

**Step 27 — Workspace-wide `use` rewrite.** For every workspace file: `use <parent>::<source_name>::X` → `use <parent>::<dest_name>::X`. Mechanism: parse each file for analysis, walk `UseTree` to find the byte span of the matching path *prefix*, and `source_edit::splice_bytes` the new prefix in (descending offset per file). `EditClass::ModuleTree`. Apply. VERIFY: `merge_modules_collapses_two_into_one`.

### P1.5e — `move_module`

**Step 28 — Validate.** source_module is Module (not Crate, not root module). new_parent is Module or Crate. If same parent + no rename → noop. Cycle check via `module_tree` descendant walk. Name collision in new_parent's children → `ModuleTreeConflict`.

**Step 29 — Compute file move.** Old path: `source_module.file`. New path: under `<dir of new_parent's file>/<new_name>.rs` (preserve mod.rs style). `FileMove { from: old, to: new }` plus directory moves if children exist.

**Step 30 — Update `mod` declarations.** Remove `mod <old>;` from the old parent file (locate `ItemMod` span via `syn` analysis → `source_edit::delete_byte_range`); add `mod <new>;` to the new parent file (`syn` analysis to find the item-list-end offset → `source_edit::insert_at_byte_offset`).

**Step 31 — Rewrite all `use` paths workspace-wide.** For each `.rs` (Merkle-filtered to those importing the moved module): parse for analysis, walk `UseTree` to locate the byte span of the `<old_qualified>` path prefix, and `source_edit::splice_bytes` `<new_qualified>` in its place (descending offset per file; existing tokens preserved verbatim — no AST re-render). `EditClass::ModuleTree`. Apply. VERIFY: `move_module_updates_all_uses`.

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

**Step 34 — Compute + inject deps.** Run `cargo metadata` through the typed `cargo_metadata::MetadataCommand::new().no_deps().exec()` (errors → `EditError::CargoMetadata { source }`; not a raw `Command` whose stdout is string-parsed). Walk lifted files (parse for analysis); collect `use <name>::...` for each `<name>` that's a workspace or registry crate. Look up version in the source crate's `Cargo.toml` (`toml_edit`); path-dep → `<name> = { path = "../<name>" }`; registry → copy version as-is. Apply via `toml_edit::DocumentMut` and render with `.to_string()` into `CargoEdit.new_contents` (format-preserving).

**Step 35 — Update workspace `Cargo.toml`.** `members.push(format!("crates/{}", new_crate_name))`. If broadly usable, add to `[workspace.dependencies]`. Emit `CargoEdit`.

**Step 36 — Update source crate's `Cargo.toml`.** Add `<new_crate_name> = { workspace = true }` (or path-form) to `[dependencies]`. Emit `CargoEdit`.

**Step 37 — Rewrite import paths workspace-wide.** Replace `<src_crate>::<source_module_path>::X` → `<new_crate_name>::X` by span-located `use`-prefix splices (`syn` analysis → `source_edit::splice_bytes`, descending offset per file). If `keep_facade`: replace the source module file contents with `pub use <new_crate_name>::*;` so existing internal callers continue to work.

**Step 38 — Classify + apply (slow path).** `EditClass::CargoManifest` → `is_full_rebuild() == true`. `Crud::apply_file_edits` routes to the cold-rebuild path: write all files, close warm host, delete working LMDB, re-run `build_and_persist`. Checkpoint records jj op id + copy of pre-edit Cargo manifests (LMDB undo log doesn't cover them). VERIFY: `lift_to_crate_full_rebuild_succeeds`, `_workspace_compiles_after`.

### P1.5e — `lower_to_module` (inverse — also FULL REBUILD)

**Step 39 — Validate.** source_crate is workspace lib. dest_parent_module in different crate. If `!keep_facade`, at most one path-dep consumer.

**Step 40 — Copy code in.** Read `crates/<src>/src/lib.rs` → new module body. Recursively walk subtree → reproduce under `<dest_parent>/<new_module_name>/`.

**Step 41 — Update consumer manifests.** For every crate depending on `<src>` (per `cargo_metadata::MetadataCommand`, errors → `EditError::CargoMetadata`): remove the dep (`toml_edit`); replace `<src>::X` → `<dest_crate>::<dest_path>::<new_module_name>::X` via span-located `use`-prefix splices.

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
- **`syn` 2 for ANALYSIS, hand-built strings + `source_edit::splice_bytes` for edits (E5).** No whole-file formatter (`prettyplease`/`rustfmt`) and no AST→source rendering anywhere. `syn`/`ra_ap_syntax` are used *only* to locate byte ranges (signature span, arg-list span, impl-method ranges, extracted statement range, `UseTree` prefix spans); replacement text is formatted by hand from each op's input fields and spliced byte-for-byte, leaving all surrounding source verbatim. This avoids the lossy "re-format the whole file" round-trip E5 forbids and keeps diffs minimal.
- **Cargo.toml lib: `toml_edit`.** `DocumentMut` preserves comments + ordering. `cargo_toml` round-trips lossily.
- **Capture analysis depends on warm RA host.** If unavailable → `EditError::HostUnavailable`. P0.2 is critical path anyway.
- **`inline` always lifts each arg.** Even if param used once — small cost, large correctness win (no double-eval, no precedence surprises).
- **Multi-file atomicity = Checkpoint's job.** Every verb calls `take_checkpoint()` BEFORE first write; on ANY failure → `restore()`. D4 covers source + graph + RA host; `EditClass::CargoManifest` extends to capture pre-edit Cargo manifest contents.
- **`lift_to_crate` / `lower_to_module` are HIGH-COST.** Unambiguous `EditClass::CargoManifest` → full cold rebuild → tens of seconds. Episode runner exposes cost in action-space metadata; agent's RL signal accounts for it. Alternative: gate behind `declare_done`. Current: emit via regular Crud API, mark `EditClass::CargoManifest`, let runner decide.
- **`modify_signature` cannot eliminate every false positive in callsite detection.** RA-unresolvable calls (`dyn Trait` over external trait, generic `F: Fn(..)`) won't appear in `who_calls`. Silently break post-edit. **Mitigation:** pair every `modify_signature` with cargo gate (P1.7) — fail → checkpoint reverts.
- **`extract_trait` self-call subtleties.** `impl Trait for Foo { fn a(...) { self.b(...) } }` works if `b` stays on Foo. Trait default method shadowing may compile-but-mean-something-subtly-different. Cargo gate catches compile-break subset.
- **`split_module` re-exports.** Default `keep_reexport = false` requires rewriting external users. Partial `splits` (not covering every item) leaves unsplit items in source_module (no error). Underspecified by design.
- **Determinism.** Every verb's source rewriting is deterministic: `syn::parse_file` parsing is deterministic; hand-built replacement strings are pure functions of the op's input fields; `source_edit::splice_bytes` is a pure byte operation; `toml_edit` round-trips deterministically; `who_calls` results iterated in LMDB-sorted order (P0.1 guarantee). Splices within a file are always applied in descending byte-offset order so the result is independent of collection iteration order.


---

