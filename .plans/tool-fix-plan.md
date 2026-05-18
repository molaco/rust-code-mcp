# Tool-Fix Plan: rust-code-mcp MCP Server Tools

Status: in progress — Phase 3 started
Basis: full-session exercise of 49 of the 50 rust-code-mcp tools against the
live workspace (2026-05-18); `clear_cache` not run (destructive). Companion to
`.plans/refactor-plan.md` and `.plans/dup-plan.md` — same crate, different
concern (see §8).

## 0. Goal

Fix 11 defects/limitations in the rust-code-mcp tool surface, found by
exercising 49/50 tools this session. **None is a crash or a wrong
computation.** They fall into four kinds: response-budget overflow, over-broad
or wrong-target matching, graph-soundness gaps (uncaptured edges), and
misleading messages. The 38 tools confirmed fully correct are not touched.

## 1. Evidence — observed symptoms

| Symptom | Tool(s) | Observed |
|---|---|---|
| Result overflowed the MCP response budget, spilled to file | `semantic_overlaps` (Method, 78 KB), `dead_pub_in_crate` (81 KB), `dead_pub_report` (118 KB), `items_with_attribute` | this session |
| Live `indexing → search` edge invisible | `get_imports` | `crate::search::bm25::Bm25Search` used inline in `tantivy_adapter.rs`/`unified.rs`, no `use` |
| Example crate flagged as an architecture violation | `forbidden_dependency_check` | `graph_burn` → `rust_code_mcp` matched `*graph*`/`*mcp*` by crate name |
| Substring name match returns unrelated symbols | `find_definition`, `find_references` | `SearchResult` → also `VectorSearchResult`; `AuditOpts` → `ChannelAuditOpts` |
| Cross-crate method calls / trait dispatch uncounted | `who_uses*`, `crate_edges`, `callers_in_crate`, `dead_pub_*` | documented "Layer 4" limit |
| `derive` returns 0 matches; `#[derive(` is required | `items_with_attribute` | attributes stored as `"#[derive(Error, Debug)]"`, prefix-matched raw |
| "No Rust files suitable for indexing were found" on a hundreds-of-files workspace | `index_codebase` | incremental no-op path |
| `private: 0` always; module-private items folded into `restricted_to` | `workspace_stats` | `530/30/1144` breakdown |
| Vendored / example-crate collisions reported as noise | `overlaps` | `fastembed::Config`, `BenchmarkResult` |
| Impl methods not seedable by qualified name | `similar_to_item` | reported by a subagent — UNCONFIRMED |

## 2. Scope & non-goals

- 49/50 tools exercised; `clear_cache` deliberately not (destructive). See T12.
- **Not touched:** the 38 tools confirmed correct this session — `health_check`,
  `build_hypergraph`, `module_tree`, `analyze_complexity`, the call-graph
  family, the 8 audit tools, `function_signature`, `get_exports`, etc. Do not
  regress them (§6 re-runs the smoke test to prove it).
- This is a behavior-fix plan, **not** a rewrite and not the structural
  refactor (`refactor-plan.md`) or the dedup (`dup-plan.md`).

## 3. Guardrails

1. **MCP surface stability.** Tools are consumed by live MCP clients. Changes
   must be **additive** — new optional params, new response fields. Never
   rename or remove a tool, never remove or retype an existing response field.
   Where an output shape genuinely must change (T8), gate it behind an opt-in
   param and keep the current default, or ship it with an explicit changelog.
2. **Bug-fix behavior changes are allowed but must be documented.** T3, T10
   change what a given input returns — that is the fix; call it out in the tool
   description and the commit message.
3. **One fix per commit.** Each commit ships with a regression test that
   reproduces the original symptom and asserts the fix.
4. **The 49 working tools must not regress.** Re-run the smoke test per phase.
5. **`vendor/` is never edited.**
6. **Dogfood.** Locate each tool's code with `find_definition` / `who_calls`;
   the file paths in §4 are the expected locations from this session's
   analysis — confirm before editing.
7. Verification command (project Nix devshell, repo root):
   `nix develop ../nix-devshells#cuda-code --command cargo check --lib`
   (fast per-commit gate; `cargo test <module>` to validate a specific fix —
   the full suite is slow here, so scope it).

