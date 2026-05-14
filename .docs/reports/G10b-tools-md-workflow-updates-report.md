# G10b — TOOLS.md + workflow documentation updates

## 1. Group summary

Three documentation commits that expand the tool catalog (`TOOLS.md`) and bring the workflow guides (`.docs/workflows.md`, `.docs/workflows-detailed.md`) up to date with the hypergraph / Layer 10 / Phase 5–8 tool surface.

| Commit | LOC | Files |
|---|---:|---|
| `53d249ea` | 998 | `TOOLS.md` (+984/-14) |
| `7f22dac9` | 2 | `TOOLS.md` (1 line edit) |
| `7557c100` | 1,515 | `.docs/workflows.md` (+58/-34); `.docs/workflows-detailed.md` (+1196/-94) |

Net effect: TOOLS.md gains catalog entries for 30 hypergraph/graph tools; workflows.md/-detailed.md gain workflow recipes for the Layer 10 call-graph, three-tier semantic tooling, forbidden-dependency rules, attribute audits, signature-based discovery, unsafe/mut-static/recursion audits, and per-crate dependency metric. The 2-LOC commit fixes a stale `semantic_overlaps` description.

The codebase under review at these commits exposes 49 MCP tools via `src/tools/search_tool_router.rs`. After the catalog expansion, TOOLS.md documents 48 of them. The 49th — `build_codemap` — is not yet in the tool router at `7557c100` (the `codemap phase 6: renderers + MCP tool wiring` commit `b39404e8` lands *after* this group), so its absence from these docs is *contextually correct*, not a defect.

## 2. Per-commit review

### `53d249e` — expand TOOLS.md tool catalog with hypergraph and graph tool entries — 998 LOC

**Adds:** 30 new `####`-level tool subsections, organized under section headers `Build & Lifecycle`, `Imports / Exports / Re-exports`, `Reverse Lookup`, `Call Graph (Layer 10)`, `Workspace Structure / Audits`, `Architectural Rules & Audits`, `Function Signatures (Phase 5)`, `Safety Audits`, `Semantic`. Also reshuffles the Overview table so `read_file_content` sits with the other query tools rather than under "Analysis."

**Tool catalog accuracy (spot-checks vs `src/tools/search_tool_router.rs` and `src/tools/search_tool.rs`):**
- `build_hypergraph` — args `directory` (req) + `force_rebuild` (opt, default false). Verified matches `BuildHypergraphParams` in source (`force_rebuild: params.force_rebuild.unwrap_or(false)`).
- `call_graph` — depth default 3, capped at 8. Matches source contract documented in the `#[tool(description=...)]`.
- `recursive_callers_count` — depth default 3, capped at 8. Matches.
- `crate_dependency_metric` — `sort_by` accepts `instability` / `item_count` / `afferent` / `efferent` / `abstractness`; unknown values → `invalid_params`. Matches the live `#[tool(description=...)]`.
- `forbidden_dependency_check` — rule fields `consumer` / `producer` (req), `except` / `severity` / `message` (opt). Matches `ForbiddenDependencyRule` shape used in `src/tools/search_tool.rs:218`.
- `functions_with_filter` — `limit` default 50, `offset` default 0, `summary` default false. Matches source contract.
- `missing_docs_audit` / `derive_audit` / `recursion_check` / `channel_capacity_audit` / `fn_body_audit` — parameter schemas (`crate_name`, `item_kind`, `skip_test_items`/`skip_test_fns`, `required_derives`, `max_cycle_length`, `patterns`, defaults) cross-checked against the `MissingDocsAuditParams` / `DeriveAuditParams` / `RecursionCheckParams` / `ChannelCapacityAuditParams` / `FnBodyAuditParams` structs at `src/tools/search_tool.rs:333-404`. All defaults and required/optional flags agree.

**Issues:**
- **MINOR — wrong tool count in architecture diagram.** The section header on line 1802 reads `### Hypergraph (build_hypergraph + 21 graph tools)`. Counting `####` subsections under the Hypergraph Tools heading (excluding `build_hypergraph` itself) yields **36 graph tools**, not 21. The "21" appears to be carried over from an earlier draft and was never updated as more tools were added in the same commit. (Verified via `jj diff -r 53d249ea` — the literal string `21 graph tools` is on a `+` line.)
- **MINOR — stray anchor artifact.** Line 1461 contains a bare `[ANCHOR](#channel_capacity_audit)` marker on its own line, immediately after the `channel_capacity_audit` description. No other tool subsection has this; it looks like leftover editing scaffolding.
- **INFO — `build_codemap` not documented.** Tool exists in the router (`build_codemap` at `src/tools/search_tool_router.rs:535+`) but is not in TOOLS.md. As noted in §1, this is contextually correct: the MCP wiring commit `b39404e8` post-dates this group.

