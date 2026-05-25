# Update Commits Port Plan

## Goal

Replicate the behavior from the last three commits in `../rust-code-mcp-final`
inside this split-crate refactor repository without directly cherry-picking the
old single-crate commits.

Source commits:

1. `496f8b78` - `Support position-based rename disambiguation`
2. `7a7d87ff` - `Add crate types hypergraph query`
3. `a35bb621` - `Use full workspace load for rename previews`

Important adaptation:

- `../rust-code-mcp-final` uses old paths under `src/...`.
- This repository uses split crates:
  - server MCP/semantic code: `crates/rmc-server/src/...`
  - graph snapshot/query code: `crates/rmc-graph/src/...`
  - docs and tool docs remain at repo root.
- Do not run `cargo fmt` or any formatting command.
- Run build/test commands through:
  `nix develop ../nix-devshells#cuda-code --command {command}`.

## Current Assessment

The three commit objects are visible in this repository, but they are not in the
current ancestry. Direct cherry-pick is not the right path because the final
repo changes target the old single-crate layout. Port the behavior manually.

`a35bb621` partly already exists here:

- `rmc-graph` already loads the hypergraph with `no_deps=false`,
  `sysroot=Discover`, all features, and all targets.
- The remaining work from that commit is mostly in `rmc-server` semantic rename
  loading and tool-description wording.

## Phase 0: Baseline And Diff Review

### Goal

Capture the relevant source diffs and current split-crate locations before
editing.

### Steps

1. Confirm working copy state.

```sh
jj status
```

2. Review source commit diffs in `../rust-code-mcp-final`.

```sh
jj diff --git -r 496f8b78 -- src/semantic/rename.rs src/semantic/position.rs src/semantic/mod.rs src/tools/analysis_tools.rs src/tools/search_tool.rs src/tools/search_tool_router.rs
jj diff --git -r 7a7d87ff -- src/graph/queries.rs src/graph/mod.rs src/tools/graph_tools.rs src/tools/search_tool.rs src/tools/search_tool_router.rs TOOLS.md
jj diff --git -r a35bb621 -- src/graph/loader.rs src/lib.rs src/ra_cargo_config.rs src/semantic/loader.rs src/semantic/mod.rs src/tools/search_tool_router.rs
```

3. Re-check current target files in this repository.

```sh
rg -n "RenameSymbolParams|rename_symbol|rename_by_name|load_project|no_deps|crate_types|CrateType" crates/rmc-server/src crates/rmc-graph/src TOOLS.md
```

### Expected Output

- No code changes.
- Clear mapping from old single-crate files to split-crate files.

### Execution Status

- [x] Step 1: `jj show --summary` and `jj status` confirmed the working copy
  only contained this new plan file before Phase 0 commit.
- [x] Step 2: reviewed all three source diffs in `../rust-code-mcp-final`.
- [x] Step 3: checked current target files with `rg`. The refactor repo still
  needs position-based rename params, full-workspace semantic rename loading,
  and the `crate_types` graph/server tool. `rmc-graph` already uses
  `no_deps=false` for hypergraph loading.
- [x] Phase 0 conclusion: direct cherry-pick remains inappropriate because the
  source commits target the old single-crate `src/...` layout; manual porting
  is required.

## Phase 1: Position-Based Rename Disambiguation

Source commit: `496f8b78`.

### Goal

Allow `rename_symbol` to disambiguate ambiguous short names with an explicit
`file_path`, `line`, and `column` position.

### Production Changes

Modify `crates/rmc-server/src/semantic/position.rs`:

- Import `ra_ap_ide::Analysis` directly.
- Add:

```rust
pub(crate) fn file_position(
    analysis: &Analysis,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<FilePosition>
```

- Implement it with the existing `path_to_file_id` and `to_offset` helpers.
- Update `goto_definition` and `find_references` to call `file_position`.

Modify `crates/rmc-server/src/semantic/rename.rs`:

- Import `Path` and `super::position`.
- Update the name-only ambiguity message to include actionable candidate
  positions.
- Add helpers:
  - `nav_position`
  - `format_nav_candidates`
  - `format_nav_candidate`
  - `rename_at_file_position`
  - `verify_expected_symbol_at_position`
  - `identifier_at_offset`
  - `is_rust_ident_byte`
- Add:

```rust
pub(crate) fn rename_by_position(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
    expected_symbol_name: &str,
    new_name: &str,
) -> Result<RenamePreview>
```

- Keep name-only rename behavior backward-compatible except for better error
  text.
- Ensure position-based rename verifies the identifier under the cursor matches
  the requested symbol leaf.

Modify `crates/rmc-server/src/semantic/mod.rs`:

