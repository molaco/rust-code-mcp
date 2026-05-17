# Multi-Provider Embedding Profiles Plan

## Goal

Make embedding models — especially API models reachable through OpenRouter —
configurable as **data**, not code. After this work:

- Adding an OpenRouter embedding model is a config change, no recompile.
- Built-in profiles are a table, not a 14-arm enum spread.
- Existing on-disk indexes keep working.
- Local Candle/ONNX models stay code-bound, because they genuinely need
  loader code (weights, tokenizer, fastembed model variant).

This plan supersedes the ad-hoc `OpenRouterCustom` idea. It absorbs a design
review with five findings; see "Review Findings Absorbed" below.

## Non-Goals

- No new local embedding runtime.
- No new local model. Adding a local ONNX/Candle model still needs code and
  is out of scope.
- No vector store schema change. Vectors stay keyed by embedder identity.
- No change to retrieval/search ranking logic.
- No `cargo fmt` or formatter run.

## Review Findings Absorbed

| # | Finding | Resolution in this plan |
|---|---|---|
| 1 | `from_identity()` (`src/tools/query_tools.rs:207`) must rebuild a *usable* backend from `metadata.json` alone — including query behavior — with no profile table loaded. | Identity string carries everything search needs: runtime, model id, dim, max_len, query policy. Phase 1. |
| 2 | `&'static str` profile fields conflict with user-defined (TOML) profiles. | One **owned** profile struct (`Arc<str>` fields). `EmbeddingBackend` becomes `Clone`, not `Copy`. Phase 2. |
| 3 | "New ONNX model = one row" is false; `src/embeddings/fastembed_cpu.rs:30` hardcodes `EmbeddingModel::BGESmallENV15Q`. | Dynamic profiles are **API-only**. Local models stay code-bound. Enforced in Phase 4. |
| 4 | OpenRouter uses `input_type` (`search_document`/`search_query`), never `format_query()`. A `query_prefix` field is local-only. | Query handling is a runtime-aware `QueryPolicy`, not a prefix. Phase 3. |
| 5 | `from_identity()` colon-splits into exactly 5 fields (`src/embeddings/backend.rs:313`); arbitrary OpenRouter model ids carry `:` (`:free`, `:nitro`). Silent mis-parse. | New round-trip-safe identity format with a percent-encoded model id and a schema version. Phase 1, done first. |

## Current State

- `EmbeddingProfile` is a `Copy` enum with 5 variants; per-model data lives in
  ~9 match methods (`name`, `parse`, `runtime`, `model`, `default_max_len`,
  `default_chunk_target_tokens`, `default_chunk_hard_max_tokens`,
  `query_formatting`, `accepted_names`) — `src/embeddings/backend.rs`.
- `EmbeddingModelSpec` is a `Copy` enum with 5 match methods (`dim`,
  `display_name`, `provider_model_id`, `openrouter_model_id`, `qwen3_variant`).
- `EmbeddingBackend` is `Copy` and stored by value inside each embedder
  (`backend: *backend` in `fastembed_cpu.rs`, `openrouter.rs`).
- `identity()` / `from_identity()` use a 5-field colon-delimited string.
- `OpenRouterEmbedder::new` picks the model only from
  `EmbeddingModelSpec::openrouter_model_id()`, which returns `Some` only for
  `Qwen3Embedding8B` (`src/embeddings/openrouter.rs:237`).
- Profile is selected by name string in two call sites:
  `src/tools/index_tool.rs:51` and `src/tools/query_tools.rs:268`, both via
  `EmbeddingProfile::parse`.
- `QueryFormatting` is used by the two local embedders only
  (`fastembed_cpu.rs:64`, `qwen3.rs:100`); OpenRouter ignores it.

## Guardrails

1. Do not break existing indexes: `from_identity()` must still parse the three
   legacy identity strings (`fastembed-candle:…:v2`, `fastembed-onnx-cpu:…:v1`,
   `openrouter:…:v1`).
2. Do not change local Candle/ONNX runtime behavior.
3. No Nix devshell change. No new heavy dependency; a percent-encoding helper
   is implemented locally or uses an already-present crate.
4. No `cargo fmt`.
5. Before any build/test command, confirm the Nix shell and run as:
   `nix develop ../nix-devshells#<shell> --command <command>`.