**Verdict:** good factual coverage of the 30 newly-documented tools; two cosmetic issues that should be cleaned up.

### `7f22dac9` — update TOOLS.md documentation — 2 LOC

**Adds:** Single-line rewrite of the opening sentence of `#### semantic_overlaps` (line 1353).

**Before:** "Workspace-wide semantic-overlap audit. Enumerates Items (optionally scoped to a crate / item_kind), embeds each one's source **via `vector_only_search`**, builds a similarity graph above `threshold` (default 0.85), and either returns deduplicated pairs or single-linkage clusters …"

**After:** "Workspace-wide semantic-overlap audit. Enumerates Items (optionally scoped to a crate / item_kind), embeds each Item's source bytes **(cached per-Item in the snapshot's LMDB env), runs an in-memory pairwise cosine scan**, and either returns deduplicated pairs or single-linkage clusters of transitively-similar items above `threshold` (default 0.85). …"

**Why this matters:** the *before* text is factually wrong — `semantic_overlaps` does NOT call `vector_only_search` for each Item; the v1.1 implementation embeds Item source directly and caches per-Item in LMDB, then runs in-memory pairwise cosine. The fix aligns the description with both the `#[tool(description=…)]` text in `search_tool_router.rs:519` and the longer notes in the same TOOLS.md section (lines 1757-1758 already say "the `embeddings_by_target` sub-DB inside the snapshot's LMDB env … `semantic_overlaps` is the only writer").

**Issues:** none.

**Verdict:** correct, minimal, surgical fix.

### `7557c100` — update workflow documentation — 1,515 LOC

**Adds:**
- `.docs/workflows.md`: appends to the quick-start recipe (steps 8–11: unsafe_audit, mut_static_audit, semantic_overlaps@0.95, forbidden_dependency_check); adds Layer-10 trait-dispatch entry to §6; adds `crate_dependency_metric` to §7; rewrites §13 into the three-tier vector tool model (get_similar_code / similar_to_item / semantic_overlaps); rewrites §14 as Layer-10 call-graph (who_calls, calls_from, call_graph, callers_in_crate, recursive_callers_count); adds full §21 cheat sheet table; adds §25–§29 sections (architectural rules, attribute audits, signatures, unsafe/static audits, workspace-wide duplicates).
- `.docs/workflows-detailed.md`: massively expanded — adds workflow chapters W14 (call graphs), W17 (architectural rules), W18 (attribute audits), W19 (signature discovery), W20 (unsafe audit), W21 (mut static), W22 (re-export chain), W23 (crate metric), W24 (enum variants). Pre-existing W2 / W6 / W8 / W13 are rewritten to incorporate the new tools.

**Internal consistency vs `TOOLS.md`:**
- The §21 cheat-sheet table in `workflows.md` lists tools with matching layer tags (Layer 4 / Layer 6 / Layer 10 / Phase 5 / Phase 6 / Phase 7 / RA / parser / LanceDB / infrastructure). Tool names, return shapes, and one-liner "best for" descriptions agree with the `####` headings and Overview table in TOOLS.md.
- `crate_dependency_metric.sort_by` enumerated values agree across both docs (`instability`, `afferent`, `efferent`, `item_count`, `abstractness`).
- `call_graph(depth=3, max=8)` and `recursive_callers_count(depth=8)` defaults agree across docs and source.
- `semantic_overlaps` is consistently described as in-memory pairwise cosine with per-Item LMDB cache (matches the `7f22dac9` correction).

**Ordering — does the workflow update reference tools the catalog expansion added?** Yes for most, but with gaps (see Issues).

