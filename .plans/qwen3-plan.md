# Plan: Generalize embedding wrapper to support Qwen3 via fastembed

## Goal

Make the embedding model a runtime/config choice instead of a hardcoded
`AllMiniLML6V2`, and add support for the Qwen3 embedding family
(`Qwen3-Embedding-0.6B`, `4B`, `8B`) by bumping `fastembed-rs` to a version
that ships them.

We stay on `fastembed-rs`. We do **not** swap to `EmbedAnything` or
`embedrs`. The pipeline (file walker, chunker, batcher, incremental index,
vector store) is untouched — only the encoder layer changes.

## Why this path

- `fastembed-rs` 5.13+ ships `Qwen3-Embedding-{0.6B, 4B, 8B}` and
  `Qwen3-VL-Embedding-2B` under a `qwen3` Cargo feature (Candle backend).
- Our `Cargo.toml:65` is unpinned to git main; the resolved checkout is
  5.2.0 (no Qwen3). We are ~11 minor versions behind.
- Our `EmbeddingGenerator` (`src/embeddings/mod.rs`) is ~560 LOC,
  well-isolated, and already the only thing that talks to fastembed.
- All other indexing logic is code-aware (AST-driven chunker, Merkle
  change detection, RAM-aware batcher, secret scanning) and is not
  replaceable by any off-the-shelf library.
- `EmbedAnything` only ships Qwen3-0.6B; swapping to it would mean a
  larger rewrite for fewer models than fastembed gives us today.

## Constraints

- Use `nix develop ../nix-devshells#code --command cargo check --lib`
  for every compile checkpoint. Do not run `cargo test` unless asked
  (snapshot build ~115s).
- Do not run `cargo fmt`.
- No behavior change for the default model path — existing indexes built
  on `AllMiniLML6V2` (384-dim) must keep working after the bump if the
  same model is selected.
- Small, compile-checkable steps. Land in this order — each step compiles
  on its own.

## Background — current state

- Single hardcoded model at `src/embeddings/mod.rs:92`:
  `InitOptions::new(EmbeddingModel::AllMiniLML6V2)`.
- Backend: fastembed-rs (ONNX via `ort` 2.0.0-rc.10) with hand-rolled
  CUDA/CPU fallback at `mod.rs:42-88`.
- Public surface (`src/embeddings/mod.rs`): `EmbeddingGenerator::{new,
  embed, embed_async, embed_batch, embed_batch_async, embed_chunks,
  dimensions}`, `EmbeddingPipeline`, the alias `Embedding = Vec<f32>`,
  and the constant `EMBEDDING_DIM: usize = 384`.
- `EMBEDDING_DIM` is imported by:
  - `src/tools/query_tools.rs:14`
  - `src/mcp/sync.rs:9`
  - `src/tools/index_tool.rs:6`
- Dim flows into LanceDB via the `vector_size` parameter at
  `src/vector_store/mod.rs:50`, `src/indexing/unified.rs:611,634`,
  `src/indexing/incremental.rs:280,313,353`,
  `src/vector_store/lancedb.rs:459`. Most call sites pass `384` literally
  in tests; the production default in `vector_store/mod.rs:50` is also
  literally `384`.

## Target design

### Config

Add an `EmbeddingBackend` enum in `src/embeddings/mod.rs`:

```rust
#[derive(Debug, Clone)]
pub enum EmbeddingBackend {
    /// fastembed ONNX path (current default).
    Onnx(OnnxModel),
    /// fastembed Candle path, behind the `qwen3` feature.
    Qwen3 { variant: Qwen3Variant, max_len: usize },
}

#[derive(Debug, Clone, Copy)]
pub enum OnnxModel {
    AllMiniLML6V2,   // 384-dim, current default
    BgeBaseEnV15,    // 768-dim
    // extend as needed; do not add models we are not actually going to use
}

#[derive(Debug, Clone, Copy)]
pub enum Qwen3Variant {
    Embedding0_6B,   // 1024-dim
    Embedding4B,     // 2560-dim
    Embedding8B,     // 4096-dim
}
```

The enum reports two things per backend:

- `fn dim(&self) -> usize` — the output vector dimension.
- `fn identity(&self) -> &'static str` — a stable string used in cache
  paths and the `EMBEDDER_VERSION` constant. Examples:
  `"fastembed-onnx:all-MiniLM-L6-v2:dim384:v1"`,
  `"fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max2048:v1"`.

The static `EMBEDDING_DIM` constant goes away.

### Max sequence length

