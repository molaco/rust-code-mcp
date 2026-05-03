
● Going by intent. For each workflow: which tools to use, what they answer, how the old (parser/semantic/file/RA) tools combine with the hypergraph.

  0. Entry-point queries — when you don't know the qualified name yet

  Before you can call hypergraph tools (which take qualified names), you often need to find the symbol first.

  - Free-text search. search(query) → keyword/BM25 across files. Catches comments, docstrings, log messages, anything in source. Useful when you remember a string but not a symbol.
  - Goto-definition by name. find_definition(name) → file:line for symbols matching the name. RA-driven, no qualified path required.
  - Semantic similarity. get_similar_code(query) → vector-store search by description ("function that parses JSON"). Returns candidate sites you can drill into.
  - Browse hierarchy. module_tree(crate) → discover symbols by walking the tree. Often the most direct route once you know the crate.
  - Read raw source. read_file_content(path) → just the file contents, when you have a path but no symbol yet.

  Common bridge pattern: search("HybridSearch") → find_definition to confirm location → derive qualified name from path/structure → who_uses("file_search_mcp::search::HybridSearch") for the structural answer.

  Hypergraph vs RA tools: for aggregate / workspace-wide answers, hypergraph tools (module_tree, crate_edges, who_uses, who_uses_summary) are faster and more precise than find_definition / find_references. Use the RA-driven tools for IDE-like single-symbol browsing, not for "where in the workspace is X?" questions.

  1. Workspace-level overview ("what is this codebase?")

  - Crate inventory + counts. workspace_stats → nodes by kind (workspace/crate/module/item/external_symbol), items by kind (Struct/Enum/Fn/Method/etc.), bindings by kind, visibility breakdown, pub_crate_share ratio.
  - Encapsulation discipline as a comparative metric. workspace_stats.pub_crate_share is a between-codebase comparison signal. We measured 0.07 on one workspace and 0.58 on its successor — same crate count, very different team discipline. A high ratio means most non-private items are crate-scoped (good); a low ratio means lots of items leaked as bare pub.
  - Canonical list of all crates. dead_pub_report.crates[].crate is the most reliable enumeration of every workspace crate, including those with zero findings. Other tools (workspace_stats, crate_edges) require deriving names indirectly.
  - Method extraction signal. workspace_stats.items_by_kind.Method — Layer 4 produces this. Compare to Fn count for a sense of how much logic is method-bound vs free-functional.
  - Cross-crate edge matrix. crate_edges → every consumer→producer edge with unique symbols + total refs (split via_imports vs via_usages).
  - Hygiene snapshot. overlaps → cross-crate type collisions, module-name shadows of workspace crates, within-crate duplicates, fn names appearing in 4+ crates.
  - Dead-code surface. dead_pub_report → workspace-wide pub-but-unused, file:byte-span navigable.
  - Complexity hotspots. analyze_complexity(file) per crate → cyclomatic / cognitive metrics. Combine with crate_edges to find "complex AND widely depended on" — the top refactor priority.
  - System health. health_check → infrastructure status (does the index exist, is the snapshot fresh, etc.).

  2. Quick-start exploration recipe

  When dropped into an unfamiliar codebase, in order:

  1. build_hypergraph(directory) — cold or reuse.
  2. workspace_stats — "how big is this thing?"
  3. crate_edges sorted by total_refs_via_usages — "what's the architectural shape?"
  4. module_tree(crate=heaviest_load_bearing_crate, depth=2) — "what's inside the central crate?"
  5. dead_pub_report — "anything obviously rotting?"
  6. overlaps — "anything obviously broken?"
  7. analyze_complexity(file=hottest_file_per_crate_edges) — "where's the gnarl?"

  3. Crate-level audit ("dissect crate X")

  - Full structural dump. module_tree(crate=X) — recursive: modules → submodules → items → methods (Layer 4). Shows visibility for each declared item.
  - Depth advice. With Layer 4 nesting methods under their host types, full-depth trees can be huge (a 15-submodule crate produced 72KB at depth=3). Default to depth=2 for "what submodules and root-level items exist?", depth=3 for "expand items inside each submodule," and full-depth only when you need methods. The pub(in <crate>) visibility for crate-internal helpers shows up in module_tree — useful as an internal-API discipline signal.
  - Public surface. get_declared_reexports(module=X) — every pub use declared at the crate root regardless of who can reach it. An empty result is informative: it means X has no facade and exposes everything at canonical paths.
  - Effective surface from a viewpoint. get_exports(module=X, consumer=other_crate).
  - Re-export facade subset. get_reexports(module=X, consumer=Y) — pub use reachable from Y.
  - Dead pub items. dead_pub_in_crate(crate=X) — candidates for pub(crate) downgrade.
  - Outgoing dependencies. Filter crate_edges by consumer_crate=X.
  - Incoming dependencies. Filter crate_edges by producer_crate=X.
  - Per-file complexity. analyze_complexity on each src/*.rs in the crate.
  - Read declaration. read_file_content(path) — pair with Node.file + Node.span from module_tree to render a specific item.

  4. Module-level audit

  - What X imports. get_imports(module=X) — use/extern crate edges.
  - Per-file imports. get_dependencies(file=X.rs) — file-level (works even if you don't have a clean module path).
  - What X exports to consumer C. get_exports(module=X, consumer=C).
  - What X re-exports. get_reexports(module=X, consumer=C) (visibility-filtered) or get_declared_reexports(module=X) (all pub use).
  - Internal structure. module_tree(crate=parent_crate) walked into X.
  - Module's call graph. get_call_graph(file) for parser-level function-call relationships within the module's files.

  5. Symbol-level forensics ("dissect Item Y")

  For any Y (struct, enum, trait, fn, method, const, type alias, assoc const, assoc type):

  - Declaration site. find_definition(name=Y) → file:line, OR module_tree followed by Node.file + Node.span.
  - Render the declaration. read_file_content(file) at the span.
  - Who imports Y? who_imports(target=Y) — every use statement bringing Y into scope.
  - Who uses Y? who_uses(target=Y) — every non-import reference site (file:byte-range, category, consumer_module).
  - Aggregated rollup. who_uses_summary(target=Y) — grouped by consumer module with Test/Other breakdown.
  - RA-driven references (alternative). find_references(name=Y) — RA's goto-references. Different scope: catches things who_uses doesn't (local var refs, etc.) but not aggregated.
  - Test-only check. who_uses_summary with all rows 100% Test → fixture builder.
  - Production-only check. who_uses_summary Test==0 → critical-path symbol.
  - Cross-crate fan-in. Group who_imports results by consumer crate.
  - Method-level fan-in. who_uses(target=Type::method) — Layer 4 unlocks this; pre-Layer-4 it errored.
  - Read context around each hit. Iterate who_uses results, read_file_content at each (file, start, end) widened by ~10 lines for context.

  6. Trait-specific analysis (Layer 4 sweet spot)

  - Trait dispatch sites. who_uses(target=Trait::method) — every x.method() and Type::method() resolves back to the trait declaration.
  - Trait callers grouped. who_uses_summary(target=Trait::method).
  - Heaviest trait methods. Iterate trait's methods from module_tree, run who_uses_summary on each, sort by total_count.
  - Trait impl coverage. who_imports(target=Trait) — modules that use Trait, typically because they implement it or take it as a generic bound.
  - Trait deletion check. who_uses(Trait) empty across crates outside the defining one → safe to delete or seal.
  - Trait method removal check. who_uses(Trait::method) empty → safe to remove the method (but check trait impls aren't hardcoding it).

  7. Cross-crate dependency analysis

  - Full edge matrix. crate_edges.
  - Heaviest dependency. Sort by total_refs_via_usages desc.
  - Most-imported single symbol. Scan crate_edges symbol breakdowns for the highest combined import_count + usage_count.
  - What flows on edge A→B. Find the row in crate_edges for (A, B) — symbols lists each carrying symbol with import vs usage counts.
  - Shared vocabulary. Symbols appearing in many crate_edges rows (cross-multiple consumers) → candidates for stabilizing in a shared crate.
  - Tight coupling. Two crates with high unique_symbols + total_refs between them → consider merging or refactoring the boundary.
  - Dependency cycles. Walk crate_edges and check for cycles.
  - Compare semantically similar fns across edges. get_similar_code(target) per cross-crate symbol → find candidates worth deduplicating.

  8. Refactor planning workflows (multi-step)

  - "Should I downgrade X from pub to pub(crate)?" → who_imports(X) + who_uses(X). If both empty cross-crate, yes (and dead_pub_report likely already flagged it).
  - "Is it safe to delete X?" → who_uses(X) + who_imports(X) + find_references(X) (catches local var shadows, lifetimes RA tracks). Empty everywhere = delete.
  - "Should I move X to a different crate?" → who_uses_summary(X) shows where references concentrate. Move X where most callers live, OR factor X's deps upstream.
  - "Is this pub use facade earning its keep?" → get_declared_reexports(module); for each, who_imports(target) to compare canonical vs facade usage.
  - "Should I make this trait sealed?" → who_uses(Trait) + who_imports(Trait) from outside the defining crate.
  - "Do crate-private types leak through pub APIs?" → manually compare get_exports(crate) vs module_tree(crate) filtered to pub.
  - "What's the minimum viable refactor target?" → crate_edges between two crates; if unique_symbols is small (1-3), that's the refactor target.
  - "Test-only helpers I can move to dev-deps?" → who_uses_summary filtering rows where category_breakdown is all Test.
  - "Verify a refactor didn't widen the API." → snapshot get_declared_reexports + dead_pub_report before, refactor, snapshot after, diff JSON.
  - "Find duplicate logic worth extracting." → get_similar_code(fn_body) for each function; cluster semantically similar fns; for each cluster, run who_uses_summary to see if a shared helper would benefit them all.
  - "Which complex functions have the highest blast radius?" → analyze_complexity to find gnarly fns; who_uses(complex_fn) to see fan-in; prioritize high-complexity × high-fan-in.
  - "Find dead facade re-exports." (high-leverage recipe) → intersect get_declared_reexports(module=crate_root) with dead_pub_in_crate(crate). Items that appear in BOTH are dead facade branches: re-exported at the crate root but nothing imports either path. Drop the `pub use` line, demote source to `pub(crate)`. Spotted on tui in coding-agent-bad: RunState, InvalidTransition, RunnerWakeError were all re-exported AND dead.
  - "Detect half-finished migrations." (high-leverage recipe) → from overlaps.cross_crate_type_collisions, find collisions where the same consumer module uses BOTH versions. who_uses_summary on each side, then look for consumer_qualified_name overlap between the two row sets. If a consumer is in both, it's converting between the two types — usually a half-finished refactor where the type was duplicated rather than moved. Spotted on coding-agent-bad: AgentConfig in agent::config and config crates, both used by coding-agent::compose.

  9. Code quality / hygiene audits

  - Cross-crate type collisions. overlaps.cross_crate_type_collisions.
  - Module shadowing. overlaps.module_shadows — mod X matching a workspace crate name. Inside that crate, X::... resolves locally instead of to the workspace crate.
  - Module shadow diagnostic (real bug vs footgun). A shadow alone isn't a bug — it's only dangerous if the shadowing crate also depends on the workspace crate of the same name. To check: filter crate_edges for (consumer_crate=shadowing_crate, producer_crate=shadowed_crate). If the dep exists, it's a real bug — references inside the shadowing crate may resolve to the local module unexpectedly. If no dep, it's a footgun (anyone trying to add `use Y::...` later gets the local module silently). Either way, rename the local module to remove the trap.
  - Within-crate type duplicates. overlaps.within_crate_type_duplicates.
  - Test-fixture heuristic. Most within-crate duplicates are test fixtures replicated across test modules. Names like Mock*, Fake*, Stub*, Recording*, *EventSender located in modules ending in tests, test, fixtures, common are almost always test-fixture dupes. Mechanical refactor: factor into <crate>::tests::common::*.
  - Common fn names. overlaps.common_fn_names — fn names in 4+ crates. Empty is common (and good — no init/run proliferation). Hits to investigate: anything other than `main` (expected for binaries) or core idioms (`new`, `default`).
  - Dead pub items. dead_pub_report workspace-wide; dead_pub_in_crate per crate.
  - Vendored-library caveat. Vendored or library-style crates have inflated dead-pub counts because their pub surface is "designed for general use" but consumed narrowly in this workspace. We measured 47 dead pubs in plurimus (a vendored UI lib) on coding-agent-bad — that's expected, not a problem. Filter or de-prioritize known external/vendored crates before reading dead_pub_report.
  - Cyclomatic complexity hotspots. analyze_complexity(file) for each crate's main files; sort outputs.
  - Cognitive complexity vs blast radius. analyze_complexity × who_uses to find the fns most worth refactoring.

  10. Test vs production analysis (Test/Other category split)

  - Test-only constructor. who_uses_summary(Type::new) 100% Test → fixture builder.
  - Mostly-tested public API. Test >> Other → either under-used in production or over-tested in isolation.
  - Production-only methods. All-Other rows = critical-path. High touch risk.
  - Mixed: legitimate API. Balanced Test/Other.
  - Read vs Write ratio. Layer 8 categories include Read/Write. Many readers + few writers = good encapsulation; many writers = diffuse invariants.

  11. Method-aware workflows (Layer 4 specific)

  - Type's full API surface. module_tree(crate=X) walked into a type → type plus all methods + assoc consts/types as children.
  - Method-by-method fan-in (literal recipe).
    1. module_tree(directory, krate=X) at sufficient depth → find the type's children (methods, assoc consts, assoc types).
    2. For each child, call who_uses_summary(target=X::Type::method) — best run in parallel since they're independent reads.
    3. Sort results by total_count desc.
    4. Empty who_uses = dead method (Layer 4 finally surfaces these). All-Test rows = test-only helper. All-Other = critical path.
    Pre-Layer-4 these queries errored because methods weren't graph nodes; post-Layer-4 they return real results.
  - Dead method API. Same pattern — methods with empty who_uses are dead.
  - Inherent vs trait method distinction. module_tree shows both as children; parent_id differs (trait Item vs struct/enum Item).
  - Method-naming consistency check. Scan module_tree outputs for naming patterns across types (every type has new, every error type has from_io, etc.).
  - Function-level call graph (within file). get_call_graph(file) is parser-based and gives function-to-function edges WITHIN one file. Complement to method-level usages across files.

  12. API surface auditing

  - Crate-root pub use facade. get_declared_reexports(module=crate_root).
  - Effective surface from downstream. get_exports(module=crate_root, consumer=other_crate).
  - Canonical vs facade path traffic. who_imports(target=symbol) lists every importer through both paths (re-exports resolve to canonical NodeId).
  - Spot accidentally-exposed internals. Items in get_declared_reexports that the team intended to be pub(crate).
  - Find pub items behind a facade that don't need to be pub. If pub use chain can become pub(crate) use, original can be pub(crate). dead_pub_in_crate finds these.
  - Empty results as signals (not errors). get_declared_reexports([]) means the crate has no facade — everything is at canonical paths. We saw this on permissions in coding-agent-bad: zero declared re-exports, an intentional design choice. Same with overlaps.common_fn_names — empty is the good sign (no init/run proliferation).

  13. Semantic similarity-driven analysis

  - Find similar functions. get_similar_code(target) → vector candidates.
  - Refactor candidate finding. Combine get_similar_code (find similar) with who_uses (verify they're called) to identify dedupe candidates.
  - Naming-convention enforcement. module_tree(crate) lists fn names; get_similar_code finds semantically similar bodies with different names.
  - Cross-crate duplicate detection. overlaps.cross_crate_type_collisions finds same-name types; get_similar_code confirms they're semantically the same.

  14. Function-level call graphs

  who_uses_in (hypergraph) gives module-level resolution; get_call_graph (parser) gives function-to-function within-file. Use cases:

  - Trace function calls within a file. get_call_graph(file=X.rs) → which fns call which.
  - Find leaf functions (no internal callers). get_call_graph outputs without incoming edges.
  - Find entry-point functions. get_call_graph outputs with no outgoing edges.
  - Verify expected call paths. "Does handle_request call validate_input?" → check the parser-level edge.

  Limitation: get_call_graph is per-file or per-fn, not workspace-wide. Cross-file calls show up only in who_uses aggregation.

  15. Reading code in context

  - Read a file. read_file_content(path).
  - Render a hit with context. who_uses(X) returns (file, start, end); widen [start - 200, end + 200] and read_file_content of the file, slice the bytes.
  - Jump to definition. find_definition(name) returns the canonical site.
  - See all references with context. find_references(name) (RA) for inline browsing OR who_uses(qualified_name) (hypergraph) for structural answer.

  16. Complexity-driven prioritization

  - Find gnarly functions. analyze_complexity(file) per file in crate.
  - Cross-reference with usage. For each high-complexity fn, who_uses_summary → "complex AND widely used" = top refactor priority.
  - Trace why a fn is complex. get_call_graph(file) → see the call tree branching out from the gnarly fn.
  - Verify simplifications. Pre-/post-snapshot analyze_complexity outputs after a refactor.

  17. Cross-crate dependency analysis (visualization)

  - Crate dependency graph. crate_edges → render as DOT/Graphviz; weight = total_refs_via_imports + total_refs_via_usages.
  - Module tree. module_tree(crate) → recursive tree as ASCII or graphviz.
  - Symbol fan-in heatmap. Per Item, who_uses_summary count → cells.
  - Sankey diagram of cross-crate flow. crate_edges → source = consumer_crate, target = producer_crate, value = total_refs.
  - Per-symbol blast-radius diagram. who_uses(symbol) results, edges from consumer module to symbol.

  18. Comparing across snapshots / branches

  - API surface change. Snapshot module_tree(crate) + get_declared_reexports(crate_root) on each branch, diff JSON.
  - Dead-pub trend. dead_pub_report per branch; compare counts.
  - Edge weight changes. crate_edges per branch; per (consumer, producer) compare unique_symbols and total_refs.
  - Method count by type. workspace_stats.items_by_kind.Method trend.
  - Complexity trend. analyze_complexity per branch on the same files.

  19. Index / cache management

  - Build/refresh hypergraph. build_hypergraph(directory, force_rebuild?).
  - Schema-bump auto-invalidation. SCHEMA_VERSION is mixed into graph_id. After a schema bump (e.g. Layer 4 was v4→v5), calling build_hypergraph(force_rebuild=false) on existing snapshots returns reused=false and cold-rebuilds correctly. You don't need force_rebuild=true after schema changes.
  - Build/refresh vector index. index_codebase(directory) — needed for search (BM25) and get_similar_code.
  - Clear corruption. clear_cache(directory?).
  - Verify infrastructure. health_check — confirms indexes exist, snapshot is current.
  - Parallelism. All read tools (everything except build_hypergraph, index_codebase, clear_cache) are independent — call them in parallel when a workflow needs several. We routinely batch 5-10 calls per round (build_hypergraph in parallel against two workspaces, who_uses_summary on 10 collision targets, etc.) without issue.

  20. Output handling and post-processing

  Some MCP outputs are large enough that the standard Read pipeline can't fully load them — e.g. crate_edges on a 17-crate workspace is ~67KB, module_tree at depth=3 on a 15-submodule crate is ~72KB. The MCP server persists oversized outputs to a tool-results JSON file and returns a preview. Post-process those with Bash + Python or jq.

  - Detect a persisted output. The tool result includes a `<persisted-output>` block naming a path under ~/.claude/projects/.../tool-results/. Parse that file rather than relying on the inline preview.
  - Common reductions on crate_edges. The full edge matrix is verbose; the load-bearing summaries are usually:
    1. Per-producer fan-in (who depends on this crate, with totals).
    2. Per-consumer fan-out (what this crate depends on, with totals).
    3. Top-N edges sorted by total_refs_via_imports + total_refs_via_usages.
    4. Symbol breakdowns within a single edge (filter to one (consumer, producer) pair).
    A small Python script reads the persisted JSON, applies these reductions, and prints a table. Reuse the same script across workspaces — only the JSON path changes.
  - Module_tree depth as the first lever. Reach for depth=2 before Bash post-processing. The full tree is rarely worth the bytes.
  - Filter crate_edges client-side. The MCP returns the full matrix; per-crate analysis requires filtering client-side by consumer_crate or producer_crate. Same for overlaps' four buckets.

  21. Tool index — cheat sheet

  ┌────────────────────────┬────────────────┬─────────────────────────────┬───────────────────────────────────────────┐
  │          Tool          │     Layer      │           Returns           │                 Best for                  │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ build_hypergraph       │ Layer 4        │ snapshot metadata           │ initialize/refresh                        │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ workspace_stats        │ Layer 6        │ counts                      │ overview                                  │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ crate_edges            │ Layer 6        │ edge matrix                 │ architecture                              │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ overlaps               │ Layer 6        │ hygiene findings            │ quality audit                             │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ module_tree            │ Layer 6        │ recursive tree              │ structural dump                           │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ dead_pub_report        │ Layer 6        │ dead pub items              │ refactor planning                         │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ dead_pub_in_crate      │ Layer 6        │ per-crate dead              │ targeted refactor                         │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_imports            │ Layer 6        │ use-edges in module         │ per-module analysis                       │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_exports            │ Layer 6        │ visibility-filtered exports │ API audit                                 │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_reexports          │ Layer 6        │ reachable pub use           │ facade audit                              │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_declared_reexports │ Layer 6        │ all pub use declarations    │ full re-export audit                      │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ who_imports            │ Layer 6        │ reverse use-edges           │ importer inventory                        │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ who_uses               │ Layer 6        │ non-import refs             │ call-site inventory                       │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ who_uses_summary       │ Layer 6        │ aggregated rollup           │ "where is this concentrated?"             │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ search                 │ tantivy/BM25   │ text matches                │ when you don't know the name              │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ find_definition        │ RA             │ file:line                   │ jump to declaration                       │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ find_references        │ RA             │ refs incl. locals           │ catch-all (different scope than who_uses) │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_dependencies       │ parser         │ per-file imports            │ file-level (not module)                   │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_call_graph         │ parser         │ function call edges         │ within-file call structure                │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ analyze_complexity     │ parser         │ metrics                     │ gnarl finding                             │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ get_similar_code       │ LanceDB        │ semantic neighbors          │ dedup candidates                          │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ read_file_content      │ filesystem     │ raw bytes                   │ render context                            │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ index_codebase         │ infrastructure │ indexes                     │ prereq for search/similar                 │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ clear_cache            │ infrastructure │ --                          │ recover from corruption                   │
  ├────────────────────────┼────────────────┼─────────────────────────────┼───────────────────────────────────────────┤
  │ health_check           │ infrastructure │ status                      │ verify ready                              │
  └────────────────────────┴────────────────┴─────────────────────────────┴───────────────────────────────────────────┘

  22. Combining old + new — frequent patterns

  - "I have a string, I want structured analysis." search(string) → find_definition(name) → derive qualified name → hypergraph queries.
  - "I have a file, I want module analysis." read_file_content(file) for headers → infer module path → get_imports(module) + get_exports(module, consumer).
  - "I have a complex function, I want to refactor it." analyze_complexity(file) → identify candidate → who_uses(fn) for fan-in → get_call_graph(file) for internal structure → get_similar_code(fn) for related code.
  - "I want to dedupe." overlaps.cross_crate_type_collisions → for each collision, get_similar_code to confirm semantic equivalence → who_uses each version to plan migration.
  - "I want to render a hypergraph hit." who_uses(X) → for each (file, start, end), read_file_content(file) widened by N bytes context.
  - "I want goto-def with cross-crate fan-in." find_definition(name) (location) + who_uses(qualified_name) (callers) — IDE-like experience composed from two tools.
  - "I want to verify a name is free." find_definition(name) (no result) + search(name) (no string match) + overlaps.cross_crate_type_collisions (no collision) = safe.
  - "I want similar fns that are also widely used." get_similar_code(target) → who_uses_summary each candidate → rank by total_count.
  - "I want gnarly + frequently-edited code." analyze_complexity + git log --since=... + who_uses for blast radius.

  23. Workflows mapped to Rust guidelines (today's tools)

  Mapping each checkable Rust guideline to existing MCP tools. Useful as a CI / review checklist starting point — every entry here is implementable as a script using only the tools already in the cheat sheet (§21). Section numbers reference rust-guidelines-final.md.

  §4 — Function size & complexity
  - Cyclomatic complexity ≥ 10/15 thresholds → analyze_complexity(file) per crate, sort by score, cross-reference with who_uses for blast radius. (Already covered in §1.)
  - Refactor-priority ranking → analyze_complexity × who_uses_summary(target=fn) — "complex AND widely used" surfaces top candidates.

  §7 — Types & invariants
  - Migration-debt detection → overlaps.cross_crate_type_collisions + who_uses_summary on each side, find shared consumer = half-finished migration. (Recipe in §8.)

  §8 — Traits & generics ("skip a trait when there's one implementation")
  - Single-implementation trait audit → for each pub trait from module_tree, run who_imports(target=Trait). If importer count is 1 and the trait isn't a Send/Debug/etc. supertrait, it's a candidate for inlining.
  - Trait method ROI → for each trait method, who_uses_summary(target=Trait::method). Methods with empty who_uses are dead trait API (Layer 4 unlocks this).

  §10 — Modules, crates, visibility
  - pub_crate_share discipline benchmark → workspace_stats.pub_crate_share for between-codebase comparison.
  - Module nesting depth → walk module_tree(crate) recursively, track max depth. Anything > 3-4 levels deep is a smell.
  - Re-export facade audit → get_declared_reexports ∩ dead_pub = dead facade. (Recipe in §8.)
  - Module name shadows → overlaps.module_shadows + the diagnostic in §9 (filter crate_edges for actual dep).
  - Visibility distribution → workspace_stats.visibility shows pub vs pub_crate vs restricted_to vs private. Surprising ratios = hygiene smell.

  §11 — Architecture
  - DAG enforcement → walk crate_edges in code, detect cycles. Pure script using existing data.
  - "Domain crates free of framework deps" → for any crate matching domain|core|model|kernel, filter crate_edges for consumer_crate=X and check producer set against a forbidden list (tokio, bevy, serde_json, hyper, etc.). Pure rule check on existing data.
  - "Translate external formats at the boundary" → if a domain crate has cross-crate edges to provider/store/http crates, that's a leak.
  - Heaviest cross-crate edges → crate_edges sorted by total_refs. The top edges define the architecture; if they're surprising, the architecture is.
  - No-cycle check between layered crates (e.g., core ↛ ui ↛ core) → DAG walk on crate_edges.

  §12 — Async as a boundary
  - "Domain crate importing tokio" → filter crate_edges for (consumer=domain_crate, producer=tokio). Same pattern works for bevy/futures/etc.
  - Note: the narrower "no .await in domain logic" is parser-territory — outside graph scope.

  §17 — Testing
  - Test-only constructor audit → who_uses_summary(Type::new) showing 100% Test = fixture builder. (Recipe in §10.)
  - Read vs Write category split for invariant checking. (Recipe in §10.)

  §23 — Review checklist
  Most checklist items are graph-checkable today:
  - "Is the dependency graph a DAG?" → crate_edges cycle check.
  - "Are traits marking real substitution boundaries?" → trait justification audit above.
  - "Did the change add public API surface?" → diff get_declared_reexports before/after.
  - "Are domain concepts represented by explicit types?" → indirectly: workspace_stats.items_by_kind shows Struct/Enum/TypeAlias counts; few of these relative to Fn means primitive-heavy APIs.
  Note: some checklist items (typed errors, state machine explicitness, source-error-chain preservation) need parser hooks or new tools — see §24.

  24. What you can't do today

  - Function-to-function call graph workspace-wide. who_uses is module-level; get_call_graph is file-level. There's no "fn → fn across the workspace" tool.
  - Trait impl enumeration. "Every concrete impl of Trait" requires Layer 4c (deferred). who_imports(Trait) is the workaround.
  - Cross-snapshot diffs as a tool. Possible by hand; no diff_hypergraph tool yet.
  - Macro-expanded code. Categorization can be murky; some macro-introduced refs don't surface cleanly.
  - Per-method visibility on the Item Node. Methods don't have Declared bindings, so Node.visibility is null. To get a method's visibility, read Node.file + Node.span and inspect source.
  - Pagination. who_uses on a popular trait can return thousands of rows. No cursor (Phase D deferred).
  - Inherent impls of foreign types. Methods on dep-crate types aren't extracted.
  - Non-Rust files. Vector store can index any text; hypergraph is Rust-only.
