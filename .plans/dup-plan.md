# Duplication Consolidation Plan: Private-Helper De-duplication

Status: ready to execute
Basis: `rust-code-mcp` semantic-overlap analysis (Qwen3-Embedding-8B via the
`openrouter-qwen3-8b` profile), every cluster cross-validated against source.
Companion to `.plans/refactor-plan.md` — see §8 for ordering.
Census refreshed 2026-05-18 against the post-tool-fix checkout (after the
`.plans/tool-fix-plan.md` work landed).
Snapshot: 2986 nodes, 5041 bindings, 7993 usages.

## 0. Goal

Remove copy-pasted private helpers in the `rust_code_mcp` crate. A workspace
semantic-overlap scan over all five item kinds found ~16 actionable duplicate
clusters, concentrated in three zones. **Every duplicated function is a
file-private `fn` whose callers all live in the same module** — so this is a
low-risk consolidation that changes no public API and no behavior.

This is not a restructure. It pairs with the refactor plan: run it first and it
collapses the duplicate clusters *before* the four mega-files are split, so the
twins are removed instead of scattered across the new files.

## 1. Evidence

Census — `semantic_overlaps`, profile `openrouter-qwen3-8b`, threshold 0.85,
crate `rust_code_mcp`:

```text
Function   127 similarity pairs   59 clusters   (382 seeds)
Method     210 pairs              90 clusters   (507 seeds)
Struct      80 pairs              29 clusters   (265 seeds)
Enum         1 pair               (0.886 — below 0.92, not actionable)
Trait        0 pairs
```

418 similarity pairs total. After verifying every cluster against source:
**~16 actionable clusters**. The remaining ~94% is architecturally-intended
twinning (see §2). Counts refreshed 2026-05-18 against the post-tool-fix
checkout; the original pre-tool-fix scan was 393 pairs (Function 113 / Method
202 / Struct 77). The tool-fix work de-duplicated nothing — every actionable
cluster below persisted — and added two repetitions of its own (see §4.1).

Calibration: the qwen3-8b ≥0.99 similarity band was 100% true clones; the
0.85–0.95 band was mixed — same structural *shape*, sometimes different
*behavior*. Embeddings group by shape, so every cluster in §6 was read against
source before being classed actionable; three (C9, C12, C17) were rejected.

## 2. Non-goals — what this plan does NOT touch

These surface in the scan but are intentional; touching them is churn:

- **MCP endpoint ↔ implementation adapter twins** — `tools::graph_tools::derive_audit`
  ↔ `graph::derive_audit::derive_audit`, and ~20 more. The `tools` layer is
  deliberately a thin wrapper; the refactor plan keeps the two layers apart.
- **Facade delegations** — `IndexerCore` forwarding to `FileProcessor`,
  `VectorStore` forwarding to its backend.
- **Parallel schema / DTO families** — the ~40 `*Params` structs in
  `search_tool.rs`, the `Enriched*` / `*Response` structs in `graph_tools.rs`.
- **Sibling helpers parallel by design** within one file — encode/decode pairs,
  with/without-`edition` variants, `p50`/`p95`/`p99`, the eight `match_*`
  pattern matchers in `fn_body_audit.rs`.
- **Same-name / different-behavior pairs** — C9, C12, C17 (see §6). The
  embedding flags them on shape; verification shows they are not duplicates.

## 3. Guardrails

1. **No public API change.** Every target is a file-private `fn`/method — no
   public path moves, no signature changes, no MCP-tool surface change.
2. **Narrowest visibility.** A consolidated helper gets `pub(in crate::graph)`
   / `pub(in crate::tools)` / `pub(in crate::embeddings)` when its consumers
   are one module family; `pub(crate)` only for the genuinely cross-family
   cases (the `graph`↔`tools` label helpers; the `embeddings`↔`indexing` batch
   planner). Never `pub`. This satisfies refactor-plan Guardrail 2.