Qwen3's `Qwen3TextEmbedding::from_hf(model_id, device, dtype, max_len)`
takes a max-length argument in tokens (not output dim — the upstream
README example uses `512`). Code chunks from our AST chunker can easily
exceed 512 tokens for large functions, so we pick a deliberate default:

- Default `max_len = 2048` for all Qwen3 variants. Big enough to swallow
  almost any function-sized chunk after the contextual-retrieval header
  is prepended; small enough to keep VRAM bounded on a 0.6B run.
- Make it a field on the `Qwen3` variant so it's part of the cache
  identity (changing it invalidates indexes). Plumb it through
  `EmbeddingBackend::identity()`.
- Document, but don't expose, the option of pushing it to 8192 for
  recall experiments later. Don't ship a CLI knob until we have a real
  reason.

The ONNX path keeps fastembed's default max-length behavior — it
already does the right thing for MiniLM/BGE.

### Instruction-aware embedding (query vs document)

Qwen3 embeddings are instruction-tuned: queries take a task instruction
prefix, documents take the raw text. Skipping this hurts recall
measurably on retrieval benchmarks. We add the role split into the
internal trait and the public surface:

```rust
trait EmbedderImpl: Send + Sync {
    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError>;
    fn embed_queries(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError>;
    fn dim(&self) -> usize;
}
```

Implementation per backend:

- **ONNX (MiniLM/BGE)** — `embed_queries` and `embed_documents` are
  identical: just call fastembed `embed`. MiniLM doesn't use prefixes;
  BGE-v1.5 *does* use a query prefix (`"Represent this sentence for
  searching relevant passages: "`) — if we ever enable BGE we add it
  there.
- **Qwen3** — `embed_documents` calls `Qwen3TextEmbedding::embed`
  directly with the raw chunk text. `embed_queries` prepends Qwen3's
  instruction template:
  `"Instruct: Given a code search query, retrieve relevant code\nQuery: {text}"`.
  Confirm the exact wording against the upstream Qwen3-Embedding model
  card on first integration — it may have evolved. Keep the instruction
  text in one place so changing it across all variants is a one-line
  edit.

Public surface on `EmbeddingGenerator`:

- Rename the existing `embed`/`embed_async`/`embed_batch`/
  `embed_batch_async`/`embed_chunks` paths into a `documents` flavor —
  call sites that build the index pass documents.
- Add `embed_query` / `embed_query_async` for the search-time path.
  `src/search/mod.rs` and `src/tools/query_tools.rs` are the only two
  call sites that need to switch.
- Keep the old `embed*` names as deprecated thin aliases for one
  release so the refactor is bisectable. Remove in a follow-up.

### Internal trait

Inside `src/embeddings/`, introduce a private trait that both fastembed
paths implement:

```rust
trait EmbedderImpl: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError>;
    fn dim(&self) -> usize;
}
```

Two implementors live in submodules:
- `src/embeddings/onnx.rs` — wraps `fastembed::TextEmbedding` (existing
  code, extracted unchanged).
- `src/embeddings/qwen3.rs` — wraps `fastembed::Qwen3TextEmbedding`,
  gated behind `#[cfg(feature = "qwen3")]` on the impl and the enum
  variant. The variant must exist regardless of feature so call sites
  that match on it don't need their own `cfg`s; the constructor returns
  `EmbeddingError::FeatureDisabled` when the feature is off.

`EmbeddingGenerator` keeps its current public methods but stores
`Arc<dyn EmbedderImpl>` plus a cached `dim: usize`. The public API
signatures do not change. We add one new constructor:

```rust
impl EmbeddingGenerator {
    pub fn with_backend(backend: EmbeddingBackend) -> Result<Self, EmbeddingError> { ... }
    pub fn new() -> Result<Self, EmbeddingError> {
        Self::with_backend(EmbeddingBackend::Onnx(OnnxModel::AllMiniLML6V2))
    }
}
```

`EmbeddingGenerator::new()` keeps the same behavior as today, so most
call sites need zero changes.

### Wiring the dim through

`EMBEDDING_DIM` is replaced by `EmbeddingGenerator::dimensions()` at the
three import sites. They all already have access to an
`EmbeddingGenerator` instance (or can be given one); concretely:

- `src/mcp/sync.rs:9` — replace import with the runtime call. Check the
  call context; if no generator is in scope, plumb one in.
- `src/tools/query_tools.rs:14` — same.
- `src/tools/index_tool.rs:6` — same.

