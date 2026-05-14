# G10a — Workflows reference docs

## 1. Group summary

This group lands the workflows documentation set in three steps:

1. `a9133044` — adds `.docs/workflows-detailed.md` (1313 LOC): the foundational 16-workflow reference doc (W1–W16) with templated sections per workflow (scope, steps, decision frames, pattern reference, worked examples).
2. `e99fa6be` — appends §23 "Workflows mapped to Rust guidelines" to `.docs/workflows.md` (47 LOC).
3. `a51822fb` — expands `.docs/workflows.md` (49 LOC) with extra recipes, depth advice, vendored-crate caveat, "empty-result as signal" guidance, and a new §20 "Output handling and post-processing" section.

The two earlier expansions modify the **summary** doc `workflows.md`; the third (chronologically last per task ordering, but committed earlier per timestamps) adds the **detailed** companion `workflows-detailed.md`. The task brief states a9133044 is the foundational doc — by file content this is true (it introduces the detailed reference), but by author timestamp it post-dates a51822fb (2026-05-03 vs 2026-05-02). The two are kept in sync (terminology, recipe names, tool references).

All tool names, parameter names, and output field names referenced in the docs match the implementation. The main accuracy issues are: one wrong parameter shape for `get_similar_code`, one missing-file cross-reference (`rust-guidelines-final.md`), and a couple of formatting glitches in tables.

## 2. Per-commit review

### a9133044 — "add detailed workflows reference doc"
- **LOC**: 1313 (new file `.docs/workflows-detailed.md`)
- **What it adds**: Foundational 16-workflow reference (W1 symbol lookup, W2 workspace overview, W3 crate audit, W4 module audit, W5 symbol forensics, W6 trait analysis, W7 → companion doc, W8 refactor planning with 13 recipes, W9 → companion doc, W10 test/prod split, W11 method-aware, W12 API surface, W13 semantic similarity, W14 call graphs, W15 complexity, W16 snapshot compare). Plus a shared "Prerequisites" section, "Output handling — when results are large", "Index / cache management", and "Quick recipe index" table.
- **Accuracy spot-checks**:
  - `build_hypergraph(directory, force_rebuild?)` — real (`src/tools/graph_tools.rs:42`, `force_rebuild` field at line 53).
  - `module_tree(directory, krate, depth)` — real (`src/tools/search_tool.rs:297-303`); `krate` is the correct param name. ✓
  - `dead_pub_report` output `crates[].crate` — real (`src/tools/graph_tools.rs:2746-2749`, serde renames `krate` → `"crate"`). ✓
  - `crate_edges` fields `unique_symbols`, `total_refs_via_imports`, `total_refs_via_usages` — real (`src/graph/queries.rs:52-54`). ✓
  - `who_uses_summary` returns rows with `category_breakdown` keyed by `Read`/`Write`/`Test`/`Other` — real (`src/graph/queries.rs:198`, router description line 287). ✓
  - `analyze_complexity` returns file-level aggregates (`Total cyclomatic`, `Avg per function`) with no cognitive metric — matches §W15 claim (`src/tools/analysis_tools.rs:344, 356, 378`). ✓
  - Companion docs `.docs/workflow-imports-exports.md` and `.docs/workflow-type-overlaps.md` exist (referenced from W7/W9 sections). ✓
  - `get_call_graph(file_path)` returns within-file edges — matches W14 (`src/tools/analysis_tools.rs:212`). ✓

- **Issues**:
  - **(LOW)** Lines 967, 971 use `get_similar_code(target=<fn>)` — the real param is `query` (string), not `target`. The same tool is correctly called with `query=` elsewhere in the doc (line 940). Inconsistent shorthand.
  - **(LOW)** Line 919 (Pattern reference table for W12) has a broken-cell layout: `` `pub(crate)` | `pub(in <crate>)` items `` — the `|` inside the cell shifts the row into 4 columns instead of 2. Render artifact.
  - **(LOW)** §W5 / §W11 mention `find_references` as "broader scope (lifetime annotations, local vars)" — this matches RA semantics but the workflow doesn't note that `find_references` only takes a short symbol name (no qualified path filter), so it can return cross-crate noise on common names. Minor caveat.
  - **(INFO)** §W6 step 5 row "Single importer + single implementer | Trait is doing nothing — inline (rust-guidelines §8)" — and §W15 row references "rust-guidelines §4". Neither indicates where rust-guidelines lives. The doc isn't in this repo (no `.docs/rust-guidelines*.md`). Not broken per se — it's an external-doc cite — but worth knowing.
  - **(INFO)** §W6 step 4 sub-claim "Modules that import `T` typically either implement it or take it as a generic bound" is reasonable but glosses over a third case (trait-method receiver via `&dyn T`). Stylistic only.

