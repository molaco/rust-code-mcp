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
    Qwen3(Qwen3Variant),
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

Each variant must report its dimension via a single method
`fn dim(&self) -> usize`, owned by the backend enum. The static
`EMBEDDING_DIM` constant goes away.

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

`Cargo.toml:65`:

```toml
fastembed = { git = "https://github.com/Anush008/fastembed-rs", tag = "v5.13.4", features = ["qwen3"] }
```

Pin to a tag, not main. If 5.13.4 is on crates.io by the time we land
this, prefer that source. Decide based on `cargo search fastembed`
output at the time.

Add a workspace feature `qwen3` that forwards to `fastembed/qwen3` so we
can build a minimal binary without the heavy Candle dep if desired.
Default features keep ONNX only.

## Step-by-step

Each step ends green: `cargo check --lib` passes inside the nix
devshell.

### Step 1 — bump fastembed, no behavior change

- Edit `Cargo.toml:65` to pin a fastembed tag that contains the Qwen3
  work. Keep no extra features for now.
- Run `cargo check --lib` to confirm no API drift between 5.2.0 and the
  pinned tag. fastembed has been adding models, not breaking existing
  ones, but the `InitOptions::new` and `TextEmbedding::try_new`
  signatures are the load-bearing ones — read the changelog and the
  current `src/output.rs` / `src/text_embedding.rs` in the checkout to
  confirm.
- Fix any breakage in `src/embeddings/mod.rs` to keep `MiniLML6V2`
  working. No other file should need to change.

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
  `Qwen3Embedder { inner: fastembed::Qwen3TextEmbedding, dim: usize }`
  implementing `EmbedderImpl`.
- The whole file is `#[cfg(feature = "qwen3")]`.
- In `EmbeddingGenerator::with_backend`, the Qwen3 arm constructs this
  under the same cfg; the `#[cfg(not(feature = "qwen3"))]` arm returns
  `EmbeddingError::FeatureDisabled` with a clear message.
- Hugging Face model IDs:
  - `Qwen3Variant::Embedding0_6B` → `"Qwen/Qwen3-Embedding-0.6B"`
  - `Qwen3Variant::Embedding4B`   → `"Qwen/Qwen3-Embedding-4B"`
  - `Qwen3Variant::Embedding8B`   → `"Qwen/Qwen3-Embedding-8B"`
- Device choice for Qwen3 is Candle's `Device` — reuse the existing
  CUDA-available check so we pick `Device::cuda_if_available(0)` and
  fall back to `Device::Cpu`. Keep the same tracing lines we have today
  so logs stay greppable.

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

### Step 7 — smoke test

- Build with `--features qwen3` in the devshell.
- Run the binary against a small fixture directory using each of:
  - default (MiniLM, ONNX)
  - Qwen3-0.6B (Candle, GPU if available)
- Confirm the LanceDB table is created with the right `vector_size`
  (1024 for 0.6B vs 384 for MiniLM) by inspecting the table schema.
- Do **not** attempt 4B/8B on first run — verify the architecture works
  with 0.6B first, then size up.

## Risks and open questions

1. **fastembed API drift between 5.2.0 and 5.13.x.** The
   `TextEmbedding::try_new(InitOptions::new(model).with_*())` builder
   has been stable in spirit but field additions are possible. Step 1
   handles this; budget time for a small fixup.
2. **`Qwen3TextEmbedding` ergonomics.** Its `embed` method takes
   `&[&str]` and is synchronous on Candle. Confirm whether it supports
   batched inference and what the batch-size sweet spot is on our 8 GB
   GPU. The README example uses two strings; we will need real batches.
3. **Memory footprint.** Qwen3-4B and -8B will not fit alongside our
   current 5.5 GB ORT memory cap on an 8 GB card. If we ever enable
   them, the ORT cap needs to drop or be removed when Candle is the
   active backend, since they don't share the same memory pool.
4. **Dim mismatch on existing indexes.** If a user switches models
   without re-indexing, LanceDB will reject the new vectors. The MCP
   surface that toggles model choice (Step 6) must refuse to start when
   the existing table's `vector_size` ≠ the configured model's dim, and
   tell the user to clear the index. Do not auto-wipe.
5. **HF token / network.** Qwen3 download is gated behind a network
   round-trip to HF Hub on first use; fastembed handles this. Confirm
   our nix sandbox does not block HF downloads at runtime, only at
   build time.
6. **Candle CUDA build complexity.** Adding the `qwen3` feature pulls
   in `candle-core` + `candle-nn` + `candle-transformers`. Verify they
   build cleanly under the `code` devshell before opening a PR — the
   nix sysroot for ORT may not be enough for Candle's CUDA bindings.

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
