# G9 — Semantic overlaps + similarity (review report)

Reviewed commits (chronological):

| Short hash | Title | LOC |
|---|---|---|
| `625054f7` | add dup-tool plan document | 295 |
| `22cfdcf8` | add similar_to_item semantic neighbors tool | 277 |
| `1d97572f` | add semantic_overlaps tool for workspace-wide semantic duplicate audit | 776 |
| `6adbcab7` | add max_cluster_size cap and tune defaults for semantic_overlaps clustering | 61 |
| `7aeef8b2` | add embedding cache for semantic_overlaps with per-item LMDB persistence | 473 |

---

## 1. Group summary

The group ships two complementary tools:

- `similar_to_item` (v0.1, 22cfdcf8): single-seed semantic neighbors. Resolves a qualified-name Item → `(file, span)`, reads the source bytes, runs `vector_only_search` (LanceDB-backed `fastembed:all-MiniLM-L6-v2`), drops self-match by `Path::ends_with` + line-range overlap, returns top-K filtered by `threshold` / `item_kind`.
- `semantic_overlaps` (v1.0 → v1.1, 1d97572f → 7aeef8b2): workspace-wide audit. v1.0 reused `vector_only_search` per seed and mapped chunks back to Items via line-overlap; v1.1 ditched the LanceDB roundtrip in favor of embedding Item source directly and doing in-memory pairwise cosine, with a persistent per-Item embedding cache in a new LMDB sub-DB (`embeddings_by_target`, schema v10 → v11).

6adbcab7 sits in the middle: empirical tuning (default threshold 0.80 → 0.85, new `max_cluster_size=15` cap to truncate noisy single-linkage chains, cluster sort by `avg_similarity` rather than size).

Plan (625054f7) is detailed and the implementation tracks it closely; v1.1c (identical-source short-circuit) and v1.1a (embedding cache with schema bump) were both shipped; v1.1b (batch queries) is partially absorbed into "we own the embedder now, embed in chunks of 64." v1.1d (k-means pre-cluster) was correctly skipped.

---

## 2. Per-commit review

### 625054f7 — plan document — 295 LOC

**What it does.** Adds `.plans/dup-tool-plan.md`. Lays out the v1.0 / v1.1 split, parameter shape, algorithm (enumerate seeds → vector search per seed → union-find clustering), research citations for threshold defaults (CodeBERT 0.95, general 0.80, etc.), explicit risk register (single-linkage chaining, embedder version invalidation, test-fixture noise), and the schema-bump plan for v1.1a.

**Issues.** None of substance. Two minor:
- The plan's "v1.1 layer in batch queries" never quite happens — the implementation ditched LanceDB altogether and went straight to in-memory cosine over a per-Item embedding cache, which is a more aggressive refactor than the plan described. This is documented in the v1.1 docstring but the plan was never updated to reflect the final design.
- The plan's "Tests" section says only union-find + chunk-to-Item-mapper are tested; the implemented `resolve_chunk_to_item` actually isn't unit-tested (deferred as "needs LMDB fixture"), only `line_range_overlaps` + `build_clusters` are. Acceptable but worth noting.

**Severity.** Info only.

**Verdict.** Clean, useful planning doc. PASS.

---

### 22cfdcf8 — `similar_to_item` — 277 LOC

**What it does.** Implements the v0.1 seed-driven semantic-neighbor tool. Pipeline:
1. `lookup_by_qualified_name(target)` → `(NodeId, Node)`.
2. Read `(file, span)` from node, slice `content[start..end]` (returns `invalid_params` on UTF-8 split / OOB).
3. `vector_only_search(seed_source, limit+1)` against the LanceDB store.
4. Filter: drop chunks whose `file_path.ends_with(seed_rel_path)` AND whose line range overlaps the seed's byte-span-derived line range; drop `score < threshold`; drop kind mismatch.
5. Build preview from `chunk.content.lines().take(3)` and return.

**Issues.**

