# Phase 6 — Parser Scope Reduction

**Authoritative reference:** `.docs/workspace-plan/DECISIONS.md`. If anything below conflicts with DECISIONS.md, DECISIONS.md wins.

**Goal.** Shrink `legacy::parser::RustParser` from a four-projection AST extractor (symbols + call graph + imports + type references) to a chunking-context-only extractor that lives inside `rcm-search`'s chunker and uses `rcm-ra-syntax` directly. Move structural MCP tools `get_dependencies`, `get_call_graph`, `analyze_complexity` from legacy into `rcm-graph`, reimplemented against `OpenedSnapshot` (HIR-resolved). Delete every parser sub-module with no remaining caller.

**Why high-risk.** It can change ingestion ordering. Today `parser::parse_source_complete` runs purely on bytes — no rust-analyzer load required. If structural facts that the chunker writes into Tantivy need a graph snapshot, `build_hypergraph` must run before chunking, ballooning cold `index_codebase`. Mitigation in Steps 3, 4, 7 — the chunker stays AST-only for *embedding context*; only Tantivy fields that demand HIR resolution would gate on the snapshot, and Step 3 confirms none do.

**Reversibility.** Partial. Code deletion is reversible from git. Tantivy schema changes are not — an index rebuild reverses them. Posture: do **not** change the Tantivy schema in Phase 6. Schema migration belongs in Phase 7.

---

## Step 1 — Audit parser usage

**What to do.** Catalog every consumer of `parser` outputs. Run:

```text
rg -t rust 'RustParser|parse_source_complete|parse_file_complete|extract_imports|extract_call_graph|build_type_references|extract_type_references|CallGraph::|TypeReference|ParseResult' \
    crates/file-search-mcp-legacy/src
```

Classify each hit into exactly one bucket:

1. **chunker-context** — embedding text only (last-segment call names, raw `use` paths, file-scoped symbol kinds, byte ranges). No resolution.
2. **ingestion-metadata** — Tantivy fields (`module_path`, `symbol_kind`, `imports`, `outgoing_calls`, `docstring`). Today: textual heuristics.
3. **structural-tool** — backs `get_dependencies` / `get_call_graph` / `analyze_complexity`. Must be HIR-resolved post-Phase-6.

Produce a table in the merge commit body:

| Caller | Parser output consumed | Bucket | Post-Phase-6 source |
|---|---|---|---|
| `chunker::Chunker::chunk_file` | `symbols`, `imports`, `call_graph` | chunker-context | `rcm-ra-syntax` direct |
| `indexing::indexer_core::process_file_sync` | `ParseResult` for chunk + Tantivy fields | ingestion-metadata | `rcm-ra-syntax` (approx) |
| `tools::analysis_tools::get_dependencies` | `imports` | structural-tool | `OpenedSnapshot::imports_in_file` |
| `tools::analysis_tools::get_call_graph` | `call_graph` | structural-tool | `OpenedSnapshot::calls_from` |
| `tools::analysis_tools::analyze_complexity` | `symbols`, fn body lines | structural-tool | `rcm-ra-syntax` AST walk inside `rcm-graph` |
| `graph::channel_audit`, `fn_body_audit` | `RustParser` (rare) | structural-tool | already on `rcm-ra-host` (no-op) |

**Files touched.** None — investigation only. Output goes into `.docs/workspace-plan/notes/phase-6-audit.md` for later review.

**Acceptance.** Every callsite is in exactly one bucket; no callsite is in zero or two. Reviewer signs off the table.

**Reversal.** N/A (no code change).

---

## Step 2 — Define `rcm-ra-syntax`-only chunker context API

**What to do.** Move chunking AST extraction into `rcm-search` as a sans-I/O sub-module, importing only `rcm-ra-syntax`. The old `parser` is no longer the chunker input. Define:

