# Phase 7 — Storage Layout v2 Migration via xtask

**Status:** optional, deferrable. Operationally the riskiest single change in the plan because it relocates user data on disk. May be skipped indefinitely if Phases 0–6 are sufficient.

**Authoritative sources:**
- `/home/molaco/Documents/rust-code-mcp-final/.docs/workspace-plan/DECISIONS.md`
- `/home/molaco/Documents/rust-code-mcp-final/.docs/workspace-investigation/specific/20-storage-layout.md`

## Goal

Migrate from layout v1:

```
<XDG-data>/rust-code-mcp/search/
├── tantivy/<dir_hash>/
├── cache/<dir_hash>/
├── vectors/<dir_hash>/
├── <sha16>.snapshot
└── graph/<dir_hash>/snapshots/<graph_id>/
```

To layout v2 (per investigation report 20):

```
<XDG-data>/rust-code-mcp/
├── LAYOUT_VERSION                      # "2"
├── tmp/                                # scratch for atomic moves
└── workspaces/<workspace_hash>/
    ├── manifest.json                   # canonical path, timestamps, backend metadata
    ├── keyword/  (was: tantivy/<dir_hash>)   { VERSION, segments + meta.json }
    ├── vector/   (was: vectors/<dir_hash>)   { VERSION, table + indices }
    ├── metadata/ (was: cache/<dir_hash>)     { VERSION, sled tree files }
    ├── merkle/   (was: <sha16>.snapshot)     { VERSION, current.snapshot, backups/ }
    └── graph/    (was: graph/<dir_hash>/...) { VERSION, CURRENT, snapshots/<graph_id>/ }
```

Plus: `clear_cache` gains a `scope` parameter; per-area `VERSION` files allow per-backend schema bumps without nuking siblings; `manifest.json` makes each workspace dir self-describing; migration is `--dry-run`-able, idempotent, resumable.

## Step 1 — Implement `xtask migrate-storage` subcommand

Add a `migrate-storage` subcommand to `xtask`. Sub-flags: `--data-root <path>` (default: XDG), `--dry-run`, `--resume`, `--from-version <v>`, `--to-version <v>`.

```rust
// crates/xtask/src/main.rs
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask", about = "Workspace automation")]
struct Xtask {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Migrate on-disk storage to layout v2
    MigrateStorage(MigrateArgs),
}

#[derive(Parser)]
struct MigrateArgs {
    /// Override the data root (default: XDG_DATA_HOME/rust-code-mcp)
    #[arg(long)]
    data_root: Option<PathBuf>,
    /// Print plan, do not execute
    #[arg(long)]
    dry_run: bool,
    /// Continue an interrupted migration
    #[arg(long)]
    resume: bool,
    /// Source layout version (default: detect from LAYOUT_VERSION file)
    #[arg(long)]
    from_version: Option<u32>,
    /// Target layout version (default: 2)
    #[arg(long, default_value_t = 2)]
    to_version: u32,
}

fn main() -> anyhow::Result<()> {
    match Xtask::parse().cmd {
        Cmd::MigrateStorage(args) => xtask::migrate_storage::run(args),
    }
}
```

Shell usage:

```bash
nix develop ../nix-devshells#code --command cargo xtask migrate-storage --dry-run
nix develop ../nix-devshells#code --command cargo xtask migrate-storage
nix develop ../nix-devshells#code --command cargo xtask migrate-storage --resume
nix develop ../nix-devshells#code --command cargo xtask migrate-storage --data-root /tmp/fixture-v1
```

**Files touched:** `crates/xtask/src/main.rs`, `crates/xtask/src/migrate_storage.rs` (new), `crates/xtask/Cargo.toml`.

**Acceptance:** `cargo xtask migrate-storage --help` lists all sub-flags. Compile-only.

**Reversal:** delete `migrate_storage.rs` and the `MigrateStorage` variant; xtask still builds.

## Step 2 — Migration algorithm

Pseudocode (real implementation in `migrate_storage.rs`):

