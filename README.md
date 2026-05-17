# rust-code-mcp

An MCP server for searching and analyzing Rust codebases. Combines hybrid BM25 + vector search with a HIR-driven workspace **hypergraph** built on rust-analyzer, exposing 45+ tools for symbol navigation, call-graph traversal, structural audits, and semantic neighborhood queries.

**Links:** [Website](https://rust-code-mcp.pages.dev/) · [Discord](https://discord.com/invite/dENhfbtCa) — come share how you're using it; we want to hear about people's workflows.

## Architecture

![Architecture](architecture.mmd.svg)

See [.docs/ARCHITECTURE.md](.docs/ARCHITECTURE.md) for the per-module breakdown, [TOOLS.md](TOOLS.md) for the full MCP tool reference, [.docs/configure-models-guide.md](.docs/configure-models-guide.md) for embedding-model configuration, and [THEORY.md](THEORY.md) for the principles each diagnostic maps to.

## Features

- **Hybrid search** - BM25 keyword search + semantic vector similarity (RRF fusion)
- **Pluggable embedding models** - local GPU (Qwen3 via Candle/CUDA), local CPU (BGE via ONNX), or API-backed (OpenRouter); new API models are added through a config file with no recompile
- **Symbol navigation** - rust-analyzer–backed `find_definition` / `find_references` / `rename_symbol` (rename returns a preview; no files are modified)
- **Persisted hypergraph** - HIR-driven workspace snapshot (LMDB) with cross-crate imports, exports, re-exports, call edges, attributes, signatures, statics, and `unsafe` blocks
- **Call-graph traversal** - `who_calls` / `calls_from` / `call_graph` / `callers_in_crate` / `recursive_callers_count`
- **Structural audits** - dead public items, name collisions, module shadowing, forbidden cross-crate edges, Robert Martin instability/abstractness
- **Safety audits** - `unsafe_audit`, `mut_static_audit`, `recursion_check`, `channel_capacity_audit`, `fn_body_audit` (unwrap/panic/lock-across-await detection)
- **Doc & API hygiene audits** - `missing_docs_audit`, `derive_audit`, `pub_use_pub_type_audit`, `re_export_chain`
- **Semantic neighbors** - `similar_to_item` and workspace-wide `semantic_overlaps` clustering via cached embeddings
- **Codemap** - `build_codemap` produces a task-conditioned subgraph (seeded by symbols, expanded over hypergraph edges) with Mermaid + outline rendering
- **Complexity metrics** - LOC, cyclomatic complexity, function counts
- **Incremental indexing** - Merkle-tree change detection; background re-sync every 5 minutes

## Tools

45+ MCP tools grouped by category. Full parameter reference in [TOOLS.md](TOOLS.md).

| Category | Tools |
|----------|-------|
| Query | `search`, `get_similar_code`, `read_file_content` |
| Symbol analysis | `find_definition`, `find_references`, `rename_symbol`, `get_dependencies`, `get_call_graph`, `analyze_complexity` |
| Index lifecycle | `index_codebase`, `health_check`, `clear_cache` |
| Hypergraph build | `build_hypergraph` |
| Imports / exports | `get_imports`, `get_exports`, `get_reexports`, `get_declared_reexports` |
| Reverse lookup | `who_imports`, `who_uses`, `who_uses_summary` |
| Call graph | `who_calls`, `calls_from`, `call_graph`, `callers_in_crate`, `recursive_callers_count` |
| Workspace structure | `dead_pub_in_crate`, `dead_pub_report`, `crate_edges`, `overlaps`, `module_tree`, `workspace_stats` |
| Architecture rules | `forbidden_dependency_check`, `crate_dependency_metric` |
| Signatures & attributes | `function_signature`, `functions_with_filter`, `enum_variants`, `item_attributes`, `items_with_attribute` |
| Safety & quality audits | `unsafe_audit`, `mut_static_audit`, `recursion_check`, `channel_capacity_audit`, `fn_body_audit` |
| Doc / API audits | `missing_docs_audit`, `derive_audit`, `pub_use_pub_type_audit`, `re_export_chain` |
| Semantic | `similar_to_item`, `semantic_overlaps` |
| Codemap | `build_codemap` |

## Skills

The [`skills/`](./skills) directory ships 26 [Claude Code skills](https://docs.claude.com/en/docs/claude-code/skills) that compose these MCP tools into ready-made audit recipes. Each skill is a self-contained `SKILL.md` with prerequisites, step-by-step prompts, and hand-offs to related skills — invoke them in Claude Code as `/<skill-name>` once installed.

| Scope | Skill | Purpose |
|-------|-------|---------|
| Workspace | `rmc-workspace-overview` | First-look audit of a Rust workspace |
| Workspace | `rmc-architecture-rules` | Enforce crate-edge rules |
| Workspace | `rmc-dependency-metric` | Rank crates by Robert Martin instability / abstractness |
| Workspace | `rmc-imports-exports` | Cross-crate imports and exports audit |
| Workspace | `rmc-type-overlaps` | Name collisions and module shadows |
| Workspace | `rmc-semantic-overlaps` | Duplicate logic detection by embedding similarity |
| Workspace | `rmc-snapshot-diff` | Compare two workspace snapshots (e.g. across branches) |
| Crate | `rmc-crate-audit` | Audit one crate end-to-end |
| Crate | `rmc-api-surface` | Audit a crate's public API |
| Crate | `rmc-module-audit` | Audit a single module |
| Symbol | `rmc-find-symbol` | Resolve a symbol's qualified name |
| Symbol | `rmc-symbol-forensics` | Deep dive on one symbol (refs, callers, attrs) |
| Symbol | `rmc-method-api` | Audit a type's methods |
| Symbol | `rmc-trait-audit` | Audit a trait and its impls |
| Symbol | `rmc-enum-variants` | Inspect an enum's variants |
| Symbol | `rmc-rename-symbol` | Preview a rename — exact reference set & refactor probe |
| Symbol | `rmc-signature-search` | Find fns by signature shape |
| Quality | `rmc-unsafe-audit` | Audit `unsafe` blocks |
| Quality | `rmc-mut-static-audit` | Audit global mutable state |
| Quality | `rmc-attribute-audit` | Audit attributes and doc-comments |
| Quality | `rmc-complexity` | Complexity hotspots by blast radius |
| Quality | `rmc-test-vs-prod` | Test vs production split |
| Quality | `rmc-call-graph` | Fn-level call graphs |
| Quality | `rmc-reexport-chain` | Trace `pub use` re-export chains |
| Planning | `rmc-codemap` | Task-conditioned workspace subgraph (nodes / edges / hierarchy) |
| Planning | `rmc-refactor-plan` | Plan a refactor with evidence from the hypergraph |

**Install** by symlinking (or copying) the directories into your Claude Code skills folder:

```bash
mkdir -p ~/.claude/skills
ln -s "$(pwd)/skills/"rmc-* ~/.claude/skills/
```

Then in Claude Code, type `/rmc-` to discover them, or `/rmc-workspace-overview` to kick off a tour of a new repo.

## Installation

### 1. Build the binary

```bash
git clone https://github.com/molaco/rust-code-mcp.git
cd rust-code-mcp
cargo build --release
```

The binary is at `target/release/rust-code-mcp`.

> **Build prerequisite:** the default embedding backend (Qwen3) runs on Candle
> with CUDA, so the build needs the CUDA toolkit (`nvcc`) on `PATH`. The
> simplest way to get a correct toolchain is the Nix dev shell (see [Nix](#nix))
> — `nix develop` provides nightly Rust plus the CUDA toolkit, cuDNN, and
> cuBLAS. See [GPU & CUDA](#gpu--cuda) for details.

Optionally, copy it somewhere on your PATH:

```bash
cp target/release/rust-code-mcp ~/.local/bin/
```

### 2. Add to Claude Code

In your Rust project directory, create `.mcp.json`:

```json
{
  "mcpServers": {
    "rust-code-mcp": {
      "command": "/absolute/path/to/rust-code-mcp"
    }
  }
}
```

Or add it globally in `~/.claude.json` so it's available in all projects:

```json
{
  "mcpServers": {
    "rust-code-mcp": {
      "command": "/absolute/path/to/rust-code-mcp"
    }
  }
}
```

### 3. Index your codebase

Once Claude Code starts, the server is running. Use the `index_codebase` tool to index your project:

```
> index my codebase at /absolute/path/to/my-rust-project
```

Or call the tool directly with the `directory` parameter set to your project root. Pass an optional `embedding_profile` to choose the embedding model (see [Embedding Models](#embedding-models)); the default is a local GPU model. Indexing is incremental — subsequent runs only process changed files (via Merkle tree change detection). A background sync also re-indexes every 5 minutes automatically, using the profile each index was built with.

### 4. Start using it

All tools accept a `directory` parameter pointing to your project root. Examples:

- **Search code**: `search` with a query like "error handling in parser"
- **Find definitions**: `find_definition` for a symbol name
- **Find references**: `find_references` to see all usages of a symbol
- **Preview a rename**: `rename_symbol` returns the full edit set without touching files
- **Call graph**: `get_call_graph` or `who_calls` / `calls_from` to trace function relationships
- **Similar code**: `get_similar_code` for semantic similarity search

For Rust-specific workspace analysis, first call `build_hypergraph` once (reuses a fingerprinted snapshot on subsequent calls), then run audits like `unsafe_audit`, `dead_pub_report`, `overlaps`, `crate_dependency_metric`, or `semantic_overlaps`. The codemap tool (`build_codemap`) produces a Mermaid-renderable subgraph seeded by symbols of interest.

Index data is stored in `~/Library/Application Support/dev.rust-code-mcp.search/` (macOS) or `~/.local/share/search/` (Linux), keyed by a hash of the project path **and the active embedding profile** — so different profiles get independent indexes, and it never writes to your project directory. The persisted hypergraph lives alongside it (under `graph/<workspace_hash>/`, in LMDB). `clear_cache` with `include_hypergraph=true` wipes both.

## Embedding Models

Semantic search and the embedding-backed audits (`get_similar_code`, `similar_to_item`, `semantic_overlaps`) run on a configurable embedding **profile**. Built-in profiles:

| Profile | Model | Runtime | Dim |
|---------|-------|---------|----:|
| `local-gpu-small` *(default)* | Qwen3-Embedding-0.6B | local Candle/CUDA | 1024 |
| `local-qwen3-4b` | Qwen3-Embedding-4B | local Candle/CUDA | 2560 |
| `local-qwen3-8b` | Qwen3-Embedding-8B | local Candle/CUDA | 4096 |
| `local-cpu-small` | BGE-small-en-v1.5 | local ONNX/CPU | 384 |
| `openrouter-qwen3-8b` | Qwen3-Embedding-8B | OpenRouter API | 4096 |

Select one by passing `embedding_profile` to `index_codebase` and the search tools. Each profile gets its own independent index, and search must use the profile its index was built with.

**API models** (OpenRouter) require an API key in the environment — keys are never read from config files:

```sh
export OPENROUTER_API_KEY=sk-or-...
```

**Adding an API model is a config change, no recompile** — drop an `embedding_profiles.toml` in your project root:

```toml
[[profile]]
name     = "openrouter-text-embedding-3-small"
model_id = "openai/text-embedding-3-small"
dim      = 1536
max_len  = 8191
```

Local models (Candle/ONNX) are code-bound and ship as built-ins. See [.docs/configure-models-guide.md](.docs/configure-models-guide.md) for the full TOML schema, OpenRouter tuning knobs, and the trade-offs between models.

## Nix

A Nix flake is provided for easy setup:

```bash
# Enter dev shell with all dependencies
nix develop github:molaco/rust-code-mcp

# Build the binary
nix build github:molaco/rust-code-mcp
```

The dev shell includes nightly Rust and the full CUDA toolchain (toolkit, cuDNN, cuBLAS) needed to build and run the GPU embedding path.

## GPU & CUDA

The default embedding profile (`local-gpu-small`, Qwen3) runs on [Candle](https://github.com/huggingface/candle) with CUDA. The local CPU profile (`local-cpu-small`) runs on ONNX and needs no GPU; OpenRouter profiles offload embedding to the API and need no local GPU either.

### Requirements

- NVIDIA GPU — ≥8 GB VRAM is comfortable for the default 0.6B model; the local 8B profile needs ~16 GB
- CUDA toolkit (`nvcc`) at **build** time — Candle's CUDA backend (`cudarc`) requires it during compilation
- CUDA runtime + cuDNN + cuBLAS libraries at **run** time

### Setup

The Nix dev shell provides the full CUDA build and runtime environment — this is the supported path:

```bash
nix develop          # nightly Rust + CUDA toolkit + cuDNN/cuBLAS
cargo build --release
```

When the MCP server is spawned by Claude Code (rather than launched from the Nix shell), its process still needs the CUDA runtime libraries on `LD_LIBRARY_PATH`. Set them in your MCP client config:

```json
{
  "mcpServers": {
    "rust-code-mcp": {
      "command": "/path/to/rust-code-mcp",
      "env": {
        "RUST_LOG": "info",
        "CUDA_HOME": "/usr/local/cuda",
        "LD_LIBRARY_PATH": "/usr/local/cuda/lib64:/usr/lib/x86_64-linux-gnu"
      }
    }
  }
}
```

Adjust the paths to your CUDA installation.

### Running without a GPU

A GPU is not required. Two options, neither needs CUDA at run time:

1. **Keep the GPU-capable build, use a CPU or API profile.** Index and search with `embedding_profile = "local-cpu-small"` (BGE on ONNX/CPU) or any OpenRouter profile — the GPU code paths are simply never exercised.
2. **CPU-only build.** For a machine with no CUDA toolkit at all, remove the `cuda` feature from the `fastembed` dependency in `Cargo.toml`. The `local-gpu-*` / `local-qwen3-*` profiles become unavailable, but the ONNX and OpenRouter profiles work and the build no longer needs `nvcc` or the CUDA libraries.

The Nix dev shell's `shellHook` documents the same options inline.

## Performance

Indexing throughput measured on this repository (~2,280 chunks) with an RTX 3090:

| Profile | Indexing throughput | Notes |
|---------|--------------------:|-------|
| `local-gpu-small` (Qwen3-0.6B) | ~55 chunks/sec | local GPU; private, deterministic |
| `openrouter-qwen3-8b` (Qwen3-8B) | ~45–50 chunks/sec | API; per-request latency varies run-to-run |
| `openrouter` text-embedding-3-small | ~220 chunks/sec | API; fastest, general-purpose (not code-tuned) |

Embedding is ~95%+ of indexing time. Larger / higher-quality models embed more slowly; `text-embedding-3-small` is fastest but not code-tuned. Vector search is exact brute-force KNN — fast at workspace scale regardless of dimension. Pick a profile for your quality / speed / privacy trade-off; see the [config guide](.docs/configure-models-guide.md).

## Stack

- [tantivy](https://github.com/quickwit-oss/tantivy) - Full-text BM25 search
- [fastembed](https://github.com/Anush008/fastembed-rs) - Embedding models: Qwen3 via Candle (GPU), BGE via ONNX (CPU)
- [candle](https://github.com/huggingface/candle) - GPU embedding runtime for Qwen3 (CUDA)
- [reqwest](https://github.com/seanmonstar/reqwest) - OpenRouter API embedding client
- [lancedb](https://lancedb.com/) - Embedded vector storage
- [ra_ap_syntax](https://github.com/rust-lang/rust-analyzer) - AST parsing
- [ra_ap_ide](https://github.com/rust-lang/rust-analyzer) - Semantic analysis (goto definition, find references, rename)
- [ra_ap_hir](https://github.com/rust-lang/rust-analyzer) - HIR-driven hypergraph extraction
- [heed](https://github.com/meilisearch/heed) - LMDB-backed persisted hypergraph store
- [sled](https://github.com/spacejam/sled) - Embedded KV for indexing metadata cache
- [rmcp](https://github.com/modelcontextprotocol/rust-sdk) - MCP protocol

## Screenshots

![Screenshot](./assets/screenshot-2026-01-06-10-25-00.png)

<details>
<summary>More screenshots</summary>

![Screenshot 2](./assets/screenshot-2026-01-06-10-25-31.png)
![Screenshot 3](./assets/screenshot-2026-01-06-10-25-54.png)
![Screenshot 4](./assets/screenshot-2026-01-06-10-26-16.png)
![Screenshot 5](./assets/screenshot-2026-01-06-10-26-34.png)

</details>

## License

MIT
