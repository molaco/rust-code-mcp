# Boundaries Cleanup Progress

## Phase 0: Baseline And Safety Checks

- Status: complete.
- Purpose: record the initial VCS, hypergraph, dependency, and layering
  baseline before implementation boundary changes.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `bf2bb57e4a7066f9e2e70b68ac79ee6ac3d637bf`, change
  `uozpxtlmwxvypwqszkrprvsswspumypx`.
- Step 2 `jj status`: completed after pre-step summary at commit
  `da52dacd54fb343c6d6a3aaa8ddeeddd7438f225`, change
  `qkxqzrmnqstknxpkpzuzqqprsntqtunx`. Status output reported no changes.
- Step 3 hypergraph baseline: completed after pre-step summary at commit
  `b86e39145d78da9ab0b35d5d0efea457a4acf92c`, change
  `lvsvwnwlkutqnuvmkponnkpprvvsomsn`.
- Step 4 layering rule check: completed after pre-step summary at commit
  `e7aa57387d7ef146bed8478a8837c866b92493e9`, change
  `rltttsuqztlllnsluvsyvzmnmswskwss`.
- Step 5 baseline ledger update: completed after pre-step summary at commit
  `2abedcedcc47cde3f99bf21a6febde4f88373b7b`, change
  `tkswovouxtwnrkspxutmpumotmlkqmnz`.
- Step 6 phase ledger commit: completed after pre-step summary at commit
  `a9651239cb0298e4001d4d04e97f3df30b2f2c1f`, change
  `sxrtnnxswovmzktvvwoxzupyzlwuwuly`. The Phase 0 baseline ledger commit is
  `e4aeefdeac6b3e4dce3041158fdc681d564dc1ce`.

### MCP Evidence

- `build_hypergraph(directory, force_rebuild=false)` reused graph
  `4fc200b6ab2a6d0ef4162f4fec31da5f`.
- Hypergraph fingerprint:
  `a2800cb435de19d32f27bf58901fd5efb037e85565033279dd50611589501073`.
- Hypergraph counts: 3040 nodes, 5371 bindings, 7963 usages.
- `workspace_stats(directory)` baseline: 45 crates, 296 modules, 2448 items,
  250 external symbols, `pub_crate_share=0.46781789638932497`.
- `crate_edges(directory, summary=true, limit=300)` baseline: 49 edges.
- `crate_dependency_metric(directory, sort_by="instability", limit=300)`
  baseline: 45 crate metrics. Core crate instability values:
  `rmc_server=0.4`, `rmc_config=0.25`, `rmc_indexing=0.125`,
  `rmc_graph=0.08333333333333333`, `rmc_engine=0.06666666666666667`.
- `forbidden_dependency_check(directory, rules=[...], summary=false,
  limit=300)` baseline: five rules, zero violations.

### Files Changed

- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`

### Verification

- Verification command: MCP `forbidden_dependency_check` with the planned
  five-rule crate layering set.
- Verification result: `violation_count=0`, `total_match_count=0`.

### Commits

- Step 1 documentation: `4b4a7775` (`docs: record phase 0 step 1`).
- Step 2 documentation: `4cec359e` (`docs: record phase 0 step 2`).
- Step 3 documentation: `46ed31f8` (`docs: record phase 0 step 3`).
- Step 4 documentation: `34ca82b8` (`docs: record phase 0 step 4`).
- Baseline ledger: `e4aeefde` (`docs: record phase 0 baseline`).

### Remaining Follow-Up

- Start Phase 1.