1. Acquire lock at `<root>/migration.lock` (open with `O_CREAT | O_EXCL`). If present and not stale (mtime <1 hour old), abort with a clear error.
2. Read `<root>/LAYOUT_VERSION`. If missing, assume v1. If already at `to_version`, exit OK.
3. Enumerate workspace hashes from the union of `search/tantivy/`, `search/cache/`, `search/vectors/`, `search/graph/`. (Phase 1 retains the `search/` subdir; the migration also lifts contents up one level.)
4. Build a list of `Move { from, to }` ops per workspace hash plus a final root-rename `search/` → "fold-and-flatten" step.
5. If `--dry-run`: print plan, release lock, exit 0.
6. Execute each move with `std::fs::rename` (atomic on a single filesystem). On `EXDEV` (cross-device), copy + verify byte-equality + delete source.
7. After all moves succeed: write `<root>/LAYOUT_VERSION` atomically (`tmp/LAYOUT_VERSION.<rand>` then rename).
8. For each `workspaces/<hash>/` without a `manifest.json`, synthesize one. Workspace path is unknown for hash-only dirs → leave `null`; `rcm-server` fills it on the next observation.
9. Best-effort move loose `merkle_v*.*.snapshot` from any external `BackupManager.backup_dir` into per-workspace `merkle/backups/`; orphans land in top-level `legacy_backups/`.
10. Remove now-empty `tantivy/`, `cache/`, `vectors/`, `graph/`, and any leftover top-level `*.snapshot` files.
11. Release lock.

**Files touched:** `crates/xtask/src/migrate_storage.rs`.

**Acceptance:** `--dry-run` prints a complete plan for a real v1 layout; non-dry run on a tempdir transitions `LAYOUT_VERSION` from missing to `2`.

**Reversal:** delete `<root>/LAYOUT_VERSION`; old paths still on disk (until cleanup release) so v1 binary keeps working.

## Step 3 — Idempotency and resumption

- Each move is idempotent: if `to` exists and `from` doesn't, skip; if both exist, log + skip (never overwrite).
- A partial migration leaves both old and new dirs in place. Old data still readable by a v1-aware binary.
- `--resume` re-reads layout state and continues from wherever the previous run stopped.

```rust
// crates/xtask/src/migrate_storage.rs (excerpt)
use std::fs;
use std::io;
use std::path::Path;
use tracing::{info, warn};

pub(crate) fn move_idempotent(from: &Path, to: &Path) -> io::Result<MoveOutcome> {
    match (from.try_exists()?, to.try_exists()?) {
        (false, true) => Ok(MoveOutcome::AlreadyDone),
        (false, false) => Ok(MoveOutcome::SourceMissing),
        (true, true) => {
            warn!(?from, ?to, "both source and target exist; leaving source for manual review");
            Ok(MoveOutcome::Conflict)
        }
        (true, false) => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            match fs::rename(from, to) {
                Ok(()) => {
                    info!(?from, ?to, "moved");
                    Ok(MoveOutcome::Moved)
                }
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                    copy_then_remove(from, to)?;
                    Ok(MoveOutcome::CopiedAcrossFilesystems)
                }
                Err(e) => Err(e),
            }
        }
    }
}

pub(crate) enum MoveOutcome {
    Moved,
    AlreadyDone,
    SourceMissing,
    Conflict,
    CopiedAcrossFilesystems,
}

pub(crate) fn migrate_one_workspace(v1_root: &Path, v2_root: &Path, hash: &str) -> io::Result<()> {
    let dst = v2_root.join("workspaces").join(hash);
    fs::create_dir_all(&dst)?;
    move_idempotent(&v1_root.join("tantivy").join(hash), &dst.join("keyword"))?;
    move_idempotent(&v1_root.join("cache").join(hash),   &dst.join("metadata"))?;
    move_idempotent(&v1_root.join("vectors").join(hash), &dst.join("vector"))?;
    move_idempotent(&v1_root.join("graph").join(hash),   &dst.join("graph"))?;
    fs::create_dir_all(dst.join("merkle").join("backups"))?;
    let prefix = &hash[..16.min(hash.len())];
    let merkle_src = v1_root.join(format!("{prefix}.snapshot"));
    if merkle_src.exists() {
        move_idempotent(&merkle_src, &dst.join("merkle").join("current.snapshot"))?;
    }
    Ok(())
}
```

**Files touched:** same as Step 2.

**Acceptance:** running migration twice produces identical state on second run (no errors, no extra moves). Killing mid-run and re-running with `--resume` completes the migration without data loss.

**Reversal:** as Step 2.

## Step 4 — Backwards-compat window

For ONE release cycle, both layouts must be readable by the running binary:

- `rcm_paths::ProjectPaths::resolve` checks v2 paths first, falls back to v1 if the v2 dir does not exist.
- `clear_cache(workspace)` deletes both v1 and v2 paths if both happen to exist.
- A `tracing::warn!` line fires once per process when a v1 path is opened, naming the migration command.

After one cycle (next minor release): drop v1 fallbacks, delete the warning, document a hard cutover in the changelog.