Test literals (`384` in `incremental.rs`, `unified.rs`, `lancedb.rs`)
stay as `384` because they are testing the default-model path. They get
a comment pointing at the active model so future changes don't drift.

### Cargo

`fastembed` 5.13.4 is published on crates.io; use the version pin, not
the git source. The previous `git = ...` line goes away.

`Cargo.toml`:

```toml
[dependencies]
# Was: fastembed = { git = "...", ... }
fastembed = { version = "5.13.4", default-features = false, features = [
    "ort-download-binaries-native-tls",
    "hf-hub-native-tls",
    "image-models",
] }

# fastembed 5.13.4 pins ort = "=2.0.0-rc.12". We currently pin =2.0.0-rc.10.
# A single-version conflict will block resolution. Bump to match.
ort = "=2.0.0-rc.12"

# Direct, optional. Only needed because src/embeddings/qwen3.rs imports
# Device/DType from candle-core to build the Qwen3 embedder. Keep the
# version in lockstep with fastembed's pin (0.10.2 at 5.13.4).
candle-core = { version = "0.10.2", optional = true }

[features]
# Forwarded workspace features. Do NOT put `features = ["qwen3"]` on the
# fastembed dep itself — that would unconditionally pull Candle in.
qwen3 = ["fastembed/qwen3", "dep:candle-core"]
qwen3-cuda = ["qwen3", "fastembed/cuda"]
qwen3-metal = ["qwen3", "fastembed/metal"]
```

Defaults are unchanged: ONNX only, no Candle, no Qwen3. The `qwen3`
feature opt-in is what pulls in `candle-core` / `candle-nn`.

GPU mapping is critical and easy to get wrong:

- Our existing CUDA fallback (`src/embeddings/mod.rs:42-88`) is for
  **ORT**, not Candle. It is irrelevant on the Qwen3 path.
- For Qwen3 on CUDA, the right knob is `fastembed/cuda`, which fastembed
  defines as `["qwen3", "nomic-v2-moe", "candle-core/cuda",
  "candle-nn/cuda"]`. The `qwen3-cuda` workspace feature above forwards
  to it.
- On the Qwen3 code path we build the Candle device explicitly:
  `Device::cuda_if_available(0).unwrap_or(Device::Cpu)`. Keep the
  tracing lines for parity with the ORT path so log scraping still
  works.
- macOS dev boxes go through `qwen3-metal`, same shape.

## Step-by-step

Each step ends green: `cargo check --lib` passes inside the nix
devshell.

### Step 1 — bump fastembed and align ORT, no behavior change

This step is bigger than it looks because of a hard version conflict.

- Replace the git-sourced `fastembed` line at `Cargo.toml:65` with
  `version = "5.13.4"` and the default-features/features set from the
  Cargo section above. Do **not** add the `qwen3` feature yet.
- **Bump `ort` from `=2.0.0-rc.10` to `=2.0.0-rc.12`** in the same
  commit. fastembed 5.13.4 pins ort with a `=` constraint, so leaving
  ours at rc.10 makes Cargo refuse to resolve. This is the most likely
  source of compile pain in the whole plan; budget time here.
- Audit `src/embeddings/mod.rs:12` — we import
  `ort::execution_providers::{CPUExecutionProvider, CUDAExecutionProvider,
  ExecutionProvider}` and pass them into fastembed's
  `with_execution_providers`. Confirm rc.12 still has these types in
  the same module path and that their builders return the same
  `ExecutionProviderDispatch`. Read the rc.10→rc.12 changelog (or just
  diff the cached source trees in `~/.cargo/registry/src/`) before
  writing code.
- Run `nix develop ../nix-devshells#code --command cargo check --lib`.
  Fix call-site breakage **only** in `src/embeddings/mod.rs`; if
  anything else needs to change, stop and flag it on the plan — it
  means we missed a leak.

### Step 2 — introduce the backend enum, keep one impl

- Create `src/embeddings/backend.rs` containing `EmbeddingBackend`,
  `OnnxModel`, `Qwen3Variant`, and the `dim()` method on each.
- Wire `EmbeddingBackend::Onnx(OnnxModel::AllMiniLML6V2).dim() == 384`.
- Add `EmbeddingGenerator::with_backend(...)` that currently only
  handles the ONNX branch (the Qwen3 branch returns
  `EmbeddingError::FeatureDisabled`).
- `EmbeddingGenerator::new()` becomes a thin wrapper over
  `with_backend`.
- Keep `EMBEDDING_DIM` as a `pub const` for one more step to avoid
  cascading edits.

