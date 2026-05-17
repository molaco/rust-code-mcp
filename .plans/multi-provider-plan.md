# Multi-Provider Embedding Profiles Plan

## Goal

Make embedding models — especially API models reachable through OpenRouter —
configurable as **data**, not code. After this work:

- Adding an OpenRouter embedding model is a config change, no recompile.
- Built-in profiles are a table, not a 14-arm enum spread.
- Existing on-disk indexes keep working.
- Local Candle/ONNX models stay code-bound, because they genuinely need
  loader code (weights, tokenizer, fastembed model variant).

This plan absorbs two rounds of design review (ten findings total); see
"Review Findings Absorbed" below.

## Non-Goals

- No new local embedding runtime.
- No new local model. Adding a local ONNX/Candle model still needs code and
  is out of scope.
- No vector store schema change. Vectors stay keyed by embedder identity.
- No change to retrieval/search ranking logic.
- No `cargo fmt` or formatter run.

## Review Findings Absorbed

### Round 1 — design shape

| # | Finding | Resolution |
|---|---|---|
| R1 | `from_identity()` must rebuild a *usable* backend from `metadata.json` alone. | Identity carries runtime/model/dim/max/query; local loader recovered via built-in resolver — Phase 1 + Phase 4. |
| R2 | `&'static str` profile fields conflict with user-defined profiles. | One **owned** profile struct (`Arc<str>`). `EmbeddingBackend` becomes `Clone`. Phase 2. |
| R3 | "New ONNX model = one row" is false (`fastembed_cpu.rs:30` hardcodes the model). | Dynamic profiles are **API-only**; local models stay code-bound. Phase 5. |
| R4 | OpenRouter uses `input_type`, never `format_query()`. | Query handling is a runtime-aware `QueryPolicy`. Phase 3. |
| R5 | `from_identity()` colon-splits; arbitrary model ids carry `:`. | Round-trip-safe identity codec. Phase 1. |

### Round 2 — sequencing and reach

| # | Finding | Resolution |
|---|---|---|
| S1 | Phase 1 was not isolated: it asserted round-tripping arbitrary model ids before `EmbeddingBackend` could hold them. | Phase 1 is now a **pure codec** (`EmbeddingIdentity` struct + encode/decode), tested on raw field values. Wiring into `EmbeddingBackend` moves to Phase 4, after the data model exists. |
| S2 | `from_identity()` underspecified for local runtimes: identity has no `local_loader`; Qwen3 needs a `Qwen3Variant` (`qwen3.rs:22`), CPU hardcodes the fastembed model (`fastembed_cpu.rs:29`). | Identity does **not** encode `local_loader`. For local runtimes, `from_identity()` recovers the loader by resolving `(runtime, model_id)` against the built-in registry — local models are built-in-only, so this always succeeds. Phase 4. |
| S3 | TOML discovery needs directory context at the real parse call sites. `resolve_requested_backend` (`query_tools.rs:354`) and `resolve_backend` (`index_tool.rs:46`) resolve a backend with no directory. | Phase 5 threads the project root into both resolver functions and into registry resolution — not only `config/indexer`. |
| S4 | Background sync (`mcp/sync.rs:132`) always syncs with `EmbeddingBackend::default()`; a non-default-profile index goes stale or gets overwritten. | New Phase 7: sync resolves the backend from the existing index `metadata.json`, not `default()`. |
| S5 | Identity codec only specified encoding for `model`. `query=` can contain `=`, `;`, newlines once `QueryPolicy::InstructionPrefix(Arc<str>)` exists. | Phase 1 codec percent-encodes **every** string-valued field, not just `model`. |

## Current State

- `EmbeddingProfile` is a `Copy` enum (5 variants); per-model data lives in
  ~9 match methods — `src/embeddings/backend.rs`.
- `EmbeddingModelSpec` is a `Copy` enum with 5 match methods.
- `EmbeddingBackend` is `Copy`, stored by value in each embedder
  (`backend: *backend`).
- `identity()` / `from_identity()` use a 5-field colon-delimited string
  (`src/embeddings/backend.rs:284,313`).
- `Qwen3Embedder::new` calls `backend.require_qwen3_variant()`
  (`src/embeddings/qwen3.rs:22`); `FastembedCpuEmbedder::new` hardcodes
  `EmbeddingModel::BGESmallENV15Q` (`src/embeddings/fastembed_cpu.rs:30`).