**Files touched:** `crates/rcm-paths/src/lib.rs`, `crates/rcm-server/src/tools/clear_cache.rs`.

**Acceptance:** smoke checklist (DECISIONS.md) passes against a v1-only data dir, a v2-only data dir, and a half-migrated mix.

**Reversal:** keep the fallback indefinitely; cost is a single extra `try_exists` per resolve.

## Step 5 — Update `rcm-paths` for v2

Rename fields on `ProjectPaths` to match the v2 area names. Phase 7 is the right time; the surface is small.

```rust
// crates/rcm-paths/src/lib.rs (target shape)
#[non_exhaustive]
pub struct ProjectPaths {
    root: PathBuf,
    workspace_root: PathBuf,
    keyword_path: PathBuf,
    vector_path: PathBuf,
    metadata_path: PathBuf,
    merkle_current: PathBuf,
    merkle_backups: PathBuf,
    graph_root: PathBuf,
    graph_current_pointer: PathBuf,
}

impl ProjectPaths {
    pub fn keyword(&self) -> &Path { &self.keyword_path }
    pub fn vector(&self) -> &Path { &self.vector_path }
    pub fn metadata(&self) -> &Path { &self.metadata_path }
    pub fn merkle_current(&self) -> &Path { &self.merkle_current }
    pub fn merkle_backups(&self) -> &Path { &self.merkle_backups }
    pub fn graph_root(&self) -> &Path { &self.graph_root }
    pub fn graph_current_pointer(&self) -> &Path { &self.graph_current_pointer }
}
```

Old field/method names (`tantivy_path`, `cache_path`) are deleted, not deprecated. Compile errors at every call site is the migration tool. The hashing recipe (`sha256(canonicalize(workspace).as_encoded_bytes())`, lower-hex) is unchanged.

**Files touched:** `crates/rcm-paths/`, every call site in `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-server`.

**Acceptance:** `cargo build --workspace` green; `cargo public-api` diff matches the documented rename.

**Reversal:** revert the rename commit; v2 layout still works because the strings inside `resolve` are independent of the public field names.

## Step 6 — `clear_cache` scope expansion

`rcm-server::tools::clear_cache_tool` gains a `scope` parameter. Default `Workspace` keeps current call-site ergonomics.

```rust
// crates/rcm-server/src/tools/clear_cache.rs
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClearScope {
    #[default]
    Workspace,
    Keyword,
    Vector,
    Metadata,
    Merkle,
    Graph,
    All,
}

#[derive(Debug, Deserialize)]
pub struct ClearCacheParams {
    /// Workspace path; ignored when scope = All.
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub scope: ClearScope,
}

pub(crate) async fn clear_cache_tool(params: ClearCacheParams, ctx: &ServerCtx) -> Result<Value, ToolError> {
    let data_root = ctx.paths.data_root();
    let workspaces_root = data_root.join("workspaces");

    let target = match params.scope {
        ClearScope::All => workspaces_root.clone(),
        scope => {
            let ws = params.workspace.ok_or(ToolError::MissingWorkspace)?;
            let pp = ProjectPaths::resolve(&ws, &ctx.storage_root)?;
            match scope {
                ClearScope::Workspace => pp.workspace_root().to_path_buf(),
                ClearScope::Keyword   => pp.keyword().to_path_buf(),
                ClearScope::Vector    => pp.vector().to_path_buf(),
                ClearScope::Metadata  => pp.metadata().to_path_buf(),
                ClearScope::Merkle    => pp.merkle_current().parent().unwrap().to_path_buf(),
                ClearScope::Graph     => pp.graph_root().to_path_buf(),
                ClearScope::All       => unreachable!(),
            }
        }
    };

    // Safety: refuse anything outside <data-root>/workspaces/.
    let canon = target.canonicalize().map_err(ToolError::Io)?;
    if !canon.starts_with(workspaces_root.canonicalize().map_err(ToolError::Io)?) {
        return Err(ToolError::PathOutsideWorkspaces { path: canon });
    }

    ctx.search.invalidate_workspace(&canon).await?;
    ctx.graph.invalidate_workspace(&canon).await?;
    ctx.ide.evict_workspace(&canon).await;
    tokio::fs::remove_dir_all(&canon).await.or_else(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(e),
    })?;
    Ok(serde_json::json!({ "cleared": canon, "scope": params.scope }))
}
```

**Files touched:** `crates/rcm-server/src/tools/clear_cache.rs`, the tool registration in the `#[tool_router]`, MCP schema docs.

