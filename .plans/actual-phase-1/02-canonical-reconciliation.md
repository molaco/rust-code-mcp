# Canonical Reconciliation — Single Source of Truth

This plan was synthesized from independent per-slice subagent designs.
Sections B and C were authored separately and **each** declared the D1–D4
types; the Errata fixed eight semantic blockers but did not dedup those
declarations, so the same type appears two or three times with different
names, shapes, and module homes. This section picks **one** of each and
names what to delete.

**Precedence (highest first): this section → Errata (E1–E8) → body
(Sections Z, A–J).** Where they disagree, the higher authority wins. The
four scrubs in §R5 are already applied to the body.

## §R1 — Canonical module layout (resolves B ⇄ C ⇄ Z)

Three layouts existed: B's `working/ affected/ checkpoint/`, C's
everything-under-`host/`, and Z's `working/ host/ checkpoint/`. **Z's
file-tree is canonical** — it is the most complete and matches Z's own
re-export list (`pub use host::affected_set::AffectedSet`, etc.). All paths
are under `crates/rmc-graph/src/graph/`:

```
graph/
  working/                  D1 working snapshot, D4 undo log, D3 patch APPLY helpers
    snapshot.rs             WorkingSnapshot, init_from_published, publish_as_new_graph_id
    identity.rs             SessionId, WorkingSnapshotIdentity
    undo_log.rs             UndoLog, UndoBatch, UndoOp, UndoMarker        (C's in-memory design)
    patch.rs                DiffPatch + compute_patch/apply_patch + the D3 matrix
                            (SubDb, InvalidationAction, InvalidationRule, invalidations_for, ALL_SUB_DBS)
    patch/                  submodules of patch.rs (file-based, no mod.rs — §10)
      nodes.rs  bindings.rs  usages.rs  contains.rs  signatures.rs  statics.rs  meta.rs
  host/                     P0.2 warm host, D2 classifier + affected-set, per-crate re-extract
    workspace_host.rs       WorkspaceHost, FileEdit, EditSeq, apply_edits
    edit_class.rs           EditClass (canonical variants), classify()
    affected_set.rs         AffectedSet, ReverseDepGraph, expand()/affected_set()
    extract_per_crate.rs    extract_partial, PartialExtractionModel
  checkpoint.rs             Checkpoint (C's fields), take()
  checkpoint/               D4 checkpoint contract — submodules (file-based, no mod.rs — §10)
    jj.rs                   jj op log/restore wrappers
    restore.rs              WorkspaceHost::rollback / restore replay
  view/  descriptions/  analyze/        P1.1 / P1.2 / P1.3 (unchanged)
```

There is **no `graph/affected/`**, no `working/patch/mod.rs`, no
`checkpoint/mod.rs`, and none of `host/edits.rs`, `host/diff_patch.rs`,
`host/re_extract.rs`, `host/rollback.rs`, `checkpoint/undo.rs` — those are
superseded B/C filenames (§R6). Per §10 the canonical homes are the
sibling files `working/patch.rs` and `checkpoint.rs` with their submodules
in the same-named directories (never `mod.rs`).

## §R2 — Canonical core types (resolves the duplicate D1–D4 declarations)

| Concept | CANONICAL | Home | Superseded |
|---|---|---|---|
| Edit class | `#[non_exhaustive] EditClass { BodyOnly, SignatureOrVis, ItemAddRemove, ModuleTree, Macro, CargoManifest }` | `host/edit_class.rs` | B `{…, SigOrVis, …, Cargo}`; C `{Body, Signature, …, CargoManifest}` |
| Host edit input | `FileEdit { path: ws-rel, new_text, edit_class }` (host trusts the class; fields private + accessors) | `host/workspace_host.rs` | B's `Edit` enum + `classify(&Edit)` — the verb sets the class by construction; no diff-inference |
| Affected set | `#[non_exhaustive] AffectedSet { dirty_files, dirty_crates, reverse_dep_crates, full_rebuild }` (struct; fields private + accessors) | `host/affected_set.rs` | C's `affected_crates() -> Vec<NodeId>` (becomes the builder that returns `AffectedSet`) |
| Undo log | **in-memory** `UndoLog { batches: Vec<UndoBatch> }`; `UndoOp` per primary + per DUP_SORT secondary | `working/undo_log.rs` | B's on-disk `BufWriter` `UndoLog`, `UndoEntry`, byte-offset marker |
| Undo marker | `UndoMarker(EditSeq)` — pop batches with `seq > marker` | `working/undo_log.rs` | B's `UndoLogMarker { byte_offset, entry_count }` |
| Checkpoint | `#[non_exhaustive] Checkpoint { jj_op_id: JjOpId, file_prior_text: HashMap<PathBuf,String>, edit_seq_marker: EditSeq }` — fields private + accessors | `checkpoint.rs` | B's `{ jj_op_id: JjOpId, undo_log_marker, ra_edit_seq, caches }` |
| D3 matrix | `#[non_exhaustive]` `SubDb`, `#[non_exhaustive]` `InvalidationAction`, `InvalidationRule`, `invalidations_for(class)`, `ALL_SUB_DBS` | `working/patch.rs` | B's `affected/matrix.rs` (same content, new home) |
| Diff/patch | `#[non_exhaustive] DiffPatch { node_inserts/updates/removes, … }` — fields private + accessors — `compute_patch`/`apply_patch` | `working/patch.rs` | C's `host/diff_patch.rs` (same content, new home) |