- Add `SemanticService::rename_by_position(...)`.
- Delegate to `rename::rename_by_position(...)`.

Modify `crates/rmc-server/src/tools/params/search.rs`:

- Extend `RenameSymbolParams` with optional:
  - `file_path: Option<String>`
  - `line: Option<u32>`
  - `column: Option<u32>`
- Document that the three optional fields must be provided together.

Modify `crates/rmc-server/src/tools/endpoints/analysis.rs`:

- Change `rename_symbol(...)` signature to accept optional `file_path`, `line`,
  and `column`.
- Validate:
  - all three position params are supplied together
  - `line > 0`
  - `column > 0`
- Resolve relative `file_path` from `directory`.
- Call `SemanticService::rename_by_position(...)` when position params exist.
- Keep existing `rename_by_name(...)` path when all three are absent.
- Add rename error classification helper:
  - return `invalid_params` for expected user/input/RA-refusal cases
  - keep load/internal failures as `internal_error`.

Modify `crates/rmc-server/src/tools/router.rs`:

- Update `rename_symbol` tool description to mention candidate-list
  disambiguation.
- Destructure and pass `file_path`, `line`, and `column` to the endpoint.

### Documentation Changes

Modify:

- `TOOLS.md`
- `skills/rmc-rename-symbol/SKILL.md`
- `.docs/architecture/semantic.md`
- `.docs/logic/semantic.md`

Document:

- name-only rename still resolves exact leaf names
- ambiguous names now return candidate `file_path:line:column`
- users can rerun with `file_path`, `line`, and `column`
- the optional position fields must be supplied together

### Tests

Add or update tests in `crates/rmc-server/src/tools/endpoints/analysis.rs`:

- partial position params return `invalid_params`
- zero line/column returns `invalid_params`
- rename invalid-param classification treats ambiguity and invalid position as
  client errors

Optional semantic test if cheap:

- create a temp Cargo project with two same-name symbols
- name-only rename fails with candidates
- position-based rename selects the intended symbol

### Verification

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server semantic
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server
```

### Execution Status

- [x] Phase start guard: ran `jj show --summary` before editing Phase 1.
- [x] Added shared `file_position` construction and wired
  `goto_definition` / `find_references` through it.
- [x] Added `rename_by_position` and candidate rerun hints for ambiguous
  name-only rename requests.
- [x] Extended the MCP params, router, endpoint validation, and error
  classification for optional `file_path`, `line`, and `column`.
- [x] Updated `TOOLS.md`, `skills/rmc-rename-symbol/SKILL.md`,
  `.docs/architecture/semantic.md`, and `.docs/logic/semantic.md`.
- [x] Verified with:
  - `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename`
  - `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server semantic`
  - `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`
- [x] Result: all Phase 1 checks passed. The commands emitted existing
  dead-code / unreachable-pub warnings and a dirty `../nix-devshells` warning;
  no compile or test failures.

### Commit

```sh
jj commit -m "server: support position-based rename disambiguation"
```

## Phase 2: Crate Types Hypergraph Query

Source commit: `7a7d87ff`.

### Goal

Add a hypergraph query and MCP tool that lists crate-owned type items with
filters for item kind, public-only items, associated types, test exclusions,
pagination, and summary mode.

### Graph Crate Changes

Modify `crates/rmc-graph/src/graph/query/model.rs`:

- Add `CrateTypeItem`:

```rust
pub struct CrateTypeItem {
    pub target: NodeId,
    pub qualified_name: String,
    pub display_name: String,
    pub item_kind: ItemKind,
    pub visibility: Option<String>,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
}
```

Modify `crates/rmc-graph/src/graph/query/modules.rs`:

- Extract existing module-tree visibility code into:

```rust
fn declared_item_visibility_map(
    &self,
    rtxn: &RoTxn<'_, heed::WithoutTls>,
    item_ids: &HashSet<NodeId>,
) -> Result<HashMap<NodeId, String>>
```

- Update `module_tree(...)` to call that helper.
- Add:

```rust
pub fn crate_types(
    &self,
    crate_id: NodeId,
    kind_filter: &HashSet<ItemKind>,
    pub_only: bool,
    skip_test_items: bool,
) -> Result<Vec<CrateTypeItem>>
```

Behavior:

- iterate `nodes_by_id`
- include only local `NodeKind::Item` nodes in the requested crate
- include only item kinds in `kind_filter`
- optionally skip `::tests::`
- compute declared visibility via `declared_item_visibility_map`
- if `pub_only`, retain only `visibility == Some("pub")`
- sort by `qualified_name`

Modify `crates/rmc-graph/src/graph/mod.rs`:

- Reexport `CrateTypeItem` from `query::model`.

### Server Changes

Modify `crates/rmc-server/src/tools/params/graph.rs`:

- Add `CrateTypesParams`:
  - `directory: String`
  - `krate: String`
  - `item_kind: Option<Vec<String>>`
  - `pub_only: Option<bool>`
  - `include_associated_types: Option<bool>`
  - `skip_test_items: Option<bool>`
  - flattened `ListPaginationParams`

Modify `crates/rmc-server/src/tools/graph/surface.rs`:

- Import `CrateTypeItem`.
- Add a shared helper equivalent to:

```rust
fn resolve_crate_or_root_module(
    snap: &OpenedSnapshot,
    qualified_name: &str,
) -> Result<NodeId, McpError>
```

- Use it in `functions_with_filter(...)` and the new `crate_types(...)` path
  to reduce duplicate crate/root-module resolution logic.
- Add:

```rust
pub(crate) async fn crate_types(params: CrateTypesParams) -> Result<CallToolResult, McpError>
```

Response shape:

- `krate`
- `type_count`
- flattened pagination metadata
- `types`

Each rendered type:

- `target`
- `qualified_name`
- `display_name`
- `item_kind`
- `visibility`
- `file`
- `span`

Add:

```rust
fn parse_crate_type_kind_filter(
    labels: Option<&[String]>,
    include_associated_types: bool,
) -> Result<HashSet<ItemKind>, McpError>
```

Behavior:

- default kinds: `Struct`, `Enum`, `Union`, `Trait`, `TypeAlias`
- include `AssocType` only when `include_associated_types=true`
- reject non-type item kinds

Modify `crates/rmc-server/src/tools/router.rs`:

- Add a `crate_types` MCP tool method in the hypergraph section.
- Delegate to `crate::tools::graph::surface::crate_types(params).await`.

### Documentation Changes

Modify `TOOLS.md`:

- Add `crate_types` to the tool table.
- Add a `crate_types` section with parameters, example, and response shape.

### Tests

Add graph query tests:

- `crate_types` returns at least known type items for `rmc_graph`.
- `pub_only` filters non-public items.
- `skip_test_items` drops `::tests::`.
- `include_associated_types=false` excludes `AssocType` by default.
- invalid non-type `item_kind` returns `invalid_params` at the server layer.

Add endpoint/router tests if the existing graph test harness supports it:

- `crate_types` endpoint returns JSON with `type_count`.
- summary mode clears `file` and `span`.
- pagination honors `limit` and `offset`.

### Verification

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph crate_types
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server crate_types
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server
```