- `OpenRouterEmbedder::new` picks the model only from
  `EmbeddingModelSpec::openrouter_model_id()`, `Some` only for the 8B model
  (`src/embeddings/openrouter.rs:237`).
- Profile is selected by name at `src/tools/index_tool.rs:51`
  (`resolve_backend`, no directory arg) and `src/tools/query_tools.rs:354`
  (`resolve_requested_backend`, no directory arg).
- `QueryFormatting` is used only by the two local embedders
  (`fastembed_cpu.rs:64`, `qwen3.rs:100`); OpenRouter ignores it.
- Background sync `sync_directory` builds `EmbeddingBackend::default()`
  unconditionally (`src/mcp/sync.rs:132`).

## Guardrails

1. Do not break existing indexes: `from_identity()` must still parse the three
   legacy identity strings (`fastembed-candle:…:v2`, `fastembed-onnx-cpu:…:v1`,
   `openrouter:…:v1`).
2. Do not change local Candle/ONNX runtime behavior.
3. No Nix devshell change. No new heavy dependency; a percent-encoding helper
   is implemented locally.
4. No `cargo fmt`.
5. Before any build/test command, confirm the Nix shell and run as:
   `nix develop ../nix-devshells#<shell> --command <command>`.
6. Never log API keys.
7. Secrets stay strictly in environment variables. `embedding_profiles.toml`
   carries model **metadata only** — never an API key, token, or credential.
   The TOML loader rejects unknown keys, so a stray `api_key` field is a hard
   parse error rather than a silently-stored secret.
8. Each phase: start with `jj show --summary`, update this file's phase
   status, commit separately.

## Phase Dependency Order

```
P1 codec ─┐
P2 data model ─┬─> P3 query policy ─> P4 identity wiring ─> P5 dynamic profiles
              │                                          ├─> P6 openrouter generalize
              └──────────────────────────────────────────┘
P4 ─> P7 sync ; all ─> P8 verify ─> P9 docs
```

P1 and P2 are independent and may be done in either order. P4 needs P1+P2+P3.

---

## Phase 1: Identity Codec (Isolated)

Status: Implemented; build/test verification pending confirmed Nix shell.

Rationale: a pure string codec with no dependency on the profile/backend data
model. This removes the Round-2/S1 sequencing trap — it is tested on raw field
values, not on `EmbeddingBackend`.

Design:

- New module `src/embeddings/identity.rs` with:

  ```rust
  pub struct EmbeddingIdentity {
      pub runtime: EmbeddingRuntime, // existing enum, the only typed field
      pub model_id: String,
      pub dim: usize,
      pub max_len: usize,
      pub query: String,            // serialized QueryPolicy tag (Phase 3)
  }
  ```

- `encode(&self) -> String` emits schema version `2`:

  ```text
  emb;v=2;rt=<runtime>;model=<enc>;dim=<n>;max=<n>;query=<enc>
  ```

  - `;`-joined `key=value` fields; parse is split-then-lookup, order
    independent and forward-extensible.
  - **Every string-valued field** (`model`, `query`) is percent-encoded:
    all bytes outside `[A-Za-z0-9._-]` are `%XX`-escaped. This makes the
    string unambiguous to split *and* filesystem-safe (identity is used in
    cache paths). Covers Round-2/S5.
- `decode(&str) -> Result<EmbeddingIdentity, _>` parses v2 and rejects
  unknown schema versions with a clear error.
- Legacy parsing (the three colon-delimited pre-existing identities) is **not**
  in this module; it stays a `from_identity()` concern handled in Phase 4.

Files:

- new `src/embeddings/identity.rs`
- `src/embeddings/mod.rs` — `mod identity;` (no public re-export yet)

Acceptance criteria:

- `encode` then `decode` round-trips for `model_id`/`query` values containing
  `/`, `:`, `=`, `;`, spaces, and a newline.
- The encoded string contains no character outside a filesystem-safe set.
- `decode` rejects malformed input and unknown schema versions without panic.
- This phase compiles and tests independently of Phases 2-4.

---

## Phase 2: Table-Driven Built-In Profile Data Model

Status: Implemented; build/test verification pending confirmed Nix shell.

Design:

- `EmbeddingProfile` becomes an **owned struct**:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub struct EmbeddingProfile {
      pub name: Arc<str>,
      pub runtime: EmbeddingRuntime,        // stays a typed enum
      pub model_id: Arc<str>,               // arbitrary provider/model id
      pub dim: usize,
      pub max_len: usize,
      pub query_policy: QueryPolicy,        // filled in Phase 3
      pub chunk_target_tokens: usize,
      pub chunk_hard_max_tokens: usize,
      pub local_loader: Option<LocalLoaderSpec>, // None => API runtime
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum LocalLoaderSpec {
      Qwen3(Qwen3Variant),
      FastembedCpu(FastembedCpuModel),
  }
  ```

- `EmbeddingRuntime` stays the typed enum — it controls real code paths.
- `EmbeddingModelSpec` and the `QueryFormatting` enum are removed; their data
  moves into profile fields. `Qwen3Variant` is kept (loader detail).
- Built-in profiles are a registry built once
  (`std::sync::LazyLock<Vec<EmbeddingProfile>>`). One struct literal per
  profile replaces the 14 match arms.
- Built-in profile data also carries tokenizer model metadata so the
  OpenRouter Qwen3 profile keeps the existing tokenizer source while request
  model ids remain provider-specific.
- `EmbeddingBackend` loses `Copy`, gains `Clone`. Update by-value uses
  (`*backend` -> `backend.clone()` in `qwen3.rs`, `fastembed_cpu.rs`,
  `openrouter.rs`, plus anything the compiler surfaces).
- `EmbeddingProfile::parse(name)` -> registry lookup by `name` + aliases.

Files:

- `src/embeddings/backend.rs` (bulk of the change)
- `src/embeddings/mod.rs` — re-export updates (drop `EmbeddingModelSpec`,
  `QueryFormatting`; add `LocalLoaderSpec`)
- `src/embeddings/qwen3.rs`, `fastembed_cpu.rs`, `openrouter.rs` — read loader
  data from profile fields; fix the `Copy` -> `Clone` ripple
- `src/tools/index_tool.rs`, `src/tools/query_tools.rs` — `parse` call sites
- `src/config/indexer.rs` — `with_embedding_profile` takes the struct

Implementation steps:

1. Define `EmbeddingProfile` struct, `LocalLoaderSpec`, `FastembedCpuModel`.
2. Build the built-in registry with the five current profiles, values copied
   verbatim from today's match arms (no behavior change).
3. Replace `EmbeddingProfile`/`EmbeddingModelSpec` method calls across the
   crate with field reads.
4. Change `EmbeddingBackend` to `Clone`; fix every dropped-`Copy` error.

Acceptance criteria:

- `cargo check --lib` and `--tests` pass.
- No change to local GPU / CPU / OpenRouter embedding behavior.
- Built-in profile dims, max_len, model ids match today's values exactly.

---

## Phase 3: Runtime-Aware Query Policy

Status: Implemented; build/test verification pending confirmed Nix shell.

Design:

- Replace `QueryFormatting` with `QueryPolicy`:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub enum QueryPolicy {
      InstructionPrefix(Arc<str>),                       // local
      InputType { document: Arc<str>, query: Arc<str> }, // API
      None,
  }
  ```

- `QueryPolicy` has `encode_tag()` / `decode_tag()` producing the string
  stored in the identity `query=` field (consumed by the Phase 1 codec, which
  percent-encodes it).
- Local embedders (`qwen3.rs`, `fastembed_cpu.rs`) use `InstructionPrefix`;
  `format_query()` becomes a `QueryPolicy` method.
- `OpenRouterEmbedder` currently hardcodes `"search_document"` /
  `"search_query"`; these move into the built-in profile as
  `QueryPolicy::InputType { .. }` and the embedder reads them from the profile.

Files:

- `src/embeddings/backend.rs` — `QueryPolicy`, profile field
- `src/embeddings/qwen3.rs`, `fastembed_cpu.rs` — use `InstructionPrefix`
- `src/embeddings/openrouter.rs` — read `input_type` pair from policy

Acceptance criteria:

- Local query formatting output is unchanged (existing prefix tests pass).
- OpenRouter still sends `search_document` / `search_query`.
- `encode_tag` / `decode_tag` round-trips every `QueryPolicy` variant.

---

## Phase 4: Wire Identity Into `EmbeddingBackend`

Status: Implemented; build/test verification pending confirmed Nix shell.

Design:

- `EmbeddingBackend::identity()` builds an `EmbeddingIdentity` from the
  backend's profile fields and calls `encode()`.
- `EmbeddingBackend::from_identity()`:
  1. If the string is v2 (`emb;v=2;`), `decode()` it.
     - For an **API runtime**: construct the backend directly from the
       decoded fields; `local_loader = None`.
     - For a **local runtime**: the identity has no `local_loader`. Resolve
       `(runtime, model_id)` against the built-in registry to recover the
       `LocalLoaderSpec`. Local models are built-in-only, so a local identity
       that is not in the registry is a hard error (stale/foreign index).
       Covers Round-2/S2.
  2. Else fall back to the legacy 5-field colon parser for the three known
     pre-existing identities (logic kept verbatim from today). Legacy
     identities are never re-serialized; new writes are always v2.
- `metadata.json` `embedder_version` now holds the v2 string for new indexes.

Files:

- `src/embeddings/backend.rs` — `identity()`, `from_identity()`, legacy parser
- tests in `src/embeddings/backend.rs`

Acceptance criteria:

- v2 identity round-trips for built-in profiles and for an API model id
  containing `/` and `:`.
- All three legacy identities still parse to the correct backend with the
  correct `local_loader`.
- A v2 local identity whose `model_id` is unknown to the registry errors
  clearly (no panic, message suggests `clear_cache`).
- `query_tools.rs` `backend_from_metadata` path (the `from_identity` caller at
  `query_tools.rs:207`) still produces a usable backend for search.

---

## Phase 5: Dynamic (User-Defined) API Profiles

Status: Implemented; build/test verification pending confirmed Nix shell.

Design:

- Extra profiles defined in a TOML file. Resolution order:
  1. `RUST_CODE_MCP_EMBEDDING_PROFILES` env var -> TOML path.
  2. `embedding_profiles.toml` in the indexed project root, if present.
- Each TOML profile is **API-only**: `runtime` must be `openrouter`;
  `local_loader` is forced `None`. A TOML profile naming a local runtime is
  rejected (Round-1/R3).
- TOML shape:

  ```toml
  [[profile]]
  name = "openrouter-e5-large"
  model_id = "intfloat/multilingual-e5-large"
  dim = 1024                       # mandatory; vector store is keyed by it
  max_len = 512
  query_document = "search_document"  # optional
  query_input = "search_query"        # optional
  chunk_target_tokens = 768           # optional
  chunk_hard_max_tokens = 1024        # optional
  ```

- User profiles merge into the registry; a name colliding with a built-in is
  an error (no silent override).
- **Secrets never live in TOML.** The OpenRouter API key continues to come
  only from `RUST_CODE_MCP_OPENROUTER_API_KEY` / `OPENROUTER_API_KEY`. The
  `serde` structs use `#[serde(deny_unknown_fields)]`, so any credential-like
  key (`api_key`, `token`, `authorization`, …) fails the parse with a clear
  message instead of being read or stored.
- **Directory-aware resolution (Round-2/S3).** Backend resolution moves behind
  a `resolve_profile(name, project_root)` API. Both call sites are updated to
  pass the directory:
  - `resolve_backend` (`src/tools/index_tool.rs:46`) — already has the
    indexed directory in scope.
  - `resolve_requested_backend` (`src/tools/query_tools.rs:354`) — currently
    takes only `embedding_profile`; it gains a `dir_path` parameter (the
    caller already holds `dir_path`).
- The env-var TOML is global; the project-root TOML is per-directory, so the
  registry is resolved per request, not cached process-wide.

Files:

- new `src/embeddings/profile_registry.rs` — TOML load, merge, validation,
  `resolve_profile`
- `src/embeddings/backend.rs` — `parse` delegates to the registry
- `src/embeddings/mod.rs` — re-exports
- `src/tools/index_tool.rs`, `src/tools/query_tools.rs` — thread the directory
  into resolution
- `src/config/indexer.rs` — accept the resolved profile

Acceptance criteria:

- A TOML-defined OpenRouter profile indexes and searches without recompiling.
- A TOML profile with a local runtime is rejected with a clear message.
- A name collision with a built-in is rejected.
- Missing/invalid TOML path or missing `dim` produces a clear error, not a
  panic.
- A TOML profile containing any unknown field (including a credential-like
  key such as `api_key`) is rejected by `deny_unknown_fields`.
- With no TOML present, behavior is identical to Phase 4.

---

## Phase 6: Generalize the OpenRouter Model Path

Status: Implemented; OpenRouter now sends `profile.model_id` directly. Build/test verification pending confirmed Nix shell.

Design:

- `OpenRouterEmbedder::new` currently fails unless
  `openrouter_model_id()` is `Some`. After Phase 2 the model id is
  `backend.profile.model_id` — an arbitrary string.
- Remove the allow-list lookup; any profile with `runtime == OpenRouter`
  sends `profile.model_id` directly as the request `model`.
- Request planner, concurrency, metrics, base64, and provider routing are
  model-agnostic and unchanged.

Files:

- `src/embeddings/openrouter.rs`

Acceptance criteria:

- Built-in `openrouter-qwen3-8b` behaves exactly as today.
- A dynamic OpenRouter profile reaches the API with its configured model id.
- No API key is logged.

---

## Phase 7: Profile-Aware Background Sync

Status: Implemented; build/test verification pending confirmed Nix shell.

Rationale: Round-2/S4. `sync_directory` (`src/mcp/sync.rs:132`) builds
`EmbeddingBackend::default()` unconditionally. A directory indexed with a
non-default profile would be re-synced under the default profile — staleness,
or the wrong index updated.

Design:

- `sync_directory` resolves the backend from the **existing on-disk index**
  rather than `default()`:
  - `ProjectPaths` is keyed by embedder identity, so a directory can hold
    indexes for several profiles.
  - For each indexed profile present under the directory, read its
    `metadata.json` `embedder_version` and reconstruct the backend via
    `EmbeddingBackend::from_identity()` (the Phase 4 path).
  - Sync each existing per-profile index with its own backend.
- If a directory has no index yet, sync does nothing (unchanged).
- Tracking: if `track_directory` records only the path, extend it to discover
  all per-profile index dirs at sync time, or record `(path, identity)`.

Files:

- `src/mcp/sync.rs`
- possibly `src/tools/project_paths.rs` (enumerate per-profile index dirs)

Acceptance criteria:

- A directory indexed with a non-default profile is synced with that profile.
- A directory with multiple profile indexes syncs each correctly.
- A directory indexed with the default profile behaves as today.
- No default-profile index is created as a side effect of syncing a
  non-default-profile directory.

---

## Phase 8: Tests And Verification

Status: Not started.

Unit tests:

- Phase 1 codec: round-trip with reserved chars in all string fields;
  filesystem-safety; malformed/unknown-version rejection.
- Built-in registry: identities unique, dims correct, no drift vs Phase 1
  encoding.
- `QueryPolicy` tag round-trip per variant.
- `from_identity`: v2 round-trip; all three legacy identities; unknown local
  model id rejected.
- TOML registry: valid overlay, local-runtime rejection, name-collision
  rejection, missing-`dim` rejection, bad-path error.
- `resolve_profile` resolves built-ins, aliases, and a project-root TOML
  profile given a directory.

Verification commands (after confirming the Nix shell):

```sh
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo check --lib'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo check --tests'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo test embeddings:: --lib'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo test indexing::embedding_batcher --lib'
```

Live check (needs an OpenRouter key): index a small tree with a TOML-defined
OpenRouter profile; confirm `search` returns results; confirm background sync
updates that index, not a default-profile one.

Acceptance criteria:

- `cargo check --lib` and `--tests` pass.
- Embedding and batcher test suites pass.
- A previously built index (legacy identity) still loads for search.

---

## Phase 9: Documentation

Status: Not started.

- Document `embedding_profiles.toml` and `RUST_CODE_MCP_EMBEDDING_PROFILES`.
- Document the v2 identity format and the built-in profile list.
- Add a follow-up report under `.docs/` after verification.
- State that local models remain code-bound by design.

Acceptance criteria:

- A reader can add an OpenRouter embedding model from docs alone.

---

## Expected Code Change Scope

Likely touched:

- `src/embeddings/backend.rs` (largest change)
- new `src/embeddings/identity.rs`
- new `src/embeddings/profile_registry.rs`
- `src/embeddings/mod.rs`
- `src/embeddings/openrouter.rs`, `qwen3.rs`, `fastembed_cpu.rs`
- `src/tools/index_tool.rs`, `src/tools/query_tools.rs`
- `src/config/indexer.rs`
- `src/mcp/sync.rs` (+ possibly `src/tools/project_paths.rs`)
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
3. The identity codec round-trips arbitrary model ids and query policy and
   stays filesystem-safe.
4. Existing on-disk indexes still load for search.
5. Local Candle/ONNX behavior is unchanged.
6. Dynamic profiles are restricted to API runtimes with clear validation.
7. Background sync operates on the profile each index was built with.
8. Verification passes and the feature is documented.