### Step 3 — extract the ONNX impl behind the internal trait

- Create `src/embeddings/onnx.rs` with the internal `EmbedderImpl` trait
  and the fastembed-TextEmbedding implementation. Move the CUDA fallback
  block out of `mod.rs` into a helper here.
- `EmbeddingGenerator` now holds `Arc<dyn EmbedderImpl>` instead of
  `Arc<Mutex<TextEmbedding>>`. The `embed_*` methods delegate.
- The CUDA/CPU dispatch logic moves into `OnnxEmbedder::new`.

### Step 4 — add the Qwen3 impl behind `qwen3` feature

- Create `src/embeddings/qwen3.rs` with
  `Qwen3Embedder { inner: fastembed::Qwen3TextEmbedding, dim: usize, instruction: &'static str }`
  implementing `EmbedderImpl`.
- The whole file is `#[cfg(feature = "qwen3")]`.
- In `EmbeddingGenerator::with_backend`, the Qwen3 arm constructs this
  under the same cfg; the `#[cfg(not(feature = "qwen3"))]` arm returns
  `EmbeddingError::FeatureDisabled` with a clear message.
- Hugging Face model IDs:
  - `Qwen3Variant::Embedding0_6B` → `"Qwen/Qwen3-Embedding-0.6B"`
  - `Qwen3Variant::Embedding4B`   → `"Qwen/Qwen3-Embedding-4B"`
  - `Qwen3Variant::Embedding8B`   → `"Qwen/Qwen3-Embedding-8B"`