- **Verdict**: PASS. Tool names, params, output fields all check out. The one outright wrong call shape (`get_similar_code(target=...)`) is in a recipe in §W13 and §W8.10; everywhere else in the same file uses `query=`.

### e99fa6be — "add rust guidelines workflow mapping section to workflows doc"
- **LOC**: 47 (additions to `.docs/workflows.md`, new §23)
- **What it adds**: A "Workflows mapped to Rust guidelines (today's tools)" section mapping checkable items in `rust-guidelines-final.md` (§4 function complexity, §7 types, §8 traits, §10 visibility, §11 architecture, §12 async, §17 testing, §23 review checklist) onto specific MCP tool compositions.
- **Accuracy spot-checks**:
  - References `analyze_complexity × who_uses_summary` — both tools exist. ✓
  - References `overlaps.cross_crate_type_collisions` — bucket exists per W9 in the detailed doc and the `overlaps` schema. ✓
  - References `workspace_stats.pub_crate_share`, `workspace_stats.visibility`, `workspace_stats.items_by_kind` — all real fields per the detailed doc and `workspace_stats` schema. ✓
  - Per-bullet trait-justification claim ("single-importer trait is inlining candidate") — restates §W6 step 6. Internally consistent.

- **Issues**:
  - **(MED)** §23 explicitly says "Section numbers reference rust-guidelines-final.md" but **that file does not exist anywhere in this repo** (only in sibling repos like `chart-refactor-parent`, `coding-agent-tui-2`, `rust-code-workspace`). The section's §4/§7/§8/§10/§11/§12/§17/§23 references are unresolvable from inside this repo. Either copy the guidelines doc into `.docs/`, link to its external location, or weaken the reference.
  - **(LOW)** Section header is `23. Workflows mapped to Rust guidelines (today's tools)` and a closing note says "Note: some checklist items … need parser hooks or new tools — see §24." §24 is "What you can't do today" — consistent. ✓
  - **(LOW)** §8 trait audit row references "(Recipe in §8.)" — that's an in-`workflows-detailed.md` cross-reference (Recipe 8.X family) but reading it from `workflows.md` the reader has to know §8 refers to the *detailed doc*. The new section lives in `workflows.md`, not the detailed doc, so the reference is ambiguous. Same for "(Recipe in §10.)" twice. Spelling out "see workflows-detailed.md W8" would resolve this.

- **Verdict**: MINOR. The tool mappings are accurate but the section depends on an external doc that isn't checked in, and several internal "§N" references are ambiguous between the two workflows files.