```rust
// crates/rcm-search/src/chunker/context.rs

use rcm_ra_syntax::{
    ast, AstNode, Edition, SourceFile, SyntaxNode, TextRange,
};

/// Embedding-context-only AST extraction. NOT name-resolved.
/// Every name here is a last-segment textual approximation.
pub(crate) struct ChunkContextExtract {
    pub symbols: Vec<SymbolHint>,
    pub imports: Vec<RawUsePath>,
    pub calls: Vec<(SymbolHintId, Vec<LastSegmentName>)>,
}

pub(crate) struct SymbolHint {
    pub id: SymbolHintId,
    pub name: String,
    pub kind: SymbolKindHint,
    pub range: TextRange,
    pub docstring: Option<String>,
}

pub(crate) struct RawUsePath {
    /// "::"-joined path text (e.g., "std::collections::HashMap").
    pub path: String,
    /// `as` rename if present.
    pub rename: Option<String>,
    pub is_glob: bool,
}

pub(crate) struct LastSegmentName(pub String);

pub(crate) fn extract_context(
    source: &str,
    edition: Edition,
) -> ChunkContextExtract {
    let parse = SourceFile::parse(source, edition);
    let file = parse.tree();
    // walk file.items() for Fn/Struct/Enum/Trait/Impl/Module/Const/Static/TypeAlias
    // walk Use trees flattening into RawUsePath rows
    // walk fn bodies for CALL_EXPR / METHOD_CALL_EXPR last-segment names
    todo!("body lifted from legacy parser; no resolution, no edition-flag mutation")
}
```

The chunker entry point becomes:

```rust
// crates/rcm-search/src/chunker.rs (driver)
pub fn chunk_file(
    file_path: &Path,
    source: &str,
    edition: Edition,
) -> Result<Vec<CodeChunk>, ChunkError> {
    let ctx = context::extract_context(source, edition);
    let module_path = derive_module_path(file_path);
    // build CodeChunk per SymbolHint, attach ctx.imports + ctx.calls subset
    // stitch overlap windows
}
```

Changes from today's signature:

- No `ParseResult` input; takes source text + edition.
- No dep on `legacy::parser`.
- `rcm-ra-syntax` is the **only** rust-analyzer dep in `rcm-search` (per DECISIONS §7).

**Files touched.**
- `crates/rcm-search/src/chunker.rs` — replace input shape; route through `context::extract_context`.
- `crates/rcm-search/src/chunker/context.rs` — new file; lift the chunker-context subset of `legacy::parser` (symbols + use-tree flatten + last-segment call names) verbatim, dropping all type-reference extraction.
- `crates/rcm-search/Cargo.toml` — confirm `rcm-ra-syntax` workspace dep is present; ensure no `ra_ap_*` direct deps.

