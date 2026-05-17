# Qwen3 Follow-Up Build Fix Plan

## Goal

Keep the completed Phase 1-7 embedding-profile implementation, but restore predictable builds and runtime behavior by fixing the ONNX Runtime dependency strategy introduced for `local-cpu-small`.

## Current State

- The working copy is clean on top of `c1b84e4b` (`qwen3 follow-up phase 7 tests`).
- Phases 1-7 are implemented and committed.
- Phase 8 benchmark work was not committed and should be redone later.
- The build-sensitive Phase 7 delta from the pre-plan baseline is the ONNX Runtime setup in `Cargo.toml`.

## Problem

The original pre-plan build used Qwen3/Candle/CUDA while preventing ONNX Runtime from entering the link path:

```toml
fastembed = { version = "5.13.4", default-features = false, features = ["hf-hub-native-tls", "qwen3", "cuda"] }
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["alternative-backend"] }
```

Phase 5 changed this to:

```toml
fastembed = { version = "5.13.4", default-features = false, features = [
  "hf-hub-native-tls",
  "ort-download-binaries-native-tls",
  "qwen3",
  "cuda",
] }
```

That makes ONNX Runtime part of the default build/link path and relies on downloaded ORT artifacts in `~/.cache/ort.pyke.io`. This is brittle in the CUDA devshell and changes the previous build shape more than necessary.

## Target Design

Use dynamic ONNX Runtime loading instead of downloaded ORT binaries.

Cargo should:

- Keep Qwen3/Candle/CUDA as the default local GPU path.
- Keep OpenRouter support through `reqwest`.
- Keep `local-cpu-small` available.
- Avoid `ort-download-binaries-native-tls` in the default build.
- Avoid `ort alternative-backend`, because `local-cpu-small` needs real ORT APIs.
- Use `ort-load-dynamic` so ONNX Runtime is loaded from the Nix shell at runtime.

Nix should:

- Provide `pkgs.onnxruntime`.
- Export `ORT_DYLIB_PATH` to `${pkgs.onnxruntime}/lib/libonnxruntime.so`.
- Include `${pkgs.onnxruntime}/lib` in `LD_LIBRARY_PATH`.
- Keep all existing CUDA env from `cuda-code.nix`.

## Phase 1: Fix Cargo ORT Strategy

Status: Implemented.

Implementation notes:

- `Cargo.toml` now uses `ort-load-dynamic` instead of `ort-download-binaries-native-tls`.
- `hf-hub-native-tls`, `qwen3`, and `cuda` remain enabled.
- `reqwest` remains enabled for OpenRouter.
- `ort alternative-backend` was not restored.
- `Cargo.lock` was refreshed by Cargo from inside `nix develop ../nix-devshells#cuda-code`; the ORT download-only dependencies were removed from the lockfile.

Implementation steps:

1. In `Cargo.toml`, replace `ort-download-binaries-native-tls` with `ort-load-dynamic` in the `fastembed` feature list.
2. Keep `hf-hub-native-tls`, `qwen3`, and `cuda`.
3. Keep `reqwest` for OpenRouter.
4. Do not restore `ort alternative-backend`.
5. Update `Cargo.lock` through Cargo only from inside the CUDA devshell.

Expected `fastembed` dependency:

```toml
fastembed = { version = "5.13.4", default-features = false, features = [
  "hf-hub-native-tls",
  "ort-load-dynamic",
  "qwen3",
  "cuda",
] }
```

Acceptance criteria:

- Cargo no longer enables `ort-sys/download-binaries`.
- Cargo enables `ort/load-dynamic`, which also disables static ORT linking through `ort-sys/disable-linking`.
- Default Qwen3 CUDA feature remains enabled.

## Phase 2: Update CUDA Devshell

Status: Implemented.

Implementation notes:

- `/home/molaco/Documents/nix-devshells/devshells/cuda-code.nix` now includes `onnxruntime` in `extraPackages`.
- The interactive shell exports `ORT_DYLIB_PATH="${pkgs.onnxruntime}/lib/libonnxruntime.so"`.
- The interactive shell prepends `${pkgs.onnxruntime}/lib` to `LD_LIBRARY_PATH`.
- The MCP server env now sets the same `ORT_DYLIB_PATH`.
- The MCP server env now includes `${pkgs.onnxruntime}/lib` in `LD_LIBRARY_PATH`.
- Existing CUDA paths and `/run/opengl-driver/lib` were preserved.
- Existing unrelated changes in `/home/molaco/Documents/nix-devshells` were not touched.

Implementation steps:

1. Edit `/home/molaco/Documents/nix-devshells/devshells/cuda-code.nix`.
2. Add `pkgs.onnxruntime` to `extraPackages`.
3. Add `ORT_DYLIB_PATH` to the interactive `shellHook`.
4. Prepend `${pkgs.onnxruntime}/lib` to `LD_LIBRARY_PATH` in `shellHook`.
5. Add the same `ORT_DYLIB_PATH` and `${pkgs.onnxruntime}/lib` path to the MCP server env.
6. Preserve the existing CUDA paths, `/run/opengl-driver/lib`, `cuda_cudart`, `libcublas`, and `cudnn`.