3. **One cluster family per commit.** Each commit consolidates one zone, must
   compile, and keep `cargo check --all-targets` green.
4. **No behavior change.** Where two copies have diverged (C7, C8, C10) keep
   the divergent copies separate — consolidate only verified-identical bodies.
5. **`vendor/` is never touched.**
6. Verification command (project Nix devshell, repo root):
   `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`

## 4. Inventory by zone

**Zone A — `graph` audit toolkit** → new flat file `src/graph/audit_util.rs`
(`pub(in crate::graph)`):

| ID | Helper | Copies | Cosine | Verdict |
|----|--------|--------|--------|---------|
| C1 | `canonical_function_path` | channel_audit, fn_body_audit | 1.00 | byte-identical |
| C4 | `resolve_workspace_relative` | fn_body_audit, unsafe_audit, channel_audit, impls, usages, extract | 0.998 | 6-way identical — `extract` copy added by the tool-fix T3 work (was 5-way; see §4.1) |
| C6 | `item_has_cfg_test` + `enclosed_by_cfg_test` | channel_audit, fn_body_audit | 0.91 | identical (import-qualifier only) |
| C16 | `enclosing_fn_for_body_offset` / `resolve_enclosing_function` | fn_body_audit, channel_audit | 0.931 | identical body, different name |

**Zone B — `graph`↔`tools` label/format twins** → new `src/graph/labels.rs`
(`pub(crate)`; `tools` imports inward — `tools → graph` is legal):

| ID | Helper | Copies | Cosine | Verdict |
|----|--------|--------|--------|---------|
| C3 | `usage_category_label` | graph::queries, tools::graph_tools | 1.00 | byte-identical |
| C5 | `binding_kind_label` / `label_binding_kind` | same two | 0.993 | identical (param name only) |
| C8 | `node_kind_label` / `label_node_kind` | same two | 0.898 | clone — see §6 note (the 3 sibling `item_kind_*` formatters are 3 distinct vocabularies: relocate, do NOT merge) |
| C18 | `owner_crate_name`/`crate_display_name` + `module_qualified_path`/`module_path_segments` | bindings, extract | 0.85–0.89 | near-clone (identical loop, different presentation) |

**Zone C — `tools` env/path/identity helpers** → `src/tools/project_paths.rs`
(`pub(in crate::tools)`):

| ID | Helper | Copies | Cosine | Verdict |
|----|--------|--------|--------|---------|
| C7 | `data_dir` | health_tool, clear_cache_tool, indexing_tools | 1.00 | identical — **these 3 only** (config.rs & graph/storage.rs genuinely differ) |
| C13 | backend resolution | query_tools, graph_tools (index_tool partial) | 0.868 | 2 identical; index_tool adds a legacy `model` fallback |
| C14 | embedder-identity read | project_paths, health_tool | 0.940 | same JSON read, different error handling |
| C15 | `compute_dir_hash` / `dir_hash` | clear_cache_tool, project_paths | 0.875 | byte-identical |

**Standalone:**