**Acceptance.**
- `cargo check -p rcm-search` is clean.
- Snapshot tests for `format_for_embedding` produce byte-identical output for a fixture file (the chunker's contract is observably unchanged for the embedding path).
- `cargo public-api -p rcm-search` shows no `ra_ap_*` types in the public API.

**Reversal.** Delete `chunker/context.rs`, restore `chunker.rs` to taking `&ParseResult`, restore the legacy parser callsite in indexing.

---

## Step 3 — Decide Tantivy field policy

**What to do.** Phase 6 must not silently degrade keyword search ranking. For every Tantivy field today populated via parser-derived structure, pick one disposition:

| Tantivy field | Today's source | Phase 6 decision | Rationale |
|---|---|---|---|
| `chunk_id` | `Chunker` (UUID) | unchanged | identity, not structure |
| `content` | source slice | unchanged | bytes, not structure |
| `symbol_name` | `parser::Symbol::name` | **ra-syntax approximation** | last-segment textual name is fine for BM25 |
| `symbol_kind` | `parser::SymbolKind` | **ra-syntax approximation** | `Fn`/`Struct`/... are syntactic kinds |
| `file_path` | walker | unchanged | filesystem, not structure |
| `module_path` | `chunker::extract_module_path` (path heuristic) | **keep heuristic** (already not parser-derived) | already on path components, not HIR |
| `docstring` | `parser::extract_docstring` | **ra-syntax approximation** | `///` / `//!` token strip is local |
| `chunk_json` | full `CodeChunk` JSON | **ra-syntax approximation** | already lossy by design |

No field requires HIR resolution for keyword search. **The chunker stays AST-only**; the snapshot is not on the cold-start critical path for Tantivy ingest. Future HIR-only fields (e.g., a `qualified_name` for exact-symbol matching) are deferred to a later phase together with the Step 4 orchestration.

**Files touched.**
- `crates/rcm-search/src/tantivy_adapter/schema.rs` — confirm field producers are routed through the new chunker only (no `ParseResult` field reads). Touch comments where the field comment claims HIR resolution.

**Acceptance.** A grep for `ParseResult` in `crates/rcm-search/src` returns zero hits. The Tantivy schema definition itself is unchanged in this phase.

**Reversal.** N/A (the Tantivy schema is not modified).

---

## Step 4 — Snapshot read API for chunker (kept simple)

**What to do.** Document — but do not implement — the orchestration required if a Tantivy field needed HIR data. Fallback contract for Phase 7+ schema changes:

```rust
// crates/rcm-server/src/composition/index_codebase.rs (illustrative)
pub async fn index_codebase(
    workspace: &Path,
    services: &Services,
) -> Result<IndexStats, ServerError> {
    // Today: walk -> chunk -> embed -> write. No graph dep.
    //
    // If a future Tantivy field needs HIR-resolved data:
    // 1. let snap = services.graph.build_and_persist(workspace).await?;
    // 2. pass `Some(snap)` into chunker; chunker enriches per-chunk
    //    via `snap.imports_in_file(...)` etc.
    //
    // The chunker MUST accept `Option<&OpenedSnapshot>` and degrade
    // gracefully to ra-syntax approximations when None.
    let stats = services.search.index(workspace, /* snap = */ None).await?;
    Ok(stats)
}
```

**Default for Phase 6:** the chunker takes `Option<&OpenedSnapshot>` in its public signature but never receives `Some(...)` from any caller. This locks the contract for future extension without paying the cold-start cost today.

**Files touched.**
- `crates/rcm-search/src/chunker.rs` — extend signature to `chunk_file(file_path, source, edition, snap: Option<&OpenedSnapshot>)`; do not yet read from `snap`.
- `crates/rcm-search/src/lib.rs` — re-export the `Option<&OpenedSnapshot>` parameter through `SearchService::index` so future enrichment doesn't require an API break.

**Acceptance.** Compiles. All callers pass `None`. The chunker body has a `// TODO(phase-7+): consume snap when schema gains HIR fields` marker exactly once.

**Reversal.** Remove the parameter; restore the prior signature.

---

## Step 5 — Reimplement structural tools in `rcm-graph`

**What to do.** Move `get_dependencies`, `get_call_graph`, and `analyze_complexity` into `rcm-graph` and route them through `OpenedSnapshot`. Each tool keeps its MCP name and request/response shape (the surface contract is in `rcm-server`); only the implementation strategy changes.

### 5a. `get_dependencies(file)` — per-file imports

```rust
// crates/rcm-graph/src/queries/file_scoped.rs

use crate::queries::OpenedSnapshot;
use crate::model::Binding;
use std::path::Path;

impl OpenedSnapshot {
    /// HIR-resolved imports declared by the module that owns `file`.
    /// Returns `Binding { kind: Import, .. }` rows from the def-map walk.
    pub fn imports_in_file(
        &self,
        file: &Path,
    ) -> Result<Vec<Binding>, QueryError> {
        // 1. resolve `file` -> ModuleId via vfs path lookup
        // 2. self.imports_of(module_id)  // already exists in queries/imports.rs
    }
}
```

The existing `OpenedSnapshot::imports_of(module_id)` query (see `legacy::graph::queries::imports`) does the heavy lifting; the new method is a thin path-to-`ModuleId` adapter. If `imports_of` is not yet exposed by qualified-name path lookup, add a `module_for_file(&Path) -> Option<ModuleId>` helper that walks `vfs()` from `RaHost`.

### 5b. `get_call_graph(file, symbol)` — per-symbol outgoing calls

```rust
impl OpenedSnapshot {
    pub fn calls_from_in_file(
        &self, file: &Path, symbol: &str,
    ) -> Result<Vec<CallEdge>, QueryError> {
        // 1. (file, symbol) -> NodeId via lookup_by_qualified_name
        // 2. self.calls_from(node_id)  // exists in queries/call_graph.rs
    }
}
```

The snapshot already has `calls_from(node_id)`. The new adapter handles disambiguation when `symbol` appears multiple times in the file — return all matches, matching today's MCP request shape.

### 5c. `analyze_complexity(file)` — per-file metrics

Complexity (cyclomatic + line counts + branch counts) is not a snapshot model concept. Re-walking the AST is the right answer. Keep this tool AST-driven, but move it into `rcm-graph` because that crate already does AST audits (`unsafe_audit`, `channel_audit`, `fn_body_audit`) via `RaHost::with_semantics`.

```rust
// crates/rcm-graph/src/audits/complexity.rs

use rcm_ra_syntax::{ast, AstNode, Edition, SourceFile};

pub struct ComplexityReport {
    pub file_lines: usize,
    pub function_count: usize,
    pub avg_function_length: f64,
    pub max_complexity: u32,
    pub findings: Vec<FunctionComplexity>,
}

pub struct FunctionComplexity {
    pub name: String,
    pub line_start: u32,
    pub line_end: u32,
    pub cyclomatic: u32,
}

pub fn analyze_complexity(
    source: &str,
    edition: Edition,
) -> ComplexityReport {
    let parse = SourceFile::parse(source, edition);
    let file = parse.tree();
    // walk fn bodies; count branches (`if`, `match` arm, `&&`, `||`, `?`,
    // loop kinds) per RFC-style cyclomatic.
}
```

This tool does **not** need a snapshot; it reads source bytes and the AST only. Wiring it into `rcm-graph` keeps the "AST in graph crate via `rcm-ra-syntax`" pattern that `unsafe_audit` already follows.

**Files touched.**
- `crates/rcm-graph/src/queries/file_scoped.rs` — new file; `imports_in_file`, `calls_from_in_file`.
- `crates/rcm-graph/src/audits/complexity.rs` — new file; pure AST walk.
- `crates/rcm-graph/src/lib.rs` — re-export the three entry points on `GraphService` (`file_dependencies`, `file_call_graph`, `file_complexity`).
- Tests: snapshot tests against fixture files; assert `imports_in_file` returns *more* rows than the old textual extractor for code that uses re-exports.

**Acceptance.**
- `cargo check -p rcm-graph` clean.
- `imports_in_file` on a fixture using `pub use foo::Bar` returns the resolved binding for `foo::Bar`, not just the textual `use` line. (This is the "strictly more accurate" property promised in DECISIONS.)
- `calls_from_in_file` resolves turbofish calls (`Vec::<u8>::new`) correctly via `ast_resolve`.
- Complexity report numbers match the legacy implementation for a regression fixture (within rounding).

**Reversal.** Delete the new files; the legacy `analysis_tools` impl is still in git.

---

## Step 6 — Delete parser code

**What to do.** Once Steps 2 and 5 land, delete:

- `legacy::parser::call_graph` — replaced by `OpenedSnapshot::calls_from` + `ast_resolve`.
- `legacy::parser::imports` — replaced by `OpenedSnapshot::imports_of` + `imports_in_file` adapter.
- `legacy::parser::type_references` — replaced by `Usage` rows + `UsageCategory`.
- `legacy::parser::ParseResult`, `RustParser::parse_source_complete`, `parse_file_complete` — no remaining consumer.

What survives moves to `crates/rcm-search/src/chunker/context.rs` (Step 2). `legacy::parser` becomes empty — delete the module and remove `pub mod parser;` from `legacy::lib`.

Verify with:

```text
rg -t rust 'use .*parser::' crates/file-search-mcp-legacy/src
rg -t rust 'parser::(RustParser|ParseResult|Symbol|CallGraph|Import|TypeReference)' crates/
```

Both should return zero hits.

**Files touched.**
- Delete `crates/file-search-mcp-legacy/src/parser/` directory.
- Edit `crates/file-search-mcp-legacy/src/lib.rs` — remove `pub mod parser;`.
- Update any `use crate::parser::*` lines in legacy code that the audit (Step 1) flagged but didn't migrate (should be none).

**Acceptance.**
- `cargo check --workspace` clean.
- `cargo build -p file-search-mcp` produces the same binary entry points (the binary may now route some tools through `rcm-graph::GraphService`; that's expected).

**Reversal.** `git revert` the deletion commit. The parser module is restored verbatim.

---

## Step 7 — Cold-start measurement gate

**What to do.** Must-pass check before merge. Steps 3 and 4 premise: the chunker needs no snapshot, so cold `index_codebase` should not regress. Verify:

1. **Bench BEFORE Phase 6.** Check out the parent of the Phase 6 merge. Run on a fixed fixture workspace (recommend `rust-analyzer`'s own repo at a pinned SHA, ~3 MLoC):

   ```text
   nix develop ../nix-devshells#code --command bash -c '
       rm -rf ~/.local/share/rust-code-mcp/search/*
       hyperfine --runs 3 --warmup 0 \
           "cargo run --release -p file-search-mcp -- index --workspace $FIX"
   '
   ```

2. **Bench AFTER Phase 6.** Same fixture, same machine, same config. Record wall-clock mean and stddev.

3. **Gate.** If `mean(after) > 1.2 * mean(before)`, do not merge as-is. Options:
   - **Opt-out flag.** Add `--legacy-chunker` to `index_codebase` params; default = new path; flag routes to a preserved parser-driven chunker copy.
   - **Profile and fix.** A 20%+ regression from a deletion is suspect — likely an allocation regression. Fix before merge.

4. **Document.** Paste `hyperfine` output into the merge commit body. Fields: cold mean, cold stddev, snapshot-build time (~0), Tantivy commit count.

**Files touched.**
- `crates/xtask/src/cmd/bench_cold_index.rs` — new xtask subcommand wrapping the hyperfine invocation.
- Optional: `crates/rcm-search/Cargo.toml` feature `legacy-chunker` (default off) for the opt-out flag.

**Acceptance.** Numbers in commit body, within 20% of pre-Phase-6, OR opt-out flag wired. Reviewer signs off the comparison.

**Reversal.** N/A (measurement, not code).

---

## Step 8 — Update tool dispatch in `rcm-server`

**What to do.** Re-route the three structural tools' rmcp handlers from `analysis_tools` (legacy parser) to `GraphService` (rcm-graph). The MCP tool surface is unchanged: same tool names, same `*Params` structs, same response JSON shapes. Only the handler body changes.

```rust
// crates/rcm-server/src/tools/analysis_tools.rs

#[tool(description = "Analyze imports and dependencies of a Rust file")]
pub async fn get_dependencies(
    &self,
    Parameters(params): Parameters<GetDependenciesParams>,
) -> Result<CallToolResult, McpError> {
    let workspace = ProjectPaths::from_directory(&params.workspace)?;
    // OLD: let parser = RustParser::new(); parser.parse_file_complete(&params.file)?;
    // NEW:
    let deps = self
        .graph
        .file_dependencies(&workspace, &params.file)
        .await
        .map_err(internal_error)?;
    let body = render_dependencies(&deps); // existing formatter
    Ok(CallToolResult::success(Content::text(body)))
}
```

The renderer functions (`render_dependencies`, `render_call_graph`, `render_complexity`) are kept verbatim — they format response strings; they do not care whether the upstream rows came from a parser or a snapshot.

**Files touched.**
- `crates/rcm-server/src/tools/analysis_tools.rs` — replace three handler bodies; drop the `RustParser`/`SemanticIndex` field on the router.
- `crates/rcm-server/src/composition.rs` — wire `Arc<GraphService>` into the router (already constructed in `main` per DECISIONS §11).
- Drop the `static SEMANTIC` reference (already targeted by Phase 2's hidden-singleton removal; verify it's gone).

**Acceptance.**
- `cargo check -p rcm-server` clean.
- Smoke checklist (DECISIONS §"Smoke checklist") passes:
  - `get_dependencies(workspace, file)` returns non-empty for a file with `use` items.
  - `get_call_graph(workspace, file, symbol)` returns the same callees as before for a non-trait, non-method-dispatch fn (regressions on trait dispatch are expected and *correct* — HIR resolves them).
  - `analyze_complexity(workspace, file)` returns identical metrics for a fixture (within rounding).

**Reversal.** Restore the prior handler bodies from git; re-add `RustParser`/`SemanticIndex` to the router state.

---

## Step 9 — Document the behavioral change

**What to do.** The MCP tool surface stays compatible, but two observable behaviors change:

1. **Accuracy improves.** `get_dependencies` now reports HIR-resolved bindings, including those from `pub use` chains and macro-introduced imports. `get_call_graph` resolves trait method dispatch and turbofish calls correctly. Both tools may now return *more rows* than before. Clients that asserted exact row counts on fixtures will need updates.

2. **Cold-path may be slower** (gated on Step 7). Opening the snapshot adds a one-time cost on the cold path of `get_dependencies` / `get_call_graph`. Warm calls reuse the snapshot via the per-service cache (DECISIONS §"Service lifetime").

Add a `CHANGELOG.md` entry under "Phase 6":

```markdown
### Changed
- `get_dependencies` and `get_call_graph` now return HIR-resolved structural
  data instead of textual heuristics. Result rows may increase for code
  that uses re-exports, trait dispatch, or turbofish.
- `analyze_complexity` is now driven by `ra_ap_syntax` directly (no behavioral
  change; line counts and cyclomatic numbers are within rounding of prior
  values).

### Removed
- The standalone `parser` module is gone. Chunking-context AST extraction
  lives in `rcm-search::chunker::context`; structural facts come from
  `rcm-graph::OpenedSnapshot`.

### Performance
- Cold `index_codebase` time: <numbers from Step 7>.
- Warm structural tool calls: unchanged (snapshot reuse via ArcSwap).
```

**Files touched.** `CHANGELOG.md`, `.docs/architecture/parser.md` (delete or replace with one paragraph saying "this module is gone, see chunker.md and graph.md"), `.docs/architecture/chunker.md` (update the data-flow paragraph that names `ParseResult` as input — replace with `(source, edition)` direct).

**Acceptance.** CHANGELOG merged. Architecture docs reflect the new shape.

**Reversal.** Revert the doc commits.

---

## Phase 6 acceptance

A Phase 6 merge candidate must satisfy *all* of the following:

- [ ] `cargo build --workspace` is green.
- [ ] `cargo public-api -p rcm-search` shows no `ra_ap_*` types in the public API.
- [ ] `rg 'parser::' crates/file-search-mcp-legacy/src` returns zero hits (or only the audit-allowed stragglers documented in Step 1).
- [ ] The chunker uses **only** `rcm-ra-syntax`; the chunker has no dep on `rcm-graph`.
- [ ] Structural tools (`get_dependencies`, `get_call_graph`, `analyze_complexity`) are dispatched through `rcm-graph` from `rcm-server`.
- [ ] Smoke checklist (DECISIONS §"Smoke checklist") passes against the fixture workspace.
- [ ] On a re-export-heavy fixture, `get_dependencies` returns **more** rows than the pre-Phase-6 baseline (positive regression; HIR is more accurate).
- [ ] Cold `index_codebase` benchmark numbers are pasted in the merge commit body, and either (a) within 20% of pre-Phase-6 wall time, or (b) the opt-out flag `--legacy-chunker` is wired and tested.
- [ ] CHANGELOG entry merged.
- [ ] `.docs/architecture/parser.md` either deleted or shrunk to a redirect.

## Reversibility

This phase is **partially** reversible. Concretely:

- **Reversible.** Code deletions (Step 6), chunker re-shaping (Step 2), structural tool migration (Step 5), tool-dispatch re-wiring (Step 8). Any of these can be rolled back via `git revert` without data loss.
- **Not reversible without rebuild.** Tantivy schema changes. Step 3's policy is explicit: do *not* change the schema. If a future regression hunt suggests dropping a field, defer that to Phase 7 with a proper xtask migration; do not touch the schema in Phase 6 hotfixes.
- **Snapshot compatibility.** This phase does not change LMDB on-disk format. Existing snapshots remain readable. Workspace fingerprints are unchanged. `build_hypergraph` short-circuits the same way it did before.

If Step 7's measurement gate trips and the team ships the opt-out flag, schedule a Phase 6.5 follow-up to profile and remove the flag. Do not let the flag rot — it forks the chunker codepath, which is exactly the duplication this phase exists to remove.