- Construction maps to fastembed's
  `Qwen3TextEmbedding::from_hf(model_id, &device, dtype, max_len)`:
  - `device`: `candle_core::Device::cuda_if_available(0).unwrap_or(Device::Cpu)`.
    This is **independent** of the ORT CUDA check on the ONNX path.
    Both checks can live side by side, but the Qwen3 path must not read
    the ORT check.
  - `dtype`: start with `DType::F32`. Revisit F16 once 0.6B works; the
    upstream commit `b39d84b` ("Fix Qwen3 F16 dtype mismatches in
    attention and l2_normalize") suggests F16 has had quirks recently.
  - `max_len`: passed through from `EmbeddingBackend::Qwen3 { max_len }`.
- `embed_documents` calls `inner.embed(&texts)` directly.
  `embed_queries` prepends the instruction template described in the
  "Instruction-aware embedding" section above to each input before the
  same call. Store the template as a `&'static str` on the embedder so
  it is easy to swap.
- Add the same tracing lines we have on the ONNX path
  (`tracing::info!("=== Qwen3 INITIALIZATION DEBUG ===")` etc.) for log
  parity, but do **not** copy the ORT environment-variable checks —
  those are meaningless here. Log Candle's device and dtype instead.

### Step 5 — remove the `EMBEDDING_DIM` constant

- Delete `pub const EMBEDDING_DIM: usize = 384;` from
  `src/embeddings/mod.rs`.
- Update the three importers (`mcp/sync.rs:9`,
  `tools/query_tools.rs:14`, `tools/index_tool.rs:6`) to call
  `generator.dimensions()` at the right point. If the call site does not
  already hold a generator, plumb one in via constructor args — do not
  add a global.
- The literal `384`s in tests stay; add a one-line comment at each test
  site naming the model so future readers understand why it's literal.

### Step 6 — let the indexer pick a backend

- Add a `model: EmbeddingBackend` field to whatever struct configures
  the indexer (read `src/indexing/unified.rs` and `indexer_core.rs`
  during this step to see where to slot it).
- Default to `EmbeddingBackend::Onnx(OnnxModel::AllMiniLML6V2)` so
  existing CLI invocations keep working.
- Expose it on the MCP tool surface only after the rest of the chain
  compiles — that's the place where wrong configuration causes the most
  user pain, so it deserves its own dedicated step.

### Step 7 — cache & path identity by model

Switching models silently is a footgun: LanceDB will reject the dim
mismatch, but the graph embedding cache and the on-disk vector path
would otherwise happily mix vectors from two different models. Fix all
three identity points in this step.

1. **`EMBEDDER_VERSION` is currently a `const`** at
   `src/tools/graph_tools.rs:990`: `"fastembed:all-MiniLM-L6-v2:dim384:v1"`.
   Replace the constant with a function `embedder_version(&EmbeddingBackend) -> String`
   that returns the backend's `identity()` value. Audit every reader of
   `EMBEDDER_VERSION` and feed them the active backend.
2. **Vector store path** in `src/tools/project_paths.rs:30` is
   `format!("code_chunks_{}", &dir_hash[..8])` — keyed only by project
   directory. Extend it to include a short model fingerprint:
   `format!("code_chunks_{}_{}", &dir_hash[..8], &model_fp[..8])` where
   `model_fp` is `sha256(backend.identity())`. This means two indexes
   for the same project under different models live in separate
   LanceDB directories instead of fighting over the same one.
3. **Health check / clear_cache** (find the MCP handlers — likely under
   `src/tools/`) must surface the active model in their output and, on
   startup, refuse to attach an existing index whose recorded
   `EMBEDDER_VERSION` doesn't match the configured backend. The refusal
   message must tell the user the exact `clear_cache` invocation to fix
   it. Do **not** auto-wipe.
4. Write the active `EMBEDDER_VERSION` into a small `metadata.json`
   alongside the LanceDB directory at first index, and verify it on
   reopen. This is the check that makes (3) possible.

### Step 8 — smoke test

- Build with `--features qwen3` in the devshell.
- Run the binary against a small fixture directory using each of:
  - default (MiniLM, ONNX)
  - Qwen3-0.6B (Candle, GPU if available)
- Confirm the LanceDB table is created with the right `vector_size`
  (1024 for 0.6B vs 384 for MiniLM) by inspecting the table schema.
- Do **not** attempt 4B/8B on first run — verify the architecture works
  with 0.6B first, then size up.

## Risks and open questions

1. **ORT pin conflict is a hard blocker, not soft.** fastembed 5.13.4
   has `ort = "=2.0.0-rc.12"`; we have `=2.0.0-rc.10`. Cargo's
   single-version rule means resolution fails unless both move. Step 1
   bumps us to rc.12; verify `CUDAExecutionProvider` /
   `CPUExecutionProvider` API parity before writing any other code.
2. **fastembed API drift between 5.2.0 and 5.13.x.** The
   `TextEmbedding::try_new(InitOptions::new(model).with_*())` builder
   has been stable in spirit but field additions are possible. Step 1
   handles this together with the ORT bump.
3. **`Qwen3TextEmbedding` ergonomics.** Its `embed` method takes
   `&[&str]` and is synchronous on Candle. Confirm whether it supports
   batched inference and what the batch-size sweet spot is on our 8 GB
   GPU. The README example uses two strings; we will need real batches.
4. **Memory footprint.** Qwen3-4B and -8B will not fit alongside our
   current 5.5 GB ORT memory cap on an 8 GB card. If we ever enable
   them, the ORT cap needs to drop or be removed when Candle is the
   active backend, since they don't share the same memory pool. Note
   the ORT cap is set unconditionally today (`mod.rs:78-79`); during
   the refactor it should move into the ONNX impl, not the generic
   constructor.
5. **Instruction template drift.** The exact Qwen3 instruction string
   for code retrieval may change between model card revisions; we are
   hard-coding what works today. Confirm against the model card on
   first integration and centralize the literal in one place.
6. **Cache/path identity (Step 7).** The graph cache, the LanceDB path,
   and the indexed-snapshot metadata must all agree on
   `EMBEDDER_VERSION` derived from the active backend. Stale
   `code_chunks_<hash>/` directories from before this change will exist
   on user machines; `clear_cache` must handle them. Do not auto-wipe.
7. **HF token / network.** Qwen3 download is gated behind a network
   round-trip to HF Hub on first use; fastembed handles this. Confirm
   our nix sandbox does not block HF downloads at runtime, only at
   build time.
8. **Candle CUDA build complexity.** The `qwen3-cuda` workspace feature
   pulls in `candle-core/cuda` + `candle-nn/cuda` plus a direct
   `candle-core` dep. Verify they build cleanly under the `code`
   devshell before opening a PR — the nix sysroot configured for ORT
   may not satisfy Candle's CUDA crate. If the devshell is missing
   pieces, that is a devshell change, not a code change.

## Out of scope for this plan

- Adding remote API providers (OpenAI / Cohere / etc.). Tracked
  separately. Local Qwen3 will be enough to evaluate quality first.
- Replacing the chunker, parser, batcher, incremental indexer, or
  vector store. None of them need to change for this.
- Replacing fastembed-rs entirely with `EmbedAnything` or `embedrs`.
  Re-evaluate only if fastembed's Qwen3 path proves unreliable or
  abandoned.
- Migrating off ORT for the legacy ONNX models. They keep working as
  they do today.
