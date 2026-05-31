# Follow-Up Plan — Close the Refactoring-Semantics Gap

Remediation plan for the `../rust-code-mcp-67` implementation of
`.plans/subset-plan.md`. Authored after the per-phase review; targets the
gaps that keep it from being a real *reference-aware* refactoring toolset.

> Devshell: every shell command runs under
> `nix develop ../nix-devshells#cuda-code --command <cmd>`.
> Implementation root: `/home/molaco/Documents/rust-code-mcp-67`.

---

## 1. Where we are

Review verdict (combined): **~7/10**. The warm-host engine (M1) is genuinely
strong, M0/M2/M4 hold, and the new crates compile with real differential/rollback
tests. The shortfall is the **G/H semantic layer (4/10)**: the verbs relocate
*text* and **punt reference updates** to `pub use` facades or refusals, the
extracted `rmc-semantic` rename engine is a **dead dependency** in `rmc-crud`,
`modify_signature` **refuses by default** instead of rewriting callsites, and the
workspace is **red under `--all-targets`** (two stale legacy test callers).

Goal of this plan: bring it to **implementation-ready (~9/10)** — references are
rewritten (not faked), the build is green, and the error idioms are tightened.

---

## 2. Findings → waves (by leverage)

| Wave | Finding (severity) | Outcome |
|---|---|---|
| **F1** | Red `--all-targets` build (MED) | workspace test build green |
| **F2** | No reference-edit substrate (HIGH) | one shared way to enumerate reference sites as byte-edits |
| **F3** | `move` + module ops facade-punt references (HIGH) | importer `use` paths + inline refs rewritten |
| **F4** | `modify_signature` refuses by default (HIGH) | rewrites callsites by default, multi-file |
| **F5** | No multi-reference test coverage (HIGH) | ≥2-site/≥2-file fixtures per semantic verb |
| **F6** | `rmc-semantic` dead dep + idiom/hygiene debt (MED/LOW) | dep wired or removed; anyhow leaks + nits cleaned |

---

## 3. Guiding constraints (unchanged idioms + new)

- Keep the Sans-I/O split: reference enumeration is **pure** (in `compute_effects`),
  applied in `apply_effects`. `simulate` must still equal `apply`.
- Edits remain **byte-splice only** (no formatters); multi-edit per file sorted
  **descending by byte offset**, char-boundary-checked, overlap-guarded.
- Typed `thiserror` errors; no new `anyhow` in libs; `#[non_exhaustive]`.
- **New — `ReferencePolicy`** (config-facing, lives in `rmc-config` per DD-D):
  `Rewrite` (default) | `LeaveFacade` | `Refuse`. The facade becomes an explicit
  opt-in, not the silent default. **Thread it end-to-end** — a field on the
  move/module `Crud` variants AND the server refactor params (with schema
  descriptions) AND simulate/apply parity tests; today `params/refactor.rs` has
  no policy field, so the enum would be unreachable from the tool surface.
- **New — reference source of truth (TWO-PRONGED; the graph does NOT carry import
  spans):** inline qualified references carry byte ranges in `Usage` rows
  (`who_uses`/`usages_of` → `file/start/end`) and can be byte-spliced. **`use`
  imports do NOT** — `Binding` rows carry only `from_module`/`visible_name`/
  `target`, no span (`model.rs`) — so import rewriting needs an AST `UseTree` pass
  per importer module (F2), not graph byte ranges. Reserve `rmc-semantic`'s RA
  `RenamePreview` for the **name** component of a rename; use `Usage` rows
  (inline) + `UseTree` rewrites (imports) for the **path** component of a move.

---

## 4. Waves

### F1 — Green the build (do first; ~½ day)
`index_codebase` now takes **four** args —
`(params, sync_manager: Option<&Arc<SyncManager>>, workspace_locks: &WorkspaceLockRegistry, search_cache: Option<&SearchRuntimeCache>)`
(`endpoints/index.rs:175`) — but legacy tests still call `index_codebase(params, None)`.
- Fix **all three** stale test files (grep `index_codebase(` under
  `crates/rust-code-mcp/tests/`): `test_gpu_index_jsonrpc.rs:94`,
  `test_burn_performance.rs:30`, and the ~16 sites in
  `test_index_tool_integration.rs`. Each call keeps its existing 2nd arg as the
  `sync_manager` slot and adds the trailing `&WorkspaceLockRegistry::new()` (or
  `&runtime.workspace_locks()`) and a `search_cache` arg (`None` in tests).
- Clear the build warnings (dead `QueryEvaluation`, unused imports) from the log.
- **Exit:** `cargo check --workspace --all-targets` is clean (0 errors); `-D
  warnings` for the new crates.

