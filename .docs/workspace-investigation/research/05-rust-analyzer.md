# 05 — rust-analyzer Workspace Reference

Source: `github.com/rust-lang/rust-analyzer` (`crates/`) and `docs/book/src/contributing/architecture.md`.

## rust-analyzer crate inventory

~36 crates, grouped by function:

- **Foundation / shared infra (6):** `stdx` (std-shaped utilities), `intern` (Arc-based interning), `paths` (path types), `span` (source spans), `edition`, `cfg`. These are leaf crates with near-zero deps.
- **VFS & loading (4):** `vfs`, `vfs-notify`, `paths`, `load-cargo`, `project-model`, `toolchain` — opaque `FileId` lives at this layer; no `std::path::Path` leaks upward.
- **Syntax (4):** `parser`, `syntax`, `syntax-bridge`, `tt` (token trees). `parser` is generic over tree representation; `syntax` has no salsa, no LSP.
- **Database (1):** `base-db` — salsa input queries, `CrateGraph`, file contents. Ground state only.
- **HIR / semantic brain (4):** `hir-expand`, `hir-def`, `hir-ty`, `hir`. The first three are ECS-style internal queries; `hir` is the OO facade.
- **Macros (5):** `mbe`, `macros`, `proc-macro-api`, `proc-macro-srv`, `proc-macro-srv-cli`.
- **IDE features (6):** `ide-db`, `ide-completion`, `ide-assists`, `ide-diagnostics`, `ide-ssr`, `ide` (the feature facade).
- **Binary / glue (1):** `rust-analyzer` — the only crate that knows LSP and JSON.
- **Test / tooling (3):** `test-utils`, `test-fixture`, `profile`, `query-group-macro`.

## Patterns observed

- **Three explicit API boundaries: `syntax`, `hir`, `ide`.** Everything between them is internal and "will never be an API boundary." The binary crate `rust-analyzer` is the LSP edge.
- **No "common" / "types" dumping crate.** Shared concerns are split into narrow leaves: `paths`, `span`, `intern`, `edition`, `cfg`, `stdx`. Each has one job. `FileId` lives in `base-db` (where it's defined as opaque), `TextRange` comes from the external `text-size` crate — *not* a project-internal "core types" crate.
- **Opacity over convenience.** `FileId` is a newtype with no `Path` accessor. Forces all path resolution through VFS queries; prevents leakage.
- **Layer-violation rule:** lower layers never depend upward; serialization lives only in the binary; HIR/IDE types are non-serializable by design so wire formats can't pin internals.
- **POD at the boundary.** `ide`'s public types are plain structs with public fields. No trait objects, no lifetimes leaking out.
- **`stdx` is for std-shaped utilities only**, explicitly *not* a project-types grab bag.

## Patterns to borrow (specific)

1. **Three-tier facade model.** For an MCP code-search server: one syntax/parsing crate, one analysis/graph crate (our HIR equivalent), one query-API crate, one binary. The binary is the only place that touches MCP/JSON-RPC.
2. **Opaque IDs at the boundary.** `FileId`, `SymbolId`, `CrateId` as newtypes in the lowest crate that owns them — never re-exported with their internals.
3. **Split "shared" into narrow leaves.** Instead of one `common`/`types` crate, follow ra's pattern: `paths`, `span`, `ids`, `cfg` — each <500 LOC, single concept.
4. **Parser independent of the database.** Keep our syntax/AST crate free of cache and indexing concerns so it can be reused (e.g., in CLI tools or tests) without spinning up the graph.
5. **Non-serializable internal types.** Only the MCP binary serializes; analysis types stay POD-with-fields but not `Serialize`. Stops the wire schema from freezing internals.
6. **`stdx`-style utility crate** for one-off helpers, kept disciplined (no project semantics).

## Patterns to avoid copying

- **36 crates is too many for our scale.** rust-analyzer compiles a whole language; we index one. Aim for 8–12 crates.
- **Salsa-driven HIR layering (`hir-expand`/`-def`/`-ty`).** Our analysis is one pass over a graph, not incremental name resolution + type inference. One `analysis` crate suffices.
- **Separate proc-macro server process.** Irrelevant to a search server.
- **`query-group-macro` / custom salsa plumbing.** Only worth it if we adopt salsa, which is overkill for batch indexing.
- **`ide-*` fan-out (completion / assists / diagnostics / ssr).** We have one feature surface (search/graph queries); splitting it five ways is premature.
- **Per-feature test-fixture crate.** For our size, in-crate `tests/` plus a single `test-utils` is enough.

## Direct lessons for our project

- Carve out a **`mcp-bin`** crate that is the *only* JSON/serde/MCP-aware code, mirroring the `rust-analyzer` binary.
- Behind it expose a single **`api`/`ide`-style facade** with POD result types; keep `hir`-equivalent (graph builder, resolver) behind it as internal.
- Put **opaque IDs and primitive shared types in tiny leaf crates** (`paths`, `ids`, `span`) — don't make a `core-types` kitchen sink.
- Keep the **parser/AST crate salsa-free and cache-free** so it stays reusable and fast to test.
- Resist re-creating ra's macro and proc-macro stack; we don't need it.
- The boundary rule worth tattooing: *"types in `ide`, `base_db` and below are not serializable by design."* Apply the same to our analysis crates — only the MCP binary serializes.
