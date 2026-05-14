# RA integration placement

## Decision

Create a dedicated `ra-host` crate that owns every `ra_ap_ide` / `ra_ap_hir` / `ra_ap_load_cargo` / `ra_ap_vfs` dependency for the workspace, plus a thin sibling `ra-syntax` crate that re-exports `ra_ap_syntax` for the parser. `semantic` and `graph` consume `ra-host` only; `parser` consumes `ra-syntax` only. No other crate may name an `ra_ap_*` type.

## Why one host crate

Today both `semantic/loader.rs` and `graph/loader.rs` call `load_workspace_at` with subtly different `CargoConfig`s (semantic: `no_deps=true`, no sysroot; graph: `no_deps=false`, sysroot Discover, `set_test=true`, all features). That divergence is real and load-bearing — semantic optimizes for ~120ms cold-start IDE answers, graph needs full cross-crate resolution for HIR extraction. A shared crate doesn't paper over the difference; it names it. `ra-host` exposes two preset constructors:

```rust
pub struct RaHost { /* opaque: RootDatabase + Vfs + workspace_root */ }

impl RaHost {
    pub fn load_ide(path: &Path) -> Result<RaHost>;     // no_deps=true, sysroot=None
    pub fn load_hir(path: &Path) -> Result<RaHost>;     // no_deps=false, sysroot=Discover, all features
    pub fn analysis(&self) -> AnalysisHandle<'_>;       // wraps ra_ap_ide::Analysis
    pub fn with_db<R>(&self, f: impl FnOnce(&RootDatabase) -> R) -> R;
    pub fn with_semantics<R>(&self, f: impl FnOnce(&Semantics<'_, RootDatabase>) -> R) -> R;
    pub fn vfs(&self) -> &VfsView;                      // file_id<->path translation
    pub fn local_crates(&self) -> &[CrateView];         // graph's filter_local_crates
}
```

`AnalysisHandle`, `VfsView`, `CrateView`, `FileLocation { path, line, col }`, and a `RaError` enum are the only types crossing the crate boundary. `RootDatabase`, `Semantics`, `Vfs`, `FileId`, `TextSize`, `NavigationTarget` stay private — consumers receive `FileLocation`s and pass closures when they need DB/Semantics access. This satisfies the "no `ra_ap_*` in public APIs" constraint without forcing every helper into the host crate.

## Lifecycle: long-lived vs snapshot-and-discard

The lifecycle split is per *consumer*, not per *host preset*, and `RaHost` is agnostic about it:

- **semantic** owns `LazyLock<Mutex<HashMap<PathBuf, RaHost>>>` keyed by canonicalized project root. Long-lived. `load_ide` + cache, exactly as today.
- **graph extraction** constructs a `RaHost::load_hir` on the stack inside `build_and_persist`, runs the extraction pipeline, drops the host before `persist_loaded()` returns. Snapshot-and-discard. The persisted LMDB snapshot is the durable artifact; the RA database is scratch.
- **graph AST audits** (`unsafe`, `channel`, `fn_body`) re-load `RaHost::load_hir` on demand at audit time — they already do this today. They borrow `&RaHost` for the audit duration.

There is no reason to share a single `RootDatabase` between the two presets: the graph load takes seconds (sysroot, deps, all features), the IDE load takes ~120ms. Forcing the graph cost onto every IDE query, or forcing the IDE preset to skip cross-crate edges, would be strictly worse than two independent loads. The fingerprint short-circuit in `build_and_persist` already prevents redundant graph loads.

## `ra_ap_syntax` is different — split it out

`ra_ap_syntax` is a tokenizer/parser with no salsa database, no VFS, no proc-macro server, no cargo metadata. It's effectively `syn` with rust-analyzer's edition support. Bundling it into `ra-host` would force `parser` to compile `ra_ap_ide` + `ra_ap_hir` + `ra_ap_load_cargo` (multi-minute build, hundreds of MB of artifacts) for what is a pure-syntactic chunker.

Solution: a 30-line `ra-syntax` crate that re-exports `SourceFile`, `ast::*`, `AstNode`, `Edition`, `SyntaxKind`. Parser depends on `ra-syntax` only. `ra-host` also depends on `ra-syntax` internally (RA's heavy crates re-export the same types) so the version is pinned in one place.

## Owner & lifecycle policy

- **Owner:** new `crates/ra-host` and `crates/ra-syntax` at the workspace root. Owned by the platform/infra layer, not by `semantic` or `graph`.
- **Version pinning:** `ra-host`'s `Cargo.toml` is the *only* place `ra_ap_*` versions are declared. Bumping rust-analyzer is a one-crate change.
- **Public surface contract:** `cargo public-api` check in CI on `ra-host` — accidental leakage of `ra_ap_*` types into its public API fails the build.
- **Cache invalidation:** `RaHost` exposes `fn workspace_fingerprint(&self) -> Fingerprint`; semantic uses it to drop stale entries when files change (currently it never invalidates).

## Top 3 risks

1. **Preset divergence drift.** Someone needs `set_test=false` for IDE or `no_deps=true` for graph and adds a third preset, then a fourth. Mitigation: presets are an enum, not a free-form `CargoConfig` builder; adding a variant requires a PR review touching `ra-host`.
2. **Closure-based `with_db` is awkward for streaming.** Graph extraction iterates millions of HIR nodes; wrapping each phase in `host.with_db(|db| ...)` may force lifetime contortions or unwanted clones. Mitigation: extraction phases take `&RootDatabase` internally, but only `graph` (a trusted in-workspace consumer) gets that access via a `pub(crate)` re-export through `ra-host::internal`. External crates see only the closure API.
3. **Build-time blow-up if `ra-syntax` accidentally pulls heavy deps.** A future RA release might collapse `ra_ap_syntax` into `ra_ap_parser` + `ra_ap_ide_db`. Mitigation: CI job that runs `cargo tree -p ra-syntax` and fails if it transitively contains `ra_ap_hir` or `ra_ap_ide_db`.
