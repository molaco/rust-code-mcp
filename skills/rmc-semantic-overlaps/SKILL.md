---
name: rmc-semantic-overlaps
description: Find duplicate Rust logic by similarity.
argument-hint: "[mode] [target-or-query]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust semantic similarity analysis

Find duplicate or near-duplicate logic that's named differently, split
across crates, or simply pasted in two places. Three vector-backed tools
cover three different question shapes; pick the right one before reaching
for any of them.

For name-equality collisions (different bodies, same name), use
`rmc-type-overlaps`. For refactor planning around the duplicates, hand off
to `rmc-refactor-plan` (Recipe 10).

## Scope — function bodies, type bodies, and Item-level semantic search

## Three tools, three jobs

| Tool | Seed | Scope | When to use |
|---|---|---|---|
| `get_similar_code(query)` | Free-text NL ("function that parses JSON") | Chunk-level matches | You don't know the symbol or its name |
| `similar_to_item(target, ...)` | A qualified-name Item | Item-level vector neighbors | You have ONE Item and want neighbors. ~100-300ms |
| `semantic_overlaps(directory, ...)` | No seed — workspace-wide | Pairs or single-linkage clusters | Audit / refactor planning. Item embeddings cached per-snapshot in LMDB |

`get_similar_code` is chunk-level, no Item awareness. `similar_to_item`
resolves the seed via hypergraph, reads source, runs vector-only search.
`semantic_overlaps` embeds every Item in scope, runs pairwise cosine,
returns deduplicated `pairs` or single-linkage `clusters`.

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
index_codebase(directory=<absolute-path>)   # required for get_similar_code, similar_to_item
```

`semantic_overlaps` v1.1 embeds Item source directly and caches in the
snapshot's LMDB env — it does NOT require a fresh `index_codebase`.

## Workflow

### Step 1. Pick the tier

```
Do you have a qualified name?
├── No → get_similar_code(query=<free text>)
└── Yes
    ├── You want neighbors of one Item → similar_to_item(target=Y)
    └── You want a workspace-wide audit → semantic_overlaps(directory=...)
```

### Step 2. Run the appropriate tool

Free-text:

```
get_similar_code(directory=..., query=<description>, limit=10)
```

Returns ranked chunk previews with `symbol_name`, `symbol_kind`, `file`,
`line_start/end`. Bridge to a qualified name via `find_definition` (use
`rmc-find-symbol` if needed).

Single-seed:

```
similar_to_item(directory=..., target=<qualified_name>, limit=10,
                threshold=0.80, item_kind="Function")
```

Returns ranked vector matches above `threshold`. Self-match dropped via
line-range overlap. Start permissive (0.80) and tighten.

Workspace audit:

```
semantic_overlaps(directory=..., crate_name=<optional>, item_kind=<optional>,
                  threshold=0.85, output_mode="clusters", max_pairs=50,
                  max_cluster_size=15, skip_test_chunks=true,
                  cross_crate_only=false)
```

Returns `pairs` or `clusters` sorted by `avg_similarity` desc. Test
fixtures dropped by default.

### Step 3. Verify

Vector similarity is necessary but not sufficient. For each candidate:

```
who_uses_summary(directory=..., target=<qualified_name>)
read_file_content(file_path=...)
```

Filters out dead candidates, quantifies migration cost. Inspect manually —
embeddings encode lexical+syntactic patterns more than logical intent.

## Recipes

### Recipe — "Find duplicate logic worth extracting"

```
semantic_overlaps(directory=..., crate_name=X, item_kind="Function", threshold=0.80)
```

Crate-scoped scans tolerate a lower threshold (smaller item count bounds
chaining). For each top cluster:

1. Read `members` — qualified names, files, spans, `avg_similarity`.
2. `who_uses_summary` per member to verify they're called and plan
   migration order.
3. Cross-reference with `analyze_complexity` to score by similarity ×
   complexity × blast radius.

### Recipe — "Type-1 clone detection (literal duplicates)"

```
semantic_overlaps(directory=..., threshold=0.95)
```

`threshold=0.95` plus the v1.1c content-hash short-circuit (identical
source bytes get `similarity = 1.0` directly, no cosine call) surfaces
literal duplicates.

Real example: unit `Error` variant duplicated in 6 different error enums
(`ToolResultKind::Error`, `StopReason::Error`, ...) — all collapse to a
single 1.0-similarity cluster.

### Recipe — "Convergent enum design"

```
semantic_overlaps(directory=..., item_kind="EnumVariant", threshold=0.95)
```

Variants whose source hash identically get clustered. Catches the case
where the same logical state (`Idle` / `Done` / `Error` / `Pending`)
was modeled as separate variants on different enums.

Cross-link with `rmc-enum-variants` for per-variant fan-in.

### Recipe — "Same-shape struct detection across crates"

```
semantic_overlaps(directory=..., item_kind="Struct",
                  cross_crate_only=true, threshold=0.85)
