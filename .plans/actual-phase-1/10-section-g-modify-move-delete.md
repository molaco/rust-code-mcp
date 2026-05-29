# Section G — P1.5a modify_body + P1.5b move / delete

## Overview

This slice delivers **`modify_body` (P1.5a)** and **`move` + `delete` (P1.5b)** — three verbs that close enough of the loop to power M3's first end-to-end episode. `modify_body` is the verb on the critical path — per the milestone order, M2a finishes P0.2 together with `modify_body`, and M3 immediately opens an end-to-end loop on `modify_body` alone (the entire point of the P1.5 split: prove the apply→gate→reward loop with the cheapest possible edit class — D2 `BodyOnly` — before adding propagation). `move` and `delete` arrive in M2b as the first two verbs that *do* propagate.

All three verbs are thin wrappers around four substrates: (a) the persisted `OpenedSnapshot` read layer for resolving `NodeId → Node`, spans, and ref-checks via `who_imports` + `usages_of`; (b) RA's `ast::Fn::body().syntax().text_range()` pattern from `crates/rmc-graph/src/graph/skeleton/source.rs` for brace-to-brace body sub-spans; (c) the **`WorkspaceHost::apply_edits`** entry point from P0.2 — does the `set_file_text` → re-extract → LMDB diff-patch pipeline, classified by D2 `EditClass`; and (d) the **`Checkpoint::take`/`Checkpoint::restore`** contract from D4. Each verb: classify the edit, compute byte-level source edits, hand a `Vec<FileEdit>` + `EditClass` to the host inside a checkpoint, translate the result into `EditOutcome` (or roll back).

## New modules / files