## 4. The fixes

| T | Tool | Severity | Phase | Effort |
|---|---|---|---|---|
| T1 | `semantic_overlaps` | P1 | 1 | complete |
| T7 | all enumerating tools | P2 | 1 | complete |
| T5 | `find_definition` / `find_references` | P2 | 2 | complete |
| T10 | `items_with_attribute` | P2 | 2 | complete |
| T3 | `forbidden_dependency_check` | P1 | 2 | complete |
| T2 | `get_imports` (new `module_dependencies`) | P1 | 3 | complete |
| T4 | Layer-4 → impl-method extraction | P2 | 3 | large |
| T8 | `workspace_stats` | P3 | 4 | small |
| T9 | `overlaps` | P3 | 4 | small |
| T11 | `index_codebase` | P3 | 4 | small |
| T6 | `similar_to_item` | P3 | 5 | small |
| T12 | `clear_cache` (optional enhancement) | P3 | 4 | small |

### Cluster A — response budget

**T1 — `semantic_overlaps` large-result overflow.**
Cause: clusters/pairs mode emits a full member object (`qualified_name` +
`file` + `[start,end]` span) per member; high-population kinds (665 methods)
exceed the response cap. Fix: add `summary: bool` (default false) dropping
per-member `file`+`span` — mirror the flag `functions_with_filter` already has;
make `max_pairs` a hard cap on emitted members; add `offset`; always return
`total_cluster_count` + `total_pair_count`. Files: `src/tools/graph_tools.rs`
(endpoint + response structs), `src/tools/search_tool.rs` (param struct).
Compat: additive.

Progress (2026-05-18): implemented additive `offset` and `summary` params;
`max_pairs` now caps returned pairs in pairs mode and total emitted cluster
members in clusters mode; responses now include `total_pair_count`,
`total_cluster_count`, `offset`, `limit`, and `summary` while preserving
`pair_count`. Added focused regression tests for member limiting, cluster
offsets, and summary serialization. Verified with
`nix develop ../nix-devshells#cuda-code --command cargo test page_clusters --lib`,
`nix develop ../nix-devshells#cuda-code --command cargo test item_ref_summary_omits_file_and_span --lib`,
and `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`.
A broader `cargo test tools::graph_tools::tests:: --lib` run was stopped after
the new T1 tests passed because unrelated snapshot-heavy tests continued
running for several minutes.

**T7 — uniform large-output convention.** Cause: only `functions_with_filter`
has `limit`/`offset`/`summary`; every other enumerating tool returns
everything (`dead_pub_in_crate`, `dead_pub_report`, `items_with_attribute` all
overflowed this session; deep `call_graph`, `fn_body_audit`, `who_uses` on a
hot symbol are latent). Fix: define one shared contract — `limit` (default 50),
`offset` (default 0), `summary` (default false), response carries
`total_match_count` — and apply it to every workspace-enumerating tool via a
shared response-builder helper. Files: response helpers in
`src/tools/graph_tools.rs` + the audit/analysis endpoints; param structs in
`src/tools/search_tool.rs`. Compat: additive. Ship T1 first, then T7
generalizes the same contract.

Progress (2026-05-18): added shared `ListPaginationParams` with `limit`
(default 50), `offset`, and `summary`; added shared response metadata
(`total_match_count`, `offset`, `limit`, `summary`, `returned_match_count`) and
applied it to list-shaped graph/audit responses including imports/exports,
who-uses/call-site lists, dead-pub tools, crate edges, enum variants,
attributes, attribute scans, forbidden-dependency violations, pub-type audit,
re-export chains, crate metrics, unsafe/mut-static/docs/derive/recursion/
channel/fn-body audits. Summary mode drops file/span on the response shapes
where those fields are the main payload bulk. Verified with
`nix develop ../nix-devshells#cuda-code --command cargo test page_list --lib`
and `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`.

### Cluster B — match semantics

