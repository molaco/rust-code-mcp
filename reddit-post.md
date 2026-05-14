**Title:** Updates to `rust-code-mcp`: 25+ new tools, safety audits, codemap, rename-preview — and how `THEORY.md` backs every one

**Body:**

Follow-up to the previous [`rust-code-mcp`](https://rust-code-mcp.pages.dev/) post. Since then the project has roughly doubled in tool surface (from ~20 to **45+** MCP tools), shipped **26 Claude Code skills** that compose them into audit recipes, and — most importantly — picked up a `THEORY.md` that names the structural principles each tool exists to check.

This post is two halves: **what's new**, then **why** — the principles that justify each addition.

---

## Part 1 — What's new

### New safety / quality audits

| Tool | What it does |
|---|---|
| `unsafe_audit` | Every `unsafe { ... }` block in local crates, with SAFETY-comment presence check |
| `mut_static_audit` | `static mut` / `LazyLock` / `OnceLock` / `OnceCell` inventory |
| `recursion_check` | Direct & mutual recursion cycles in fn-body call edges |
| `channel_capacity_audit` | Channel construction call sites — bounded vs unbounded |
| `fn_body_audit` | Walks fn bodies for unwrap / panic / lock-across-await / loop patterns |
| `missing_docs_audit` | Pure-`pub` items lacking `///` doc-comments |
| `derive_audit` | `pub` items missing required derive macros (configurable expectations) |

### Call graph (new Layer 10)

`who_calls`, `calls_from`, `call_graph` (bounded recursive descent), `callers_in_crate`, `recursive_callers_count` — fn-body reference graph, not just import edges.

### Workspace metrics & rules

- `crate_dependency_metric` — Robert Martin instability (Ce / (Ca + Ce)) and abstractness per crate
- `forbidden_dependency_check` — assertion-style enforcement of crate-edge rules
- `overlaps` — name collisions, module shadowing, within-crate duplicates
- `re_export_chain`, `pub_use_pub_type_audit`, `get_declared_reexports` — re-export hygiene

### Signatures & attributes

- `function_signature`, `functions_with_filter` — HIR signatures, shape-based search
- `enum_variants` — variant inventory with payload shapes
- `item_attributes`, `items_with_attribute` — outer attributes + doc-comment lines

### Semantic neighbors

- `similar_to_item` — vector-embedding nearest neighbors for a hypergraph item
- `semantic_overlaps` — workspace-wide clustering of semantically-similar items (with cached embeddings, configurable threshold + max cluster size)

### `rename_symbol`

rust-analyzer rename, but returns a **preview** (`RenamePreview`) — exact reference set, exact text edits, exact file-move list. **No files are modified.** Use it as a refactor probe before committing.

### `build_codemap`

Task-conditioned subgraph. Seed with symbols, get back a pruned Mermaid + outline showing the relevant neighborhood — call edges, type uses, module hierarchy. Useful for handing context to an agent without dumping the whole repo.

### Infrastructure

- **Persisted hypergraph** (`build_hypergraph`) — fingerprinted, reused across calls. ~10× cheaper than re-walking HIR every time.
- **`clear_cache` with `include_hypergraph`** — wipes the LMDB store when fingerprints get stale
- **stdio-safe metrics** — `tracing::info!` only, never `println!` (the MCP transport reserves stdout for JSON-RPC)

### 26 skills

The whole tool surface composed into invokable Claude Code skills: `/rmc-workspace-overview`, `/rmc-crate-audit`, `/rmc-unsafe-audit`, `/rmc-mut-static-audit`, `/rmc-refactor-plan`, `/rmc-rename-symbol`, `/rmc-codemap`, `/rmc-architecture-rules`, `/rmc-dependency-metric`, `/rmc-symbol-forensics`, `/rmc-trait-audit`, `/rmc-call-graph`, `/rmc-semantic-overlaps`, … 26 total. Each is a self-contained `SKILL.md` with prereqs, prompts, and hand-offs to related skills.

---

## Part 2 — Why these tools? `THEORY.md`

The new `THEORY.md` names **15 structural principles** that justify every tool above. The point: when an agent suggests a refactor, it cites a principle, not a vibe. Each principle pins to a check the agent can run.

The framework is a ladder: **files → modules → signatures → crates → workspaces**. Each rung has a unit, a morphism, and an rmc tool that returns the rung's structure (`get_imports` at the file level, `module_tree` at the module level, `crate_edges` at the crate level, `workspace_stats` at the top).

Here's how the new tools map back to principles:

- **P1 Boundary cost** — "a boundary is expensive proportional to morphism density × instability" → `crate_edges`, `get_imports`. Why `crate_dependency_metric` exists: it quantifies the density side.
- **P3 Acyclicity** — "units in an SCC cannot be partitioned across containers" → `forbidden_dependency_check` is the assertion form; `recursion_check` is the fn-level analog.
- **P6 Callsite-usage set** — "the honest trait surface = union of methods actually invoked at observed call sites, not the full type" → `who_uses`, `function_signature`, `similar_to_item`. This is why the call-graph layer (`who_calls`, `calls_from`) matters — you can't extract an honest trait without it.
- **P8 Bridge unit** — "a unit whose removal disconnects the graph is the highest-leverage refactor target" → `who_imports`, `recursive_callers_count`. The new call-graph tools surface bridges at the fn level.
- **P10 Hub / leaf / midstream** — classifying units by fan-in/fan-out shape → `recursive_callers_count` plus `call_graph`. Renaming a hub fans out; `rename_symbol` previews exactly that fan-out before you commit.
- **P13 Visibility as projection** — "`pub` is a structural decision, not a style decision" → `dead_pub_in_crate`, `dead_pub_report`. Items used cross-module but not cross-crate are `pub(crate)` candidates.
- **P14 Re-export transparency** — "`pub use` chains are syntactic redirection; treat re-export and original as the same morphism target" → `re_export_chain`, `get_reexports`, `pub_use_pub_type_audit`.
- **P15 Trait coherence as ordering** — "supertrait hierarchies must be a partial order" → cross-check against P6 callsite-usage. `function_signature` + `who_calls` is the diagnostic pair.

The safety audits (`unsafe_audit`, `mut_static_audit`, `fn_body_audit`, `channel_capacity_audit`) sit slightly outside the structural ladder — they're about *invariants*, not *boundaries* — but the framework's diagnostic style (named check + pinned tool + failure mode) carries over cleanly.

`semantic_overlaps` and `similar_to_item` are the embedding-based complement to the structural tools: where the hypergraph misses duplicate *logic* (because two impls don't share a type or a call edge), embedding clustering catches it. Treat the output as a candidate set for P6 (extract trait) or P5 (named-surface test on the surrounding container).

---

**Why the theory layer matters.** Without it, an MCP server is `grep` + LSP wrapped in JSON. The named-principle framing makes the agent's reasoning checkable — you can ask *which principle was cited* and *which tool produced the evidence*. That's harder to fake than "this looks cleaner."

**Links:**

- Website: https://rust-code-mcp.pages.dev/
- Discord: https://discord.com/invite/dENhfbtCa — drop in and tell us what your workflow looks like; we genuinely want to know what people are doing with this
- Repo: (link your GitHub here)
- `THEORY.md` is in the repo root if you want to read the principles directly

Open questions / known gaps from `THEORY.md` §9: P11 co-change locality needs git-log integration (not wired up yet), §3 signature-rung inference has no one-shot tool, and §5–§9 (operations catalog, diagnostics drill, composition, worked walkthrough) are still stubs. Feedback on the principle set very welcome.
