# Handwritten graph truth corpus

`expected.tsv` is a semantic oracle authored from these sources, before inspecting
extractor output. It is not a golden dump from either extraction algorithm.
Markers name individual identifier sites (byte ranges are computed from the
marker and the explicit `token` column). The table is intentionally a selected
set of exact edges, not a claim that every identifier in the workspace is listed.

`owner` describes the semantic owner of the site; fields and trait implementation
names are oracle identities, even when the current schema cannot represent them.
`[lib]` / `[bin]` must remain distinct. Inherent methods belong to `Device` but
their physical module is `remote`. Closures attribute to their enclosing function;
nested `fn` bodies belong to the nested function. Generic/dyn dispatch resolves
to the trait contract, without claiming a unique runtime implementation.
Concrete/UFCS calls also reference the trait declaration, but must retain the
known implementation target separately.

Each marked site has source multiplicity one. `macro_twice` has two expanded call
sites; `macro_body` is in a macro definition and expands at `define_fn!()`.
Source and expansion multiplicities must not be silently interchanged.
Imports are checked as bindings, separately from usages. `UnresolvedCall` is an
explicit unknown; a reference to the assigned function is not the indirect call.

`coverage` distinguishes required included code from declared loader exclusions.
The fixture compiles and tests with both default and all features, including
the procedural macro and build-generated code. Hypergraph loading currently uses
all features, no test cfg, no proc-macro server and no build-script output.

Reproduce (from the repository root, using the required Nix development shell):

```
cargo test --manifest-path crates/rmc-graph/tests/fixtures/graph_truth/Cargo.toml --workspace --all-targets
cargo test --manifest-path crates/rmc-graph/tests/fixtures/graph_truth/Cargo.toml --workspace --all-targets --all-features
cargo build --release -p rmc-graph --example truth_probe
target/release/examples/truth_probe extract WORKSPACE DATA_DIR extract.json
target/release/examples/truth_probe build WORKSPACE DATA_DIR persisted.json
python3 crates/rmc-graph/tests/graph_truth.py WORKSPACE persisted.json assessment.json
```

`mcp_truth_probe.py BINARY WORKSPACE OUTPUT.json` records the running API and
selected queries in its own `--in-process` server. Its Linux `--race` mode edits
the source after the second close-read event during load, checks immediate and
reused results, then forces a rebuild. It restores the source on exit. Always
use a disposable copy; the exact event timing is recorded and must be checked
before treating a run as a reproduced race. The script never stops the shared
daemon. `--race-read N` selects another read event for loader changes.

Use a copy of the fixture outside this workspace for probes. `DATA_DIR` stores
isolated snapshots, not the shared daemon's data. `truth_probe open` can inspect
an existing snapshot without rebuilding it. The assessor reports defects rather
than accepting them as expected behavior; `--strict` fails on included defects.
