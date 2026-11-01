# Graph truth corpus: baseline, 2026-10-23

This document preserves the pre-fix baseline. Current corrections and the v15
contract are described in [Edge semantics](GRAPH_EDGE_SEMANTICS.md); the oracle
source sites and expected semantic edges below have not been rewritten.

The independent oracle is [expected.tsv](../crates/rmc-graph/tests/fixtures/graph_truth/expected.tsv),
with [structural facts](../crates/rmc-graph/tests/fixtures/graph_truth/expected-structure.json).
It was written from Rust source before inspecting extraction output. There are
51 selected sites, of which 43 require included coverage. This is not an
exhaustive precision/recall benchmark for every identifier or for real workspaces.
See the [fixture README](../crates/rmc-graph/tests/fixtures/graph_truth/README.md)
for the semantic conventions and commands.

Baseline source: MCP `1962bb64`, RA fork `d468e401`, salsa fork `30795ac6`, clean
production code in all three. Added research files do not modify extraction.
The running executable is identified independently by SHA-256
`e461424da2cb4aae1a63a3f2910ac52d2f186268a179867bc10873056f67a040`
and ELF build ID `6417fe8b846458360c29eb051722c25c4f0274e6`.
Its source revision is not embedded and must not be asserted from checkout HEAD.
Its snapshot is schema 14. A newly compiled `truth_probe` reads that snapshot
successfully. Live and new extraction have the same usages/bindings; the merged
lib/bin helper node can retain either file depending on iteration order.

## Executed checks

- The three-package fixture passes `cargo test --workspace --all-targets` with
  default features and with `--all-features`: two unit tests and one integration
  test per configuration; lib, bin, example and proc-macro targets compile.
  Procedural expansion and build-generated code compile in both configurations.
- `cargo build --release -p rmc-graph --example truth_probe` passes (one existing
  dead-code warning in `fn_body_audit.rs`). No formatting was run. The prescribed
  neighboring Nix shell and Nix store are absent on this machine; these probes
  use the local Cargo toolchain as in the machine's fork-maintenance procedure.
- Both extraction passes produce 46 nodes, 40 bindings and **58 raw usages**.
  Their usage multisets agree, including the defects below. That equality is
  not an oracle of correctness.
- Per-crate persistence produces 46 nodes, 40 bindings and **57 usage rows**.
  Manifest counts equal actual stored rows. The sole raw multiplicity difference
  is `macro_twice`: two expansions map to the same original range and UsageId.
- Live MCP projections executed: `build_hypergraph`, `calls_from`, `who_calls`,
  `who_uses`, `get_imports`, `module_tree`, `module_dependencies`, `build_codemap`,
  and the separate parser's `get_call_graph`. Successful full, incremental and
  reused fixture builds were observed. Codemap is a bounded projection.
- The assessor's two tests pass: independently detect missing/wrong target,
  missing owner, absent relation and UTF-8 byte offsets. `--strict` on the
  baseline exits **1**, as it must; no buggy output was promoted to expected truth.

## Findings and declared limits