```

`cross_crate_only=true` drops same-crate pairs (~76% of pairs in measured
workspaces). The remaining clusters are structurally similar structs
living in different crates.

Real example: `TokenUsage` defined in 3 crates (HTTP-client,
chat-completion, token-budget), all carrying `prompt_tokens: u32`,
`completion_tokens: u32`, `total_tokens: u32` — collapses to one
extraction candidate.

### Recipe — "Refactor candidate ranking"

```
semantic_overlaps(directory=..., crate_name=X)
```

Clusters sorted by `avg_similarity` desc. Combine with `analyze_complexity`
+ `who_uses_summary` and score by `avg_similarity × complexity × fan_in`.
Use `output_mode="pairs"` for migration planning (one pair = one decision).

### Recipe — "Naming-convention enforcement"

```
semantic_overlaps(directory=..., cross_crate_only=true, threshold=0.85)
```

Cross-crate clusters whose members carry different names but similar source
signal a naming inconsistency: `now_ms` vs `now_ts` vs `unix_now_secs`.
Rename to a single convention before extracting.

## Decision frames

| Situation | Tier / parameters |
|---|---|
| Don't know the symbol's name | `get_similar_code` |
| Have one symbol, want neighbors | `similar_to_item(target=Y, threshold=0.80)` |
| Workspace audit | `semantic_overlaps(directory=...)` |
| Crate-scoped scan | `semantic_overlaps(crate_name=X, threshold=0.80)` |
| Workspace-wide scan | `semantic_overlaps(directory=..., threshold=0.85)` (0.80 produces useless mega-clusters via chaining) |
| "Is anything duplicated literally?" | `semantic_overlaps(threshold=0.95)` |
| "Duplicated across crate boundaries?" | `semantic_overlaps(cross_crate_only=true)` |
| Raw edges for migration planning | `output_mode="pairs"` |
| Grouped signal for extraction planning | `output_mode="clusters"` (default) |

## Pattern reference

| Pattern | Invocation |
|---|---|
| Crate audit | `semantic_overlaps(crate_name=X, item_kind="Function", threshold=0.80)` |
| Workspace audit | `semantic_overlaps(directory=..., threshold=0.85)` |
| Type-1 clones | `semantic_overlaps(threshold=0.95)` |
| Cross-crate structs | `semantic_overlaps(item_kind="Struct", cross_crate_only=true)` |
| Variant convergence | `semantic_overlaps(item_kind="EnumVariant", threshold=0.95)` |
| Single-seed lookup | `similar_to_item(target=Y, threshold=0.80)` |
| Free-text needle | `get_similar_code(query="function that parses JSON")` |

## Output format

```
Mode: <free-text | single-seed | workspace-audit>
Tool: <get_similar_code | similar_to_item | semantic_overlaps>
Clusters/pairs returned: <n>
Top by avg_similarity:
  1. <member1>, <member2>, ... (sim <s>)
  2. ...
Verified call sites (who_uses_summary): <n live / k dead>
Recommended extractions: <top-3 with rationale>
```

## Limitations

- Single-linkage clustering can chain through outliers — one bridging pair
  pulls two distant clusters together. `max_cluster_size=15` drops the
  worst; bump or disable to inspect. Tightening `threshold` is the
  principled fix.
- Embedder is `fastembed:all-MiniLM-L6-v2:dim384:v1`. Cache key is
  `(NodeId, content_hash, embedder_version)` — switching the model
  invalidates every cached entry.
- First-scan latency is seconds-to-minutes at workspace scale; subsequent
  scans on unchanged code are near-instant.
- Embeddings encode lexical+syntactic more than logical intent. High
  cosine on two similarly-shaped fns can hide diverged behavior — always
  verify with `read_file_content`.
- `semantic_overlaps` does NOT subsume `rmc-type-overlaps`. Name-equality
  collisions and content-similar clusters are complementary signals.
- Single-linkage only — no HDBSCAN / k-means / density-based variants.
  No streaming partial results.
