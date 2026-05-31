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
  `Rewrite` (default) | `LeaveFacade` | `Refuse`. The facade behavior becomes an
  explicit opt-in, not the silent default.
- **New — reference source of truth:** the graph already carries every reference
  with a byte range (`Usage` rows via `who_uses`/`usages_of`; import `Binding`
  rows via `who_imports`). Drive reference edits from those rows — it is fully
  under the warm-host engine and consistent with how `modify_signature` already
  locates callsites. Reserve `rmc-semantic`'s RA `RenamePreview` for the **name**
  component of a rename (where RA's rename is the correct semantics); use graph
  import rows for the **path** component of a move.

---

## 4. Waves

### F1 — Green the build (do first; ~½ day)
The two stale integration tests in the unchanged `rust-code-mcp` bin crate call
`index_codebase(params, None)` but the endpoint now takes
`(params, &WorkspaceLockRegistry, Option<&SearchRuntimeCache>)`.
- Fix `crates/rust-code-mcp/tests/test_gpu_index_jsonrpc.rs:94` and the 16 sites
  in `crates/rust-code-mcp/tests/test_index_tool_integration.rs` to pass a test
  `WorkspaceLockRegistry::new()` (or `&runtime.workspace_locks()`) and `None`.
- Clear the 9 build warnings (dead `QueryEvaluation`, unused imports) flagged by
  the check log.
- **Exit:** `cargo check --workspace --all-targets` is clean (0 errors); ideally
  `-D warnings` for the new crates.

### F2 — Reference-edit substrate (~2–3 days)
Add `crates/rmc-crud/src/references.rs`:
- `fn reference_edits(snap, target: NodeId, change: &ReferenceChange) -> Result<Vec<FileEdit>, EditError>`
  that, for a target item, collects:
  - **import sites** from `who_imports(target)` (Binding rows) → rewrite/insert/remove the `use` path,
  - **inline qualified uses** from `usages_of(target)` (Usage rows with byte ranges) → rewrite the path/name.
- `ReferenceChange` enum: `Renamed{ new_name }`, `Moved{ new_module_path }`,
  `RenamedAndMoved{..}`, `Removed`.
- Emits `FileEdit`s grouped per file (the existing descending-offset splicer
  applies them). Pure — no I/O.
- Wire `rmc-semantic` here for the rename case: `RenamePreview` → map its
  multi-file `RenameEdit`s into `FileEdit`s. **This kills the dead-dep finding**
  (F6 note) by giving `rmc-semantic` a real consumer in `rmc-crud`.
- **Exit:** unit tests prove `reference_edits` returns the correct byte-edit set
  for a target referenced from 2 files (1 `use`, 1 qualified inline) under each
  `ReferenceChange`.

### F3 — `move` + module ops rewrite references (~3–4 days)
Replace the facade default with real reference rewriting, gated by
`ReferencePolicy` (default `Rewrite`):
- `move` (`module_ops.rs`): relocate the item text, then apply
  `reference_edits(target, Moved{new_module_path})` to every importer + inline
  use. Drop the `pub use … as …` facade to `ReferencePolicy::LeaveFacade` only.
  Optional `new_name` routes the rename component through `rmc-semantic`.
- `split_module`/`merge_modules`/`move_module`/`lift_to_crate`/`lower_to_module`:
  rewrite dependent `use` paths via the import rows instead of `pub use ::*`
  re-export shims. Keep facade as opt-in.
- Keep refusal only for cases the graph genuinely can't resolve (macro-generated
  refs, `dyn`/generic indirection) — and surface them as a typed
  `EditError::UnresolvedReference { sites }` so the caller sees *what* blocked it.
- **Exit:** for each verb, a differential fixture with the item imported from a
  **second file** asserts the importer's `use`/refs are rewritten and the
  published graph equals a **cold rebuild of the hand-edited intended source**
  (not the facade source); rollback restores all touched files + fingerprint.

### F4 — `modify_signature` rewrites by default (~2 days)
- Flip the default policy from `Conservative` (refuse-if-referenced) to rewrite:
  locate callsites via `who_calls`/`usages_of`, apply the arg edits with
  `CallsiteFill` (default `Todo`). Keep `Refuse` as an explicit opt-in.
- Broaden beyond direct `name()` calls: handle qualified (`path::f()`) and method
  (`x.f()`) callsites, across files. Where a callsite shape is genuinely
  unsupported, refuse that **site** with a typed reason rather than the whole op.
- **Exit:** a signature change with required-new-param and callers in **2 files**
  rewrites every callsite, fills new args with `todo!(…)`, and `cargo check`
  passes on the result; `CallsiteFill::Refuse` still refuses.

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
| F1 build fix | ~150 (test edits) |
| F2 reference substrate | ~600 + tests |
| F3 move/module rewrite | ~900 + tests |
| F4 modify_signature rewrite | ~500 + tests |
| F5 multi-ref matrix | ~800 (tests/fixtures) |
| F6 cleanup | ~300 |
| **Total** | **~3.2k** (mostly tests) |

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