**Acceptance:** smoke test for each scope: `clear_cache(scope=keyword)` then `search` rebuilds the keyword index without re-embedding; `clear_cache(scope=all)` empties `workspaces/` but preserves `LAYOUT_VERSION` and `tmp/`.

**Reversal:** drop the new variants; `Workspace` default keeps existing callers working.

## Step 7 — Per-area `VERSION` file handling

Each backend area carries a small `VERSION` file. On open, the backend reads its `VERSION` and refuses to open if the schema doesn't match the binary's expectation; the error includes the exact `clear_cache(scope=…)` invocation needed to rebuild that area.

```rust
// crates/rcm-search/src/keyword/version.rs
const KEYWORD_SCHEMA_VERSION: &str = "tantivy-1";

pub(crate) fn check_or_init(dir: &Path) -> Result<(), CorpusError> {
    let path = dir.join("VERSION");
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let found = s.trim();
            if found != KEYWORD_SCHEMA_VERSION {
                return Err(CorpusError::SchemaMismatch {
                    area: "keyword",
                    expected: KEYWORD_SCHEMA_VERSION,
                    found: found.to_owned(),
                    fix: "clear_cache(scope=keyword)",
                });
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            std::fs::write(&path, format!("{KEYWORD_SCHEMA_VERSION}\n"))?;
            Ok(())
        }
        Err(e) => Err(CorpusError::Io(e)),
    }
}
```

The same pattern applies in `rcm-search` (vector, metadata), `rcm-graph` (graph), `rcm-search` (merkle). The version strings encode the smallest meaningful schema unit, e.g. `"lancedb-1.dim384"` so a model dim change is detectable without parsing the manifest.

**Files touched:** one `version.rs` per backend area (5 files total), each backend's open path.

**Acceptance:** simulating a `VERSION` mismatch produces an error message that names the exact `clear_cache` scope to invoke.

**Reversal:** make the check non-fatal (warn + auto-rebuild).

## Step 8 — `manifest.json` schema

```json
{
    "layout_version": 2,
    "workspace_path": "/home/user/projects/foo",
    "created_at": "2026-05-06T12:34:56Z",
    "last_seen_at": "2026-05-06T13:00:00Z",
    "backends": {
        "keyword":  { "schema_version": "tantivy-1" },
        "vector":   { "schema_version": "lancedb-1.dim384", "dim": 384, "model": "AllMiniLML6V2" },
        "metadata": { "schema_version": "sled-1" },
        "merkle":   { "schema_version": "merkle-bincode-1" },
        "graph":    { "schema_version": "heed-1", "graph_id": "9b7e..." }
    }
}
```

Rules:
- `workspace_path` is `null` for hash-only dirs synthesized by migration; `rcm-server` fills it on the next successful `index_codebase` for that hash.
- `last_seen_at` is bumped on every successful indexing run.
- `created_at` is set once and never modified.
- All other fields are additive; readers ignore unknown keys (forward-compat).
- A future GC xtask deletes workspaces with `last_seen_at` older than 90 days. **Not in Phase 7.**

```rust
// crates/rcm-paths/src/manifest.rs
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields = false)] // explicit forward-compat
pub struct WorkspaceManifest {
    pub layout_version: u32,
    pub workspace_path: Option<PathBuf>,
    pub created_at: String, // RFC 3339
    pub last_seen_at: String,
    #[serde(default)]
    pub backends: BTreeMap<String, serde_json::Value>,
}
```

**Files touched:** `crates/rcm-paths/src/manifest.rs` (new), reader/writer call sites in `rcm-server` and `rcm-search`.

**Acceptance:** synthesized manifests round-trip; an unknown future field is preserved across a read-modify-write.

**Reversal:** stop writing manifests; existing files become inert metadata.

## Step 9 — Migration tests

Test fixtures live under `crates/xtask/tests/fixtures/v1/` as a tree of empty files mimicking a real v1 layout (multiple `dir_hash`es, some with full data, some partial).

