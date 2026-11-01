# Complete graph export (JSONL v1, snapshot schema v16)

`export_graph` is the lossless MCP read path. `build_codemap` remains a bounded
navigation view and must not be used as an architectural dataset.

1. Call `export_graph {"directory":"/project"}` for metadata, all table names,
   exact row counts, units, coverage, manifest and `graph_id`.
2. For **each** listed table, request its first page with that `graph_id`.
3. Serialize the returned `next_cursor` object as a JSON string and pass it as
   `cursor` until it is null. Page size is 1–10000 (default 100).
4. Check the collected counts against `table_counts`. A replaced CURRENT is an
   error: restart the entire export. No offset, summary, hidden truncation or
   resolution by qualified names is involved.

The cursor is opaque and bound to graph/table. It advances after `(key,value)`,
so DUP_SORT rows sharing a key survive page boundaries. The offline exporter
holds a single `OpenedSnapshot` throughout, including when CURRENT is replaced.
It starts ranges at the previous key instead of scanning all preceding rows.

For large datasets use the streaming CLI, built in the project's nix shell:

```sh
cargo build --release -p rmc-graph --example graph_export
target/release/examples/graph_export --build /project /graph-data /data/raw.jsonl
# Omit --build to export an existing snapshot without loading rust-analyzer.
python3 tools/graph_aggregate.py /data/raw.jsonl /data/files.jsonl --level file
```

The default LMDB map reserves 16 GiB of virtual address space on 64-bit hosts
(1 GiB on narrower hosts). RAM and database-file allocation follow actual pages,
not the reservation. The complete graph of a large workspace exceeds the former 1 GiB cap
once canonical rows and secondary indexes are written. Embedders can override
`GraphEnvOptions.map_size`; `MDB_MAP_FULL` rejects the staged build and preserves
CURRENT rather than publishing a truncated graph.

The first JSONL line has `metadata`; subsequent lines have `table`, `key`,
`record`. Record NodeIds use the existing JSON array of 32 bytes; row keys and
cursor bytes are lowercase hex. Keys are scoped to the manifest's workspace
and snapshot. Tables:

| Table | Record / unit |
| --- | --- |
| nodes | Exact Node: target identity, semantic parent, declaration module, physical file/span, attributes |
| bindings | Exact resolved namespace Binding, including Declared, import kind, explicit pub-use and effective visibility |
| usages | Exact typed Usage observation, structural/function owner, physical and macro expansion sites |
| contains | Parent key → child NodeId (each distinct pair) |
| unresolved_calls | Lexical-owner key → complete vector of unresolved observations |
| signatures | Node key → FunctionSignature |
| statics | Node key → StaticMetadata |
| stub_owners | External-symbol key → consumer crate (each pair) |

Secondary indexes, deletion bookkeeping and mutable embedding caches are
reconstructible storage details, excluded explicitly. The export includes all
canonical graph rows, not every internal LMDB row. No inferred dynamic call or
consumer-to-consumer clique is emitted.

## Provenance and configuration

New manifests append optional JSON `extraction_context`; no bincode record or
schema migration is necessary. Existing v16 manifests remain readable and
report absent extraction provenance as null. Exporter versions are separate
from builder versions and never substitute for them.

The builder captures MCP/RA/Salsa Git revisions (with `+dirty` when applicable),
all-features/lib-bin/test/build-output/proc-macro policies, per-target edition
and sorted actual `CfgOptions` debug strings. Those strings include enabled
features and target cfg atoms; they are diagnostics, not Rust expressions.
`targets` describes the loaded configuration. `row_origins[target_key]` records
the extractor/RA/configuration that produced that target's rows; clean targets
retain their original provenance during incremental builds. An old base without
this information produces an explicit unknown origin. `incremental_base` and
the build report explain reuse without an unbounded chain of manifests.
External stubs can have several consumer owners; use `stub_owners` for them.

The source fingerprint is not hermetic for external dependencies/environment.
For research, `tools/graph_freeze.py SOURCE DESTINATION` copies tracked and
nonignored untracked files, including dirty content, bracketed by matching
content inventories, Git status, diff and revision. Explicit exclusions are
recorded; symlinks fail rather than silently depend on a live target. Freeze
outside-root path dependencies separately while preserving relative layout.
The resulting provenance inventory and dirty patch sit beside the copy.
Ignored inputs remain an explicit limitation. The snapshot builder additionally
checks its source slice before loading and publication. Git moves/edits after a
successful freeze cannot change the copied graph input.

`tools/graph_reference_check.py CORPUS.json --source-repo /repo --graph raw.jsonl
--graph-sources /frozen-repo --output check.json` validates reference-case selectors
against a complete export and Git source revision. Selectors include exact Cargo
target, physical file, qualified name and item kind. Literal anchors are checked
in Git blobs; an existing symbol in a changed file is reported as stale, not a
usable match. The tool does not open the corpus's holdout-answer file or score any
architectural expectations. Exit 2 means one or more seeds are unavailable/stale.
Schema/data errors fail explicitly. Run its standard-library tests with
`python3 -m unittest discover -s tools -p 'test_graph_reference_check.py'`.

