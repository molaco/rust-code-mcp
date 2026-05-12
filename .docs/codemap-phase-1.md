# Codemap Phase 1 — foundation types + `ItemKind` predicates

**Status:** complete.
**Scope:** §2 + §3 + ItemKind-predicates row of `.plans/codemaps-proposal.md`.

## Files changed

| File | Change | LOC delta |
|---|---|---|
| `src/graph/codemap.rs` | **new** — response types only (no algorithm) | +97 |
| `src/graph/model.rs` | added `impl ItemKind { is_callable, is_type }` after the enum | +15 |
| `src/graph/mod.rs` | wired `pub mod codemap;` (alphabetical) | +1 |

Total: ~113 LOC across 3 files.

## What was added

- `Codemap`, `CodemapNode`, `CodemapEdge`, `CodemapStats` — `Serialize` + `Deserialize` for MCP JSON output. All fields `pub`.
- `EdgeKind { Calls, Uses, Imports, Contains }` — `#[non_exhaustive]` so future variants don't break the MCP wire format.
- `CodemapOptions` — internal struct, not serializable; the MCP tool's `BuildCodemapParams` (Phase 6) will translate JSON params to this.
- `EmbeddingPolicy { NoRerank, UseCachedOnly, ComputeMissing }` — used by `CodemapOptions`.
- `ItemKind::is_callable(self) -> bool` (`Function | Method | AssocFunction`).
- `ItemKind::is_type(self) -> bool` (`Struct | Enum | Union | Trait | TypeAlias`).

## What was NOT added (deferred to later phases)

- `OpenedSnapshot` span index / line→byte cache → Phase 2.
- `callees_of` / `referrers_of` graph adapters → Phase 3.
- `ensure_embeddings_for` helper → Phase 4.
- `build_codemap` algorithm, `rank_referrer`, `min_call_distance` → Phase 5.
- Mermaid + outline renderers, MCP tool wiring → Phase 6.

## Build verification

`nix develop ../nix-devshells#code --command cargo check --lib` → success in 2.99s. 17 pre-existing warnings (unrelated to codemap files; `cargo fix` suggestions in `ids.rs` / `position.rs`). No new warnings in the modified code.

## Notes for Phase 2

- `Node.file` is `Option<String>` — span index must skip nodes with `None`.
- `NodeKind` and `ItemKind` are already re-exported from `src/graph/mod.rs`. Future phases can use either path.
- `ModuleTreeNode` lives in `queries.rs`; `codemap.rs` imports it directly from `crate::graph::queries` (not the re-export) — matches existing intra-`graph` convention.
- No naming conflicts workspace-wide for `Codemap*` / `EdgeKind`.