```rust
// crates/xtask/tests/migrate_storage.rs
#[test]
fn dry_run_lists_all_moves() {
    let fx = build_fixture_v1();
    let out = run_xtask(&["migrate-storage", "--data-root", fx.path(), "--dry-run"]);
    assert!(out.contains("workspaces/abcd1234.../keyword"));
    assert!(out.contains("workspaces/abcd1234.../merkle/current.snapshot"));
    assert_eq!(fx.read_file("LAYOUT_VERSION"), None, "dry-run must not write");
}

#[test]
fn full_migration_then_idempotent_second_run() {
    let fx = build_fixture_v1();
    run_xtask_ok(&["migrate-storage", "--data-root", fx.path()]);
    assert_eq!(fx.read_file("LAYOUT_VERSION").unwrap().trim(), "2");
    let snapshot = fx.snapshot_tree();
    run_xtask_ok(&["migrate-storage", "--data-root", fx.path()]);
    assert_eq!(snapshot, fx.snapshot_tree(), "second run must be a no-op");
}

#[test]
fn interrupted_migration_resumes() {
    let fx = build_fixture_v1();
    inject_failure_after_n_moves(2);
    let _ = run_xtask(&["migrate-storage", "--data-root", fx.path()]);
    clear_failure_injection();
    run_xtask_ok(&["migrate-storage", "--data-root", fx.path(), "--resume"]);
    assert!(fx.path().join("workspaces").exists());
    assert!(!fx.path().join("tantivy").exists());
}

#[test]
fn cross_filesystem_move_falls_back_to_copy() {
    let mock = MockFs::with_exdev_on(&["vectors/abcd*"]);
    let fx = build_fixture_v1_on(&mock);
    run_xtask_ok(&["migrate-storage", "--data-root", fx.path()]);
    assert!(fx.path().join("workspaces/abcd.../vector").exists());
}
```

Memo from auto-memory: avoid running `cargo test` here unless asked; each invocation costs ~115s+. Use `cargo check --lib` during development; gate full tests on CI.

**Files touched:** `crates/xtask/tests/migrate_storage.rs`, fixture tree.

**Acceptance:** all four tests pass on CI.

**Reversal:** delete the test file.

## Step 10 — Documentation and rollout

- Update `README.md` with a "Storage layout" section showing the v2 tree.
- Add `docs/migration-runbook.md`: when to migrate, how to back up first (`cp -a "<XDG-data>/rust-code-mcp" "<XDG-data>/rust-code-mcp.bak"`), how to verify success (`cat <root>/LAYOUT_VERSION`, sample MCP tool call).
- Add a CHANGELOG entry naming the new `clear_cache` scopes and the per-area `VERSION` mechanic.

Example user-facing migration session:

```bash
# 1. back up first
cp -a "$XDG_DATA_HOME/rust-code-mcp" "$XDG_DATA_HOME/rust-code-mcp.bak"
# 2. preview
nix develop ../nix-devshells#code --command cargo xtask migrate-storage --dry-run
# 3. execute
nix develop ../nix-devshells#code --command cargo xtask migrate-storage
# 4. verify
cat "$XDG_DATA_HOME/rust-code-mcp/LAYOUT_VERSION"   # → 2
ls "$XDG_DATA_HOME/rust-code-mcp/workspaces"         # one dir per indexed project
```

**Files touched:** `README.md`, `docs/migration-runbook.md` (new), `CHANGELOG.md`.

**Acceptance:** runbook step-through completes without ambiguity on a developer machine.

**Reversal:** revert the docs commit.

## Phase 7 acceptance (end-to-end)

- `xtask migrate-storage --dry-run` prints a complete plan for a real v1 layout.
- The same command without `--dry-run` performs the migration; subsequent reads work.
- A migration killed mid-run resumes cleanly with `--resume`.
- During the compatibility window all MCP tools work against both v1 and v2 layouts.
- After cleanup release: v1 fallbacks removed; only v2 supported; `LAYOUT_VERSION=2` everywhere.
- Smoke checklist (DECISIONS.md) passes against the migrated workspace.

## Reversibility

The migration creates new paths but does **not** delete old ones until the move is verified. To revert: edit `LAYOUT_VERSION` back to `1` (or delete it) and restart with the old binary. New paths remain on disk as orphans, cleaned up by a future `xtask gc-storage` command or by hand. The `migration.lock` file is removed on every successful exit, including the dry-run path.

## When to do Phase 7

Phase 7 is genuinely optional. If Phases 0–6 ship and the existing v1 layout is fine, Phase 7 can wait. Triggers that justify it:

- A backend gets a schema change (new Tantivy field, new LanceDB schema rev) — bump per-area `VERSION` and ship the migration in the same release.
- A new backend is added — clean place for it.
- User feedback that `clear_cache` is too coarse (today nukes the whole workspace) — scope expansion alone justifies it.

Per DECISIONS.md the phase order is fixed (0 → 7); each phase keeps `cargo build --workspace` green and the smoke checklist passing. Phase 7 inherits that invariant: at no point during migration may the running binary be unable to open a workspace, because the v1 fallback in `rcm-paths::resolve` is in place throughout the compatibility window.
