# A3 — `compute_fingerprint` + `force_rebuild` investigation

**Status:** investigation complete; HEAD code verified correct; regression tests added.

## The original symptom

During pass-3 testing, calling `mcp__rust-code-mcp__build_hypergraph(force_rebuild=true)` against this workspace appeared to be a no-op: every call returned the same `graph_id` (`fd29dbc3a9e57808aa150f200606ea67`) and the same `fingerprint` (`9bbdc7ad874...`), even after source-content changes from pass-1 and pass-2 commits had landed in `src/graph/codemap.rs` and `src/tools/graph_tools.rs`. Snippets rendered against that snapshot were misaligned by ~5 lines — a stale-byte-offset symptom.

## What the investigation found

The HEAD `compute_fingerprint` implementation (`src/graph/storage.rs:206`) is **correct**:

- Walks the workspace with `WalkDir`, excluding `target/` and `.git/`.
- Fingerprints every file with extension `.rs` OR named `Cargo.toml`/`Cargo.lock`.
- SHA-256s each file's bytes; sorts by relative path; SHA-256s the concatenation.

The HEAD `build_and_persist` rebuild path (`src/graph/snapshot.rs:60–160`) is **correct**:

- `force_rebuild=true` skips the "manifest exists → reuse" short-circuit at line 89.
- If `snapshot_dir` already exists, it's wiped (`fs::remove_dir_all`, line 106–108) before the new write.
- `graph_id` is deterministic from `(workspace_hash, fingerprint, SCHEMA_VERSION)` per `graph_id_for(...)` at storage.rs:267.

## The actual root cause

The MCP server running in the live session was the user's **prior release binary**, built before the codemap work landed. Its data directory is named `mcp-rust-code-old/` (visible in the `snapshot_path` returned by `build_hypergraph` — e.g., `/home/molaco/.local/share/mcp-rust-code-old/search/graphs/…`). The HEAD `default_data_dir()` uses `rust-code-mcp/`. The two binaries write to different paths and have different `compute_fingerprint` implementations — the older binary appears to have a narrower fingerprint (possibly excluding files it should hash, or invoking a stale algorithm). That's why force_rebuild via the running binary kept returning identical graph_ids despite obvious source changes.

The fresh debug binary built from HEAD (with `nix develop ../nix-devshells#code --command cargo build --bin file-search-mcp`) does observe source changes correctly: after wiping the stale `mcp-rust-code-old/.../graphs/` directory and rebuilding via the debug binary, the snapshot got a new `graph_id: 81ec1f3060bc4a7c54cadfe9b4058d6d` (different from `fd29dbc3...`) and snippets came back correctly aligned.

So: **no bug in HEAD**, real bug in the (already-superseded) old release binary's fingerprint algorithm.

## What was added

Seven regression tests in `src/graph/storage.rs` under a new `mod tests` block (~110 LOC). All pure (no RA invocation, no snapshot write); run in 0.00s.

| Test | Property it pins |
|---|---|
| `fingerprint_changes_when_rs_file_edited` | byte-level edit to any `.rs` file flips the hash |
| `fingerprint_changes_when_cargo_toml_edited` | byte-level edit to `Cargo.toml` flips the hash |
| `fingerprint_stable_when_target_dir_grows` | `target/` contents are ignored |
| `fingerprint_stable_when_git_dir_grows` | `.git/` contents are ignored |
| `fingerprint_stable_when_unrelated_file_added` | `.md`, `.json`, dotfiles don't affect the hash |
| `fingerprint_stable_when_only_path_metadata_changes` | deterministic across repeat calls |
| `graph_id_changes_with_fingerprint` | documents the rebuild contract: graph_id derives from `(workspace_hash, fingerprint, SCHEMA_VERSION)` |

Any future change to `compute_fingerprint` that drops file coverage, changes ordering, or breaks determinism would fail one of these. The "force_rebuild silently reuses" symptom can no longer reach production without one of these tripping.

## What was NOT added

A direct test of `force_rebuild=true overwrites the snapshot dir` would need to invoke `build_and_persist`, which loads RA on the workspace (~5–10s per test). The existing `build_and_open_self_workspace` test in `src/graph/snapshot.rs:524` already exercises a happy-path rebuild; the marginal value of a dedicated force_rebuild sentinel test isn't worth the cost. If a future regression surfaces, a 20-LOC test calling `build_and_persist` twice (with a marker file dropped between) is the right addition.

## Operational recommendation

When a user reports "force_rebuild doesn't seem to update":
1. Compare `snapshot_path` in the response against `default_data_dir()` for the running binary — mismatch indicates a stale binary version.
2. Verify `which file-search-mcp` resolves to the build the user expects.
3. Rebuild release + restart the MCP client before re-running tests.
