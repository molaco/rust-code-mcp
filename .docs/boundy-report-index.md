# Boundary Report Index

## Objective

Track execution of `.plans/boundry-anal-plan.md` for the four requested crates:
`rmc-engine`, `rmc-graph`, `rmc-indexing`, and `rmc-server`.

## Current Sequence

| Order | Crate | Report | Status |
| --- | --- | --- | --- |
| 1 | `rmc-engine` | `.docs/boundry-rmc-engine-report.md` | Phase 3 complete |
| 2 | `rmc-graph` | `.docs/boundry-rmc-graph-report.md` | Pending |
| 3 | `rmc-indexing` | `.docs/boundry-rmc-indexing-report.md` | Pending |
| 4 | `rmc-server` | `.docs/boundry-rmc-server-report.md` | Pending |

## Phase Status

| Crate | Phase 0 | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 |
| --- | --- | --- | --- | --- | --- | --- |
| `rmc-engine` | Complete | Complete | Complete | Complete | Pending | Pending |
| `rmc-graph` | Pending | Pending | Pending | Pending | Pending | Pending |
| `rmc-indexing` | Pending | Pending | Pending | Pending | Pending | Pending |
| `rmc-server` | Pending | Pending | Pending | Pending | Pending | Pending |

## Evidence Notes

- The analysis is MCP-tool first.
- `jj show --summary` is run before each phase.
- Each phase updates the relevant report document before committing.
- Commits are made with `jj commit -m` after each phase update.