Rationale for the load-bearing picks:
- **EditClass names** follow the source plan (`phase-1-implementation.md`)
  and `m0-spikes.md` prose (`BodyOnly`, `SignatureOrVis`, `CargoManifest`).
  B's D3 `invalidations_for` and E4's table use the abbreviations
  `SigOrVis`/`Cargo` — read those as `SignatureOrVis`/`CargoManifest`.
- **In-memory UndoLog** is sufficient: crash-recovery is "drop the
  working-snapshot dir and re-`mdb_copy` from the published base" (the
  slow-path bailout already in Section C). Durability buys nothing the
  recopy doesn't, and the `Vec<UndoBatch>` is what C's apply/rollback
  (Steps 7, 11) and G/J already use.
- **Checkpoint = C's shape** because `file_prior_text` is the *mechanism*
  RA restore needs (replay `set_file_text` with prior text); B's bare
  `ra_edit_seq` can't restore RA without it. `edit_seq_marker` doubles as
  the undo-log marker (the log is keyed by `EditSeq`), so all three D4
  domains are covered: source = `jj_op_id`, graph + RA-seq =
  `edit_seq_marker`, RA-replay data = `file_prior_text`.
- **`jj_op_id` keeps B's `JjOpId` newtype** (DD-4): the field is an opaque
  jj operation-log handle, not free text, so it stays a domain newtype
  rather than a stringly-typed ID (§7) — an earlier draft downgraded it to
  `String`; that regression is reverted here. The newtype has a private
  inner field + accessor.

**Consequence for `PartialExtractionModel`** (`extract_per_crate.rs`): it
must carry the affected-set context E4's scan-window reads. Canonical
(fields **private + accessors**, `#[non_exhaustive]` — it grows as
re-extraction widens; §5/§7):
```rust
#[non_exhaustive]
pub struct PartialExtractionModel {
    edit_class: EditClass,            // NEW — E4 reads partial.edit_class()
    dirty_crates: Vec<NodeId>,
    reverse_dep_crates: Vec<NodeId>,  // NEW — E4 reads partial.reverse_dep_crates()
    nodes: BTreeMap<NodeId, Node>,
    bindings: Vec<Binding>,
    usages: Vec<Usage>,
    contains: Vec<(NodeId, NodeId)>,
    signatures: Vec<(NodeId, FunctionSignature)>,
    statics: Vec<(NodeId, StaticMetadata)>,
}
// accessors: edit_class(&self), dirty_crates(&self), reverse_dep_crates(&self), …
```
The builder copies `edit_class` / `dirty_crates` / `reverse_dep_crates`
from the `AffectedSet`; E4's `partial.edit_class()` /
`.reverse_dep_crates()` accessors then resolve as written.

## §R3 — One M0.1 deliverable, not two

Sections B ("type-first contracts") and C ("warm host") both define D1–D4.
**B is the home of the canonical declarations; C consumes them by `use`,
never re-declares them.** M0.1 (Section B) ships the §R2 types at the §R1
homes; M2a (Section C) implements the methods (`apply_edits`,
`compute_patch`, `apply_patch`, `rollback`, `extract_partial`) against
those exact types. Where Section C's text appears to re-declare `EditClass`
/ `UndoLog` / `Checkpoint`, read it as:
```rust
use crate::graph::{
    host::edit_class::EditClass,
    working::undo_log::UndoLog,
    checkpoint::Checkpoint,
};
```
Section B's `working::patch` helper signatures (the M0.1 exit gate) are the
`working/patch/*` files in §R1.