### F2 — Reference-edit substrate (~3–4 days)
**Design correction (review finding #1):** the graph stores no import spans —
`Binding` has only `from_module`/`visible_name`/`target` (no `file`/`start`/
`end`); only `Usage` carries a byte range (`model.rs`). So the substrate is
two-pronged, and `use`-import rewriting needs an AST layer, not graph offsets.

Add two files in `crates/rmc-crud/src/`:
- **`use_tree.rs`** — pure `syn` `UseTree` parse + structured rewrite. Given a
  module's source and a target path/name, locate and rewrite the binding,
  handling **nested groups, globs, aliases, and shared path prefixes**:
  `rename_in_use_tree(old_name, new_name)`, `repath_in_use_tree(old_path, new_path)`;
  returns the byte edit(s) for that `use` item, or `None` if the target isn't
  bound there. This is the load-bearing new piece F3 depends on.
- **`references.rs`** — `reference_edits(snap, target, change) -> Result<Vec<FileEdit>, EditError>`,
  pure, combining:
  - **inline qualified refs:** `usages_of(target)` → `Usage` rows give
    `file/start/end` → direct byte-splice;
  - **`use` imports:** `who_imports(target)` gives the importing *modules* (no
    span) → for each, parse its source and run `use_tree.rs` to rewrite the
    binding.
- `ReferenceChange { Renamed{new_name}, Moved{new_module_path}, RenamedAndMoved{..}, Removed }`.
  Emits per-file `FileEdit`s for the existing descending-offset splicer.
- Wire `rmc-semantic` here for the `Renamed` name component (`RenamePreview` →
  `FileEdit`s) — **this kills the dead-dep finding** by giving it a real
  `rmc-crud` consumer; use `Usage` rows + `UseTree` for the `Moved` path component.
- **DD-F5 (import-span source):** AST `UseTree` scan per importer (no schema
  change) vs persisting import spans on `Binding` (schema bump → invalidates
  snapshots). Default: **AST scan** — `UseTree` handling is required for
  globs/aliases regardless, and it avoids a migration.
- **Exit:** `reference_edits` returns the correct edit set for a target
  referenced from ≥2 files via each `use` shape — `use a::{b::Target, c as d}`
  (group), `use a::Target as T` (alias: rewrite path, keep `T`, leave inline `T`
  refs), `use a::*` (glob: detect + policy-driven leave/expand), and
  fully-qualified inline `a::b::Target` — with `simulate`==`apply` on the edits.

### F3 — `move` + module ops rewrite references (~4–5 days)
Replace the facade default with real reference rewriting via the F2 substrate
(the `UseTree` layer is mandatory — `use` rewriting is structured AST editing,
not byte-range splicing — review finding #5), gated by `ReferencePolicy`
(default `Rewrite`):
- `move` (`module_ops.rs`): relocate the item text, then apply
  `reference_edits(target, Moved{new_module_path})` to every importer + inline
  use. Demote the `pub use … as …` facade to `ReferencePolicy::LeaveFacade`.
  Optional `new_name` routes the rename component through `rmc-semantic`.
- `split_module`/`merge_modules`/`move_module`/`lift_to_crate`/`lower_to_module`:
  rewrite dependent `use` paths via the `UseTree` layer instead of `pub use ::*`
  shims. Facade opt-in only.
- **Thread `ReferencePolicy`** onto these `Crud` variants AND the server refactor
  params (schema descriptions) AND simulate/apply parity — `params/refactor.rs`
  has no policy field today (review finding #2), so without this the enum is
  unreachable from the tool surface.
- Keep refusal only for genuinely unresolvable cases (macro-generated refs,
  `dyn`/generic indirection); surface as typed
  `EditError::UnresolvedReference { sites }` so the caller sees *what* blocked it.
- **Exit:** for each verb, differential fixtures where the item/module is
  referenced from a **second file** through each `use` shape —
  `use a::{b::X, c as d}`, `use a::X as T`, `use a::*`, and fully-qualified
  inline `a::b::X` — assert refs are rewritten, the published graph equals a
  **cold rebuild of the hand-edited intended source** (not the facade), and
  rollback restores all touched files + fingerprint.

### F4 — `modify_signature` rewrites by default (~2–3 days)
- Flip the default policy from `Conservative` (refuse-if-referenced) to rewrite:
  locate callsites via `who_calls`/`usages_of`, apply the arg edits with
  `CallsiteFill` (default `Todo`). Keep `Refuse` as an explicit opt-in.
- Broaden beyond direct `name()` calls: handle qualified (`path::f()`) and method
  (`x.f()`) callsites, across files. Where a callsite shape is genuinely
  unsupported, refuse that **site** with a typed reason, not the whole op.
- **Resolve the gate conflict (DD-F4, review finding #3):** the gate scanner
  (`refactor.rs` `scan_source_risks`) flags `todo!` as a **Hard** panic finding
  (`["panic","todo","unimplemented","unreachable"]`), so a default `Todo` fill
  would make `modify_signature` refuse itself at the pre-write gate. Fix: thread
  the set of *op-generated* callsite-fill offsets into the gate and **downgrade
  those specific panic findings to Soft (a `gate_penalty`), not Hard** — they are
  intentional, greppable "fill me" markers (the whole point of `CallsiteFill::
  Todo`). Human-authored `todo!` elsewhere stays Hard.
- **Exit:** a signature change with a required-new-param and callers in **2
  files** rewrites every callsite, fills new args with `todo!(…)`, **passes the
  gate** (generated fills downgraded to Soft), and `cargo check` passes;
  `CallsiteFill::Refuse` still refuses; the gate still Hard-refuses a
  hand-written `todo!`.

### F5 — Multi-reference test matrix (~2 days)
Add fixtures (shared helper) exercising the **referenced** path, not just the
trivial one, for `move`, `modify_signature`, `inline`, `move_module`,
`merge_modules`:
- target referenced from ≥2 sites and ≥2 files; a module imported from elsewhere.
- assert: references rewritten correctly, result compiles, published graph ==
  cold rebuild of intended source, rollback restores every file + fingerprint,
  and `simulate(op).effects == apply(op).effects` including the reference edits.
- **Exit:** the matrix is green and each semantic verb has ≥1 genuinely-multi-ref
  case (the current suites only cover single-site/refusal).

### F6 — Dead dep + idiom/hygiene cleanup (~1–2 days)
- **`rmc-semantic`:** now used by F2 — confirm it's wired, not dead. If F2's
  rename path is deferred, instead **remove** `rmc-semantic` from
  `rmc-crud/Cargo.toml` (the verb-side cycle it was extracted to break does not
  exist, since the verbs don't use it).
- **anyhow leaks:** wrap the `rmc-graph` boundary in concrete variants instead of
  `#[from] anyhow::Error` on `HostError` (`workspace_host.rs`) and `ViewError`
  (`view/navigate.rs`) — or, if the grandfathered anyhow-pervasive loader makes
  that impractical, downgrade to a single documented `#[error(transparent)]
  Graph(#[from] GraphError)` newtype rather than bare `anyhow`.
- **`inline` hygiene:** keep the narrow zero-arg/single-expr restriction and
  document it; if it ever broadens to args, add capture/shadowing checks before
  textual substitution.
- **nits:** remove dead `CrudError::ApplyNotImplemented`; GC stale `pub use`
  facades if `LeaveFacade` is retained; optionally replace the per-crate
  `recursion_check` SCC approximation in `rmc-gates` with a real Tarjan over the
  full edge set (add `petgraph`, the plan's deferred dep) if cross-crate/long
  cycles matter; make `graph_id` a structural content hash (M0 nit) if drift is
  observed.
- **Exit:** no dead deps; new-crate public errors carry no bare `anyhow`; clippy
  clean on the new crates.

---

## 5. Sequencing & size

F1 first (unblocks CI), then F2 (substrate) gates F3/F4, then F5 (tests) and F6
(cleanup) in parallel.

| Wave | New/changed LOC (est.) |
|---|---|
| F1 build fix (3 files, ~18 call sites) | ~150 (test edits) |
| F2 reference substrate (+ `use_tree.rs` AST layer) | ~900 + tests |
| F3 move/module rewrite | ~1,000 + tests |
| F4 modify_signature rewrite (+ gate downgrade) | ~600 + tests |
| F5 multi-ref matrix | ~800 (tests/fixtures) |
| F6 cleanup | ~250 |
| **Total** | **~3.7k** (over half tests) |

This is additive to the existing implementation; no engine rework — F2–F4 ride
the same `compute_effects`/`apply_effects` + warm-host diff-patch path already
proven in M1.

---

## 6. Open decisions

- **DD-F1 — Reference mechanism per verb.** Default taken: **graph-row-driven**
  edits for path/import rewriting (`who_imports`/`usages_of` byte ranges);
  **`rmc-semantic` `RenamePreview`** for the name component of a rename. Flip:
  route *all* reference edits through `rmc-semantic` RA rename if you'd rather
  trust RA's resolver than the graph's `Usage` rows (heavier, needs the warm RA
  host live during `compute_effects`, which currently is graph-only/pure).
- **DD-F2 — Facade retention.** Default: facades become `ReferencePolicy::
  LeaveFacade` (opt-in), `Rewrite` is default. Flip: drop facade support entirely
  if no caller wants re-export-preserving moves.
- **DD-F3 — Unresolved references.** Default: refuse the op with a typed
  `UnresolvedReference { sites }` listing what couldn't be rewritten (macro/`dyn`/
  generic indirection). Flip: rewrite what's resolvable and report the rest as a
  soft warning if partial edits are acceptable.
- **DD-F4 — Generated `todo!` vs the gate.** Default: downgrade op-generated
  callsite-fill `todo!`s to a **Soft** gate penalty (human `todo!` stays Hard).
  Flip: forbid panic-style fills entirely and refuse `modify_signature` when a
  required new param can't be filled non-panickingly.
- **DD-F5 — Import-span source.** Default: **AST `UseTree` scan** per importer
  module (no schema change; needed for globs/aliases anyway). Flip: persist
  import spans on `Binding` during extraction (schema bump → invalidates
  snapshots) so `use` edits skip re-parsing. Chosen default avoids a migration.