**T5 — `find_definition` / `find_references` substring matching.** Cause:
unanchored substring name match, no exact option. Fix: add `exact: bool`
(default false — keeps current behavior); when true, anchor on the full symbol
name; always rank exact hits first; tag each result `exact: true/false`. Files:
`src/tools/analysis_tools.rs` (endpoints), `src/semantic/position.rs`
(resolver). Compat: additive.

Progress (2026-05-18): added optional `exact` params to both MCP tools,
preserving substring/fuzzy search by default. Exact hits are now ranked before
substring hits, `exact=true` filters to full-name matches only, and each text
result line includes an `exact=true/false` tag. Added focused unit coverage for
the exact ranking/filtering helper. Verified with
`nix develop ../nix-devshells#cuda-code --command cargo test rank_and_filter_exact --lib`
and `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`.

**T10 — `items_with_attribute` prefix-matches the raw `#[…]` string.** Cause:
attributes stored as `"#[derive(Error, Debug)]"`; the `pattern` is
prefix-matched against that, so `derive` → 0 matches, only `#[derive(` works —
a silent wrong-empty result. Fix: match against the attribute *path*
(`derive`, `must_use`, `cfg`) — parse it out, or store path + args separately;
accept both bare and wrapped patterns. Stop the tool description's examples
from mixing forms. Files: `src/graph/attributes.rs` (extraction/storage),
`src/graph/queries.rs` (`items_with_attribute`). Compat: a bug-fix behavior
change — `derive` starts matching; document it.

Progress (2026-05-18): updated `items_with_attribute` matching so bare
attribute paths such as `derive`, `must_use`, and `cfg` match the parsed
attribute path while wrapped raw forms like `#[derive(` still work. Kept
anchored behavior so prose inside unrelated attributes is not matched. Updated
tool parameter/description text and changed the derive query regression to use
the bare `derive` form. Verified with
`nix develop ../nix-devshells#cuda-code --command cargo test match_attribute_accepts_bare_attribute_paths --lib`
and `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`.
The snapshot-backed `items_with_attribute_finds_derive_users` test was stopped
after it did not finish in roughly two minutes, consistent with the existing
snapshot-test hangs observed during T1.

**T3 — `forbidden_dependency_check` crate-name-glob false positives.** Cause:
rules glob-match crate *names*; non-library members (examples/tests/benches)
are treated as ordinary consumers, so `graph_burn` matched `*graph*`. Fix:
extract each crate's target kind (lib/bin/example/test/bench/build) from
`cargo metadata` into the crate node; add a `consumer_kinds` rule field
(default `["lib","bin"]`) excluding example/test/bench consumers; document that
patterns match crate names. Files: `Cargo.toml`, `src/graph/loader.rs`,
`src/graph/extract.rs` (crate-kind extraction), `src/graph/model.rs` (crate
node), `src/graph/storage.rs` (schema bump), `src/graph/queries.rs` (rule
filter), `src/tools/graph_tools.rs` + `src/tools/search_tool.rs`
(`ForbiddenDependencyRuleParam`). Compat: additive param; default behavior
changes (a bug fix) — document.

Progress (2026-05-18): implemented Cargo target-kind extraction through
`cargo metadata`, keyed by both normalized target name and target root file;
crate nodes now carry `crate_target_kind`, and the graph schema bumped to v12
so old bincode snapshots rebuild instead of decoding stale `Node` records.
`forbidden_dependency_check` rules now accept optional `consumer_kinds`;
omitting it defaults to `["lib", "bin"]`, while explicit kinds (or `*`) can
opt examples/tests/benches/build scripts back in. Updated tool descriptions to
state that glob patterns match crate names and that target-kind filtering is
consumer-side. Verified with
`nix develop ../nix-devshells#cuda-code --command cargo test forbidden_dependency_rule --lib`,
`nix develop ../nix-devshells#cuda-code --command cargo test target_kind_label_collapses_cargo_kinds --lib`,
`nix develop ../nix-devshells#cuda-code --command cargo test load_crate_target_kinds_finds_workspace_targets --lib`,
and `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`.

### Cluster C — graph soundness