| ID | What | Cosine | Verdict / home |
|----|------|--------|----------------|
| C2 | `arc` helper — `embeddings::backend` ↔ `embeddings::profile_registry` | 1.00 | byte-identical → `embeddings` (`pub(in crate::embeddings)`) |
| M1–3 | `graph::ids` — `NodeId`/`BindingId`/`UsageId` each clone `from_components` + `to_hex` + `as_bytes` | 0.99–1.0 | ~9 identical method bodies → one `define_id!` macro in `graph/ids.rs` |
| C10 | `line_of_offset` — `parser/mod` ↔ `parser/type_references` | 1.00 | identical → `parser/mod.rs` (codemap's `line_of_byte` is a *different* algorithm — exclude) |
| C11 | batch planner — `openrouter` ↔ `indexing::embedding_batcher` | 0.883 | identical greedy bin-pack, different types → generic in `embeddings` (see §5) |
| S1 | `ForbiddenDependencyRuleParam` ≡ `graph::queries::ForbiddenDependencyRule` | 0.956 | field-for-field identical (schemars param mirror) |
| S2 | `CallSitesResponse` ≈ `CallersInCrateResponse` (both `graph_tools.rs`) | 0.902 | near-identical response DTOs |

### 4.1 Repetition introduced by the tool-fix work

`.plans/tool-fix-plan.md` was a behavior-fix pass, not a dedup pass — it left
every cluster above intact and added two repetitions of its own:

- **C4 grew 5-way → 6-way.** `graph::extract::resolve_workspace_relative`
  joined the cluster when T3 added crate-kind extraction to `extract.rs`. No
  extra work — it consolidates into `audit_util.rs` with the other five
  (Commit 1), just one more copy to delete.
- **T7 `summary`-drop copy-paste (not a scan cluster).** The T7 pagination
  work inlined the same `if summary { x.file = None; x.span = None; }` block
  into ~8 enumerating endpoints in `tools/graph_tools.rs` (`dead_pub_in_crate`,
  `dead_pub_report`, `enum_variants`, `items_with_attribute`,
  `pub_use_pub_type_audit`, `mut_static_audit`, `missing_docs_audit`,
  `derive_audit`). It is *intra-function* copy-paste, so `semantic_overlaps`
  does not surface it as an Item cluster — it is code-review-found. The T7
  follow-up commit `62ebd363` already solved this correctly for the call/usage
  tools via a shared `call_site_views` helper + an `enrich_usages(summary)`
  param; apply the same centralization to the remaining ~8. Small and
  `tools`-local — fold into Commit 4 or 5.

## 5. Dependency-direction constraints

Per refactor-plan §2 a shared helper's home must be a module every consumer is
allowed to depend on (`graph → graph internals only`; `embeddings ↛ indexing`).

- **Zone A & B & C2 & C18** — all consolidate *inside* a single module family
  (`graph` / `embeddings`). No rule conflict.
- **Zone B** crosses `graph`↔`tools`: home is `graph` (legal: `tools → graph`),
  `tools` imports inward — never the reverse.
- **C7 — partially BLOCKED.** A workspace-wide `data_dir` is impossible:
  `graph/storage.rs` has its own `default_data_dir` and `graph ↛ config`; and
  `config.rs`'s copy genuinely differs (different `ProjectDirs` tuple +
  fallback). Fix only the 3 `tools/*` copies; leave `config.rs` and
  `graph/storage.rs`.
- **C11 — home constrained.** `plan_openrouter_batches` is in `embeddings`,
  `plan_embedding_batches` in `indexing`. `embeddings ↛ indexing`, so the
  shared generic must live in `embeddings` (legal: `indexing → embeddings`).
  Doable, just not free-choice of home.
- **C10 — partial.** Only the 2 `parser/*` copies merge; `codemap`'s
  `line_of_byte` uses a snapshot line-table + binary search — different
  algorithm, leave it.

## 6. Verified clusters — rejected as non-duplicates

The embedding grouped these by shape; source verification rejects them — do
**not** merge:

- **C9** `format_binding_visibility` vs `visibility_label` (0.911) — different
  output strings (`"private"` vs `"pub(self)"`; graph_tools resolves the node
  for `pub(crate={qname})`). Same shape, different product. Leave separate.
- **C12** `sort_openrouter_inputs` vs `sort_embedding_inputs` (0.853) —
  different element types and sort keys. One-liners. Leave (folds into C11 if
  the generic planner is done).
- **C17** `default_kind_filter` in derive_audit vs docs_audit (0.899) —
  different kind sets ({Struct,Enum,Union} vs 9 documentable kinds). Same name,
  different intent. Optionally rename (`derivable_kinds` / `documentable_kinds`);
  do not merge.

## 7. Commit plan

Each commit is one zone, compiles independently, keeps `cargo check
--all-targets` green.

