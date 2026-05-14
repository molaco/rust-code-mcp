# Clean-Slate On-Disk Storage Layout

Goal: a layout that is workspace-partitioned (one `rm -rf` per project), self-describing (versioned), backend-agnostic in its naming (no crate/tool names baked in), and trivial to extend when new backends are added.

## Proposed tree

```
<XDG-data>/rust-code-mcp/                       # product root (renamed from "...mcp/search")
├── LAYOUT_VERSION                              # plain text "2"; root-level layout schema marker
├── tmp/                                        # scratch for atomic moves; safe to wipe
└── workspaces/
    └── <workspace_hash>/                       # SHA-256 of canonical workspace path (full hex; 16-char prefix shown in logs)
        ├── manifest.json                       # { layout_version, workspace_path, created_at, last_seen_at, backends: {...} }
        ├── keyword/                            # Tantivy BM25 index dir (was: tantivy/<dir_hash>)
        │   ├── VERSION                         # tantivy schema/format version
        │   └── <segments + meta.json>
        ├── vector/                             # LanceDB dir (was: vectors/<dir_hash>)
        │   ├── VERSION                         # vector_dim + arrow schema rev
        │   └── <table 'chunks'/ + indices>
        ├── metadata/                           # sled MetadataCache (was: cache/<dir_hash>)
        │   ├── VERSION
        │   └── <sled tree files>
        ├── merkle/                             # change-detection state (was: <sha16>.snapshot at root)
        │   ├── VERSION
        │   ├── current.snapshot                # active FileSystemMerkle bincode
        │   └── backups/
        │       └── merkle_v{ver}.{unix_ts}.snapshot   # rotated by BackupManager
        └── graph/                              # heed/LMDB hypergraph (was: graph/<workspace_hash>/...)
            ├── VERSION                         # graph schema (sub-DB layout) version
            ├── CURRENT                         # atomic pointer file → snapshots/<graph_id>
            └── snapshots/
                └── <graph_id>/                 # one heed Env per build
                    ├── data.mdb
                    ├── lock.mdb
                    └── manifest.json           # fingerprint + build metadata
```

## Why this shape

- **Top-level `workspaces/<workspace_hash>/`** makes the per-project boundary explicit. `clear_cache(workspace)` becomes `rm -rf workspaces/<hash>/`; no scattering across `tantivy/`, `vectors/`, `cache/`, `graph/`, plus a stray top-level `<sha16>.snapshot`.
- **Backend names describe roles, not crates.** `keyword/` (not `tantivy/`), `vector/` (not `vectors/` or `lancedb/`), `metadata/` (not `cache/sled`), `graph/` (engine-agnostic). Swapping Tantivy for another BM25 engine or LanceDB for a different ANN store requires no path migration.
- **`LAYOUT_VERSION` at the root** controls the *directory layout itself*. **Per-backend `VERSION` files** control the on-disk format inside one area. The two evolve independently: a Tantivy schema bump touches `keyword/VERSION` only.
- **Per-workspace `manifest.json`** records the canonical workspace path (so a hash-only directory is still debuggable), creation/last-seen timestamps (for orphan GC), and a small per-backend descriptor block (e.g. `{ "vector": { "dim": 384, "model": "AllMiniLML6V2" } }`). The workspace dir is self-describing: nothing else has to be consulted to know what model produced its embeddings.
- **`merkle/` holds both the live snapshot and rotated backups.** `BackupManager`'s `merkle_v{ver}.{unix_ts}.snapshot` files move from a separately-configured `backup_dir` into `workspaces/<hash>/merkle/backups/`, so backups follow the workspace they describe and a workspace nuke also removes its rotated history.
- **`graph/` keeps the existing `CURRENT` + `snapshots/<graph_id>/` model**, since heed MVCC requires that readers can pin an old `graph_id` while a writer publishes a new one. Only the parent path changes.
- **`tmp/`** is a single shared scratch area for atomic-move staging (snapshot publish, manifest rewrites). Survives crashes; safe to wipe on startup.
- **No "search" or crate name in any path.** Renaming the crate (`rust-code-mcp-final` → anything) or splitting into sub-crates leaves on-disk state untouched; only the `directories::ProjectDirs` `application` arg need stay stable as `"rust-code-mcp"`.