### Commit

```sh
jj commit -m "graph: add crate types query"
```

## Phase 3: Full Workspace Load For Rename Previews

Source commit: `a35bb621`.

### Goal

Make rename previews load the full RA workspace so renames include downstream
workspace reverse dependencies. Keep fast `no_deps=true` loading for ordinary
definition/reference paths.

### Server Changes

Modify `crates/rmc-server/src/semantic/loader.rs`:

- Keep existing fast loader behavior for `load_project(...)`.
- Add:

```rust
pub(super) fn load_project_full(path: &Path) -> Result<(AnalysisHost, Vfs)>
```

- Implement full loader with:
  - `sysroot: Some(RustLibSource::Discover)`
  - `no_deps: false`
  - `features: CargoFeatures::All`
  - `all_targets: true`
  - `set_test: true`
- Optionally factor common `LoadCargoConfig` creation into a private helper.
- If adding a shared config module, make it crate-local under
  `crates/rmc-server/src/semantic` or `crates/rmc-server/src`; do not force
  `rmc-graph` to depend on server code.

Modify `crates/rmc-server/src/semantic/mod.rs`:

- Add `LoadKind`:

```rust
enum LoadKind {
    Fast,
    Full,
}
```

- Add `load_kind: LoadKind` to `ProjectContext`.
- Change current `get_or_load(...)` into `get_or_load_kind(...)`.
- Keep `get_or_load(...)` as fast.
- Add `get_or_load_full(...)`.
- If a project is already cached as `Fast` and a rename requests `Full`, reload
  it and replace the cached context.
- Make `rename_by_name(...)` call `get_or_load_full(...)`.
- Make Phase 1 `rename_by_position(...)` call `get_or_load_full(...)`.
- Leave `symbol_search` and `find_references_by_name` on fast load.

Modify `crates/rmc-server/src/tools/router.rs`:

- Change `build_hypergraph` description from `no_deps=true` to `no_deps=false`.
  The graph implementation already uses full workspace loading.

### Tests

Add a semantic test in `crates/rmc-server/src/semantic/mod.rs`:

- create a temp workspace with two crates:
  - `engine_sdk` defines a trait `Engine`
  - `engine_consumer` depends on `engine_sdk`, imports `Engine`, implements it,
    and uses it through `dyn Engine`
- call `SemanticService::rename_by_position(...)` on the trait declaration
- assert the preview contains edits in both:
  - `engine_sdk/src/lib.rs`
  - `engine_consumer/src/lib.rs`

Also test or inspect cache behavior:

- a fast load can be upgraded to full when rename is called.
- repeated full rename does not downgrade the cached project.

### Verification

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename_preview_includes_workspace_reverse_dependencies
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server
```

### Execution Status

- [x] Phase start guard: ran `jj show --summary` before editing Phase 3.
- [x] Added `load_project_full(...)` in `rmc-server` semantic loading with
  `sysroot=Discover`, `no_deps=false`, all features, all targets, and test cfg.
- [x] Added `LoadKind` tracking in the semantic cache. Fast contexts are
  upgraded to full for rename; full contexts are not downgraded by later fast
  calls.
- [x] Changed `rename_by_name` and `rename_by_position` to request full
  workspace semantic loading.
- [x] Updated `build_hypergraph` wording in both router docs and `TOOLS.md`
  from `no_deps=true` to the graph implementation's current `no_deps=false`.
- [x] Updated semantic architecture/logic docs for fast vs full load behavior.
- [x] Added and passed
  `rename_preview_includes_workspace_reverse_dependencies`, proving a rename
  preview for a trait declaration includes edits in a downstream workspace
  crate.
- [x] Verified with:
  - `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename_preview_includes_workspace_reverse_dependencies`
  - `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename`
  - `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`
- [x] Result: all Phase 3 checks passed. The commands emitted existing
  dead-code / unreachable-pub warnings and a dirty `../nix-devshells` warning;
  no compile or test failures.

### Commit

```sh
jj commit -m "server: use full workspace load for rename previews"
```

## Phase 4: Integration Verification

### Goal

Verify the three ports work together and that only intended MCP schema changes
occur.

### Tests

Run:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server rename
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server semantic
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server crate_types
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph crate_types
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server -p rmc-graph
```

Optional broader checks if time permits:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
```

### MCP Behavior Checks

After building/running the MCP server with the refactor repo:

1. `rename_symbol` name-only ambiguous case returns candidate positions.
2. `rename_symbol` with `file_path`, `line`, and `column` returns a preview.
3. Partial position params return `invalid_params`.
4. `crate_types` works after `build_hypergraph`.
5. `crate_types(summary=true)` omits `file` and `span`.
6. `crate_types(pub_only=true)` filters to pure public type items.
7. `build_hypergraph` tool description says `no_deps=false`.

### Boundary Checks

Confirm crate layering remains unchanged:

```sh
rg -n "rmc_server|crate::semantic|crate::tools" crates/rmc-graph/src crates/rmc-engine/src crates/rmc-indexing/src
rg -n "rmc_graph|rmc_server|rmc_indexing" crates/rmc-engine/src
```

Expected:

- `rmc-graph` does not depend on `rmc-server`.
- `rmc-engine` does not depend on higher crates.
- server owns MCP parameter schema and routing.
- graph owns snapshot query logic and DTOs.

### Final Commit

If Phase 4 only updates this plan or docs with verification results:

```sh
jj commit -m "docs: record update commit verification"
```

## Recommended Implementation Order

1. Phase 1: position-based rename disambiguation.
   - Required before Phase 3 if the full-workspace rename test uses
     `rename_by_position`.

2. Phase 3: full workspace load for rename previews.
   - Builds directly on Phase 1 and fixes rename completeness.

3. Phase 2: crate types hypergraph query.
   - Independent of rename work; can be done before Phase 3 if desired, but it
     touches graph/server tool surfaces and is best kept separate.

4. Phase 4: integration verification.

## Expected Public MCP Changes

Intentional schema additions:

- `rename_symbol`
  - add optional `file_path`
  - add optional `line`
  - add optional `column`

- new `crate_types` tool
  - `directory`
  - `krate`
  - `item_kind`
  - `pub_only`
  - `include_associated_types`
  - `skip_test_items`
  - `limit`
  - `offset`
  - `summary`

Intentional behavior changes:

- ambiguous rename errors become actionable with candidate positions
- rename previews load the full workspace and include downstream workspace
  references
- `crate_types` exposes graph-owned type inventory
- `build_hypergraph` description reflects current `no_deps=false` behavior

Non-goals:

- Do not apply rename previews to disk.
- Do not introduce formatting-only churn.
- Do not move graph query ownership into server.
- Do not add a server dependency to `rmc-graph`.
- Do not directly cherry-pick the old commits without adapting paths.