**Issues:**
- **NEEDS WORK — Phase 8 tools added to TOOLS.md but missing from workflows.** `53d249ea` adds catalog entries for `missing_docs_audit`, `derive_audit`, `recursion_check`, `channel_capacity_audit`, and `fn_body_audit`. `7557c100` lands afterwards yet never mentions any of these five tools. Verified by grepping both `.docs/workflows.md` and `.docs/workflows-detailed.md` — zero hits for those five tool names. Concretely:
  - The §21 cheat-sheet table in `workflows.md` (lines 256–299) stops at `mut_static_audit` (Phase 7). No row for any Phase 8 tool.
  - `workflows-detailed.md` chapters stop at W24 (`enum_variants`). There is no W25/W26/W27 for the Phase 8 audit tools, even though W20 (unsafe) and W21 (mut static) provide a natural template.
  - The §23 "Workflows mapped to Rust guidelines" section in `workflows.md` references §13 (unsafe) and §NEW (global mutable state) but does NOT reference §16 (docs) → `missing_docs_audit`, §8 (Debug derives) → `derive_audit`, §22 (recursion) → `recursion_check`, §12 (bounded channels) → `channel_capacity_audit`, or §3/§9/§19/§22 (body patterns) → `fn_body_audit`. The Phase 8 tools were specifically designed for these guideline mappings.
- **MINOR — §1 of `workflows.md` calls `pub_crate_share` a workspace_stats field.** Existing claim from prior commits, not introduced here, but worth flagging in a doc review: source confirmation needed.
- **MINOR — small numeric inconsistency carried over.** `workflows.md` line 233 references "Layer 4 was v4→v5"; TOOLS.md mentions v11 snapshot. Layer naming vs version-number naming may confuse readers but is not introduced by this commit.

**Verdict:** large, mostly accurate workflow expansion. The omission of the five Phase 8 audit tools is a real gap — the cheat sheet should be the canonical "what tools exist" reference and currently lies by omission about 5 tools that the same group of commits documents in `TOOLS.md`.

## 3. Cross-commit observations

- **Ordering is reasonable.** `53d249ea` (catalog) → `7f22dac9` (single-line catalog fix) → `7557c100` (workflow rewrite). The workflow rewrite cites the tools the catalog introduced, with one notable exception (Phase 8 tools — see above).
- **Internal consistency between `TOOLS.md` and the workflow files is high.** Same tool names, same parameter names, same default values, same layer tags. Stale-vs-fresh mismatches are confined to the "21 graph tools" line in the architecture diagram, the orphan `[ANCHOR](#channel_capacity_audit)` marker, and the missing-from-workflows Phase 8 tools.
- **`semantic_overlaps` is described identically across the three places it appears** after `7f22dac9` lands: TOOLS.md head paragraph (line 1353), TOOLS.md "Storage" notes (line 1757), and workflows.md §13/§29. Earlier inconsistency (in-memory cosine vs `vector_only_search`) is resolved.
- **No stale tool entries.** No tool documented in TOOLS.md is absent from the router. The reverse (router-but-missing-from-docs) is true for `build_codemap` only, which is a chronology artifact, not a defect at these commits.
- **Missing-from-docs alongside the 49→48 gap:** if a follow-up commit adds `build_codemap` to TOOLS.md, the architecture diagram label `21 graph tools` should become `37 graph tools` (36 currently documented + `build_codemap`).

## 4. Overall verdict — MINOR (close to NEEDS WORK on the workflow gap)

The catalog expansion (`53d249ea`) is factually accurate against the source for the 30 tools it adds. The single-line correction (`7f22dac9`) fixes a real bug in the prior description. The workflow rewrite (`7557c100`) is mostly accurate and consistent with `TOOLS.md`, but it stops short of integrating the five Phase 8 audit tools (`missing_docs_audit`, `derive_audit`, `recursion_check`, `channel_capacity_audit`, `fn_body_audit`) that the same group ships in `TOOLS.md`. That gap is the only issue significant enough to warrant follow-up; the other two findings (stale "21 graph tools" label, stray `[ANCHOR]` marker) are cosmetic.

Concrete fixes recommended (in priority order):
1. Update the §21 cheat-sheet table in `workflows.md` and add `## W25`–`## W29` chapters in `workflows-detailed.md` for the five Phase 8 audit tools, mirroring the W20 (unsafe) / W21 (mut static) structure.
2. Replace `21 graph tools` with the current count (36 today; 37 if `build_codemap` is later documented) in `TOOLS.md` line 1802.
3. Delete the stray `[ANCHOR](#channel_capacity_audit)` line at `TOOLS.md:1461`.
