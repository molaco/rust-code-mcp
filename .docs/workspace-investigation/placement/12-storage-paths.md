# 12 — Storage Path Resolution

## Decision

A new leaf crate `rcm-paths` (no internal deps) owns all storage path resolution. Every other workspace crate that needs a path takes a `&ProjectPaths` value (or a single field of it) as a function argument. The binary crate (`server`) is the only place that constructs `ProjectPaths`; library crates never call `ProjectPaths::from_directory` themselves.

This is "tiny `paths` crate" + "server injects fully-resolved paths" combined: the crate owns the recipe, the binary owns the lifecycle. Library crates own neither.

## Why this owner, not the others

- **Each crate computes its own paths.** Rejected. Two crates drifting on the SHA-256 input (trailing slash? canonicalized? lowercased on Windows?) silently corrupts an installed user's data — readers and writers stop agreeing on `<hash>/`. The current code already has a foot-gun: `dir.to_string_lossy()` is fed to SHA-256, so a non-canonicalized vs canonicalized input produces different hashes. That recipe must live in exactly one place.
- **`code-search` exports the API for `graph`.** Rejected. `graph` does not depend on `code-search` today and shouldn't start: graph snapshots, BM25, and LanceDB are peers, not a hierarchy. Forcing `graph -> code-search` just to read a `PathBuf` inverts the dependency edges for no reason.
- **Server-injected, no shared crate.** Rejected on its own. Without a shared crate, every consumer re-derives the recipe to *validate* or *invalidate* a path (e.g. `clear_cache` enumerates per-project subtrees), and the same drift problem reappears.

The hybrid — shared recipe crate, single construction site — gives one canonical recipe and one canonical lifecycle.

## Contract

```rust
// crate: rcm-paths
pub struct ProjectPaths {
    pub workspace_dir: PathBuf,   // canonicalized
    pub dir_hash: String,         // hex SHA-256, 64 chars
    pub data_root: PathBuf,       // <storage-root>
    pub tantivy_path: PathBuf,    // <root>/search/tantivy/<hash>
    pub vector_path: PathBuf,     // <root>/search/vectors/<hash>
    pub cache_path: PathBuf,      // <root>/search/cache/<hash>      (sled)
    pub graph_path: PathBuf,      // <root>/graph/<hash>             (LMDB tree)
    pub merkle_path: PathBuf,     // <root>/snapshots/<hash>.snapshot
    pub collection_name: String,  // "code_chunks_<hash[..8]>"
}

impl ProjectPaths {
    pub fn resolve(workspace: &Path, root: &StorageRoot) -> Result<Self, PathError>;
}

pub enum StorageRoot {
    Xdg,                  // directories::ProjectDirs("dev","rust-code-mcp","search")
    Explicit(PathBuf),    // override
}

impl StorageRoot {
    pub fn from_env() -> Self; // reads RUST_CODE_MCP_DATA_DIR, else Xdg
}
```

`resolve` is the *only* function that hashes. Recipe is frozen as: `sha256(canonicalize(workspace).as_os_str().as_encoded_bytes())`, lower-hex. Switching from `to_string_lossy` is a one-time, version-gated migration (see risks). The hash is documented and tested with a fixed-input fixture so any accidental change fails CI.

## Override mechanism

One env var, one config field, one precedence rule:

1. CLI flag `--data-dir` on the binary (highest).
2. `RUST_CODE_MCP_DATA_DIR` env var.
3. `[storage] root = "..."` in the existing `Config` (which already lives in `config/`; future: `rcm-config` crate).
4. XDG default (lowest).

The `server` crate is responsible for resolving precedence and producing one `StorageRoot` at startup. Library crates see only the resolved `ProjectPaths`. Per-workspace overrides are out of scope — too many footguns for too little value.

## XDG / `directories` placement

`directories` is a transitive dep of **only** `rcm-paths`, which re-exports nothing from it. No other crate links it. This keeps the heavy `dirs-sys` / Windows `KNOWNFOLDERID` machinery off the dependency graph of `graph`, `search`, `indexing`, etc. Library crates compile faster and have no opinion on where data lives.

## Top 3 risks

1. **Hash recipe migration.** Today's `to_string_lossy` + non-canonicalized input means existing users have a hash. Switching to `canonicalize().as_encoded_bytes()` will orphan their indices. Mitigation: ship a one-shot rehash-and-rename on first run of the new version, gated by a `paths_recipe_version` file in `<root>/`.
2. **Library crates leaking the abstraction.** A graph helper that takes `&Path` "for now" becomes the de-facto second construction site. Mitigation: make the relevant function signatures take `&ProjectPaths` (or a typed newtype like `GraphDir(PathBuf)`) and forbid `ProjectPaths::resolve` calls outside `server` and tests via a clippy `disallowed_methods` lint.
3. **Test isolation.** Tests that touch the real XDG dir cross-contaminate developer machines. Mitigation: `StorageRoot::Explicit(tempdir)` is the only form used in tests; `Xdg` resolution is exercised only by one integration test that asserts the path *shape*, not contents.