- *Embedding source*: query is the **seed's raw source bytes** (`content[start..end]`, not trimmed in this tool — note divergence from v1.1 which `trim()`s). Comments and attributes inside the span are included.
- *Determinism*: fastembed `all-MiniLM-L6-v2` runs on CPU and is deterministic given identical input bytes (no temperature / sampling). ✓
- *Self-match logic* (good): Path-component suffix match (`ends_with`) handles abs-vs-rel mismatch between vector store (absolute) and hypergraph (workspace-relative). The line-range overlap check correctly avoids over-filtering — items in the same file as the seed are preserved unless they overlap the seed's span. The docstring's "Limitation: self-match detection is file-path-only" comment is **stale / misleading** — the code does line-range overlap.
- *`limit_plus_one` (minor)*: the code asks LanceDB for `limit + 1` to account for the self-match being dropped, but the loop early-exits with `if matches.len() >= limit { break }`. If the dropped self-match is not at rank ≤ K, the user may get fewer than `limit` matches even though K+1 results were available. Could over-request a larger margin (e.g. `limit * 2` clamped). Minor.
- *No threshold validation*: a caller-supplied `threshold = 2.0` would silently produce zero matches. Probably fine; the param is documented as 0.0–1.0.
- *Preview is line-count-based*, not byte-truncated — fine in practice.

**Severity.** Minor (stale docstring comment + tight `limit+1` margin).

**Verdict.** Solid v0.1. PASS with a one-line doc-comment fix recommended.

---

### 1d97572f — `semantic_overlaps` v1.0 — 776 LOC

**What it does.** Workspace-wide audit:
1. Open snapshot, resolve optional `crate_name` to `NodeId` (handles Crate or root Module, errors on other kinds).
2. Iterate `nodes_by_id`, keep `NodeKind::Item` matching `crate_id` / `item_kind`, drop synthetic items (no `file`/`span`), drop `::tests::` paths (default on).
3. For each seed, read source from `file_cache` (per-file), `trim()`, `vector_only_search(seed_source, 20)`. For each result chunk above `threshold`:
   - File-path suffix match + line-range overlap → self-match drop.
   - `resolve_chunk_to_item(snap, chunk_file, chunk_ls, chunk_le, &mut file_cache)` → maps the chunk back to an Item by iterating `nodes_by_id` (!) and finding the first Item whose byte-span-derived line range overlaps.
   - Apply `cross_crate_only` / `skip_tests` filters, insert into `edges: HashMap<(NodeId, NodeId), Vec<f32>>` canonicalized smaller-id-first.
4. Average per-pair scores, sort descending, build pairs OR run union-find for clusters mode.
5. `build_clusters` over `(a,b,score)` edges: union-find with path compression, drop singletons, cap members at `max_pairs`, set `truncated`.

**Issues.**