1. **`graph: add audit_util.rs, dedupe HIR/syntax audit helpers`** — C1 + C4 +
   C6 + C16. New `src/graph/audit_util.rs` (`pub(in crate::graph)`); delete
   ~13 copied bodies across channel_audit, fn_body_audit, unsafe_audit, impls,
   usages. *Largest payoff, all trivial-identical. Effort: small.*
2. **`graph: add labels.rs, unify label/path helpers`** — C3 + C5 + C8 + C18.
   New `src/graph/labels.rs`: `usage_category_label`, `binding_kind_label`, one
   parameterized `node_kind_label`, the 3 distinctly-named `item_kind_*`
   formatters, `crate_display_name`, `module_path_segments`. queries.rs and
   graph_tools.rs import it. *Effort: medium.*
3. **`graph: define_id! macro for ids.rs`** — M1–M3. One macro generates the
   three ID newtypes; removes ~9 identical method bodies. *Independent of the
   file splits — can land anytime. Effort: small.*
4. **`tools: centralize data_dir / dir_hash / embedder-identity`** — C7 (3
   tools copies only) + C14 + C15 in `project_paths.rs` (`pub(in crate::tools)`).
   Explicitly leave config.rs and graph/storage.rs. *Effort: small.*
5. **`tools: unify embedding-backend resolution`** — C13. Shared resolver in
   `project_paths.rs`; `index_tool`'s `resolve_backend` keeps its `model`
   fallback layered on top. *Effort: small.*
6. **`embeddings: dedupe arc + extract generic batch planner`** (optional) —
   C2 + C11. `arc` to an `embeddings` util; generic `plan_batches<T>` in
   `embeddings`, reused by `indexing`. *Lowest priority. Effort: medium.*
7. **`tools: collapse duplicate DTO structs`** (optional) — S1 (`*RuleParam`
   derives from / newtypes the graph struct), S2 (merge the two response
   DTOs). *Effort: small.*

Commits 1–5 are the core deliverable; 6–7 are follow-ups.

## 8. Ordering relative to the refactor plan

Run this plan as **refactor-plan Phase 0.5** — after Phase 0 (baseline),
before Phase 1 (split `tools`). Rationale:

- C3/C5/C8 are `graph::queries` ↔ `tools::graph_tools` twins. Dedupe first →
  refactor Phase 1 deletes the `graph_tools.rs` copies outright (it imports
  inward from `graph`), so the 3976-line file has less to split. Dedupe after →
  the twins scatter into `tools/graph/{core,surface,response}.rs` and
  `graph/query/{model,…}.rs`.
- C1/C4/C6/C16 live in the `graph/*_audit.rs` files refactor-plan §1.1 leaves
  as-is. A flat `graph/audit_util.rs` sibling is compatible with §1.1 (not a
  subdirectory) and removes ~13 copies before any phase touches those files.

If Phase 0.5 is skipped, this document stands as the "known duplication,
deferred" inventory that refactor-plan Guardrail 4 otherwise leaves implicit.

## 9. Verification

After each commit:

- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green.
- no formatter run; no public path changed.

After all commits:

- re-run `semantic_overlaps(item_kind="Function", embedding_profile="openrouter-qwen3-8b")`
  and confirm the exact-clone clusters (C1, C3, C4, C5, C15, C16) are gone.
  Post-tool-fix baseline to beat: 127 Function pairs / 59 clusters.
- `who_imports` on the new `graph::audit_util` / `graph::labels` /
  `tools::project_paths` symbols shows only intended consumers.

## 10. Success criteria

- The ~16 actionable clusters of §4 are resolved (or, for C7/C10/C11,
  resolved as far as the dependency rules allow).
- ~35–40 duplicated function bodies + ~9 duplicated ID-method bodies collapsed
  to single definitions.
- No public API, MCP-tool, or behavior change; `cargo check --all-targets`
  green at every commit.
- A re-run semantic-overlap scan shows the ≥0.99 exact-clone clusters cleared.