**T2 — `get_imports` misses fully-qualified inline paths.** Cause: it reads
only `use`/`extern crate` binding nodes; the live `indexing → search` edge,
written inline, is invisible — a false-negative for boundary verification
(refactor-plan §14 depends on this tool). Fix: add a **new** tool
`module_dependencies(module)` returning the complete set of modules a module
references — `use` + `extern crate` + fully-qualified inline paths — from the
hypergraph's usage edges; leave `get_imports` as-is and cross-link the two
descriptions. Files: new endpoint in `src/tools/graph_tools.rs`, new query in
`src/graph/queries.rs` over `src/graph/usages.rs` data. Compat: purely
additive (new tool).

Progress (2026-05-18): added the new `module_dependencies` tool and
`OpenedSnapshot::module_dependencies`, grouping each source module's
`use`/extern-crate bindings plus non-import usage edges by target module.
Results include target module/kind/crate, import and usage counts, and
per-symbol contributors; `summary=true` omits the symbol list under the same
pagination convention introduced in T7. Kept `get_imports` binding-only and
updated its description to point callers at `module_dependencies` for complete
module-level dependency checks. Dogfooded the original symptom: `get_imports`
for `rust_code_mcp::indexing::tantivy_adapter` does not list `search::bm25`,
while `who_uses(rust_code_mcp::search::bm25::Bm25Search)` shows inline usage
from that module. Verified with
`nix develop ../nix-devshells#cuda-code --command cargo test dependency_node_for_climbs_item_parents_to_module --lib`
and `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`.
The snapshot-backed `mcp_round_trip_against_self` test was stopped after it
was still running past two minutes, matching the existing snapshot-test hang
pattern from earlier phases.

**T4 — Layer-4 gap: impl methods / trait dispatch not first-class.** Cause:
impl-block and trait-impl items aren't extracted as Item nodes with usage/call
edges, so `who_uses*`/`crate_edges`/`callers_in_crate` under-report and
`dead_pub_*` can false-positive "unused". Fix, in two stages:
(4a, interim) lift the limitation out of the mid-description into a prominent
note, and have `dead_pub_in_crate`/`dead_pub_report` tag each finding "may be
live via method/trait dispatch — verify";
(4b) extract impl-block methods and trait-impl associated items as Item nodes
with their own usage/call edges ("Layer 5").
Files: `src/graph/extract.rs`, `impls.rs`, `usages.rs`, `bindings.rs`,
`model.rs`; interim tagging in `queries.rs`. Compat: additive (more
nodes/edges); usage counts will *rise* — update any test asserting exact
counts. **Largest, highest-risk item — touches the core extraction pipeline;
do it last, gated on a full re-index + smoke test.**

### Cluster D — output clarity

**T8 — `workspace_stats` visibility taxonomy.** `private` is always 0;
module-private items are folded into `restricted_to`. Fix: relabel
(`module_private` vs `restricted`) — but that is a breaking output change, so
either gate it behind a param or, recommended, **document the current
bucketing precisely now** and relabel only later with a changelog. Files:
`src/graph/queries.rs` (`workspace_stats`), visibility classification in
`src/graph/bindings.rs`.

**T9 — `overlaps` vendored/example noise.** Collisions include vendored
`fastembed` and example-crate names. Fix: add `scope: "all" | "local" |
"local_no_vendor"` (default keeps current behavior for compat); detect
vendored crates by path (`vendor/`). Files: `src/graph/queries.rs`
(`overlaps`), `src/tools/graph_tools.rs`. Compat: additive.

**T11 — `index_codebase` misleading no-op message.** `force_reindex:false` on
the fully-indexed workspace returns success in 33 ms but says "No Rust files
suitable for indexing were found" / "Skipped files: 28". Fix: distinguish the
incremental-no-change path — "already up to date — N files unchanged, 0
changed"; reconcile the skip counter. Files: `src/tools/index_tool.rs`,
incremental path in `src/indexing/incremental.rs` / `unified.rs`. Compat:
message text only.

### Cluster E — investigate

