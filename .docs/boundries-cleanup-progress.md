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

- Status: complete.
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
- Step 9 ledger update: completed after pre-step summary at commit
  `d2f9b7f18ace12b089248775d918b27097b86ac1`, change
  `nrpymunwwkmwmkzsxourumvqlnzrmoup`.
- Phase completion report: completed after pre-step summary at commit
  `85ef0c5adf1561983d1de656796d3e956adeb496`, change
  `zkkswxqloywvsptlrwzplxtkqxpvouxr`.
- Post-review read-only BM25 correction: completed after pre-step summary at
  commit `b794b50a483091a2f1f0536196c4c04c0dabbad8`, change
  `qzznxxrwznurrnsmxwkzuroxwzrznrpw`.

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
- Post-review source evidence showed `TantivyAdapter::new` and
  `Bm25Search::new` both open or create a missing Tantivy index. The search
  facade now opens an existing Tantivy index directly with
  `tantivy::Index::open_in_dir`, and the server health probe uses the same
  facade.

### Files Changed

- `crates/rmc-indexing/src/indexing/search.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rmc-server/src/tools/graph/codemap.rs`
- `crates/rmc-server/src/tools/endpoints/health.rs`
- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-2-boundrie-fix-report.md`

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
- Post-review focused nix check passed with CUDA thread caps:
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
- Post-review focused tests passed with CUDA thread caps:
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo test -p rmc-indexing open_bm25_search --jobs 1`.
  Result: four `open_bm25_search` tests passed.

### Commits

- Step 1 documentation: `93e2b5b7` (`docs: record phase 2 step 1`).
- Search facade: `18e8e7c8` (`refactor: add indexing search facade`).
- Adapter ownership docs: `29b87d19` (`docs: record phase 2 adapter ownership`).
- Query migration: `dee9f48e` (`refactor: use indexing search facade in query`).
- Codemap migration: `6d6f4a21` (`refactor: use indexing search facade in codemap`).
- Compatibility export docs: `1cb8e884` (`docs: record phase 2 compatibility export`).
- Dependency verification docs: `f30e7981` (`docs: verify phase 2 dependencies`).
- Check-result docs: `c56b74ee` (`docs: record phase 2 check result`).
- Ledger docs: `c2ae6cf0` (`docs: record phase 2 ledger`).
- Read-only BM25 correction: `2ae2e365` (`fix: open bm25 search read-only`).

### Remaining Follow-Up

- Start Phase 3.

## Phase 3: `rmc-indexing` Incremental Indexing Facade

- Status: complete.
- Purpose: stop server index/sync production code from constructing
  `IncrementalIndexer` directly while keeping `IncrementalIndexer` public for
  compatibility.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `1246ed40d952f65679ea505e67194973d857de67`, change
  `zqwzqttxromrslzpsupuympxunyyqvrq`.
- Step 2 incremental service facade: completed after pre-step summary at
  commit `cc2120bcf258f176c0a0699a87b8dc1d8ecf94d6`, change
  `nqxrrlqkuzrnvlspsoyxxrqsmsvomroq`.
- Step 3 facade shape check: completed after pre-step summary at commit
  `0152c6a925058b57321b1492e746cf5aa24dbef5`, change
  `pklqnpxkpkrmlnokklzvpxyumkkltmvy`. The new facade accepts directory,
  backend, path, identity, snapshot, codebase-size, and force options while
  keeping `IncrementalIndexer` construction and change detection inside
  `rmc_indexing`.
- Step 4 server index endpoint migration: completed after pre-step summary at
  commit `479b445fcd47137579d8163c82a1c708da2e0d11`, change
  `vzrzzmwnoowxuqryywumzotpoqzyntxz`.
- Step 5 `SyncManager` migration: completed after pre-step summary at commit
  `dda24e869997d055f9695b08ca0d8e35ac39a2f4`, change
  `zuqzzsspslnonryqzuktruxpxtkqwxlm`.
- Step 6 compatibility export check: completed after pre-step summary at
  commit `ae3f14da1e4348b5fb46115b610861532c401ad3`, change
  `pqltzkurvwyynrwnpxnkmnlrlmlmxuqn`.
- Step 7 production dependency verification: completed after pre-step summary
  at commit `5d5ca5eb3e431d1b093b24d4b2c088ddc7dea252`, change
  `uyvppnvykoqwuvquzxtmtpllusxxrtnt`.
- Step 8 focused nix check: completed after pre-step summary at commit
  `15406891d014fe215287d928780808286fb44c87`, change
  `xvmqvywzlmwrwmurvnquzuvtolwxqpxy`.
- Step 9 ledger update: completed after pre-step summary at commit
  `93f05ca7ba0647954f8055ed3f2d1290a6abbc56`, change
  `ymyrwrtuxnrnoskvtwxmkuqntpmvknnl`.
- Phase completion report: completed after pre-step summary at commit
  `494fc714c223d82d77d29f69388bdf814596252d`, change
  `umpvurtuuzqmzkozulwzkpkrwwxmqnxv`.

### MCP Evidence

- `build_hypergraph(directory, force_rebuild=true)` produced graph
  `73fff61394cb3013da54fdacb4324029` with fingerprint
  `8847750a44d5137b0523263cd98697d2e8406fd96d7716d9e51530b9d32c2e24`.
- `who_imports(target="rmc_indexing::indexing::incremental::IncrementalIndexer")`
  returned 14 bindings. Production server imports were
  `rmc_server::tools::endpoints::index` and `rmc_server::mcp::sync`; remaining
  bindings included indexing tests, server tests, benches, tools, and the
  compatibility reexport.
- `get_imports` for `rmc_server::tools::endpoints::index` and
  `rmc_server::mcp::sync` both showed named imports of `IncrementalIndexer`.
- `module_dependencies` for `rmc_server::tools::endpoints::index` showed a
  direct dependency on `rmc_indexing::indexing::incremental` through
  `IncrementalIndexer`, `IncrementalIndexer::with_backend`,
  `IncrementalIndexer::clear_all_data`, and
  `IncrementalIndexer::index_with_change_detection`.
- `module_dependencies` for `rmc_server::mcp::sync` showed a direct dependency
  on `rmc_indexing::indexing::incremental` through `IncrementalIndexer`,
  `IncrementalIndexer::with_backend`, and
  `IncrementalIndexer::index_with_change_detection`.
- `functions_with_filter(krate="rmc_indexing", has_param_type="IncrementalIndexer")`
  returned zero matches.
- Compatibility source check confirmed `rmc_indexing::indexing::incremental`
  remains `pub mod`, `IncrementalIndexer` remains a public struct, and
  `rmc_indexing::indexing` still reexports `IncrementalIndexer`.
- After migration, `build_hypergraph(directory, force_rebuild=true)` produced
  graph `b2f982db0f3dcfb48cf162255b8d6696` with fingerprint
  `052f58122ab03d6f58ef20e1a01491d24c9db336182d78ffb39be166f8dc8792`.
- Refreshed `module_dependencies` for `rmc_server::tools::endpoints::index`
  and `rmc_server::mcp::sync` no longer listed
  `rmc_indexing::indexing::incremental`; both now depend on
  `rmc_indexing::indexing::incremental_service`.