A new workspace crate `rmc-crud` is the cleanest home. It cannot live inside `rmc-graph` (would force `rmc-graph` to depend on `ra_ap_ide`'s rename machinery via `rmc-server::semantic`); cannot live inside `rmc-server` (would drag MCP binary surface into every consumer).

- `crates/rmc-crud/Cargo.toml` — new crate. Deps: `rmc-graph` (path), `rmc-host` (path — or the host module re-exported from rmc-graph), `rmc-semantic` (NEW crate, see below) for `SemanticService`/`RenamePreview`/`RenameEdit`/`RenameFileMove`; `ra_ap_syntax`; `thiserror` (workspace `"1"`; `rmc-crud` is a library so it gets a typed `EditError`, **not** `anyhow`); `tracing`; `tempfile` in dev-deps.
- `crates/rmc-crud/src/lib.rs` — facade re-exporting `Crud`, `EditOutcome`, `EditError`, `CascadePolicy`, `BodyEdit`, `MoveOp`, `DeleteOp`, `GraphDiffSummary`.
- `crates/rmc-crud/src/edit.rs` — pure data types.
- `crates/rmc-crud/src/source_edit.rs` — byte-level splicing helpers.
- `crates/rmc-crud/src/body_span.rs` — given `Node` + file text, returns brace-to-brace `(body_start, body_end)`. Uses `ra_ap_syntax::SourceFile::parse(..., Edition::Edition2024)` then `ast::Fn::cast(...).body().syntax().text_range()`.
- `crates/rmc-crud/src/modify_body.rs` — P1.5a.
- `crates/rmc-crud/src/move_item.rs` — P1.5b move.
- `crates/rmc-crud/src/delete.rs` — P1.5b delete.
- `crates/rmc-crud/src/preview_apply.rs` — translates `RenamePreview { edits, file_moves }` into `Vec<FileEdit>` (NEW APPLY logic — RA's preview is unapplied today; converts `(line, col)` → byte offsets via `OpenedSnapshot::line_to_byte` and sorts edits descending by `(file, byte_start)`).
- `crates/rmc-crud/src/cycle_check.rs` — pure-graph helper: walk `Node.parent_id` from `dest_parent` upward; refuse if `target.id` appears.

**Required upstream changes (cross-slice):**
1. **NEW crate `crates/rmc-semantic/`** (recommended). Promote `crates/rmc-server/src/semantic/` to its own crate. Types `SemanticService`, `RenamePreview`, `RenameEdit`, `RenameFileMove` become `pub`. `rmc-server` then depends on `rmc-semantic`.
2. `crates/rmc-server/src/semantic/mod.rs:53` — `SemanticService` → `pub`.
3. `crates/rmc-server/src/semantic/rename.rs:15,41,61` — `RenameEdit`, `RenameFileMove`, `RenamePreview` → `pub` (fields too).
4. `crates/rmc-server/src/semantic/rename.rs:70,168` — `rename_by_name`, `rename_by_position` → `pub`.
5. `crates/rmc-graph/src/graph/snapshot.rs:629` — `OpenedSnapshot::line_to_byte` → `pub`.

## Type definitions

```rust
// crates/rmc-crud/src/edit.rs

pub use rmc_host::FileEdit;   // re-export from P0.2

#[derive(Debug, Clone)]
pub struct BodyEdit {
    pub target: NodeId,
    /// MUST include outer braces. Convention: agent supplies complete block,
    /// e.g. `"{ self.x + 1 }"`. Bodies not starting with `{` and ending with `}` rejected.
    pub new_body_block: String,
}

#[derive(Debug, Clone)]
pub struct MoveOp {
    pub target: NodeId,
    pub dest_parent: NodeId,           // MUST be a Module
    pub new_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteOp {
    pub target: NodeId,
    pub cascade: CascadePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CascadePolicy {
    #[default] Refuse,
    DeleteCallers,           // bounded-depth (cap 5); recursive delete of caller fns
    DeleteUnused,            // not implemented in P1.5b; reserved
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
#[must_use]
pub struct GraphDiffSummary {
    pub nodes_added: usize, pub nodes_removed: usize,
    pub bindings_added: usize, pub bindings_removed: usize,
    pub usages_delta: i64,
}

/// Result of a successful edit. Owns the [`Checkpoint`] that can undo it (see
/// Step 7 for commit/restore semantics). `#[must_use]`: dropping it silently
/// commits the live edit, which is almost always a caller bug.
#[derive(Debug)]
#[non_exhaustive]
#[must_use]
pub struct EditOutcome {
    checkpoint: Checkpoint,
    affected_items: Vec<NodeId>,
    affected_files: Vec<PathBuf>,
    edit_class: EditClass,
    graph_diff_summary: GraphDiffSummary,
}

impl EditOutcome {
    pub fn affected_items(&self) -> &[NodeId] { &self.affected_items }
    pub fn affected_files(&self) -> &[PathBuf] { &self.affected_files }
    pub fn edit_class(&self) -> EditClass { self.edit_class }
    pub fn graph_diff_summary(&self) -> &GraphDiffSummary { &self.graph_diff_summary }
    /// Consume the outcome and undo the edit via its checkpoint.
    /// # Errors
    /// Returns [`EditError`] if the host rejects the restore.
    pub fn rollback(self, host: &mut WorkspaceHost) -> Result<(), EditError>;
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EditError {
    #[error("target node {0:?} not found in snapshot")] TargetNotFound(NodeId),
    #[error("target node {target:?} has wrong kind: expected {expected}, got {actual:?}")]
    WrongKind { target: NodeId, expected: &'static str, actual: Option<ItemKind> },
    #[error("target has no span/file recorded (likely macro-generated)")] TargetHasNoSource(NodeId),
    #[error("body splice failed: {reason}")] BodySpliceFailed { reason: String },
    #[error("new body must start with '{{' and end with '}}': got {first_bytes:?}…{last_bytes:?}")]
    BodyConvention { first_bytes: String, last_bytes: String },
    #[error("rust-analyzer refused the rewrite: {reason}")] RaRefused { reason: String },
    #[error("item has live references; cascade=Refuse")] RefsExist { refs: Vec<Binding>, usages: Vec<Usage> },
    #[error("move would introduce a module cycle")] ModuleCycle,
    #[error("destination already contains '{path}'")] PathConflict { path: String },
    #[error("warm-host apply rejected the edit: {0}")] HostRejected(String),
    // No blanket `#[from] io::Error`: that loses *which* file and *which* op
    // failed. Carry path + a static op label and preserve the cause via `#[source]`.
    #[error("io error during {op} on {path}: {source}")]
    Io { path: PathBuf, op: &'static str, #[source] source: io::Error },
    #[error("cascade depth limit (5) exceeded")] CascadeDepthExceeded,
}
```

```rust
// crates/rmc-crud/src/lib.rs

pub mod edit;
mod body_span; mod source_edit; mod modify_body; mod move_item;
mod delete; mod preview_apply; mod cycle_check;
pub use edit::*;

// Separate lifetimes: the two `&mut` borrows (`host`, `semantic`) must NOT be
// pinned to the shared-snapshot lifetime `'snapshot`. Tying them to one `'a`
// would force `host`/`semantic` to live exactly as long as `&snapshot`, which
// blocks re-borrowing the host after the snapshot read window closes. Fields
// are private; construct via `new`, read via accessors.
pub struct Crud<'host, 'snapshot, 'semantic> {
    host: &'host mut WorkspaceHost,
    snapshot: &'snapshot OpenedSnapshot,
    semantic: &'semantic mut SemanticService,
    workspace_root: PathBuf,
}

impl<'host, 'snapshot, 'semantic> Crud<'host, 'snapshot, 'semantic> {
    pub fn new(
        host: &'host mut WorkspaceHost,
        snapshot: &'snapshot OpenedSnapshot,
        semantic: &'semantic mut SemanticService,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self { host, snapshot, semantic, workspace_root: workspace_root.into() }
    }

    /// # Errors
    /// Returns [`EditError`] if the target is missing/wrong-kind/macro-generated,
    /// the body convention is violated, or the warm host rejects the apply.
    pub fn modify_body(&mut self, op: BodyEdit) -> Result<EditOutcome, EditError>;
    /// # Errors
    /// Returns [`EditError`] on non-module destination, a module cycle, a path
    /// conflict, an RA refusal, or a host rejection.
    pub fn move_item(&mut self, op: MoveOp) -> Result<EditOutcome, EditError>;
    /// # Errors
    /// Returns [`EditError`] on live references with `cascade = Refuse`, a
    /// cascade-depth overflow, or a host rejection.
    pub fn delete(&mut self, op: DeleteOp) -> Result<EditOutcome, EditError>;
}
```

## Step-by-step implementation

### P1.5a — `modify_body`

**Step 1 — Resolve target + validate kind.** WHERE: `modify_body.rs::run`.
```rust
let read_txn = crud.snapshot.read_txn()?;
let node = crud.snapshot.node(&read_txn, op.target)?
    .ok_or(EditError::TargetNotFound(op.target))?;
let kind = node.item_kind;
if !kind.map(|k| k.is_callable()).unwrap_or(false) {
    return Err(EditError::WrongKind { target: op.target, expected: "callable", actual: kind });
}
let (item_start, item_end) = node.span.ok_or(EditError::TargetHasNoSource(op.target))?;
let rel_file = node.file.clone().ok_or(EditError::TargetHasNoSource(op.target))?;
drop(read_txn);
```
DEPENDS: `ItemKind::is_callable` (`model.rs:50`). VERIFY: `modify_body_rejects_non_fn`.

**Step 2 — Convention check.**
```rust
let body = op.new_body_block.trim_start();
let trailing = op.new_body_block.trim_end();
if !body.starts_with('{') || !trailing.ends_with('}') {
    return Err(EditError::BodyConvention {
        first_bytes: body.chars().take(4).collect(),
        last_bytes:  trailing.chars().rev().take(4).collect::<String>().chars().rev().collect(),
    });
}
```
VERIFY: unit test on `"self.x + 1"` returns `BodyConvention`.

**Step 3 — Find body sub-span.** WHERE: `body_span.rs`. Takes a **pre-validated
span** `(item_start, item_end)` (resolved in Step 1 via
`node.span.ok_or(EditError::TargetHasNoSource(..))`) — never reaches back into
`node.span`, so there is no `unwrap()` on a `None` span (that case is already
modeled as `EditError::TargetHasNoSource`).
```rust
pub(crate) fn body_byte_range(
    file_text: &str,
    item_span: (u32, u32),
) -> Result<(u32, u32), EditError> {
    use ra_ap_syntax::{SourceFile, Edition, TextRange, TextSize, ast, AstNode};
    let parse = SourceFile::parse(file_text, Edition::Edition2024);
    if !parse.errors().is_empty() {
        return Err(EditError::BodySpliceFailed {
            reason: format!("source already has {} parse errors before edit", parse.errors().len()),
        });
    }
    let parsed = parse.tree();
    let (s, e) = item_span;
    let wanted = TextRange::new(TextSize::from(s), TextSize::from(e));
    let fn_syntax = parsed.syntax().descendants()
        .filter_map(ast::Fn::cast)
        .find(|f| {
            let r = f.syntax().text_range();
            r == wanted || r.contains_range(wanted) || wanted.contains_range(r)
        })
        .ok_or_else(|| EditError::BodySpliceFailed {
            reason: "could not locate ast::Fn matching the node's span".into(),
        })?;
    let body = fn_syntax.body().ok_or_else(|| EditError::BodySpliceFailed {
        reason: "fn is a trait-declaration only (no body)".into(),
    })?;
    let r = body.syntax().text_range();
    Ok((u32::from(r.start()), u32::from(r.end())))
}
```
DEPENDS: `ra_ap_syntax` (already in `rmc-graph` deps; mirror version in `rmc-crud`). The caller passes the span it already validated; `body_byte_range` cannot panic on a missing span. VERIFY: `body_byte_offsets_correct` on `pub fn foo() -> u32 { 1 + 2 }`.

**Step 4 — Take checkpoint before writing.**
```rust
let checkpoint = Checkpoint::take(crud.host)
    .map_err(|e| EditError::HostRejected(format!("checkpoint failed: {e}")))?;
```
DEPENDS: D4 contract. VERIFY: contract test `Checkpoint::take` + immediate `restore` is no-op.

**Step 5 — Compute new file text in memory.**
```rust
let abs_path = crud.workspace_root.join(&rel_file);
let original = std::fs::read_to_string(&abs_path)
    .map_err(|source| EditError::Io { path: abs_path.clone(), op: "read", source })?;
// `(item_start, item_end)` was validated in Step 1 — pass it in, no span unwrap.
let (body_start, body_end) = body_span::body_byte_range(&original, (item_start, item_end))?;
let new_text = source_edit::splice_bytes(&original, body_start as usize, body_end as usize, &op.new_body_block);
let byte_delta: i64 = (op.new_body_block.len() as i64) - ((body_end - body_start) as i64);
```
Helper:
```rust
pub(crate) fn splice_bytes(src: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(src.len() - (end - start) + replacement.len());
    out.push_str(&src[..start]);
    out.push_str(replacement);
    out.push_str(&src[end..]);
    out
}
```
Host re-extracts the file; agent does not patch downstream spans.

**Step 6 — Hand edit to host with BodyOnly class.**
```rust
let edit = FileEdit {
    path: abs_path.clone(),
    new_text,
    edit_class: EditClass::BodyOnly,
};
let apply = crud.host.apply_edits(&[edit]).map_err(|apply_err| {
    if let Err(restore_err) = Checkpoint::restore(crud.host, &checkpoint) {
        tracing::error!(?apply_err, ?restore_err, "modify_body: both apply AND restore failed");
    }
    EditError::HostRejected(format!("apply rejected: {apply_err}"))
})?;
```
**Load-bearing decision:** host owns the `fs::write`, not the CRUD layer. Keeps atomicity in one place (RA `set_file_text` + LMDB write txn + disk write under same lock).

**Step 7 — Translate `ApplyOutcome` → `EditOutcome`.** On the **success** path the
`Checkpoint` is *moved into* the returned `EditOutcome` rather than dropped — i.e.
ownership of the rollback handle transfers to the caller. This is the RAII commit
boundary: the apply already happened, so the edit is *live*; holding the checkpoint
lets the caller (gate/reward loop) either keep the result (commit by simply dropping
`EditOutcome`, which must NOT auto-restore) or explicitly call
`EditOutcome::rollback(self)` to undo it. The take/restore boundary therefore commits
on the `Ok` arm and only restores on the `Err` arms above. `EditOutcome` is
`#[must_use]` so an accidentally-discarded outcome (and its checkpoint) is caught.
```rust
Ok(EditOutcome {
    checkpoint,
    affected_items: apply.affected_node_ids,
    affected_files: apply.affected_files,
    edit_class: EditClass::BodyOnly,
    graph_diff_summary: GraphDiffSummary {
        nodes_added: apply.nodes_added,
        nodes_removed: apply.nodes_removed,
        bindings_added: apply.bindings_added,
        bindings_removed: apply.bindings_removed,
        usages_delta: apply.usages_delta,
    },
})
```
DEPENDS: `ApplyOutcome` shape from P0.2 (returns counts + `affected_node_ids` so P1.6 doesn't re-scan).

### P1.5b — `move`

**Step 8 — Resolve + validate.** WHERE: `move_item.rs::run`. Resolve `target_node` + `dest_node`; require `dest_node.kind == Module`; extract `target_kind`, `span`, `rel_src_file`; compute `new_name = op.new_name.unwrap_or_else(|| target_node.display_name.clone())`; `new_qualified = format!("{}::{}", dest_node.qualified_name, new_name)`. VERIFY: `move_item_rejects_non_module_dest`.

**Step 9 — Compute dest file + cycle check.** WHERE: `cycle_check.rs`.
```rust
pub(crate) fn would_introduce_cycle(
    snapshot: &OpenedSnapshot,
    read_txn: &GraphRoTxn<'_>,
    target: NodeId,
    dest_parent: NodeId,
) -> Result<bool, EditError> {
    let mut cursor = Some(dest_parent);
    while let Some(id) = cursor {
        if id == target { return Ok(true); }
        let node = snapshot.node(read_txn, id)?;
        cursor = node.and_then(|node| node.parent_id);
    }
    Ok(false)
}
```
Then in `run`: `if cycle_check::would_introduce_cycle(...)? { return Err(EditError::ModuleCycle); }`. VERIFY: `move_cycle_refused`.

**Step 10 — Compute identifier (line, col) for RA.** RA's `rename_by_position` needs `(file, line, column)`, not bytes. Re-parse file with `ra_ap_syntax`; find `ast::Fn::name().syntax().text_range().start()` (mirrors `declaration_name` in `skeleton/source.rs:228`); convert byte → (line, col) via `OpenedSnapshot::line_to_byte` binary search.
```rust
let abs_src_path = crud.workspace_root.join(&rel_src_file);
let file_text = std::fs::read_to_string(&abs_src_path)
    .map_err(|source| EditError::Io { path: abs_src_path.clone(), op: "read", source })?;
let ident_byte_offset = body_span::identifier_byte_offset(&file_text, &target_node)?;
let (line, col) = byte_offset_to_line_col(&file_text, ident_byte_offset);
let preview = crud.semantic.rename_by_position(
    &crud.workspace_root, &abs_src_path, line, col,
    &target_node.display_name, &new_name,
).map_err(|source| EditError::RaRefused { reason: source.to_string() })?;
```
**UPSTREAM CHANGES REQUIRED** — see top-level visibility list above.

**Step 11 — Translate RenamePreview → FileEdits.** WHERE: `preview_apply.rs`.

**Coordinate assumption (documented):** RA's `RenameEdit` line/column are
**1-based** and `line_to_byte` is indexed by **0-based** line. `start_column`
is a **byte** column offset *within* the line (UTF-8 byte count from the line
start, not a Unicode scalar count) — `line_to_byte[line] + (column - 1)` is a
valid byte offset only under that assumption; a `RenameEdit::column_is_bytes()`
invariant on the `rmc-semantic` side documents/guarantees it. Every 1-based →
0-based step uses `checked_sub` (so a stray `0` becomes an `EditError`, not an
underflow panic), and every `line_to_byte` index is bounds-checked with `.get`.
```rust
pub(crate) fn preview_to_file_edits(
    snapshot: &OpenedSnapshot,
    workspace_root: &Path,
    preview: &RenamePreview,
) -> Result<Vec<FileEdit>, EditError> {
    // Resolve one 1-based (line, byte-column) RA coordinate into a 0-based byte
    // offset, rejecting 0-coordinates and out-of-range lines instead of panicking.
    fn resolve_byte(
        line_to_byte: &[u32],
        line_1based: u32,
        col_1based: u32,
    ) -> Result<u32, EditError> {
        let line0 = line_1based.checked_sub(1).ok_or_else(|| EditError::BodySpliceFailed {
            reason: format!("RA line {line_1based} is below 1 (1-based expected)"),
        })?;
        let col0 = col_1based.checked_sub(1).ok_or_else(|| EditError::BodySpliceFailed {
            reason: format!("RA column {col_1based} is below 1 (1-based expected)"),
        })?;
        let line_start = line_to_byte.get(line0 as usize).copied().ok_or_else(|| {
            EditError::BodySpliceFailed {
                reason: format!("RA line {line_1based} out of range ({} lines)", line_to_byte.len()),
            }
        })?;
        Ok(line_start + col0)
    }

    let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32, String)>> = BTreeMap::new();
    for edit in &preview.edits {
        let rel = edit.file_path.strip_prefix(workspace_root).unwrap_or(&edit.file_path);
        let line_to_byte = snapshot.line_to_byte(rel.to_string_lossy().as_ref())?;
        let start_byte = resolve_byte(&line_to_byte, edit.start_line, edit.start_column)?;
        let end_byte   = resolve_byte(&line_to_byte, edit.end_line,   edit.end_column)?;
        by_file.entry(edit.file_path.clone()).or_default()
            .push((start_byte, end_byte, edit.new_text.clone()));
    }
    let mut out = Vec::new();
    for (path, mut edits) in by_file {
        edits.sort_by(|a, b| b.0.cmp(&a.0));        // descending so earlier splices keep offsets
        let mut text = std::fs::read_to_string(&path)
            .map_err(|source| EditError::Io { path: path.clone(), op: "read", source })?;
        for (start, end, replacement) in &edits {
            text = source_edit::splice_bytes(&text, *start as usize, *end as usize, replacement);
        }
        out.push(FileEdit { path, new_text: text, edit_class: EditClass::ModuleTree });
    }
    Ok(out)
}
```
DEPENDS: `OpenedSnapshot::line_to_byte` must be `pub`.

**Step 12 — Source-file move (delete old, insert new).** Two cases:
(a) **Same-file move:** RA's `rename` doesn't handle item-level moves. Manually cut bytes `[item_start..item_end]`, insert at end of dest module's range (`dest_end - 1` before closing `}` or end of file for file-modules).
(b) **Cross-file move:** delete from src, append to dest with newline+indent.

```rust
let dest_rel_file = dest_node.file.clone().ok_or(EditError::TargetHasNoSource(op.dest_parent))?;
let same_file = dest_rel_file == rel_src_file;
let item_text = file_text[item_start as usize .. item_end as usize].to_string();
let mut src_new_text = source_edit::delete_byte_range(&file_text, item_start as usize, item_end as usize);
src_new_text = source_edit::collapse_blank_lines(&src_new_text, item_start as usize);
let dest_file_text = if same_file {
    src_new_text.clone()
} else {
    let dest_abs = crud.workspace_root.join(&dest_rel_file);
    std::fs::read_to_string(&dest_abs)
        .map_err(|source| EditError::Io { path: dest_abs, op: "read", source })?
};
let insertion_point = compute_dest_insertion_byte(&dest_file_text, &dest_node);
let dest_new_text = source_edit::insert_at_byte_offset(&dest_file_text, insertion_point, &format!("\n\n{}\n", item_text));

let mut file_edits = preview_to_file_edits(crud.snapshot, &crud.workspace_root, &preview)?;
upsert_file_edit(&mut file_edits, FileEdit { path: abs_src, new_text: src_new_text, edit_class: EditClass::ModuleTree });
if !same_file {
    upsert_file_edit(&mut file_edits, FileEdit { path: abs_dst, new_text: dest_new_text, edit_class: EditClass::ModuleTree });
}
```

**Step 13 — EditClass selection.** Cross-file or rename → `ModuleTree`. Pure no-op (same file + no rename) → `SigOrVis` (shouldn't happen — early-out).

**Step 14 — Checkpoint + apply + finalize.** Same pattern as Step 6/7.

### P1.5b — `delete`

**Step 15 — Resolve target.** Same shape as Step 1/8.

**Step 16 — Ref-check.**
```rust
let refs   = crud.snapshot.who_imports(op.target)?;
let usages = crud.snapshot.usages_of(op.target)?;
if (!refs.is_empty() || !usages.is_empty()) && matches!(op.cascade, CascadePolicy::Refuse) {
    return Err(EditError::RefsExist { refs, usages });
}
```
DEPENDS: `who_imports` (`query/usage.rs:798`), `usages_of` (line 802) — both already `pub`. VERIFY: `delete_refuses_with_refs`.

**Step 17 — Cascade plan (DeleteCallers).**
```rust
let mut deletions: Vec<NodeId> = vec![op.target];
if matches!(op.cascade, CascadePolicy::DeleteCallers) {
    cascade_collect(&mut deletions, crud.snapshot, op.target, 0)?;
}
fn cascade_collect(
    out: &mut Vec<NodeId>,
    snapshot: &OpenedSnapshot,
    target: NodeId,
    depth: u8,
) -> Result<(), EditError> {
    const MAX_DEPTH: u8 = 5;
    if depth >= MAX_DEPTH { return Err(EditError::CascadeDepthExceeded); }
    let usages = snapshot.usages_of(target)?;
    let caller_fns: HashSet<NodeId> =
        usages.iter().filter_map(|usage| usage.consumer_function).collect();
    for caller in caller_fns {
        if !out.contains(&caller) {
            out.push(caller);
            cascade_collect(out, snapshot, caller, depth + 1)?;
        }
    }
    Ok(())
}
```
DEPENDS: `Usage.consumer_function` (`model.rs:193`). VERIFY: cascade test.

**Step 18 — Per-file deletion edits.** Group by `Node.file`, sort ranges descending within each file:
```rust
let read_txn = crud.snapshot.read_txn()?;
let mut by_file: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
for id in &deletions {
    if let Some(node) = crud.snapshot.node(&read_txn, *id)? {
        if let (Some(file), Some(span)) = (node.file.clone(), node.span) {
            by_file.entry(file).or_default().push(span);
        }
    }
}
drop(read_txn);
let mut file_edits = Vec::new();
for (rel_file, mut ranges) in by_file {
    ranges.sort_by(|a, b| b.0.cmp(&a.0));
    let abs = crud.workspace_root.join(&rel_file);
    let mut text = std::fs::read_to_string(&abs)
        .map_err(|source| EditError::Io { path: abs.clone(), op: "read", source })?;
    for (start, end) in &ranges {
        text = source_edit::delete_byte_range(&text, *start as usize, *end as usize);
    }
    file_edits.push(FileEdit { path: abs, new_text: text, edit_class: EditClass::ItemAddRemove });
}
```
*Optional:* drop `mod foo;` if delete removed last item from a child file (out of MVP scope).

**Step 19 — Checkpoint + apply + return.** Identical pattern. `EditClass::ItemAddRemove` (or `ModuleTree` if removing a module file).

## Tests

(`crates/rmc-crud/tests/`)

1. **`modify_body_roundtrip`** — 2-crate fixture (`producer` exporting `pub fn add(a, b)`, `consumer` calling it). Cold-build; resolve `add` via `lookup_by_qualified_name("producer::add")`; call `Crud::modify_body(BodyEdit { target, new_body_block: "{ a.wrapping_add(b) }".into() })`. Then: (a) file body replaced; (b) `usages_of(add)` count unchanged; (c) cold-rebuild against post-edit source matches incremental state on affected crate (differential test mandated by Issue #3).

2. **`modify_body_rollback_on_compile_break`** — body that breaks the parse: `"{ a + }"`. `apply_edits` rejects → `Checkpoint::restore` → file bytes match pre-edit + `Node` span identical.

3. **`move_updates_imports`** — `core_crate::utils::foo` moved to `core_crate::helpers::foo`; consumer crate had `use core_crate::utils::foo;`. After: (a) `lookup_by_qualified_name("core_crate::helpers::foo")` resolves; (b) `imports_of(consumer_module)` returns binding with `target = foo_id`, `visible_name = "foo"`; (c) consumer file contains `use core_crate::helpers::foo;`.

4. **`delete_refuses_with_refs`** — same `producer`/`consumer`. `Crud::delete(DeleteOp { target: add_id, cascade: Refuse })` → `Err(EditError::RefsExist { refs, usages })`. Then `cascade: DeleteCallers` succeeds; consumer's caller fn deleted; `who_imports(add_id)` empty.

5. **`move_cycle_refused`** — `core_crate::a::b::c`; `MoveOp { target: a_id, dest_parent: c_id }` → `Err(EditError::ModuleCycle)` without file mutation.

6. **`body_byte_offsets_correct`** — pure unit test: (a) `pub fn foo() { return 1; }` → body range includes braces and `return 1;`; (b) two fns in same file `pub fn foo(){}\npub fn bar(){ panic!() }` — `bar`'s byte range does not shift after splicing longer body into `foo`.

7. **`preview_apply_byte_offsets_match_line_col`** — synthetic `RenamePreview { edits: vec![RenameEdit { start_line: 3, start_column: 5, end_line: 3, end_column: 8, new_text: "BAR".into() }] }`; assert resulting `FileEdit.new_text` has `"BAR"` at the byte offset `(line=3, col=5)` resolves to via `line_to_byte`.

8. **`cross_file_apply_ordering`** — 3-file fixture with multiple non-overlapping edit positions per file; verify three `FileEdit`s with all positions correctly spliced (descending-sort trick).

## Open decisions / risks

- **Body-span source-of-truth.** `Node.span` covers the whole item; we re-parse on every `modify_body`. 100k-LOC files parse in ~50ms — acceptable for ~500ms P0.2 target. If hot: cache parses by `(file_path, file_mtime)`. Alternative (rejected): store `body_span: Option<(u32, u32)>` on `Node` (LMDB bloat + schema bump).
- **`syn` vs `ra_ap_syntax`.** Use `ra_ap_syntax` — codebase already uses it; same edition handling; RA's error recovery on partially-broken files.
- **Applying RA's RenamePreview is net-new.** `SemanticService::rename_by_position` is preview-only today. P1.5b adds APPLY logic; complexity is the `(line, col) → byte` conversion. RA's `LineCol` is 0-indexed then `+1`'d in `source_change_to_preview` (`rename.rs:296-301`); we `-1` on the way back. Test 7 pins this down.
- **RA's `FileSystemEdit::CreateFile` / `MoveFile`.** Appear in `RenamePreview.file_moves` for module renames. For P1.5b move we're moving items, not modules — should be empty. Defensive: if `preview.file_moves` non-empty → `EditError::RaRefused { reason: "RA proposed file move; not supported in P1.5b" }`. Lift in P1.5e.
- **DeleteCallers cascade depth.** Hard cap 5. Above → `CascadeDepthExceeded`. Not configurable in MVP — predictable behavior for reward signal.
- **`new_body_block` convention.** Braces required. Zero ambiguity; byte-range we splice IS the braced range; easy to validate.
- **Source-write ownership.** Host owns `fs::write` (recommended) — atomicity in one place. Fallback: CRUD does `fs::write` then `host.notify_files_changed(...)`. Lock this in D4 contract before P0.2 ships.
- **Upstream visibility changes.** Cleanest: extract `rmc-server::semantic` to new `rmc-semantic` crate (one PR; `rmc-server` imports from `rmc_semantic::`). Smaller intervention: add `pub mod semantic_api` re-export with the `pub use` items — but `rmc-crud` then depends on `rmc-server` (a binary host crate). **Recommend crate-extraction.**
- **Multi-file transactionality.** Each verb takes one `Checkpoint::take`, submits one `host.apply_edits(&[...])` with full edit set. Host implements all-or-nothing per D4. CRUD calls `Checkpoint::restore` on `Err` arm.
- **D2 BodyOnly assumption.** Body-only = editing fn's outgoing usages only, no reverse-dep walk. If M0 spike #1 shows body edits still invalidate cross-crate inference, `modify_body` latency tracks P0.2's actual incremental performance; CRUD code unchanged.


---

