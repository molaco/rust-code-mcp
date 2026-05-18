# Duplication Consolidation Plan Report

Date: 2026-05-18
Plan: `.plans/dup-plan.md`
Status: complete

## Scope

Implemented the full duplication-consolidation plan for the `rust_code_mcp`
crate. The work removed verified private helper clones, kept rejected
lookalikes separate, and preserved the existing public surface.

The plan was executed phase by phase. Before each step, `jj show --summary` was
run. After each step, `.plans/dup-plan.md` was updated and the step was
committed with `jj commit -m ...`.

## Commits

- `b38906d1` - `graph: add audit_util.rs, dedupe HIR/syntax audit helpers`
- `6036ab5b` - `graph: add labels.rs, unify label/path helpers`
- `6ff8f55a` - `graph: define_id macro for ids.rs`
- `eec6caf5` - `tools: centralize data_dir dir_hash embedder identity`
- `1e5960c1` - `tools: unify embedding backend resolution`
- `91bdc91b` - `embeddings: dedupe arc and batch planner`
- `7df3ed48` - `tools: collapse duplicate DTO structs`
- `8dab6f6e` - `parser: dedupe line_of_offset helper`
- `d60a5e06` - `visibility: narrow new helper modules`

## Implementation

### Zone A: graph audit helpers

Added `src/graph/audit_util.rs` and moved the repeated audit helpers there:

- `canonical_function_path`
- `resolve_workspace_relative`
- `enclosed_by_cfg_test`
- `item_has_cfg_test`
- `resolve_enclosing_function`

The module is scoped to `pub(in crate::graph)`. It is used by the channel,
function-body, unsafe, impl, usage, and extraction audits. This resolved the
C1, C4, C6, and C16 clusters without changing audit behavior.

### Zone B: graph label and path helpers

Added `src/graph/labels.rs` for the shared label/path vocabulary used by
`graph` and `tools`:

- `usage_category_label`
- `binding_kind_label`
- `node_kind_label`
- item-kind formatting helpers
- `crate_display_name`
- `module_path_segments`

The home is `graph`, so the dependency direction remains `tools -> graph`.
This resolved C3, C5, C8, and C18 while keeping intentionally different
vocabularies separate.

### Zone C: tools path, hash, and backend helpers

Centralized the tools-local path and embedder metadata logic in
`src/tools/project_paths.rs`:

- `data_dir`
- `compute_dir_hash`
- `read_embedder_identity`
- `write_embedder_identity`
- `resolve_embedding_backend`

The shared backend resolver keeps the `index_tool` legacy `model` fallback as a
thin layer above the common path. `config.rs` and `graph/storage.rs` were left
untouched because their directory defaults are intentionally different.

The existing public `indexing_tools::data_dir` path remains as a wrapper, so no
public caller path was removed.

### IDs

Replaced the repeated `NodeId`, `BindingId`, and `UsageId` method bodies with a
single `define_id!` macro in `src/graph/ids.rs`. This removed the repeated
`from_components`, `to_hex`, and `as_bytes` implementations while preserving
the concrete newtype names and APIs.

### Embeddings

Added two embeddings-local helpers:

- `src/embeddings/util.rs` with the shared `arc` helper.
- `src/embeddings/batching.rs` with a generic `BatchPlan` and `plan_batches`.

`openrouter` and `indexing::embedding_batcher` now share the same greedy batch
planner while preserving their input/output adapters. The helper lives in
`embeddings`, which keeps the legal dependency direction `indexing ->
embeddings`.

### Tool DTOs and summaries

Collapsed duplicated DTO shapes in `tools::graph_tools`:

- `ForbiddenDependencyRuleParam` is now an alias of
  `graph::queries::ForbiddenDependencyRule`, with schema metadata moved to the
  graph type.
- `CallersInCrateResponse` was removed in favor of `CallSitesResponse` with an
  optional `krate` field.

The repeated `summary` location-stripping blocks in list-shaped graph tools
were centralized with `clear_locations_for_summary`.

### Parser line offsets

Moved the shared `line_of_offset` helper to `parser/mod.rs` with
`pub(in crate::parser)` visibility. `parser/type_references.rs` imports that
helper. The graph codemap `line_of_byte` implementation remains separate
because it uses a different line-table algorithm.

### Visibility follow-up

The final code commit tightened the new helper surfaces:

- `graph::audit_util` is `pub(in crate::graph)`.
- `graph::labels` is `pub(crate)`.
- `tools::project_paths::data_dir` is `pub(in crate::tools)`.
- `indexing_tools::data_dir` remains as the existing public wrapper.

This matches the plan guardrail of adding the narrowest practical visibility
while preserving public paths.

## Verification

All build checks were run through the project devshell:

```text
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

`cargo check --all-targets` passed after every implementation commit. Existing
workspace warnings remain, but no new build errors were introduced. No
formatting command was run.

Final MCP verification used the requested OpenRouter Qwen3 8B embedding
profile, `openrouter-qwen3-8b`.

Rebuilt hypergraph:

```text
graph_id: 39f296bbda88732ac8a306a8e9d687c1
fingerprint: 6f09a69743a67f689232c70b4c155a9b3bb4136c3d96e78de5553f65b40af180
node_count: 2973
binding_count: 5056
usage_count: 7935
reused: false
```

Semantic overlap checks:

- Exact-clone pass: `semantic_overlaps` over functions at threshold `0.99`
  returned `0` pairs and `0` clusters.
- Broad function pass: threshold `0.85` returned `88` pairs and `47` clusters,
  down from the plan baseline of `127` pairs and `59` clusters.

Import and usage audits confirmed the new helper consumers stayed inside the
intended boundaries:

- `graph::audit_util::resolve_workspace_relative` is imported only by graph
  audit/extraction modules and their tests.
- `graph::labels::usage_category_label` is imported by
  `tools::graph_tools`, `graph::queries`, and related tests.
- `tools::project_paths::data_dir` is imported only by tools-local modules and
  tests; `indexing_tools` uses the wrapper path.
- `tools::project_paths::resolve_embedding_backend` is used by
  `tools::graph_tools`, `tools::index_tool`, and `tools::query_tools`.
- `embeddings::batching::plan_batches` is used by
  `embeddings::openrouter` and `indexing::embedding_batcher`.
- `parser::line_of_offset` is used by `parser` and
  `parser::type_references`.
- `embeddings::util::arc` is used by the embeddings backend/profile registry
  code.

## Guardrails

- No public MCP tool surface was intentionally changed.
- No formatter was run.
- `vendor/` was not touched.
- Rejected non-duplicates from the plan, including C9, C12, and C17, were left
  separate.
- Intentional adapter/facade twins remain out of scope.

## Result

The actionable private-helper duplicate clusters from the plan are resolved, or
resolved to the boundary allowed by dependency direction and existing public
paths. The remaining function-level semantic clusters at threshold `0.85` are
the expected non-goals: adapter twins, facade delegations, parser variants,
endpoint wrappers, and same-shape helpers with different behavior.
