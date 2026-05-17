# Qwen3 Follow-Up Fix Plan Report

## Scope

Implemented `.plans/qwen3-follow-up-fix-plan.md` after Phase 1-7 of the Qwen3 follow-up work were already present.

The fix preserved the completed profile implementation and focused on:

- Removing ONNX Runtime download/link fragility from the default build.
- Providing ONNX Runtime through the CUDA Nix devshell.
- Verifying default GPU and test builds through the correct Nix shell.
- Adding runtime smoke coverage for the local GPU, local CPU, and OpenRouter profiles.
- Restoring profile-aware benchmark entry points and collecting local measurements.

## Main Repository Commits

- `22b3b7e6` - `qwen3 follow-up build fix`
- `16a188b0` - `qwen3 follow-up runtime smoke checks`
- `700f8974` - `qwen3 follow-up benchmark profiles`

## Nix Devshell Commit

In `/home/molaco/Documents/nix-devshells`:

- `4ffb5d2f` - `add onnxruntime to cuda code devshell`

Only `devshells/cuda-code.nix` was committed there. Pre-existing dirty files in that repo were left untouched.

## Build Fix

`Cargo.toml` now uses:

```toml
fastembed = { version = "5.13.4", default-features = false, features = [
  "hf-hub-native-tls",
  "ort-load-dynamic",
  "qwen3",
  "cuda",
] }
```

This removes `ort-download-binaries-native-tls` and avoids restoring `ort alternative-backend`.

`Cargo.lock` was refreshed by Cargo inside `nix develop ../nix-devshells#cuda-code`; ORT download-only dependencies were removed.

## Devshell Fix

`/home/molaco/Documents/nix-devshells/devshells/cuda-code.nix` now:

- Includes `onnxruntime` in `extraPackages`.
- Exports `ORT_DYLIB_PATH="${pkgs.onnxruntime}/lib/libonnxruntime.so"`.
- Prepends `${pkgs.onnxruntime}/lib` to `LD_LIBRARY_PATH`.
- Applies the same ONNX Runtime env to the MCP server entry.
- Preserves existing CUDA and `/run/opengl-driver/lib` paths.

## Verification

All project builds/runs were executed through:

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc '<command>'
```

Passed:

```sh
cargo check --lib
cargo check --tests
cargo build --release --example index_codebase --example gpu_batch_matrix
```

All passed with existing warnings.

## Runtime Smoke Checks

Added:

- `examples/embedding_profile_smoke.rs`

Results:

- `local-gpu-small`: indexed 1 file, 1 chunk, `smoke=ok`.
- `local-cpu-small`: indexed 1 file, 1 chunk, `smoke=ok`.
- `openrouter-qwen3-8b`: no API key was present; smoke confirmed a clear missing-key error.

One compile-only issue in the new smoke example was found on first run and fixed immediately.

## Benchmark Work

Updated:

- `examples/index_codebase.rs`
- `examples/gpu_batch_matrix.rs`

`index_codebase` now accepts `--profile PROFILE` and emits machine-readable metrics.

`gpu_batch_matrix` now passes `--profile`, reports vector dimension, and parses padded token metrics when present.

Measured results:

| profile | batch | dim | chunks | total | embed | padded tokens | padded tokens/sec |
|---|---:|---:|---:|---:|---:|---:|---:|
| local-gpu-small | 16 | 1024 | 2084 | 34.74s | 33.17s | 122693 | 19365.1 |
| local-cpu-small | 32 | 384 | 4691 | 201.23s | 198.18s | ~1678944 | ~8472 |
| openrouter-qwen3-8b | 32 | 4096 | 2084 | 139.63s | 138.05s | ~631335 | ~4573.1 |

OpenRouter benchmark was initially skipped because neither `RUST_CODE_MCP_OPENROUTER_API_KEY` nor `OPENROUTER_API_KEY` was present. It was later rerun successfully with `OPENROUTER_API_KEY` set. The key was not written to the report.

## Remaining Notes

- The GPU batch-16 result is now close to the requested 20k padded tokens/sec target.
- `local-cpu-small` works through Nix-provided ONNX Runtime and does not rely on `~/.cache/ort.pyke.io`.
- Peak GPU memory was not captured in this pass; the current benchmark output still relies on external GPU monitoring for that number.
- No formatting commands were run.