## §R4 — Canonical crate set

| Crate | Status | Note |
|---|---|---|
| `rmc-semantic` | **NEW, mandatory** | rename engine extracted from `rmc-server::semantic`; breaks the `rmc-server` ⇄ `rmc-crud` cycle (rmc-server deps rmc-crud via `rl`; rmc-crud needs the rename engine). M2a prereq. |
| `rmc-host` | optional, **default SKIP** | keep `WorkspaceHost` in `rmc-graph::graph::host`; extract only if a real circular dep appears. G's `pub use rmc_host::FileEdit` reads as `pub use rmc_graph::graph::host::FileEdit`. |
| `rmc-spikes`, `rmc-crud`, `rmc-gates`, `rmc-reward`, `rmc-episode`, `rmc-rl` | new | as Section Z. |

**`prettyplease` is banned** (E5) — removed from every dep list and the
workspace `Cargo.toml`. `syn` / `ra_ap_syntax` are for byte-range
**analysis only**; replacement text is string-built and spliced.
`toml_edit` is kept (format-preserving, not a whole-file formatter).

**Section H has been converted off `prettyplease::unparse`** (resolved).
Every verb body (`modify_signature`, `extract_*`, `inline`, `*_module`,
`lift/lower_to_crate`) now locates the target byte range with
`syn`/`ra_ap_syntax` (analysis only), builds the replacement string from the
op's fields, and `splice_bytes` — no AST unparse anywhere. The `syn`
`printing` feature and the `quote` / `proc-macro2` codegen deps are dropped
(they existed only to support unparse). E5 sketches the splice for
§3/5/9/11/12/16; Section H's file lists, step order, and tests carry the rest.

## §R5 — Scrubs applied to the body

1. **rmc-semantic extracted** — Z crate inventory, `members`, file-tree,
   and rmc-crud deps updated; rmc-crud deps `rmc-semantic`, not `rmc-server`.
2. **prettyplease removed** — rmc-crud deps, rmc-graph deps, workspace
   `Cargo.toml` diff.
3. **`Episode` de-self-referenced** — Section J's `Episode` no longer
   stores `Commit<'static>` / owned `Crud` / `Navigator`; it stores owned
   config + `host`/`snap`/`semantic` and builds the borrowing structs
   per-step (matches E2).
4. **E7 `affected/` removed** — D2/D3 live under `host/` + `working/patch/`
   per §R1; E7's stray `affected/` dir and `pub mod affected;` deleted.

## §R6 — Superseded names (grep map for Sections B/C)

| Body text | Read as |
|---|---|
| `EditClass::SigOrVis`, `::Signature` | `EditClass::SignatureOrVis` |
| `EditClass::Cargo` | `EditClass::CargoManifest` |
| `EditClass::Body` (Section C) | `EditClass::BodyOnly` |
| `affected/edit.rs`, `affected/set.rs`, `affected/matrix.rs` | `host/edit_class.rs`, `host/affected_set.rs`, `working/patch.rs` |
| `host/edits.rs`, `host/diff_patch.rs`, `host/re_extract.rs`, `host/rollback.rs` | `host/workspace_host.rs`, `working/patch.rs`, `host/extract_per_crate.rs`, `checkpoint/restore.rs` |
| `checkpoint/mod.rs`, `checkpoint/checkpoint.rs`, `checkpoint/undo.rs` | `checkpoint.rs` (home; no `mod.rs` — §10), `checkpoint.rs`, `working/undo_log.rs` |
| `working/patch/mod.rs` | `working/patch.rs` (home; no `mod.rs` — §10) |
| B's `Edit` enum + `classify(&Edit)` | build `FileEdit { edit_class }` directly in the verb |
| `Commit<'static>` stored in `Episode` (C) | per-step `Commit<'_>` (E2 / scrub #3) |
| `rmc_host::FileEdit` | `rmc_graph::graph::host::FileEdit` (rmc-host skipped) |
| `OpenedWorkingSnapshot` (Section J) | `WorkingSnapshot` (D1 — already an opened env+dbs handle) |
| `prettyplease` | (removed — string-splice, E5) |
| `OpenedSnapshot::line_to_byte` @ `snapshot.rs:629` | real loc `snapshot.rs:665` |

---