| Case | Observed result | Classification / source |
| --- | --- | --- |
| `renamed()` and implicit `{renamed_count}` | Import bindings exist, usages absent | Confirmed loss; `usages.rs::scan_ident`, `scan_format_string` retain target-name equality |
| `references_only() { register(leaf) }` | codemap emits `Calls` to both `register` and `leaf` | Confirmed false call; `codemap/build.rs` infers relation from target kind |
| Type/const references inside `data` | `calls_from` returns all eight non-import references, all `Other` | Existing broad query contract, not eight calls; schema has no Call/FunctionReference/TypeRef relation |
| Trait impl body | `leaf` usage exists, caller absent; impl method node absent | Declared omission in `impls.rs`, harmful to architecture; concrete and UFCS calls retain only trait declaration |
| Generic and dyn dispatch | Both reference `Act::act` | Correct contract target; dispatch mode/unknown runtime implementation is unrepresented |
| Closure and nested `fn` | Closure caller correct; nested body has no caller and call to `inner` is absent | Confirmed nested-item coverage loss; block modules and nested definitions are omitted |
| Inline module | Correct target and `inline::run` caller, but consumer module is root `truth_app` | Confirmed false module boundary; `Sites::site` uses file-root scope for module attribution |
| Field, alias, const and static structure | Type/value references exist without structural owner; field reads/writes have no target | Declared schema gaps, not absence of a dependency; ownership only carries optional containing function |
| Enum variants | Three variant nodes exist but all three marked variant usages are absent | Declared usage-target exclusion in `usages.rs`; enum-path references are not variant edges |
| Macro argument repeated twice | raw 2, persisted 1, codemap weight 1 | Source-site count 1 and expanded-site count 2 differ; no expansion provenance survives |
| Macro-generated function | Function node/call exist; generated body's `leaf` usage absent | Confirmed expanded-body loss; macro-definition tokens are not enough to scan generated bodies |
| Same-named lib/bin | One crate and one `helper` ID; `calls_from(helper)` combines both files | Confirmed collision; `crate_groups`, `emit_skeleton`, `bindings::local_item_node_id`; retained file varies between loads |
| Inherent method outside type module | Physical file is correctly `remote.rs`, parent is `model::Device`; module queries project by that parent | Semantic owner is useful, physical module needs an independent projection; neither can substitute for the other |
| Comment before impl | Same method name/file acquires a different ID | Confirmed offset-sensitive identity; `impls::assoc_item_node_id` |
| Features/tests | All-feature function included; inactive no-feature, cfg(test) and bare #[test] bodies still produce root-module usages, without their function nodes | **Three false active-graph edges**, not cleanly excluded coverage; scanner walks inactive syntax and resolves against module scope |
| Integration test/example targets | No marked usages | Intended production-target exclusion |
| Proc macro / OUT_DIR-generated functions | Neither generated target resolves in the graph | Intended loader limitation: no proc-macro server, no build-script outputs; the proc-macro crate itself is indexed |
| Local variable `f()` | No edge and no unresolved record | Points-to analysis is not promised; explicit unresolved-site representation is still missing |
| Old `get_call_graph` | Short unresolved names, inactive functions present, inline functions omitted, nested calls attributed to enclosing fn | A separate syntactic view, not an interchangeable HIR edge source |

The assessor reports nine missing included sites: renamed call, format alias,
nested call, two field accesses, three variant accesses, generated macro body.
These are a mix of confirmed bugs and declared gaps, as distinguished above.
It also reports two missing function owners, four absent structural owners, two
wrong inline consumer modules and three inactive-code edges. Relation gaps are
reported independently and must not be counted as additional missing sites.

## Reproduced inconsistent snapshot

`mcp_truth_probe.py --race --race-read 2` watches the copied library source with
Linux inotify, then appends `race_added()` after its second close-read during
workspace loading. It uses a separate `--in-process` server and restores the
file on exit. On two runs against the frozen 50-site pre-reference-only corpus:

1. First forced build: **45 nodes**, fingerprint of the changed tree.
2. Immediate query: `race_added` is absent.
3. Next ordinary build: **reused=true**, same fingerprint and graph ID, 45 nodes.
4. Forced rebuild: **46 nodes**, same fingerprint and graph ID; `race_added`
   now has its expected call to `leaf`.

An earlier attempt editing on the post-load timing log was too late and safely
invalidated on the next build; editing on the first read was too early and
produced a consistent new graph. These are not reproductions of the defect.
The two successful runs retain full JSON responses and timing logs externally.

Root cause: `snapshot::build_and_persist` loads source into the RA VFS, then
hashes the live filesystem and publishes without checking that the VFS bytes
match the hashed inputs. A correct fresh build and a poisoned build can have
the same graph ID. Opening CURRENT or comparing fingerprints is insufficient.
The fix must validate/retry or freeze the input slice; it must not replace
content hashing with mtimes. Per-crate row ownership is unrelated and must stay.

## Next implementation boundary

Use this oracle when changing edge semantics, then target/snapshot identity.
Prioritize aliases, typed relations, implementation/nested/structural ownership
and inactive/inline attribution. Preserve raw source sites and unknowns. Keep
the old parity tests for unchanged behavior, but do not require parity for a
confirmed defect. Full export and private-workspace architecture analysis remain later
tasks; this corpus makes no recommendation to move that workspace's source files.