**T6 — `similar_to_item` impl-method seeding (UNCONFIRMED).** A subagent could
not seed it on `OpenedSnapshot::*` methods by qualified name, while
`semantic_overlaps item_kind="Method"` resolves all 665 methods. Action:
reproduce first, capture the exact error. Likely a qualified-name-resolution
inconsistency between the two tools — and likely resolved for free once T4
makes impl methods first-class Item nodes, so **verify T6 after T4**. Files:
`src/tools/graph_tools.rs` (`similar_to_item` target resolution).

**T12 — `clear_cache` dry-run (optional enhancement, not a defect).**
`clear_cache` was not exercised — it is destructive (wipes the index/embedding
cache, forcing a costly OpenRouter re-index). Suggested: add `dry_run: bool`
reporting what *would* be cleared (paths, sizes) without deleting. Files:
`src/tools/clear_cache_tool.rs`. Compat: additive.

## 5. Execution order

```text
Phase 1  Response budget        T1  -> T7              (P1; contained, no graph risk)
Phase 2  Match semantics        T5, T10 -> T3          (P1/P2; small query fixes)
Phase 3  Graph soundness        T2 -> T4 (4a -> 4b)    (P1/P2; T4 is the big/risky one)
Phase 4  Output clarity         T8, T9, T11, T12       (P3; cosmetic, interleavable)
Phase 5  Investigate            T6                     (verify after Phase 3 / T4)
```

Rationale: Phase 1 first — it is the only thing that *broke* calls this
session and is pure response-serialization (zero graph-model risk); T1 is the
acute fix, T7 generalizes its contract. Phase 2 — independent, low-risk query
fixes; T3 last in the phase as it needs the crate-kind extraction. Phase 3 —
T2 is additive; T4 is the largest and riskiest, split 4a (cheap interim
caveat) then 4b (the extraction, behind a full re-index). Phase 4 — small
output/UX fixes, may interleave with any earlier phase. Phase 5 — T6 last, as
T4 likely resolves it.

Dependencies: T7 builds on T1's contract · T3 needs the crate-kind extraction ·
T6 depends on T4 · T4b must be gated on a full `index_codebase --force` +
`build_hypergraph --force_rebuild`.

## 6. Verification

After each commit:
- a regression test reproduces the original symptom and asserts the fix (for
  overflow fixes, the test asserts the `summary`/`limit` path stays within
  budget);
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green; `cargo test <touched_module>` to validate the fix.

After each phase: re-run the smoke test for the tools that phase touched.

End of plan: full smoke test of all 50 tools — the 38 confirmed-correct, the 11
fixed, and `clear_cache` via its new `dry_run`. For T4 specifically: full
`index_codebase --force` + `build_hypergraph --force_rebuild`, then re-run
`who_uses` / `dead_pub_in_crate` and confirm usage counts rose and no
method-only-used item is still reported "dead".

## 7. Success criteria

- All 11 symptoms in §1 no longer reproduce.
- No tool renamed or removed; no existing response field removed or retyped;
  every change additive or gated (Guardrail 1).
- The 38 confirmed-correct tools are behavior-unchanged.
- Every workspace-enumerating tool has `limit`/`offset`/`summary` and does not
  overflow the response budget on this workspace.
- `who_uses` / `dead_pub_in_crate` no longer miss method/trait-dispatch usage
  (T4); the Layer-4 caveat is either resolved or prominently surfaced.
- One regression test per fix; `cargo check --all-targets` green at every
  commit.

## 8. Relationship to the other plans

This plan, `refactor-plan.md`, and `dup-plan.md` all target the `rust_code_mcp`
crate but are independent concerns: refactor = file/module structure, dup =
private-helper consolidation, this = tool behavior.

Several fixes land in files the refactor splits — `tools/graph_tools.rs`,
`graph/queries.rs`, `graph/extract.rs`. Two sane orders:
- **Tool fixes first** — fixes user-facing behavior sooner; the fixes land in
  the mega-files and the later refactor moves them.
- **Refactor first** — each fix then lands in a small, focused file.

Recommended: do **Phases 1–2 of this plan first** (high-value, contained),
then the refactor + dup-plan, then **Phases 3–5** (T4's extraction work is
cleaner against the post-refactor `graph/` layout). Any order works — but do
not interleave T4b with the refactor's `graph::queries` split; finish one
before starting the other.