For comparisons across source versions, `tools/graph_boundary_compare.py
BEFORE.jsonl AFTER.jsonl SPEC.json OUTPUT.json` labels explicitly declared
physical boundaries without matching NodeIds. `SPEC` version 1 contains
`boundaries: {name: [{cargo_target, files?, file_prefixes?}]}` and
`seeds: {name: {before: selector|null, after: selector|null}}`. A selector has
exact `cargo_target`, `file`, `name`, `item_kind`. A null selector requires the
other slice's address to be absent. Overlapping boundaries and missing or
ambiguous seeds fail. Target identity uses `name[kind]@target-root-file`.

The result keeps both metadata manifests, exact seed addresses (physical file,
lexical module and semantic parent separately), incident evidence, unresolved
observations and typed boundary deltas. Usage sources follow the observation's
physical file, including macro sites; binding sources follow their lexical
module and have no invented source range. `Outside` means unlabelled local
code, `External` means a persisted external stub. Containment is not a dependency
layer. Typed pairs, source sites, observations and bindings have separate units;
site counts are not additive across relation layers. Renames must be declared
in selectors; this tool does not infer architectural owners or causal effects.
It loads O(nodes + selected evidence) memory and checks all canonical table
counts before comparing. Tests: `test_graph_boundary_compare.py` uses a small
independent lib/bin corpus with moved bodies, shifted ids/offsets, typed edges,
macro source files and unresolved callbacks.

Usage extraction never skips tokens/items after an RA panic. A failed query
rejects the entire staged snapshot. A propagated parallel-query cancellation
can be transient: the builder retries with a fresh RA database (at most three
attempts, shared with source-change retries); local cancellation and arbitrary
panics are not retried. Exhaustion returns the error and preserves CURRENT.
The CLI reports cancellation and the consumer scanner reports the failing site.
This prevents a partially recovered RA database from becoming a "complete" graph.

## Aggregation contract

`tools/graph_aggregate.py` accepts `--level item|file|module|crate`. It checks
all canonical table counts and keeps isolated scopes. Its memory is O(nodes +
aggregated edges + evidence); estimate it before a large run, and run levels
sequentially. The raw exporter itself uses O(page size) decoded rows.

- Item: nearest `consumer_item`, falling back explicitly to the lexical module;
  targets remain original symbols. No invented function caller for a field,
  alias or initializer.
- File: actual usage file → target declaration file, each qualified by Cargo
  target. A semantic type owner never relocates its method's body.
- Module: lexical consumer module → `declaration_module` of the target.
- Crate: Cargo-target NodeId, not package name. lib/bin remain distinct.
- Missing location becomes `Unlocated(node_id, projection)`, never a guessed
  module/file. `parent_id` in raw data still supplies the semantic-owner view.

Relations remain separate (`Call`, `TraitDispatch`, `FunctionReference`,
`TypeRef`, etc.). Bindings add `Declaration`, `Import` and `ReExport`; declared
bindings are not imports. Containment is a separate `Contains` relation. Every
edge retains raw table/key evidence; DUP_SORT evidence includes the target id.
Self-loops remain available for internal/cross-boundary analysis. Unresolved
calls remain raw observations with counters, never guessed edges.

Weights are independent columns: persisted usage observations, distinct source
sites `(consumer_module,file,start,end)`, expanded observations, unique target
symbols, unique consumer items/modules, resolved namespace bindings, and
containment pairs. Source-site counts apply only to usages. Bindings have no
physical token ranges in v16: their row count is neither a use-token count nor
runtime frequency. Expanded observations are a subset of usage observations,
not an additive weight. Relations/projections can share a physical source site;
their site weights are not generally additive across edges.

## Coverage and verification

Full export means complete **persisted** graph within declared extraction
coverage: lib/bin, all features, no `cfg(test)`, no build-script outputs or
proc-macro server. Inactive/unexpanded code can be absent; external stubs are
partial. Trait dispatch does not establish a unique dynamic implementation;
indirect calls remain unresolved. Sharing a physical file between several
modules of the same target can remain ambiguous. See also
[edge semantics](GRAPH_EDGE_SEMANTICS.md) and
[snapshot identity](GRAPH_IDENTITY_AND_SNAPSHOT.md).

`complete_export` checks the independent fixture counters (55 nodes, 41
bindings, 67 observations; 64 source sites, 3 expanded observations), direct
storage equality, page sizes 1/2/17, duplicate keys, invalid/stale cursors,
incremental origins and reading an old v16 JSON manifest. Server export tests
check wire JSON, mandatory graph pinning and CURRENT replacement with a live
reader. Python tests check macro multiplicity, typed relations, target-scoped
files, lexical vs semantic ownership, binding units and truncated-input refusal.