- **(MAJOR) `resolve_chunk_to_item` is O(N) per chunk.** Inside the inner loop, it iterates all nodes again to find the Item covering a chunk's lines. With ~M chunks per seed and N seeds, this is O(N × K × N_nodes) — bad asymptotic. Plan estimated 3–6 minutes; this is plausibly the dominant cost on big workspaces.
- **(MAJOR) Holds an LMDB read txn while doing async `vector_only_search` awaits.** The `rtxn` in `resolve_chunk_to_item` is opened per call (`snap.env.read_txn().ok()?`) which is OK, but the *outer* seed-enumeration `rtxn` is closed before the await loop (good). However, `resolve_chunk_to_item` opens a fresh `rtxn` per chunk — sub-optimal but correct. No actual cross-await holding.
- **(MINOR) `cross_crate_only` semantics**: the filter compares `other_node.crate_id == seed_node.crate_id`. If either side is a `Module` directly (defensive) it could behave oddly, but Items always have a `crate_id` so this is fine.
- **(MINOR) Self-match line check ignores duplicate-name overloads.** Two unrelated Items at the same byte span (impossible in practice) would share a line range; harmless.
- **(MINOR) The planned `spawn_blocking` wrapper was dropped.** The docstring explains why (`vector_only_search` is async, can't reenter the runtime); reasonable. The bulk of cost is LanceDB I/O which yields. Acceptable divergence from the plan.
- **(MINOR) Cluster member-cap behavior at the cap**: capped via `Vec::take(max_members)` over `group.into_iter()` — **the surviving members are an arbitrary subset** (HashMap iteration order over root groups → unspecified). No tie-breaker. The plan didn't specify either. Should at least sort by NodeId for stable output or by a deterministic key for diffability. (Severity bumped because semantic_overlaps results otherwise should be deterministic.)
- **(MINOR) Threshold default 0.80 produces large noisy clusters** — flagged and fixed in 6adbcab7.
- **(INFO) `EdgeKind` ordering compares NodeId bytes directly (32-byte BLAKE-ish hashes)**; canonical and stable. ✓

**Severity.** Major for performance (`resolve_chunk_to_item` quadratic) but tractable for crate-scoped scans; minor for cluster cap determinism.

**Verdict.** Functionally correct v1.0. PASS with a perf concern that v1.1 then largely sidesteps by ditching `resolve_chunk_to_item` from the hot path.

---

### 6adbcab7 — defaults + `max_cluster_size` cap — 61 LOC

**What it does.**
- New optional param `max_cluster_size: Option<usize>` (default 15, 0 = disabled). Applied **after** `build_clusters` via `clusters.retain(|c| c.size <= max_cluster_size)`.
- Threshold default bumped 0.80 → 0.85 (in `threshold.unwrap_or(0.85)`).
- Cluster sort order changed: was `size desc → min_similarity desc`; now `avg_similarity desc → size desc → min_similarity desc`. High-signal small clusters surface first.
- Updates the existing test (`build_clusters_two_groups_drops_singletons`) to match the new sort order. Other tests unchanged.

**Issues.**

- **(MINOR) `retain` happens after `truncated` is set on clusters.** A cluster with `size=16, max_cluster_size=15` is dropped entirely — never seen — whereas a cluster with `size=15` survives with possibly-truncated members. That's the documented intent ("drop chained mega-clusters") but interaction with `max_pairs` (still default 50, also doubles as per-cluster member cap) is now confusing:
  - `max_pairs=50` caps cluster *members* (set `truncated=true` if `size>50`).
  - `max_cluster_size=15` drops the cluster entirely (no `truncated` distinction).
  - With defaults, `max_cluster_size=15 < max_pairs=50`, so `max_pairs` truncation is **unreachable for clusters** unless `max_cluster_size` is disabled. The two knobs overlap awkwardly. Worth a docstring note.
- **(MINOR) TOOLS.md note still references "Default 0.80"** in one paragraph while the param table says 0.85 — partially fixed but inconsistent. (The "Notes from validation" section actually clarifies this.)
- **(INFO) JJ diff display artifacts**: the raw diff appears to show `0.8085` and `clusters[01]` — these are jj-side word-merging of "0.80"+"0.85" and "0"+"1". The actual file at this revision has `0.85` and `clusters[1]` (verified via `jj file show`). No code bug.

**Severity.** Minor.

**Verdict.** Empirical tuning, well-motivated. PASS.

---

### 7aeef8b2 — embedding cache (schema v10 → v11) — 473 LOC

**What it does.**
- Adds `EmbeddingRecord { content_hash: [u8; 16], vector: Vec<f32>, embedder_version: String, generated_at_unix: u64 }` to `graph::model`.
- New LMDB sub-DB `embeddings_by_target: Database<Bytes, SerdeBincode<EmbeddingRecord>>`, registered in `GraphDatabases::open_databases` and `open_existing`.
- `SCHEMA_VERSION: 10 → 11` — old snapshots auto-rebuild because `graph_id_for` hashes `SCHEMA_VERSION`.
- Rewrites `semantic_overlaps` to:
  1. Enumerate seeds (unchanged).
  2. First pass (read-only txn): for each seed, read+trim source, `SHA-256(source)[..16]` → `content_hash`, lookup `embeddings_by_target[seed_id]`. Hit iff `content_hash` AND `embedder_version == "fastembed:all-MiniLM-L6-v2:dim384:v1"` match.
  3. Batch-embed misses via `EmbeddingGenerator::embed_batch_async(texts)` in chunks of 64, persist each new vector under a single write txn (`wtxn.commit()` at end).
  4. v1.1c: items sharing a `content_hash` get a direct `score=1.0` edge (skipping cosine).
  5. v1.1a': in-memory pairwise cosine over `seeds_ctx[i].cached_vec × seeds_ctx[j].cached_vec` for non-identical hashes. `cosine` handles zero-norm → 0.0 (no NaN), unequal lengths silently truncated via `zip`.
- Dead code: `resolve_chunk_to_item` + `line_range_overlaps` retained with `#[allow(dead_code)]` for future use.
- Drops `vector_only_search` / hybrid_search initialization (no LanceDB roundtrip on this path anymore). Tool docs updated: "index_codebase no longer required for this tool".

**Issues.**

- **(MEDIUM) Write transaction holds across embedder `.await`.** A single `wtxn` is opened **before** the batch loop and committed **after** all chunks complete. `embed_batch_async` runs the fastembed model on a `spawn_blocking` worker per chunk; meanwhile the wtxn pins the LMDB writer. LMDB has only one writer at a time, so any *concurrent* `semantic_overlaps` call (or `build_hypergraph` write) on the same snapshot will block until the entire embedding pass finishes (potentially minutes). Heed 0.22 `WithoutTls` allows the `RwTxn` to be `Send` across `.await`, so this **compiles and runs**, but the lock-hold is long. Mitigation: open `wtxn` once per chunk (or batch commits every N chunks), accepting some throughput cost for predictability. Not a correctness bug but a denial-of-service vector under concurrent use.
- **(MEDIUM) `clear_cache` does NOT clear `embeddings_by_target`.** At this commit, `clear_cache_tool.rs` only removes the metadata-cache / tantivy / vector_store directories (under `data_dir()`), but the hypergraph LMDB env lives elsewhere (snapshot dir) and contains the new sub-DB. The docstring claims `build_hypergraph --force_rebuild` is the way to invalidate — that's true (new graph_id → new env directory) — but `clear_cache` is the natural user-facing escape hatch and silently does nothing for embeddings. (Later commit `f8a7378` "clear_cache hypergraph wipe" addresses this — outside G9 scope.)
- **(MINOR) `embedder_version` string is the sole signal for model swaps.** `EMBEDDING_DIM` is **not** included in the cache record nor cross-checked at read time. The string `"fastembed:all-MiniLM-L6-v2:dim384:v1"` literal does pin the dimension via its substring, but if a future change swaps the model and forgets to bump the version literal, you'd get a silent dimension mismatch → `cosine` truncates via `zip` to the shorter length → **wrong scores, no error**. A defensive `if rec.vector.len() != EMBEDDING_DIM { miss }` check would harden this. Severity Minor because today there's only one model.
- **(MINOR) `cosine` silently truncates unequal-length inputs.** Same root cause as above. Even within one model run, defensive-program practice would assert lengths match.
- **(MINOR) Pairwise cosine is O(N²) on 384-dim vectors.** Plan estimated this is "comfortable for a few thousand items"; at 5k items that's 12.5M dot products × 384 muladds = ~5G FLOPs — single-threaded, ~5–15s. Reasonable. For 10k+ items this gets unpleasant. v1.1d pre-cluster wasn't built; documented as future work.
- **(MINOR) `EmbeddingGenerator::new()` is invoked inside the tool body** (after the misses-pass, only if `!miss_texts.is_empty()`). Model init can take a couple of seconds (downloading on first use). Acceptable lazy strategy.
- **(MINOR) `generated_at_unix` is stored but never read.** Useful for forensics; not load-bearing. Note that it's set even on cache hits' new-write path is never taken (hits skip the write entirely) — fine.
- **(INFO) `embed_batch_async` invocation pattern is correct.** `spawn_blocking` inside the embedder shields the runtime; the outer tool stays async-friendly.
- **(INFO) Identical-hash short-circuit (v1.1c) correctly groups by `content_hash` keys** and emits direct `score=1.0` edges *post* cache-fill (predicate `ctx.cached_vec.is_some()`), so it ignores items that failed embedding. Good.
- **(INFO) New sub-DB requires `DEFAULT_MAX_DBS` headroom.** Plan said 13 → 14; current is 11 named DBs (rough count), well under the `16` cap. Fine.

**Severity.** Two medium issues (wtxn hold across await; `clear_cache` blind spot). Both are operational/concurrency concerns, not correctness regressions for a single user.

**Verdict.** Substantively complete v1.1. PASS with two recommended hardenings (chunked commits; defensive dim check). The `clear_cache` blind spot is a real footgun until fixed in the next commit.

---

## 3. Cross-commit observations

### Plan → implementation fidelity (625054f7 → 1d97572f → 7aeef8b2)

| Plan element | v1.0 (1d97572f) | v1.1 (7aeef8b2) |
|---|---|---|
| `SemanticOverlapsParams` shape | shipped as planned | + `max_cluster_size` (6adbcab7) |
| Algorithm step 1–4 (enumerate / vector search per seed / chunk → Item) | implemented | replaced by direct-embed + pairwise cosine |
| Step 5 symmetric dedup, avg scores | implemented | unchanged |
| Step 6 pairs / step 7 union-find clusters | implemented | unchanged |
| `spawn_blocking` wrapper | dropped (justified in docstring) | still dropped |
| Default threshold 0.80 | shipped at 0.80 | bumped to 0.85 (6adbcab7) |
| Default `max_pairs=50`, clusters output | shipped as planned | unchanged |
| `skip_test_chunks=true`, `cross_crate_only=false` defaults | shipped as planned | unchanged |
| v1.1a embedding cache (NodeId → EmbeddingRecord) | n/a | shipped, schema v10→v11 ✓ |
| v1.1b batch queries (single LanceDB batch) | n/a | **replaced** by direct embed + in-memory cosine (more aggressive; bypasses LanceDB entirely) |
| v1.1c skip identical-source | n/a | shipped (score=1.0 short-circuit) |
| v1.1d k-means pre-cluster | n/a | correctly deferred |
| Unit tests | clusterer + chunk-to-Item | clusterer + line_range_overlaps + `cosine_basic_identities` (added); chunk-to-Item still deferred |

The v1.1 *design* diverges from the plan in one important way: the plan envisioned the cache as a speedup on top of `vector_only_search`, with LanceDB still in the pipeline. The implementation **removed LanceDB from this tool entirely**, owning the embedding model directly. This is a better design (avoids the chunk → Item lossy mapping, decouples this tool from `index_codebase`) and the docstring documents it, but the plan was never amended. Reviewers reading the plan first will be slightly confused.

### Cache invalidation across commits

Layers of invalidation, by priority:

1. **`SCHEMA_VERSION` hashed into `graph_id_for`** (storage.rs). Bumping `SCHEMA_VERSION: 10 → 11` in 7aeef8b2 means the *entire* LMDB env path changes for every workspace → effectively the cache starts empty post-upgrade. Correct.
2. **`embedder_version` literal `"fastembed:all-MiniLM-L6-v2:dim384:v1"` checked at every cache read.** Mismatch → miss → re-embed. Correct, but the literal lives inside `semantic_overlaps` rather than in the embedder module — drift risk.
3. **`content_hash = SHA-256(trimmed source)[..16]`.** If the Item's bytes change (rename, body edit, attr add inside span, comment edit inside span), the hash flips → miss → re-embed. Correct, with two subtleties:
   - The hash is over the **trimmed** source. Whitespace-only edits before/after the span are absorbed by trim — but rust-analyzer's `(file, span)` always covers the Item from its first to last byte, so leading/trailing whitespace inside the span shouldn't normally occur. Negligible.
   - `content_hash` does NOT include the qualified name or kind. Two Items with byte-identical source but different qualified names (e.g. two `pub fn new() -> Self {}` in different impl blocks) get the same hash → they collide in the v1.1c short-circuit and edge each other with `score=1.0`. That's the *desired* behavior (find duplicates), so this is correct.
4. **`build_hypergraph --force_rebuild` clears it.** Forces a new fingerprint (the `compute_fingerprint` walks the workspace tree) → new `graph_id` → new env. Indirect but reliable.
5. **`clear_cache` does NOT clear it (at 7aeef8b2).** Footgun, fixed in a later out-of-scope commit.

A failure mode the design doesn't catch: if an Item's source moves to a new file (rename `src/foo.rs` → `src/bar.rs`), the `NodeId` is computed from the qualified name path, not the file path — so `NodeId` is stable, `content_hash` is stable, cache reuses the old vector. Correct ✓. If only the qualified name changes (rename a fn), `NodeId` changes, the old entry orphans (never read again), the new entry gets embedded fresh. Orphans accumulate over time — there's no GC sweep. Minor; LMDB writes are cheap and the env is rebuilt on every schema bump.

### Embedding source consistency between `similar_to_item` and `semantic_overlaps`

- `similar_to_item` (22cfdcf8) uses `content[start..end]` **un-trimmed** as the vector_only_search query.
- `semantic_overlaps` v1.0 (1d97572f) uses `seed_source_slice.trim()` then runs `vector_only_search`.
- `semantic_overlaps` v1.1 (7aeef8b2) uses `trimmed.as_bytes()` for both the cache hash and the embedding text.

The trimming divergence means a `similar_to_item(target=X)` call and a `semantic_overlaps` scan looking at `X` would query the embedder with *different* byte sequences and could get marginally different scores. Minor — fastembed is fairly robust to whitespace — but worth flagging for future investigations into "why does the audit miss things the seed-query tool finds?".

---

## 4. Overall verdict

**PASS with two MINOR hardenings recommended.**

The group is well-planned, well-tested where testable, and ships a coherent semantic-duplicate-detection feature with a sensible v1.0 → v1.1 progression. The schema bump is handled correctly via `graph_id_for(SCHEMA_VERSION)`. Defaults are tuned by empirical validation (6adbcab7's notes section). The cache design is sound: keyed on `(NodeId, content_hash, embedder_version)`, all three sources of invalidation work.

Issues worth addressing:

1. **MEDIUM** — `clear_cache` blind spot for `embeddings_by_target` (at 7aeef8b2; fixed in the immediately-following `f8a7378` "clear_cache hypergraph wipe", which is outside G9 scope but resolves it).
2. **MEDIUM** — `wtxn` is held across all `embed_batch_async` awaits, serializing concurrent semantic_overlaps callers on the embedding model. Recommend per-chunk commits or chunked sub-transactions for production deployments expecting concurrent use.
3. **MINOR** — `cosine` silently truncates unequal-length vectors; `embedder_version` is the only guard against dim mismatch. Add a defensive `rec.vector.len() == EMBEDDING_DIM` check at cache-read time.
4. **MINOR** — `similar_to_item`'s "self-match detection is file-path-only" docstring comment is stale.
5. **MINOR** — `max_cluster_size` (default 15) interacts confusingly with `max_pairs` (default 50, doubles as cluster member cap). With defaults, the latter knob is unreachable. Worth a docstring note or aligning the two.
6. **MINOR** — Cluster member truncation order is HashMap-iteration-order; output is not deterministic when `truncated=true`. Sort by `NodeId` for stable diffs.
7. **MINOR** — `similar_to_item` trims-not, `semantic_overlaps` trims; embeddings of "the same Item" differ between the two tools.

None of these are blockers. The two MEDIUM items are operational concerns under concurrent use or stale-cache recovery, not correctness regressions for a single user on a single workspace.