## Path resolution contract

A single helper (`StoragePaths::for_workspace(workspace_dir) -> WorkspacePaths`) is the only thing that knows the layout:

```
WorkspacePaths {
    root, manifest, keyword, vector, metadata,
    merkle_current, merkle_backups, graph_root, graph_current_pointer,
}
```

Every backend takes paths from this struct; no module recomputes path strings. Crate moves cannot break paths because no path string mentions a crate.

## Migration plan (v1 → v2)

On startup, if `<XDG-data>/rust-code-mcp/LAYOUT_VERSION` is missing or `< 2`:

1. Create `workspaces/`, `tmp/`, write `LAYOUT_VERSION=2`.
2. For each `<dir_hash>` discovered in the union of `tantivy/`, `cache/`, `vectors/`, and `graph/`:
   - `mkdir workspaces/<dir_hash>/`
   - `mv tantivy/<dir_hash>     → workspaces/<dir_hash>/keyword`
   - `mv cache/<dir_hash>       → workspaces/<dir_hash>/metadata`
   - `mv vectors/<dir_hash>     → workspaces/<dir_hash>/vector`
   - `mv graph/<dir_hash>/*     → workspaces/<dir_hash>/graph/`  (preserves `CURRENT` + `snapshots/<graph_id>/`)
   - `mkdir workspaces/<dir_hash>/merkle/{,backups}` and `mv <sha16>.snapshot → workspaces/<dir_hash>/merkle/current.snapshot` if the prefix matches.
   - Synthesize `manifest.json` from whatever metadata can be inferred (workspace path is unknown for hash-only dirs → leave `null`, fill on next observation).
3. Move all loose files in any external `BackupManager.backup_dir` matching `merkle_v*.*.snapshot` into the workspace's `merkle/backups/` (best-effort; orphans stay in a top-level `legacy_backups/` for manual review).
4. Remove now-empty `tantivy/`, `cache/`, `vectors/`, `graph/`, and any leftover `*.snapshot` files at the root.

The migration is idempotent and read-resumable: if a run is interrupted, the next run resumes by re-checking each source dir's existence. A `--dry-run` flag prints the planned moves.

## Version policy

- **`LAYOUT_VERSION`** (root): bump only when the directory tree changes. Migration is mandatory.
- **`<area>/VERSION`** (per-backend): bump on Tantivy schema, LanceDB schema, sled tree shape, graph sub-DB layout, or Merkle bincode-format changes. On mismatch, that area is rebuilt for the affected workspace; the others are untouched. This is what lets `dead_pub_audit`-style schema additions land without blowing away keyword/vector data.
- **Manifest fields** are additive; readers ignore unknown keys.

## `clear_cache` semantics

The MCP `clear_cache` tool gains an explicit scope:

- `clear_cache(scope = "workspace", workspace = path)` → `rm -rf workspaces/<hash>/`. Full nuke; next index rebuilds everything.
- `clear_cache(scope = "keyword" | "vector" | "metadata" | "merkle" | "graph", workspace = path)` → `rm -rf workspaces/<hash>/<area>/`. Per-backend reset (e.g. clear a corrupt Tantivy lock without re-embedding).
- `clear_cache(scope = "all")` → `rm -rf workspaces/`. Global wipe; preserves `LAYOUT_VERSION` and `tmp/`.

Default (unspecified scope) stays `"workspace"` to preserve current call-site ergonomics. The tool refuses any path outside `<XDG-data>/rust-code-mcp/workspaces/` as a safety check.
