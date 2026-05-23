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

## Phase 1: Workspace Boundary Rules

- Status: complete.
- Purpose: make the intended crate layering explicit before implementation
  boundary refactors.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `ce5e84a39da64908d800cff4cf51aaa79fa7fb8c`, change
  `pskqvuyvmmnpltszoqrwtupkvkkowuwo`.
- Step 2 source-read architecture/audit locations: completed after pre-step
  summary at commit `e9e69ce0e33c85c99debb1341b939344a8728455`, change
  `nnowvmrvvvmunoktxvowswutmqlruokx`.
- Step 3 boundary rule set: completed after pre-step summary at commit
  `de436c0ef016f208ae059a2e512b9ea34987bdcd`, change
  `xywskktvkkwvkywrnyxzsuwxwqnxnpqu`.
- Step 4 documentation-only command/result: completed after pre-step summary
  at commit `eb7c2f0aa4737f437cf7e94cfa27be831580755c`, change
  `nkyplvkotxlpnrszkkqnsumlukxmxqvy`.
- Step 5 dependency direction: completed after pre-step summary at commit
  `a5f20835a668708b6d04b426b4806e55ded0cd97`, change
  `zmxqmmqluqpkxvroqtnymtrusxlpmxrt`.
- Step 6 forbidden dependency verification: completed after pre-step summary
  at commit `f587295397281439ef951c0286b01a9d16033ff1`, change
  `qpquqlooskzunwwvvxukkkntlxvquvnr`.
- Step 7 focused nix check: completed as not required after pre-step summary
  at commit `5886379b36dec1c556d5b35f0331a153b60d3e96`, change
  `kulsqlwqynuwtnnrlypvklnrmnvoozpn`.
- Step 8 ledger update: completed after pre-step summary at commit
  `cad14ff665f1b64cd1a18395970a6242365130ae`, change
  `lvwtntvqmtozyvpoolxyqzzuupnlqknl`.

### MCP Evidence

- `get_imports(directory, module="rmc_server", summary=true, limit=300)`:
  zero root-module imports.
- `module_dependencies(directory, module="rmc_server", summary=true,
  limit=300)`: zero root-module dependencies.
- `forbidden_dependency_check(directory, rules=[...], summary=true,
  limit=300)`: five rules, zero violations.

### Source Reads

- `.docs/architectural-rules.md`: existing machine-enforceable rules document,
  currently written for older Phase B/Phase C state.
- `crates/rmc-graph/src/graph/query/tests.rs`: generic
  `forbidden_dependency_check` behavior tests.
- `crates/rmc-graph/src/graph/query/model.rs`: `ForbiddenDependencyRule`
  public shape.
- `crates/rmc-graph/src/graph/query/crates.rs`: rule matching semantics.

### Files Changed

- `.docs/architectural-rules.md`
- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`

### Verification

- MCP `forbidden_dependency_check` with the documented five-rule set:
  `rule_count=5`, `violation_count=0`, `total_match_count=0`,
  `returned_match_count=0`.

### Commits

- Step 1 documentation: `aa3264b7` (`docs: record phase 1 step 1`).
- Step 2 documentation: `43e54fff` (`docs: record phase 1 step 2`).
- Boundary rule set: `b9eb418c` (`docs: update boundary rule set`).
- Boundary rule check docs: `26631423` (`docs: document boundary rule check`).
- Dependency direction docs: `77af592d` (`docs: record boundary dependency direction`).
- Verification docs: `cd53e088` (`docs: verify boundary rule check`).
- Check-status docs: `60b4789a` (`docs: record phase 1 check status`).

### Remaining Follow-Up

- Start Phase 2.

## Phase 2: `rmc-indexing` Search Facade

- Status: in progress.
- Purpose: stop server query/codemap production code from opening
  `TantivyAdapter` directly.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `fca77ee055ae15c0176a62da9d84654bbc0beb7b`, change
  `vpzltotxvvrvnosvqzsytlpnwoklzupw`.
- Step 2 indexing search facade: completed after pre-step summary at commit
  `7f3a08365114f8cddf7a3b8b01ee41b7fe057e25`, change
  `ysuwplquvvkqwyptnskkxlqmzymykvkw`.
- Step 3 Tantivy ownership check: completed after pre-step summary at commit
  `826a427bd20ff885143d396195828ca36321d25e`, change
  `xvtmnykxqyvylnvynkktokluvtwmkqut`.
- Step 4 server query migration: completed after pre-step summary at commit
  `21f9c6e315ce37e8daf902f72316778732fb576e`, change
  `quoqqmqpumlzytqmpkwxsnluuutxltxl`.
- Step 5 server codemap migration: completed after pre-step summary at commit
  `af97b059f19b35161ab72ff31e03f9ca2ea11bbd`, change
  `xqyxwtslpwrvqyykyumqswzzqmwsrwql`.
- Step 6 compatibility export check: completed after pre-step summary at
  commit `71cbda4fd1d6e709c24b4702942295dc688a7dcb`, change
  `nnoruxukxwskzmzwotszmzronrklnkvl`.
- Step 7 server dependency verification: completed after pre-step summary at
  commit `5b923b94b5bf4227102442c81d7766111c23d9a9`, change
  `rpuomsqkxvovryzpmpslxlnwlkxomsss`.
- Step 8 focused nix checks: attempted after pre-step summary at commit
  `a30f01c4c2f463ca12c0ef66f165c5fc8436538f`, change
  `ymxovsnvyuzuoolovssznxttulpkqkly`.

### MCP Evidence

- `who_imports(target="rmc_indexing::indexing::tantivy_adapter::TantivyAdapter")`
  returned four bindings, all in indexing modules/tests or the compatibility
  reexport.
- `module_dependencies` showed server `query` and `codemap` depend on
  `rmc_indexing::indexing::tantivy_adapter` through inline references.
- `get_exports(module="rmc_indexing::indexing", consumer="rmc_server")`
  confirmed `TantivyAdapter` and the implementation module are still public.
- After the code migration, `build_hypergraph(force_rebuild=true)` produced
  graph `06c80cff231427cb53c75e7c071397fd`.
- Refreshed `module_dependencies` for server `query` and `codemap` no longer
  listed `rmc_indexing::indexing::tantivy_adapter`; both now depend on
  `rmc_indexing::indexing::search`.

### Files Changed

- `crates/rmc-indexing/src/indexing/search.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rmc-server/src/tools/graph/codemap.rs`
- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`

### Verification

- MCP verification passed after rebuilding the hypergraph: server `query` and
  `codemap` depend on `rmc_indexing::indexing::search`, not
  `rmc_indexing::indexing::tantivy_adapter`.
- Focused nix check attempted:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`.
  Result: failed before checking touched crates because `candle-kernels v0.10.2`
  hit a CUDA/GCC `cc1plus` internal compiler error compiling
  `src/moe/moe_wmma_gguf.cu`; Cargo then did not exit promptly and was
  terminated.
- Focused nix check retry attempted:
  `nix develop ../nix-devshells#cuda-code --command env CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
  Result: same `candle-kernels` CUDA/GCC internal compiler error.

### Remaining Follow-Up

- Record the Phase 2 ledger commit.