6. Never log API keys.
7. Each phase: start with `jj show --summary`, update this file's phase
   status, commit separately.

---

## Phase 1: Round-Trip-Safe Identity Format

Status: Not started.

Rationale: this is the precondition for everything else. A model id that
cannot survive `identity()` -> `from_identity()` corrupts both cache paths and
search-time backend reconstruction. Do it first, in isolation.

Design:

- New identity schema, version `2`, format:

  ```text
  emb;v=2;rt=<runtime>;model=<percent-encoded-model-id>;dim=<n>;max=<n>;query=<policy>
  ```

  - Joined by `;`, each field `key=value`. Parse is delimiter-split then
    key lookup — order-independent and extensible.
  - `model` value is percent-encoded so `/`, `:`, and any reserved char
    round-trip exactly and the whole string stays filesystem-safe (identity is
    used in cache paths).
  - `query` records the resolved query policy so search can reconstruct it
    without the profile table (review finding 1).
- Keep a legacy parser: `from_identity()` first tries the v2 format, then
  falls back to the existing 5-field colon parser for the three known legacy
  identities. Legacy identities never re-serialize; new writes use v2.
- `EMBEDDER_VERSION` / `metadata.json` `embedder_version` now hold the v2
  string.

Files:

- `src/embeddings/backend.rs` — `identity()`, `from_identity()`, percent
  encode/decode helper (local, small).
- tests in `src/embeddings/backend.rs`.

Implementation steps:

1. Add `percent_encode_model` / `percent_decode_model` helpers (encode all
   non-`[A-Za-z0-9._-]` bytes).
2. Rewrite `identity()` to emit the v2 string.
3. Rewrite `from_identity()`:
   - If string starts with `emb;v=2;`, parse v2 key/value fields.
   - Else parse the legacy 5-field colon form (existing logic, kept verbatim).
4. Reject unknown schema versions with a clear error.

Acceptance criteria:

- v2 identity round-trips for a model id containing `/` and `:` (e.g.
  `meta-llama/llama-x:free`).
- All three legacy identities still parse to the correct backend.
- `from_identity` rejects malformed/unknown-version strings without panic.
- Existing `from_identity` round-trip tests still pass.

---

## Phase 2: Table-Driven Built-In Profiles

Status: Not started.

Design:

- `EmbeddingProfile` becomes an **owned struct**, not an enum:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub struct EmbeddingProfile {
      pub name: Arc<str>,
      pub runtime: EmbeddingRuntime,      // stays a typed enum
      pub model_id: Arc<str>,             // provider/model id, arbitrary
      pub dim: usize,
      pub max_len: usize,
      pub query_policy: QueryPolicy,      // Phase 3
      pub chunk_target_tokens: usize,
      pub chunk_hard_max_tokens: usize,
      pub local_loader: Option<LocalLoaderSpec>, // None => API runtime
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum LocalLoaderSpec {
      Qwen3(Qwen3Variant),
      FastembedCpu(FastembedCpuModel), // wraps the BGE variant today
  }
  ```

- `EmbeddingRuntime` stays the typed enum — it controls real code paths.
- `EmbeddingModelSpec` and the `QueryFormatting` enum are removed; their data
  moves into profile fields. `Qwen3Variant` is kept (loader detail).
- Built-in profiles live in a registry built once
  (`std::sync::LazyLock<Vec<EmbeddingProfile>>` or a builder fn). One struct
  literal per profile replaces the 14 match arms.
- `EmbeddingBackend` loses `Copy`, gains `Clone`. Update by-value uses
  (`*backend` -> `backend.clone()` in `fastembed_cpu.rs`, `openrouter.rs`,
  any others surfaced by the compiler).
- `EmbeddingProfile::parse(name)` -> registry lookup by `name` (and accepted
  aliases). `accepted_names()` lists registry names.

Files:

- `src/embeddings/backend.rs` (bulk of the change).
- `src/embeddings/mod.rs` — re-export updates (drop `EmbeddingModelSpec`,
  `QueryFormatting`; add `QueryPolicy`, `LocalLoaderSpec`).
- `src/embeddings/qwen3.rs`, `fastembed_cpu.rs`, `openrouter.rs` — read loader
  data from `backend.profile` fields; fix `Copy` -> `Clone` ripple.
- `src/tools/index_tool.rs`, `src/tools/query_tools.rs` — `parse` call sites.
- `src/config/indexer.rs` — `with_embedding_profile` takes the struct.

Implementation steps:

1. Define the new `EmbeddingProfile` struct, `LocalLoaderSpec`,
   `FastembedCpuModel`.
2. Build the built-in registry with the five current profiles, values copied
   verbatim from today's match arms (no behavior change).
3. Replace `EmbeddingProfile`/`EmbeddingModelSpec` method calls across the
   crate with field reads.
4. Change `EmbeddingBackend` to `Clone`; fix every compiler error from the
   dropped `Copy`.
5. Re-point `identity()` field sources at the struct fields.

Acceptance criteria:

- `cargo check --lib` and `--tests` pass.
- Every built-in profile's `identity()` is byte-identical to Phase 1 output
  for that profile (proves no data drift in the table).
- `from_identity` round-trip tests still pass for all built-ins.
- No change to local GPU / CPU embedding behavior.

---

## Phase 3: Runtime-Aware Query Policy

Status: Not started.

Design:

- Replace `QueryFormatting` with `QueryPolicy`:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub enum QueryPolicy {
      /// Local: prepend an instruction prefix to query text.
      InstructionPrefix(Arc<str>),
      /// API: send distinct input_type values for doc vs query.
      InputType { document: Arc<str>, query: Arc<str> },
      /// No query-side transformation.
      None,
  }
  ```

- Local embedders (`qwen3.rs`, `fastembed_cpu.rs`) use `InstructionPrefix`;
  `format_query()` becomes a `QueryPolicy` method.
- `OpenRouterEmbedder` currently hardcodes `"search_document"` /
  `"search_query"` (`openrouter.rs:300,308`). These move into the profile as
  `QueryPolicy::InputType { .. }` and the embedder reads them from the
  profile.
- The policy is encoded in the identity `query=` field (Phase 1) so search
  reconstructs it without the registry.

Files:

- `src/embeddings/backend.rs` — `QueryPolicy`, profile field.
- `src/embeddings/qwen3.rs`, `fastembed_cpu.rs` — use `InstructionPrefix`.
- `src/embeddings/openrouter.rs` — read `input_type` pair from policy.

Acceptance criteria:

- Local query formatting output is unchanged (existing prefix tests pass).
- OpenRouter still sends `search_document` / `search_query` for the built-in
  profile.
- `query=` round-trips through identity for each policy variant.

---

## Phase 4: Dynamic (User-Defined) API Profiles

Status: Not started.

Design:

- A user can define extra profiles in a TOML file. Resolution order:
  1. `RUST_CODE_MCP_EMBEDDING_PROFILES` env var pointing at a TOML path.
  2. `embedding_profiles.toml` in the indexed project root, if present.
- Each TOML profile is **API-only**: `runtime` must be `openrouter`.
  `local_loader` is forced `None`. A TOML profile naming a local runtime is
  rejected with a clear error (review finding 3).
- TOML shape:

  ```toml
  [[profile]]
  name = "openrouter-e5-large"
  model_id = "intfloat/multilingual-e5-large"
  dim = 1024
  max_len = 512
  # optional; default = InputType { search_document, search_query }
  query_document = "search_document"
  query_input = "search_query"
  # optional chunk overrides; sensible API defaults if omitted
  chunk_target_tokens = 768
  chunk_hard_max_tokens = 1024
  ```

- User profiles are merged into the registry at load. A user `name` colliding
  with a built-in is an error (no silent override).
- Profile resolution (`parse`) consults the merged registry. If a name is not
  found, the error lists built-ins and notes how to add a TOML profile.
- `dim` is mandatory: the vector store is keyed by it and there is no safe
  default for an arbitrary model.

Files:

- new `src/embeddings/profile_registry.rs` (TOML load + merge + lookup).
- `src/embeddings/backend.rs` — `parse` goes through the registry.
- `src/embeddings/mod.rs` — re-exports.
- `src/config/indexer.rs` — pass the project root so a local
  `embedding_profiles.toml` can be discovered.

Implementation steps:

1. Define `serde` structs for the TOML file.
2. Implement registry load: built-ins first, then TOML overlay, with
   collision and API-only validation.
3. Make `EmbeddingProfile::parse` (or a new `resolve` fn) registry-backed.
4. Surface clear errors: bad path, bad TOML, local runtime in TOML, name
   collision, missing `dim`.

Acceptance criteria:

- A TOML-defined OpenRouter profile indexes and searches without recompiling.
- A TOML profile with a local runtime is rejected.
- A name collision with a built-in is rejected.
- Missing/invalid TOML path produces a clear error, not a panic.
- With no TOML present, behavior is identical to Phase 2.

---

## Phase 5: Generalize the OpenRouter Model Path

Status: Not started.

Design:

- `OpenRouterEmbedder::new` currently fails unless
  `EmbeddingModelSpec::openrouter_model_id()` is `Some`. After Phase 2 the
  model id is just `backend.profile.model_id` — an arbitrary string.
- Remove the `openrouter_model_id()` allow-list. Any profile with
  `runtime == OpenRouter` sends `profile.model_id` directly as the request
  `model`.
- Keep the existing request planner, concurrency, metrics, base64, and
  provider-routing code unchanged — they are model-agnostic.

Files:

- `src/embeddings/openrouter.rs` — drop the allow-list lookup; read
  `model_id` from the profile.

Acceptance criteria:

- The built-in `openrouter-qwen3-8b` profile behaves exactly as today.
- A dynamic OpenRouter profile reaches the API with its configured model id.
- No API key is logged.

---

## Phase 6: Tests And Verification

Status: Not started.

Unit tests:

- Identity v2 round-trip incl. model ids with `/` and `:`.
- Legacy identity parsing for all three pre-existing identities.
- Built-in registry: identities unique, dims correct, no drift vs Phase 1.
- `QueryPolicy` round-trip per variant.
- TOML registry: valid overlay, local-runtime rejection, name-collision
  rejection, missing-`dim` rejection, bad-path error.
- `EmbeddingProfile::parse` resolves built-ins and aliases.

Verification commands (after confirming the Nix shell):

```sh
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo check --lib'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo check --tests'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo test embeddings:: --lib'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo test indexing::embedding_batcher --lib'
```

Live check (needs an OpenRouter key): index a small tree with a TOML-defined
OpenRouter profile, confirm it indexes and `search` returns results.

Acceptance criteria:

- `cargo check --lib` and `--tests` pass.
- Embedding and batcher test suites pass.
- A previously built index (legacy identity) still loads for search.

---

## Phase 7: Documentation

Status: Not started.

- Document `embedding_profiles.toml` and `RUST_CODE_MCP_EMBEDDING_PROFILES`
  in the embeddings docs / proposal docs.
- Add a short follow-up report under `.docs/` after verification: built-in
  profile list, the new identity format, and how to add an API model.
- Note that local models remain code-bound by design.

Acceptance criteria:

- A reader can add an OpenRouter embedding model from docs alone, no source
  reading.

---

## Expected Code Change Scope

Likely touched:

- `src/embeddings/backend.rs` (largest change)
- `src/embeddings/mod.rs`
- `src/embeddings/openrouter.rs`
- `src/embeddings/qwen3.rs`
- `src/embeddings/fastembed_cpu.rs`
- new `src/embeddings/profile_registry.rs`
- `src/tools/index_tool.rs`
- `src/tools/query_tools.rs`
- `src/config/indexer.rs`
- `.plans/multi-provider-plan.md`
- `.docs/` follow-up report

Possibly:

- `Cargo.toml` — only if a `toml` dependency is not already present.

Not expected:

- No Nix devshell change.
- No vector store schema change.
- No new local runtime or local model.

## Completion Criteria

1. Adding an OpenRouter embedding model is a TOML/config change, no recompile.
2. Built-in profiles are table-driven; identity has no data drift.
3. The identity format round-trips arbitrary model ids and stays
   filesystem-safe.
4. Existing on-disk indexes still load for search.
5. Local Candle/ONNX behavior is unchanged.
6. Dynamic profiles are restricted to API runtimes with clear validation.
7. Verification passes and the feature is documented.