### a51822fb — "expand workflows doc with recipes and depth advice"
- **LOC**: 49 (additions to `.docs/workflows.md`)
- **What it adds**:
  - "Hypergraph vs RA tools" guidance after §0.
  - `workspace_stats.pub_crate_share` discipline explanation (§1).
  - `dead_pub_report.crates[].crate` as canonical crate-enumeration source (§1).
  - Module-tree depth advice for §3 (depth=2 default; depth=3 expands items; full-depth = methods).
  - "Method-by-method fan-in (literal recipe)" 4-step recipe in §11.
  - Empty-result-as-signal note in §12.
  - Two new high-leverage recipes in §8 ("Find dead facade re-exports", "Detect half-finished migrations") with worked examples on `coding-agent-bad`.
  - Module-shadow diagnostic (bug vs footgun) in §9.
  - Test-fixture heuristic in §9.
  - Vendored-library caveat for `dead_pub_report` in §9.
  - Parallelism callout in §19.
  - New §20 "Output handling and post-processing" with `<persisted-output>` block detection and `crate_edges` post-processing recipe.
  - Section renumbering: old §20 → §21 (and so on); old §21 (Combining old + new) → §22; old §22 (What you can't do) → §23.

- **Accuracy spot-checks**:
  - `<persisted-output>` block — real (the rust-code-mcp tool layer persists large outputs; this matches the project's tool-results JSON convention).
  - "47 dead pubs in plurimus" / "AgentConfig in agent::config and config crates" / "RunState, InvalidTransition, RunnerWakeError" — example findings from `coding-agent-bad`. Cannot verify from this repo (cross-repo data) but they parallel the same examples used in `workflows-detailed.md` (added in a9133044 lines 198, 678, 687) so the docs are internally consistent.
  - "5-10 calls per round" parallelism advice — sensible; tool layer is async and independent reads have no shared mutable state.
  - "72KB at depth=3" / "67KB on 17-crate workspace" — same numbers in `workflows-detailed.md` lines 240, 1227. Consistent.
  - Vendored-library `plurimus` example: same number (47) cited in both docs (`workflows.md` and `workflows-detailed.md:198`). Internally consistent.

- **Issues**:
  - **(LOW)** Two diff hunks show transitional line content `2122.` and `2223.` — these are *diff render artifacts* from in-place line-prefix renumbering (the `+`/`-` overlay shows both the old and new digit). The committed final file (current `workflows.md` lines 301, 313) has clean `22.` and `23.` numbering. No actual bug.
  - **(LOW)** The new §20 "Output handling" recipes mention a "small Python or jq script" for `crate_edges` reductions but doesn't link to or include such a script. A reader has to write their own. Not wrong, just under-served.
  - **(LOW)** "module_tree depth as the first lever. Reach for depth=2 before reaching for Bash post-processing." — `workflows.md`'s §20 says "Reach for depth=2 before Bash post-processing"; `workflows-detailed.md`'s "Output handling" §1244-1246 says "Reach for `depth=2` before reaching for Bash post-processing". Same intent, slightly different wording. Cosmetic.

- **Verdict**: PASS. New material is factually consistent with `workflows-detailed.md` and matches the codebase. Section renumbering is clean in the final file.

## 3. Cross-commit observations

- **Two-doc structure is intentional**: `workflows.md` is a bullet-list "intent" overview; `workflows-detailed.md` is the templated reference. The detailed doc references the summary at the top ("Expanded specifications for every workflow surfaced in `.docs/workflows.md`"). a51822fb and e99fa6be add to the summary, a9133044 introduces the detailed reference. The two stay terminologically aligned (workflow numbering W1–W16, recipe numbering 8.x / 10.x / 13.x / 16.x).
- **External reference rot**: e99fa6be cites `rust-guidelines-final.md`'s section numbers in detail (§4 / §7 / §8 / §10 / §11 / §12 / §17 / §23) but the file is not in this repo. `workflows-detailed.md` similarly cites `rust-guidelines §4` and `rust-guidelines §8` without a path. If the guidelines doc is intended to live alongside, it needs to be added; otherwise the references should point to the canonical location or be weakened.
- **Parameter shorthand inconsistency**: `get_similar_code(query=<...>)` (correct) appears in `workflows-detailed.md` lines 653, 940, but `get_similar_code(target=<...>)` (wrong) appears in lines 967, 971, and indirectly in `workflows.md` recipe "Find duplicate logic worth extracting" (line 111). Worth normalizing to `query=`.
- **Worked-example consistency**: Both docs cite the same numbers from `coding-agent-bad` (17 crates, 1441 fan-in on `domain`, 89 dead pubs, 47 in `plurimus`, `RunState`/`InvalidTransition`/`RunnerWakeError` dead re-exports, `AgentConfig` migration debt). Either both are right (verified once against the workspace) or both are wrong together. Cannot verify cross-repo from here, but the internal consistency is good.
- **Companion docs**: `.docs/workflow-imports-exports.md` and `.docs/workflow-type-overlaps.md` are referenced from W7/W9 in `workflows-detailed.md`. Both exist. ✓
- **No companion for new sections in `workflows.md`**: The §23 Rust-guidelines mapping (added in e99fa6be) and the §20 Output-handling expansion (added in a51822fb) live only in `workflows.md`, not in `workflows-detailed.md`. The detailed doc has its own "Output handling — when results are large" section; the rust-guidelines mapping has no detailed-doc counterpart. Slight asymmetry but not broken.

## 4. Overall verdict — MINOR

Doc set is broadly accurate against the implementation. Tool names, param names, output field names, output sizes, and behavioral claims all check out. The two outright accuracy issues are:

1. **`get_similar_code(target=...)`** in three places (`workflows-detailed.md:967, 971`, `workflows.md:111`) — should be `query=...`. Easy fix.
2. **`rust-guidelines-final.md` reference** in `workflows.md` §23 — file doesn't exist in this repo. Either ship the guidelines doc with the project, link out, or remove the file-specific reference.

Plus one rendering glitch (`workflows-detailed.md:919` table cell with stray `|`), some ambiguous "§N" cross-references between the two workflows docs, and a missing script for the `crate_edges` post-processing recipe.

None of these block adoption — the docs are usable as-is — but the `get_similar_code` parameter mismatch will mislead an agent that copies the recipe verbatim, and the missing rust-guidelines doc will break the mapping-section reader on first click. Recommend a small follow-up to address those two specifically.