Target shell additions:

```sh
export ORT_DYLIB_PATH="${pkgs.onnxruntime}/lib/libonnxruntime.so"
export LD_LIBRARY_PATH="${pkgs.onnxruntime}/lib:$LD_LIBRARY_PATH"
```

Acceptance criteria:

- Qwen3 CUDA runtime still sees CUDA libraries.
- BGE CPU runtime can load ONNX Runtime from the Nix store.
- No hardcoded `~/.cache/ort.pyke.io` path is needed in `cuda-code.nix`.

## Phase 3: Verify Build From Correct Shell

Status: Verified.

Verification notes:

- The user confirmed `../nix-devshells#cuda-code` for project builds.
- `nix develop ../nix-devshells#cuda-code --command zsh -lc 'cargo check --lib'` passed with existing warnings.
- `nix develop ../nix-devshells#cuda-code --command zsh -lc 'cargo check --tests'` passed with existing warnings.
- Nix printed `Git tree '/home/molaco/Documents/nix-devshells' is dirty` because the devshell changes were still uncommitted during verification.

Run all verification from:

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc '<command>'
```

Verification commands:

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc 'cargo check --lib'
nix develop ../nix-devshells#cuda-code --command zsh -lc 'cargo check --tests'
```

Do not run Cargo directly from the ambient shell for GPU/default-profile checks.

Acceptance criteria:

- `cargo check --lib` passes in `cuda-code`.
- `cargo check --tests` passes in `cuda-code`, allowing existing warnings.
- Link lines no longer depend on ORT cache downloads for the default build.

## Phase 4: Runtime Smoke Checks

Status: Verified.

Implementation notes:

- Added `examples/embedding_profile_smoke.rs` as a small runtime smoke entry point.
- The smoke example creates a temporary one-file Rust codebase under `/tmp`, initializes the requested profile, clears its temporary index data, and runs `IncrementalIndexer::index_with_change_detection`.
- The smoke example is intentionally separate from benchmark examples and does not collect throughput metrics.
- A compile-only issue in the new smoke example's OpenRouter expected-error guard was found on the first GPU smoke attempt and fixed before rerunning.

Verification commands and results:

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc 'cargo run --example embedding_profile_smoke -- local-gpu-small'
```

Result:

- `profile=local-gpu-small`
- `identity=fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2`
- `indexed_files=1`
- `total_chunks=1`
- `smoke=ok`

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc 'cargo run --example embedding_profile_smoke -- local-cpu-small'
```

Result:

- `profile=local-cpu-small`
- `identity=fastembed-onnx-cpu:BGESmallENV15Q:dim384:max512:v1`
- `indexed_files=1`
- `total_chunks=1`
- `smoke=ok`

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc 'if [ -n "${RUST_CODE_MCP_OPENROUTER_API_KEY:-}${OPENROUTER_API_KEY:-}" ]; then cargo run --example embedding_profile_smoke -- openrouter-qwen3-8b; else cargo run --example embedding_profile_smoke -- openrouter-qwen3-8b --expect-missing-key; fi'
```

Result in this shell:

- `profile=openrouter-qwen3-8b`
- `identity=openrouter:qwen/qwen3-embedding-8b:dim4096:max32768:v1`
- `smoke=expected_missing_openrouter_key`
- Clear error text: `missing OpenRouter API key; set RUST_CODE_MCP_OPENROUTER_API_KEY or OPENROUTER_API_KEY`

Only after Phase 3 passes:

1. Run a small default profile check through the CUDA shell.
2. Run a small `local-cpu-small` initialization/indexing check through the CUDA shell.
3. If an OpenRouter key is configured, run a small `openrouter-qwen3-8b` check.

Acceptance criteria:

- `local-gpu-small` still initializes Qwen3/CUDA.
- `local-cpu-small` initializes `BGESmallENV15Q` through Nix-provided ONNX Runtime.
- OpenRouter still fails clearly when no API key is configured.

## Phase 5: Redo Phase 8 Benchmark Work

Status: Pending; should remain separate from this build fix.

Redo benchmark/profile work only after the build fix is committed.

Rules:

- Do not reintroduce the uncommitted Cargo `cuda` feature split unless explicitly needed.
- Do not run Cargo outside `nix develop ../nix-devshells#cuda-code` for GPU/default checks.
- Keep benchmark example changes separate from the build-fix commit.

Acceptance criteria:

- Benchmark changes are isolated from dependency/Nix fixes.
- Phase 8 metrics are collected from the correct shell.

## Not Needed

- Do not redo Phases 1-4.
- Do not redo Phase 6.
- Do not redo Phase 7 tests.
- Do not add ONNX Runtime cache paths from `code.nix` to `cuda-code.nix`.
- Do not use `ort-download-binaries-native-tls` for the default build.
- Do not use `ort alternative-backend` if `local-cpu-small` must run.

## Commit Plan

1. Commit Cargo ORT strategy fix.
2. Commit `cuda-code.nix` ONNX Runtime environment update, if the Nix repo is managed separately.
3. Commit verification notes in this plan.
4. Redo and commit Phase 8 benchmark work separately.