- Refreshed `who_imports(target="rmc_indexing::indexing::incremental::IncrementalIndexer")`
  returned 11 bindings, down from 14. Remaining direct importers are
  compatibility consumers, tests, benches, tools, the public reexport, and
  `rmc_indexing::indexing::incremental_service`.

### Files Changed

- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`
- `crates/rmc-indexing/src/indexing/incremental_service.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-server/src/tools/endpoints/index.rs`
- `crates/rmc-server/src/mcp/sync.rs`
- `.docs/phase-3-boundrie-fix-report.md`

### Verification

- `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing --jobs 1`
  passed with existing warnings.
- `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-server --jobs 1`
  passed with existing warnings.
- `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-server --jobs 1`
  passed again after the `SyncManager` migration, with existing warnings.
- `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`
  passed with existing warnings.

### Commits

- Step 1 documentation: `dadd4305` (`docs: record phase 3 step 1`).
- Incremental service facade: `cfbfd981` (`refactor: add incremental indexing service facade`).
- Facade shape docs: `7d0b595c` (`docs: record phase 3 facade shape`).
- Index endpoint migration: `faaa16a6` (`refactor: use incremental service in index endpoint`).
- Sync manager migration: `5c88f5e7` (`refactor: use incremental service in sync manager`).
- Compatibility export docs: `78f38279` (`docs: record phase 3 compatibility export`).
- Dependency verification docs: `60fb890b` (`docs: verify phase 3 dependencies`).
- Check-result docs: `0e28bf4e` (`docs: record phase 3 check result`).
- Ledger docs: `53d5393b` (`docs: record phase 3 ledger`).

### Remaining Follow-Up

- Start Phase 4.

## Phase 4: Project Path And Identity Boundary

- Status: complete.
- Purpose: move or centralize project/index identity logic so server does not
  own mixed engine/indexing path policy.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `6310c735f1fa7e5662a932a85bb1b0bcfff08ac2`, change
  `quywnkvozwmxoypwwmorprwvnzkmxqvk`.
- Step 2 responsibility split: completed after pre-step summary at commit
  `f8e17fd39098744e20a0b3d3a81d4e45a73db846`, change
  `vxylvrnxzysozqmnrxplqmllwlyuomll`.
- Step 3 indexing-owned path helpers: completed after pre-step summary at
  commit `8d26a4cd8a5bb97bba499da09a2efae75a54c6fa`, change
  `xluswuuvtvolvqxmwkwzlwquqpxlyuqs`.
- Step 3 documentation catch-up: completed after pre-step summary at commit
  `658a8667da56fcb73411eb895f0c0e8c33a8787c`, change
  `yvnultyorsvxxzzzzvmpxvomxnsytvxo`.
- Step 4 compatibility wrapper evidence: completed after pre-step summary at
  commit `05c745e3e40b51d4440229c2d32aadea4226d6c0`, change
  `rwtpxuxtslyrnpsrqmxnpmvyvwyypoqq`.
- Step 5 duplicate `data_dir` helper consolidation: completed after pre-step
  summary at commit `69e7b3d7daf2059e70c1e0bf4766dfa0a8afc309`, change
  `kkpottulqvowxsykxzsokrmwvsnyrmtt`.
- Step 6 backend resolver consolidation: completed after pre-step summary at
  commit `54bcf32df81709850cc9f7941a72ecb57bf1bb7c`, change
  `mvqwqpkyuoxumrkotrmvqurkxuuqpqps`.
- Step 7 semantic/import verification: completed after pre-step summary at
  commit `3144906691a9fa8896a4f4af7d8bffe665ba5474`, change
  `qzmtzqumnzsrlrkxnnyumrprqpvkxwyv`.
- Step 8 focused nix checks: completed after pre-step summary at commit
  `f39ef4930e029d287024ee687529aec909c539a8`, change
  `qmowvluwlrxounuyozuztlmtkykqklpx`.
- Step 9 ledger update: completed after pre-step summary at commit
  `ed4702930e69011dd382d14da4f25609465062f8`, change
  `qzyowuwkxmunktkrkonlmvwxqpkuzsxl`.
- Phase completion report: completed after pre-step summary at commit
  `e200d879e10f59b95d28e632234c17b37cd81eb3`, change
  `rqltnpzptqsxmlovkmloswopywrmyopu`.

### MCP Evidence

- `build_hypergraph(directory, force_rebuild=false)` reused graph
  `b2f982db0f3dcfb48cf162255b8d6696` with fingerprint
  `052f58122ab03d6f58ef20e1a01491d24c9db336182d78ffb39be166f8dc8792`.
- `who_imports(target="rmc_server::mcp::project_paths::ProjectPaths")`
  returned eight bindings. Production server importers include query, health,
  index, and the compatibility `tools::project_paths` module; other importers
  are tests and integration compatibility.
- `functions_with_filter(krate="rmc_server", has_param_type="ProjectPaths")`
  returned six query helpers: `clean_stale_index`, `create_hybrid_search`,
  `ensure_indexed`, `resolve_query_backend`, `try_open_bm25`, and
  `vector_metadata_exists`.
- `module_dependencies(module="rmc_server::mcp::project_paths")` showed mixed
  ownership dependencies on engine embedding backend/profile APIs,
  `rmc_indexing::indexing::identity`,
  `rmc_indexing::indexing::incremental::get_snapshot_path_for_identity`,
  `directories::ProjectDirs`, and local hashing via `sha2`.
- `semantic_overlaps(crate_name="rmc_server", item_kind="Function")` found the
  relevant Phase 4 duplicate clusters:
  `rmc_server::mcp::project_paths::data_dir` with
  `rmc_server::tools::endpoints::indexing_support::data_dir`, and
  backend-resolution helpers in project paths, query, graph similarity, and
  index.
- After Step 3, `build_hypergraph(directory, force_rebuild=true)` produced
  graph `ce626950ad825420375344f20d145a95` with fingerprint
  `14b1c6e11aa003aff90494fcd4cfbc98dc57aaff04722176bd7258e4b379476f`.
- Refreshed
  `module_dependencies(module="rmc_server::mcp::project_paths")` now lists
  `rmc_indexing::indexing::project_paths` for indexing path policy, while
  `rmc_indexing::indexing::identity`,
  `rmc_indexing::indexing::incremental`, and `sha2` are no longer direct
  server project-path dependencies.
- Refreshed
  `module_dependencies(module="rmc_indexing::indexing::project_paths")` shows
  the indexing-owned module depends on `rmc_indexing::indexing::identity`,
  `rmc_indexing::indexing::incremental::get_snapshot_path_for_identity`,
  `EmbeddingBackend`, and `sha2`.
- `who_imports(target="rmc_indexing::indexing::project_paths::IndexingProjectPaths")`
  returned three bindings: the public indexing reexport, the server
  compatibility wrapper, and server wrapper tests.
- `who_imports(target="rmc_indexing::indexing::identity::indexing_identity")`
  and
  `who_imports(target="rmc_indexing::indexing::identity::active_chunking_identity_for_backend")`
  no longer show server production importers; remaining importers are indexing
  modules/tests.
- `who_imports(target="rmc_indexing::indexing::incremental::get_snapshot_path_for_identity")`
  shows the new indexing project-path module and indexing tests as the
  remaining importers.
- `who_imports(target="sha2::Sha256")` shows no server importers after the
  direct server `sha2` dependency was removed.
- Step 4 `who_imports(target="rmc_server::mcp::project_paths::ProjectPaths")`
  still returns eight bindings: production users in query, health, and index;
  compatibility export in `rmc_server::tools::project_paths`; tests; and the
  integration compatibility importer.
- Step 4 function filtering for `ProjectPaths` parameters still returns the
  six query helper users:
  `clean_stale_index`, `create_hybrid_search`, `ensure_indexed`,
  `resolve_query_backend`, `try_open_bm25`, and `vector_metadata_exists`.
- Step 4 `module_dependencies(module="rmc_server::tools::project_paths")`
  shows that the tools module only reexports
  `rmc_server::mcp::project_paths` symbols.
- After Step 5, `build_hypergraph(directory, force_rebuild=true)` produced
  graph `5f91461896d45246c51e9fa601cd5d90` with fingerprint
  `d856a1f930500d5630add0af711efda87321d1409341278e47f14d2e5d4bb5c1`.
- Step 5
  `functions_with_filter(krate="rmc_server", returns_type_pattern="PathBuf")`
  returned `ProjectPaths::vectors_root`, `project_paths::data_dir`, and
  `SyncManager::get_tracked_directories`; the previous
  `indexing_support::data_dir` wrapper is gone.
- Step 5
  `module_dependencies(module="rmc_server::tools::endpoints::indexing_support")`
  shows direct usage of `rmc_server::mcp::project_paths::data_dir`.
- Step 5 `who_imports(target="rmc_server::mcp::project_paths::data_dir")`
  returned cache, health, the `tools::project_paths` compatibility export, and
  tests as importers.
- Step 5 `semantic_overlaps(crate_name="rmc_server", item_kind="Function")`
  no longer reports the old `data_dir` duplicate cluster. The backend
  resolver cluster remains for Step 6.
- After Step 6, `build_hypergraph(directory, force_rebuild=true)` produced
  graph `2c6dfe88c8bad3b7db1838a94b00287b` with fingerprint
  `680958b42dd9eaa0c1d72a5958fc985c38673f053fd17072d09aeda0eaa58b6d`.
- Step 6 `rg` evidence found no remaining
  `resolve_requested_backend`, `resolve_graph_tool_backend`, or
  `resolve_embedding_backend(` references.
- Step 6
  `functions_with_filter(krate="rmc_server", returns_type_pattern="EmbeddingBackend")`
  now returns two helpers: the shared
  `rmc_server::mcp::project_paths::resolve_embedding_backend_for_mcp` and
  the index endpoint's legacy-model wrapper
  `rmc_server::tools::endpoints::index::resolve_backend`.
- Step 6 `who_imports(target="rmc_server::mcp::project_paths::resolve_embedding_backend_for_mcp")`
  shows production importers in index, query, and graph similarity, plus
  compatibility/test imports.
- Step 6 `module_dependencies(module="rmc_server::tools::graph::similarity")`
  now lists `rmc_server::mcp::project_paths`; the local
  `resolve_graph_tool_backend` function is gone.
- Step 6 `module_dependencies(module="rmc_server::tools::endpoints::query")`
  now lists `rmc_server::mcp::project_paths`; the local
  `resolve_requested_backend` function is gone.
- Step 6 `semantic_overlaps(crate_name="rmc_server", item_kind="Function")`
  shows the backend resolver cluster reduced to
  `resolve_embedding_backend_for_mcp` and the index endpoint's
  `resolve_backend`, which remains for the legacy `model` parameter.
- Step 7 `build_hypergraph(directory, force_rebuild=false)` reused graph
  `2c6dfe88c8bad3b7db1838a94b00287b` with fingerprint
  `680958b42dd9eaa0c1d72a5958fc985c38673f053fd17072d09aeda0eaa58b6d`.
- Step 7 `module_dependencies(module="rmc_server::mcp::project_paths")`
  confirms server project paths now depend on
  `rmc_indexing::indexing::project_paths` for indexing path policy, while
  server-owned dependencies remain `ProjectDirs`, `EmbeddingBackend` /
  profile registry, and MCP error mapping.
- Step 7 `module_dependencies(module="rmc_server::tools::endpoints::query")`
  and
  `module_dependencies(module="rmc_server::tools::graph::similarity")` both
  route profile resolution through `rmc_server::mcp::project_paths`.
- Step 7 `semantic_overlaps(crate_name="rmc_server", item_kind="Function")`
  reports no `data_dir` cluster and the backend resolver cluster remains
  reduced to the shared MCP helper plus the index legacy-model wrapper.

### Responsibility Split

- Server-owned: MCP-facing data-root discovery, compatibility wrappers,
  endpoint parameter parsing, MCP error wording, and endpoint-specific fallback
  behavior.
- Engine-owned: `EmbeddingBackend`, `EmbeddingProfile`, profile parsing, and
  profile registry resolution.
- Indexing-owned: active chunking identity, indexing identity, identity hash,
  snapshot path derivation, cache/Tantivy/vector artifact path bundle, vector
  collection naming, and indexed-profile discovery from vector metadata.
- Compatibility path: keep `rmc_server::mcp::project_paths::ProjectPaths` as a
  wrapper during Phase 4 while moving the indexing-owned path/identity bundle
  into `rmc_indexing`.
- Step 4 source reads confirmed the compatibility wrapper shape:
  `ProjectPaths` and `IndexedProfilePaths` preserve the server-facing fields,
  convert from indexing-owned DTOs with `From` impls, and delegate path
  construction/discovery to `IndexingProjectPaths`.
- Step 4 source reads confirmed
  `crates/rmc-server/src/tools/project_paths.rs` remains a compatibility
  reexport of `crate::mcp::project_paths::*`.

### Files Changed

- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`
- `Cargo.lock`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-indexing/src/indexing/project_paths.rs`
- `crates/rmc-server/Cargo.toml`
- `crates/rmc-server/src/mcp/project_paths.rs`
- `crates/rmc-server/src/tools/endpoints/index.rs`
- `crates/rmc-server/src/tools/endpoints/indexing_support.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rmc-server/src/tools/graph/similarity.rs`
- `.docs/phase-4-boundrie-fix-report.md`

### Verification

- Documentation-only responsibility split; no build command required for
  Step 2.
- Step 4 compatibility-wrapper verification was MCP/source-read only; no build
  command required because no code changed.
- Step 7 verification was MCP/source-read only; no build command required
  because no code changed.
- Step 8 focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`.
- Step 5 focused server check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Step 6 focused resolver tests passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server resolve_backend`.
  Result: three resolver tests passed.
- Step 3 focused check passed before commit with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
- Step 3 regular focused test passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server project_paths`.
  Result: two project-path tests passed.

### Commits

- Step 1 documentation: `e14043cb` (`docs: record phase 4 step 1`).
- Responsibility split docs: `838381d3` (`docs: record phase 4 responsibility split`).
- Indexing project paths: `8755d084` (`refactor: move indexing project paths`).
- Project path move docs: `7a54b668` (`docs: record phase 4 project path move`).
- Compatibility wrapper docs: `31d872eb` (`docs: record phase 4 compatibility wrapper`).
- Data dir helper consolidation: `9c666fdd` (`refactor: consolidate server data dir helper`).
- Backend resolver consolidation: `bdc2d9f4` (`refactor: consolidate backend resolver helpers`).
- Helper consolidation verification: `d216b1ba` (`docs: verify phase 4 helper consolidation`).
- Check-result docs: `b8b107e8` (`docs: record phase 4 check result`).
- Ledger docs: `1d050d0c` (`docs: record phase 4 ledger`).

### Remaining Follow-Up

- Start Phase 5.

## Phase 5: `rmc-graph` Query And Response Facade

- Status: complete.
- Purpose: give server graph tools a narrower graph-owned query/DTO API so
  server response code depends less on raw graph model/storage/snapshot
  internals.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `1244e9892186d5c681827698217f9393db4642aa`, change
  `vkxwsvmtrwvvuzvrsuuqznxrlwoyrurx`.
- Step 2 response-helper inventory: completed after pre-step summary at
  working-copy commit `46cf2ba5e96637b7f6f24525b6adbb8079db2d16`, change
  `psuzmtoxpqzwynpxqtrosrnozstxmqpx`.
- Step 3 graph-owned enrichment facade: completed after pre-step summary at
  working-copy commit `7f998139160dc1b189254ff967624d9de7fc7784`, change
  `nxmnrtrpuvqmnowsxqzywykvwqvuzyno`.
- Step 4 DTO shape-stability check: completed after pre-step summary at
  working-copy commit `a1fefcb699275b61cf7645b3b00e205b112da2c9`, change
  `pzwkkmyoltqmtxossowuttulpnpzqqzs`.
- Step 5 server call-site migration: completed for the repeated enrichment
  path after pre-step summary at working-copy commit
  `4921f7af669a97f6121d01dc59f2f65c3a5e5657`, change
  `mpturlnmpmxxypolrmpqkmuvyoorttso`.
- Step 6 MCP verification: completed after pre-step summary at working-copy
  commit `de6bccdac20a01f7ad783bfbd2aebc13a465e680`, change
  `rsuvtstwwrvpurzwpsxzwysqvxtzuxnu`.
- Step 7 graph export compatibility: completed after pre-step summary at
  working-copy commit `7e351bd2886522f405b4e8dae5c7a03398372960`, change
  `ywoymvywpomsortlzsrsysswkolussyz`.
- Step 8 focused checks: completed after pre-step summary at working-copy
  commit `80ca87896b7e6251766396439f3e4f47d9c93d95`, change
  `ytzttuwsmnotnkospomqzxuyvnnrvznw`.
- Step 9 ledger update: completed after pre-step summary at working-copy
  commit `70a9dd8ae962004c496c0c1d1f725b519bf11a26`, change
  `lswunxmsqyoykmzuyryssvvqrtupsqrt`.

### MCP Evidence

- `build_hypergraph(force_rebuild=false)` reused graph
  `2c6dfe88c8bad3b7db1838a94b00287b`, fingerprint
  `680958b42dd9eaa0c1d72a5958fc985c38673f053fd17072d09aeda0eaa58b6d`.
- `get_imports` and `module_dependencies` show server graph response/core/surface
  still importing raw graph internals:
  - `response`: `rmc_graph::graph::{ids, labels, model, query::model,
    snapshot, storage}`
  - `core`: `rmc_graph::graph::{labels, model, query::model, snapshot}`
  - `surface`: `rmc_graph::graph::{derive_audit, docs_audit, ids, labels,
    model, query::model, snapshot}`
- `functions_with_filter(has_param_type="OpenedSnapshot")` reported seven
  server graph helpers with direct snapshot parameters:
  `core::enrich_bindings`, `core::enrich_usages`,
  `response::resolve_chunk_to_item`, `response::resolve_required_node`,
  `response::visibility_label`, `surface::enrich_crate_dead_pub`, and
  `surface::enrich_dead_pub`.
- After Step 5, `build_hypergraph(force_rebuild=false)` built graph
  `085eaff90b1189f8e7a4dc3374610742`, fingerprint
  `349e4a62bdb66681623fdc7432c538e80f98e667ffd92cac4a9400383a022759`.
- After Step 5, `functions_with_filter(has_param_type="OpenedSnapshot")`
  reported two remaining server helpers: `response::resolve_chunk_to_item`
  and `response::resolve_required_node`.
- After Step 5, `module_dependencies` shows:
  - `core` no longer depends on `rmc_graph::graph::labels`.
  - `surface` has `rmc_graph::graph::snapshot` import count `0`; remaining
    snapshot usage is through the opened snapshot value and non-enrichment
    endpoint calls.
  - `response` still depends on `snapshot` and `storage` because
    `open_workspace_snapshot` remains server-owned in this phase.
- `get_exports(module="rmc_graph::graph", consumer="rmc_server",
  summary=true, limit=120)` reported 68 visible exports. Existing
  compatibility exports remain visible, including `snapshot`, `storage`,
  `model`, `ids`, `OpenedSnapshot`, `GraphPaths`, `GraphEnvOptions`, `Node`,
  `NodeKind`, `Binding`, `Usage`, `DeadPubFinding`, and `CrateDeadPub`.
  New graph-owned enrichment DTO exports are also visible:
  `EnrichedBinding`, `EnrichedUsage`, `EnrichedDeadPub`, and
  `EnrichedCrateDeadPub`.

### Source-Read Result

- `response::open_workspace_snapshot` should stay server-owned in this phase
  because it converts directory/storage failures into MCP tool errors.
- `response::resolve_required_node` and `response::visibility_label` are graph
  lookup/label translations and are candidates for graph-owned helpers, but
  the lower-risk first move is the repeated enrichment path.
- `core::enrich_bindings`, `core::enrich_usages`,
  `surface::enrich_dead_pub`, and `surface::enrich_crate_dead_pub` only enrich
  graph findings for response DTOs. Step 3 should move or mirror these behind
  graph-owned query/DTO helpers while keeping MCP wrapping in the server.
- `response::resolve_chunk_to_item` also translates graph internals but has no
  production caller, so it is not the first facade target.

### DTO Shape Check

- Graph-owned `EnrichedBinding` preserves the existing JSON fields:
  `visible_name`, `namespace`, `kind`, `visibility`, `from_module`, `target`,
  and `target_kind`.
- Graph-owned `EnrichedUsage` preserves `file`, `start`, `end`, `category`,
  `consumer_module`, and `consumer_function`.
- Graph-owned `EnrichedDeadPub` preserves `qualified_name`, `item_kind`,
  `declared_visibility`, `file`, and `span`.
- Graph-owned `EnrichedCrateDeadPub` preserves the `crate` rename for `krate`
  and the nested `findings` array.
- Label fields are `String` in graph-owned DTOs instead of `&'static str` in
  the previous server-local DTOs; serialized MCP JSON remains the same.

### Files Changed

- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`
- `crates/rmc-graph/src/graph/query/enrichment.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-graph/src/graph/query/mod.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-server/src/tools/graph/core.rs`
- `crates/rmc-server/src/tools/graph/surface.rs`
- `crates/rmc-server/src/tools/graph/response.rs`
- `.docs/phase-5-boundrie-fix-report.md`

### Verification

- Step 1 was VCS-only; no build command required.
- Step 2 was evidence/docs-only; no build command required.
- Step 3 graph-only check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Step 4 was source/serde-shape verification only; no build command required.
- Step 5 server check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Step 6 used MCP verification only; no build command required.
- Step 7 used MCP export verification only; no build command required.
- Step 8 combined focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server`.

### Commits

- Step 1 documentation: `ecd3f445` (`docs: record phase 5 step 1`).
- Step 2 documentation: `5c12e38e`
  (`docs: record phase 5 response boundary evidence`).
- Step 3 implementation: `558106bc` (`refactor: add graph enrichment facade`).
- Step 4 documentation: `03e73ec4` (`docs: record phase 5 dto shape check`).
- Step 5 implementation: `d35b211d`
  (`refactor: use graph enrichment facade in server`).
- Step 6 documentation: `0420a460`
  (`docs: verify phase 5 snapshot boundary`).
- Step 7 documentation: `51ea5085` (`docs: verify phase 5 graph exports`).
- Step 8 documentation: `af625dd1` (`docs: record phase 5 check result`).
- Step 9 ledger update: `a9a303a0` (`docs: record phase 5 ledger`).
- Phase 5 report: `72fb3231` (`docs: add phase 5 boundaries report`).

### Remaining Follow-Up

- None.

## Phase 6: `rmc-graph` Audit Facade

- Status: complete.
- Purpose: move graph audit orchestration behind graph-owned entry points so
  server audit tools only parse MCP params and wrap graph-owned results.

### Step Evidence

- Step 1 `jj show --summary`: completed at working-copy commit
  `aba7ca27e917ab5b3dd8633befc7f65e6a1b3584`, change
  `ulzuvpoonzuyywyvqlrxrlrwsuvuwsuw`.
- Step 2 graph-owned audit entry points: completed after pre-step summary at
  working-copy commit `c4258945e608ce3aea72681b42390440dccb7aeb`, change
  `rzwtmmytrvlpqpmoyznrtqolszssmsxv`.
- Step 3 server audit migration: completed after pre-step summary at
  working-copy commit `87635fe52b0bd23abe2fdfe0fca66bc73faf9888`, change
  `ulwzstolouqvzutxpyurqulxlrltxzxy`.
- Step 4 server responsibility split: completed after pre-step summary at
  working-copy commit `54f0a84a61c9f2c8ad24c9bfab56568a61b435c2`, change
  `wzorvywrvosrvoptsrylxvtwylxtrutx`.
- Step 5 MCP dependency verification: completed after pre-step summary at
  working-copy commit `a0754394bde35ae2c361e2740f99f87eedc72902`, change
  `ysmstvvnqltlrnsutlvzotpvpuruprvq`.
- Step 6 focused checks: completed after pre-step summary at working-copy
  commit `d2632d8bb7318e88322e234a0d6dededcb8eae53`, change
  `qszylstrynwtssmtkvukptpnmttornwp`.
- Step 7 ledger update: completed after pre-step summary at working-copy
  commit `0285ccff1860dd0910983e9caa69aee9e75b8b58`, change
  `pqkwwxvqumuxlvovltqznxzlrmkpuomz`.

### MCP Evidence

- `build_hypergraph(force_rebuild=false)` reused graph
  `085eaff90b1189f8e7a4dc3374610742`, fingerprint
  `349e4a62bdb66681623fdc7432c538e80f98e667ffd92cac4a9400383a022759`.
- `who_imports(target="rmc_graph::graph::loader::load")` returned direct
  import bindings in a debug binary, the graph facade reexport, and graph
  loader tests. This missed server inline fully qualified usage, so
  `module_dependencies` is the authoritative boundary check for this phase.
- `module_dependencies(module="rmc_server::tools::graph::audits")` shows direct
  server audit dependencies on:
  - `rmc_graph::graph::loader::load`
  - `rmc_graph::graph::channel_audit::{ChannelAuditOpts, ChannelFinding,
    channel_capacity_audit}`
  - `rmc_graph::graph::fn_body_audit::{FnBodyAuditOpts, FnBodyFinding,
    fn_body_audit, parse_pattern_filter}`
  - `rmc_graph::graph::recursion_check::{RecursionOpts,
    clamp_cycle_length, enclosing_fn_qualified_names, recursion_check}`
  - `OpenedSnapshot::{lookup_by_qualified_name, mut_static_audit,
    unsafe_audit}`
- `get_exports(module="rmc_graph::graph", consumer="rmc_server")` still shows
  compatibility exports available before the audit facade migration.

### Source-Read Result

- Server `graph::audits` owns MCP response wrapping, pagination, summary
  location stripping, and `spawn_blocking`.
- The graph-owned facade added in Step 2 now owns canonicalizing the directory,
  opening the persisted snapshot, resolving optional crate filters, loading RA
  workspace data for AST-backed audits, dispatching audit internals, and
  rendering graph IDs to external DTO strings.
- Newly exported graph facade functions:
  `run_unsafe_audit`, `run_mut_static_audit`, `run_recursion_check`,
  `run_channel_capacity_audit`, and `run_fn_body_audit`.
- Newly exported option/result DTOs include
  `RecursionCheckOptions`, `ChannelCapacityAuditOptions`,
  `FnBodyAuditOptions`, `UnsafeAuditFinding`, `MutStaticAuditFinding`,
  `RecursionCheckOutput`, `RecursionCycle`, `ChannelCapacityFinding`,
  `FnBodyAuditFinding`, and `FnBodyAuditOutput`.
- After Step 3, source search in `rmc_server::tools::graph::audits` found no
  remaining direct references to graph `loader`, individual audit modules,
  `NodeId`, `NodeKind`, snapshot lookup, or `to_hex`. The server audit module
  retains MCP response envelopes, pagination, summary location stripping,
  parameter defaults, error mapping, and `spawn_blocking` orchestration.
- After Step 3, `build_hypergraph(force_rebuild=false)` built graph
  `350719e344857be9514c69be176c11a7`, fingerprint
  `59335f0aaf01780beb5032be2ff2022bbe20c2903f067ec4c6c8cd60e802adaf`.
- After Step 3, `module_dependencies(module="rmc_server::tools::graph::audits")`
  reports server dependencies on graph audit facade functions/options and
  graph audit DTOs only:
  - `rmc_graph::graph::query::audits`
  - `rmc_graph::graph::query::model`
- The same MCP dependency result no longer reports production server
  dependencies on `loader`, `channel_audit`, `fn_body_audit`,
  `recursion_check`, `unsafe_audit`, or snapshot audit methods.
- `get_exports(module="rmc_graph::graph", consumer="rmc_server")` reports 83
  visible exports, including the new audit facade functions/options and DTOs.

### Files Changed

- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`
- `crates/rmc-graph/src/graph/query/audits.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-server/src/tools/graph/audits.rs`
- `.docs/phase-6-boundrie-fix-report.md`

### Verification

- Step 1 was VCS-only; no build command required.
- Step 2 graph-only check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Step 3 server check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Step 6 combined focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server`.

### Commits

- Step 1 documentation: `f6989e95` (`docs: start phase 6 audit facade`).
- Step 2 implementation: `c045a04f` (`refactor: add graph audit facade`).
- Step 3 implementation: `dcc6665e`
  (`refactor: use graph audit facade in server`).
- Step 4 documentation: `e37adafd`
  (`docs: verify phase 6 server audit split`).
- Step 5 documentation: `1c6d886b`
  (`docs: verify phase 6 audit dependencies`).
- Step 6 documentation: `550a943e` (`docs: record phase 6 check result`).
- Step 7 ledger update: `7b74638e` (`docs: record phase 6 ledger`).
- Phase 6 report: `3100af84` (`docs: add phase 6 boundaries report`).

### Post-Phase 6 Remediation

- User review found a Phase 5 server test compile regression: the graph
  endpoint test still constructed `EnrichedUsage` through the old local import
  surface after the DTO moved to `rmc_graph`.
- Pre-step `jj show --summary` reported working-copy commit
  `e281eb189679deb5589ba1caabfc0f1cd6edfdde`, change
  `uyrmqyvukmwsqsqsyoknllwkvqkylvvx`.
- Updated `crates/rmc-server/src/tools/graph/tests.rs` to import
  `EnrichedUsage` from `rmc_graph::graph` and construct its `category` as a
  `String`.
- Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server
  --no-run`.

## Phase 7: `rmc-graph` Similarity Facade

### Progress

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `039b45e753bd7fb5203b19681768cd5997ad2aa6` on change
  `snlqzpzouynzrmunmsuomvuupqoovtvq`, with no description set.
- Step 2 add graph-owned semantic-overlap operation: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `6f802d4afdc3ed0b87731cef56667bad87ef4038` on change
  `qspkvyrummotnxnwqokkmuqsxrlqrzmy`, with no description set. Added
  `crates/rmc-graph/src/graph/query/similarity.rs`, graph-owned
  `SemanticOverlapOptions`, `GraphSimilarityError`, `run_semantic_overlaps`,
  and public similarity output DTOs. The facade owns graph item enumeration,
  embedding-cache refresh, cosine scoring, pair output, and cluster output.
  Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Step 3 keep embedding cache and cosine implementation details inside graph:
  completed for the new facade. Pre-step `jj show --summary` reported
  working-copy commit `1fb5b0e52f108d220ef5d47affbd41f4f9a458e1` on change
  `wnusvywuxmpqkmxourpqzutosmryxvoq`, with no description set.
  `rg` evidence showed the new graph facade calls
  `embedding_cache::ensure_embeddings_for` and `math::cosine` through private
  graph module paths. The existing public compatibility reexports are left in
  place until Step 4 migrates server `semantic_overlaps`; the remaining server
  low-level calls are in `crates/rmc-server/src/tools/graph/similarity.rs`.
- Step 4 migrate the server `semantic_overlaps` tool to the facade:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `d6e25d0d55f8190fce2e9e6c05eada5207aac4e3` on change
  `ozymokktwuqnqpzwqrnopwrprqznopqy`, with no description set. Updated
  `crates/rmc-server/src/tools/graph/similarity.rs` so
  `semantic_overlaps` calls graph-owned `run_semantic_overlaps` and keeps
  only MCP parameter adaptation/error mapping/JSON serialization. Removed
  server-local similarity DTO and cluster helpers from
  `crates/rmc-server/src/tools/graph/response.rs`, moved their pure tests to
  `crates/rmc-graph/src/graph/query/similarity.rs`, removed the public graph
  reexports of `ensure_embeddings_for` and `cosine`, and updated graph
  codemap internals to use private graph module paths. Source search found no
  remaining server production calls to graph `ensure_embeddings_for` or
  `cosine`. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server` and `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph similarity_`.
- Step 5 keep `similar_to_item` server-owned: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `0de06380951010fdc893e2954a649b6246c661d7` on change
  `msuwkqmlltplmzuznwwuorqxpzwzmlyr`, with no description set. Source search
  found `similar_to_item` only in server routing/params/implementation code,
  not as a graph facade. The tool still depends on server project path
  resolution, server hybrid-search construction, and vector-only search, so
  it intentionally stays server-owned for Phase 7.
- Step 6 verify server production modules no longer reach graph
  `embedding_cache` or `math` for semantic-overlap behavior: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `7e840eff61b539599656d74b6f9f659983a5ebb8` on change
  `vovntxlqqmtptkmqntkvvowoxsqyyqzw`, with no description set.
  `build_hypergraph(force_rebuild=false)` rebuilt graph
  `56dbddbd49bf25977fef1d75a269d455`, fingerprint
  `53b0c34cc7a90b62bade00ab81ce4ae4baf13a37429fee9d4dd4c740b5364aae`.
  `module_dependencies(rmc_server::tools::graph::similarity)` reported graph
  dependencies on `rmc_graph::graph::query::similarity` facade exports and no
  server dependency on graph `embedding_cache` or `math`. `who_imports`
  confirmed `embedding_cache::ensure_embeddings_for` and `math::cosine` are
  imported only inside graph query/test modules. MCP `semantic_overlaps`
  evidence for `rmc_graph` functions returned 178 seeds, 18 pairs, and 15
  clusters.
- Step 7 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `9a27b5751bc24252d875d8e87c761c0b7f097c5a` on change
  `ykpxzxowosoplyyukxtpwrrpqmquwwzu`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server` and `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph similarity_` (6 tests passed).
- Step 8 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `72cbf9b80d9d36ae0582bf93ebed808260226dda` on change
  `npyxysnzuxnrkzyswolsvsrttvqzltkq`, with no description set. Phase 7
  implementation work is complete; the separate phase report remains to be
  written and committed.

### MCP Evidence

- `build_hypergraph(force_rebuild=false)` rebuilt graph
  `56dbddbd49bf25977fef1d75a269d455`, fingerprint
  `53b0c34cc7a90b62bade00ab81ce4ae4baf13a37429fee9d4dd4c740b5364aae`.
- `module_dependencies(module="rmc_server::tools::graph::similarity")`
  reports graph dependencies on `rmc_graph::graph::query::similarity`
  facade exports for semantic overlaps. It does not report server
  dependencies on graph `embedding_cache` or `math`.
- `who_imports(target="rmc_graph::graph::embedding_cache::ensure_embeddings_for")`
  reported only graph query/test imports.
- `who_imports(target="rmc_graph::graph::math::cosine")` reported only graph
  math/query/test imports.
- `semantic_overlaps(crate_name="rmc_graph", item_kind="Function",
  summary=true, max_pairs=40)` returned 178 seeds, 18 total pairs, and 15
  total clusters.

### Source-Read Result

- Graph now owns workspace-wide semantic overlap mechanics through
  `rmc_graph::graph::run_semantic_overlaps`.
- Server `semantic_overlaps` resolves the embedding backend for MCP, builds
  `SemanticOverlapOptions`, delegates to graph, maps typed graph similarity
  errors, and serializes graph DTOs.
- `similar_to_item` remains server-owned because it still depends on server
  project path resolution, server hybrid-search construction, and vector-only
  search.
- Public graph reexports of `ensure_embeddings_for` and `cosine` were removed;
  graph-internal codemap callers now use private graph module paths.

### Files Changed

- `.plans/boundries-plan.md`
- `.docs/boundries-cleanup-progress.md`
- `crates/rmc-graph/src/graph/codemap/build.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/query/mod.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-graph/src/graph/query/similarity.rs`
- `crates/rmc-server/src/tools/graph/response.rs`
- `crates/rmc-server/src/tools/graph/similarity.rs`
- `crates/rmc-server/src/tools/graph/tests.rs`

### Verification

- Graph-only check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Combined focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`.
- Graph similarity tests passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  similarity_` (6 tests passed).

### Commits

- Step 1 documentation: `7a3b26a8`
  (`docs: start phase 7 similarity facade`).
- Step 2 implementation: `94f20c92`
  (`refactor: add graph similarity facade`).
- Step 3 documentation: `2091f947`
  (`docs: verify phase 7 graph similarity internals`).
- Step 4 implementation: `e3ba55e4`
  (`refactor: use graph similarity facade in server`).
- Step 5 documentation: `d4d74fd2`
  (`docs: keep similar item search server owned`).
- Step 6 documentation: `1c9f904e`
  (`docs: verify phase 7 similarity dependencies`).
- Step 7 documentation: `e97f982b`
  (`docs: record phase 7 check result`).

### Phase Completion

- Phase 7 report: completed after pre-report `jj show --summary` reported
  working-copy commit `32d3b1dd585f2eb4fa63471d5b56893d884f98de` on change
  `yqznxlswloqrkoptsswsskoztlxmoooo`, with no description set. Wrote
  `.docs/phase-7-boundrie-fix-report.md` and marked the Phase 7 progress
  ledger complete.

## Phase 8: `rmc-graph` Storage Cleanup Facade

### Progress

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `b1c1d1efc726c59be81c2bab2173c5cc9901db53` on change
  `orrluuuuxommuvvnvqkuspowlszrkpoo`, with no description set.
- Step 2 add graph-owned cache/snapshot cleanup API: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `a142565cdcf8f1465c568be4234eff49b5e6fe2c` on change
  `omtlysvxxlpvylyzmkrtntnulupxynym`, with no description set. Added
  `clear_workspace_snapshots`, `clear_all_workspace_snapshots`,
  `GraphSnapshotCleanupOptions`, `GraphSnapshotCleanupEntry`, and
  `GraphSnapshotCleanupReport` in `rmc_graph::graph`. The graph API owns
  workspace graph path calculation, all-workspace graph root resolution,
  dry-run reporting, and removal/error collection. Verification passed with
  existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-graph` and `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph clear_` (3 tests passed).
- Step 3 migrate server cache endpoint to call the graph API: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `98082c4c3d3d4cd0988387bcd31359ef4c51ff00` on change
  `txpklmnkupptwsnopypttlxnwktzzzqy`, with no description set. Updated
  `crates/rmc-server/src/tools/endpoints/cache.rs` so hypergraph cleanup goes
  through graph-owned `clear_workspace_snapshots` and
  `clear_all_workspace_snapshots`, with the server only formatting graph
  cleanup reports into the existing MCP text response. Source search found no
  remaining direct server references to `rmc_graph::graph::GraphPaths` or
  `rmc_graph::graph::storage`. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server` and `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-server cache` (7 tests passed).
- Step 4 storage dependency verification: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `163531c3b4b477154d3f85a6f1b867785003e94d` on change
  `yypzuvnosopmvpvmwnkskzonvqntrplt`, with no description set. Moved the
  public cleanup facade to `rmc_graph::graph::snapshot` and added
  `open_current_for_workspace` so server graph response code can open the
  current workspace snapshot without constructing `GraphPaths`. Refreshed MCP
  hypergraph evidence with graph `6a0f0a501756b0c9b36c694e073a60fc` and
  fingerprint
  `d291e5830be17d570abd3d5892e8c467a858c35d3bfcce3f5617e62be37f118d`.
  `module_dependencies(module="rmc_server::tools::endpoints::cache")` shows
  graph dependencies only on snapshot cleanup symbols:
  `GraphSnapshotCleanupOptions`, `GraphSnapshotCleanupReport`,
  `clear_workspace_snapshots`, and `clear_all_workspace_snapshots`.
  `get_imports` for the cache endpoint imports only the two snapshot cleanup
  DTOs from graph. `who_imports(target="rmc_graph::graph::GraphPaths")`
  returned 16 bindings, all in graph modules/tests, debug binaries, the
  compatibility reexport, or `probe_workspace`; no server module imports
  `GraphPaths`. `functions_with_filter(krate="rmc_graph",
  has_param_type="GraphPaths")` returned only graph snapshot functions:
  `open_current`, `open_specific`, and `publish_current`. Supporting focused
  checks passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`, `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph clear_` (3 tests passed), and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server cache` (7
  tests passed).
- Step 5 focused nix checks: completed. Pre-step `jj show --summary`
  reported working-copy commit
  `1ca43a0b76b242c5e4804561f0e9fefe6fc17772` on change
  `mwvzzoypltvwrsuvusomvyvplyouxzpl`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`, `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph clear_` (3 tests passed), and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server cache` (7
  tests passed).
- Step 6 ledger update: completed. Pre-step `jj show --summary` reported
  working-copy commit `7c408edae0a83fd3b20a5b30bd00004fdb3287a9` on change
  `mxqwsnznktqlvmsnomxrrupkxonkvmvz`, with no description set. Phase 8 code
  and verification steps were complete; the separate Phase 8 report was still
  pending at this step.

### Commits

- Step 1 documentation: `8306e692` (`docs: start phase 8 storage cleanup facade`).
- Storage cleanup facade: `253b76c1` (`refactor: add graph storage cleanup facade`).
- Server cache migration: `ea86c85d` (`refactor: use graph storage cleanup facade in cache`).
- Graph path opening facade: `28d27cd2` (`refactor: hide graph paths behind snapshot facade`).
- Check-result docs: `36e10267` (`docs: record phase 8 check result`).
- Ledger docs: `aa0815de` (`docs: record phase 8 ledger`).

### Phase Completion

- Phase 8 report: completed after pre-report `jj show --summary` reported
  working-copy commit `fa9e7c158816499cbb23a3aa5578d840a4463b60` on change
  `tymoomxzuunzzpxyyuvrtunyzppoutsy`, with no description set. Wrote
  `.docs/phase-8-boundrie-fix-report.md` and marked the Phase 8 progress
  ledger complete.

## Phase 9: `rmc-server` Internal Boundary Cleanup

### Progress

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `418aceba11919b3c1a46448f3bc5885e1044b4ea` on change
  `mrozmyslwyvyymmnwlmwmqnylrooxqtu`, with no description set.
- Step 2 keep `tools::router` thin: completed as verification-only. Pre-step
  `jj show --summary` reported working-copy commit
  `d0bd6a43da799a6069e09c4f775f642225020e06` on change
  `lwwmnsznmwowomykvtwmuvsrwwkwmuyq`, with no description set. Source review
  confirmed `crates/rmc-server/src/tools/router.rs` declares MCP tool methods
  and delegates to `tools::endpoints::*` or `tools::graph::*`; it does not own
  lower-layer graph/indexing business logic. The larger `build_codemap`
  method only destructures parameters before delegating to
  `tools::graph::codemap::handle_build_codemap`.
- Step 3 remove server-side helper duplication: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `5f834598d6cdc20a22e216c90c65c7568e867f3f` on change
  `powrovouytsxlwomwklzwuvrplmuvmov`, with no description set. Removed the
  unused `tools::endpoints::indexing_support` module, which only wrapped
  low-level Tantivy index/cache opening after the indexing facades existed,
  and removed stale router/module documentation references to it. Source
  search found no remaining `indexing_support` references. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server`.
- Step 4 keep `tools::params` internal: completed as verification-only.
  Pre-step `jj show --summary` reported working-copy commit
  `f4da094f0a1f7c729a74b93373fb3d2abb1d1615` on change
  `rnvkvtnqvwkyutvmlllprxzvmnxmtrzl`, with no description set. Source review
  confirmed `tools::params` is a private module, its family modules are
  private, its flat reexports are `pub(crate)`, and all MCP parameter structs
  in `tools::params` remain `pub(crate)` rather than public API.
- Step 5 treat `semantic` privacy as late cleanup: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `36e3e7cd2696bbaded99e62b9cc9fcb11bd7b340` on change
  `punsvpsnkrslukpusrworzvopyzxyzll`, with no description set. Source search
  found `crates/rmc-graph/src/graph/statics.rs` tests that assert the
  qualified name `rmc_server::semantic::SEMANTIC`, so Phase 9 leaves
  `semantic` public until those symbol-level expectations move. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  semantic` (1 test passed, 200 filtered out, finished in 108.38s).
- Step 6 migrate `tools::project_paths` compatibility caller: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `c71c3b4f61a2e1d3feeacc64a3db2c471222b1d4` on change
  `orykxsrltvovutxuxqsvnvmmyvsyltky`, with no description set. Updated the
  stdio regression test to import `ProjectPaths` from
  `rmc_server::mcp::project_paths`, removed the `tools::project_paths`
  compatibility module, and removed the `tools` module declaration for it.
  Source search found no remaining `rmc_server::tools::project_paths`,
  `crate::tools::project_paths`, or `tools::project_paths` callers.
  Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server -p rust-code-mcp` and `nix develop ../nix-devshells#cuda-code
  --command cargo test -p rust-code-mcp --test test_mcp_stdio_transport
  --no-run`.
- Step 7 verify server public exports: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `7ea2b94aa4faf206c22435ac3f2792c703ba7f0a` on change
  `kknqyswrtonszvszpttxolopwmmoystr`, with no description set. Refreshed the
  MCP hypergraph: graph `e669ac6eeba2bb252aa05150b435baa2`, fingerprint
  `cff3f2f33f298d34766a50b6578f4212466eadbfdf76f6399bf5b36567eddb29`.
  `get_exports(module="rmc_server::tools", consumer="rmc_server")` returned
  the intended tools facade: `SearchToolRouter`, `SearchTool`,
  `index_codebase`, and `IndexCodebaseParams`. `get_exports` for
  `rmc_server::mcp` returned `SyncManager` plus the public `sync` and
  `project_paths` modules. Root `rmc_server` exports remain `tools`, `mcp`,
  and `semantic`. `get_declared_reexports` for `rmc_server::tools` returned
  only the four intended tools reexports, and `rmc_server::mcp` returned the
  `SyncManager` glob reexport.
- Step 8 focused nix checks: completed. Pre-step `jj show --summary`
  reported working-copy commit
  `656d72cdc727c084cd25e212ea8599beb4b0324c` on change
  `xvwksozxvkqultozmvrtwmsozlrmrmpx`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server -p rust-code-mcp`, `nix develop ../nix-devshells#cuda-code
  --command cargo test -p rmc-server --no-run`, and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test
  test_mcp_stdio_transport --no-run`.
- Step 9 ledger update: completed. Pre-step `jj show --summary` reported
  working-copy commit `1af4c10e99da408c7ac5071ee5aa8b58b53a4fc7` on change
  `zpqmmtnpxssynwlokxqoplwvwwxzsuqr`, with no description set. Phase 9 code,
  export verification, and focused checks were complete; the separate Phase 9
  report was still pending at this step.

### Commits

- Step 1 documentation: `d3c0c78f` (`docs: start phase 9 server cleanup`).
- Router boundary docs: `a59551c9` (`docs: record phase 9 router boundary`).
- Helper cleanup: `27faf679` (`refactor: remove unused server indexing helpers`).
- Params boundary docs: `0c84f62c` (`docs: record phase 9 params boundary`).
- Semantic visibility docs: `9a6b22db` (`docs: record semantic visibility decision`).
- Project paths compatibility cleanup: `fccbc47a` (`refactor: remove project paths compatibility reexport`).
- Export verification docs: `28e8e683` (`docs: verify phase 9 server exports`).
- Check-result docs: `200aaa7d` (`docs: record phase 9 check result`).
- Ledger docs: `2febe4d1` (`docs: record phase 9 ledger`).

### Phase Completion

- Phase 9 report: completed after pre-report `jj show --summary` reported
  working-copy commit `03d824b9b855fb22d08bcc52ff5f32818257eb45` on change
  `xsqrqployrzwyptkmoqurswywpmxlukn`, with no description set. Wrote
  `.docs/phase-9-boundrie-fix-report.md` and marked the Phase 9 progress
  ledger complete.

## Phase 10: `rmc-engine` Public Surface Tightening

### Progress

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `93da984cc37b76983fbf24e64a16df3136ddd97b` on change
  `rpymxmznvyyowomornmtylzpsuuqonsy`, with no description set.
- Step 2 confirm active engine implementation-module consumers: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `e6de81386071f5530aeeea7ea33e7e14452ff4ee` on change
  `xvpssnupvstnqqynwwspkxsszrzzumrm`, with no description set. MCP
  `who_imports` found only engine test-module glob imports for
  `search::bm25`, `search::resilient`, `search::rrf_tuner`,
  `vector_store::lancedb`, and `vector_store::traits`; no cross-crate import
  edges depend on those implementation modules. Source search found inline
  production references to `rmc_engine::search::bm25::Bm25Search` in
  indexing/server code, which can use the existing
  `rmc_engine::search::Bm25Search` facade reexport. Parser helper-module
  search found no external importers for `imports` or `call_graph`, and
  `type_references` is used inside `rmc_engine::parser` itself. Export
  checks confirmed `rmc_engine::search` and `rmc_engine::vector_store` still
  expose both implementation modules and facade reexports, while
  `rmc_engine::embeddings` exposes `EmbeddingBackend`, `EmbeddingProfile`,
  `EmbeddingGenerator`, and related embedding boundary types.
- Step 3 migrate production consumers to facade reexports: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `61f941d7e1f85ff3994f81e05ad0ab3473ed01ac` on change
  `pssllkpopytptyqvltpyzkpwtvxnxypu`, with no description set. Updated
  indexing and server production code to use `rmc_engine::search::Bm25Search`
  instead of inline `rmc_engine::search::bm25::Bm25Search` paths. Source
  search found no remaining production inline `search::bm25::Bm25Search`
  references. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-indexing -p rmc-server`.
- Step 4 tighten implementation-module visibility: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `7f68455c8ce3445f05842d283d6c380de6b7ac8f` on change
  `yvurtoqvqqvokrpylxpqrmyoxmzwmuky`, with no description set. Made the
  `rmc_engine::search`, `rmc_engine::vector_store`, and `rmc_engine::parser`
  implementation modules private while preserving their existing facade
  reexports. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-engine -p rmc-indexing -p rmc-server -p rust-code-mcp`.
