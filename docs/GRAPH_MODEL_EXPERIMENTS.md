# Reproducible code-graph experiments

These offline research tools consume the complete JSONL export, rather than a
bounded codemap. They do not move source files, infer dynamic calls, or turn
model membership into a refactoring instruction.

Create an environment from `tools/graph-experiments-environment.yml` using
micromamba/conda. It pins the validated graph-tool implementation. Keep an
explicit package lock beside research data. Rust builds still use the project's
Nix shell; these scripts do not build Rust or require a running MCP daemon.

```sh
python tools/graph_reference_check.py CASES.json --source-repo REPO \
  --graph RAW.jsonl --graph-sources FROZEN --output applicability.json
python tools/graph_experiment_data.py RAW.jsonl model-input.json.gz \
  --corpus CASES.json --applicability applicability.json --sources FROZEN \
  --forwarders forwarders.json
OMP_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1 python tools/graph_model_experiments.py \
  model-input.json.gz config.json runs
python tools/graph_experiment_report.py model-input.json.gz config.json runs report
python -m unittest discover -s tools -p 'test_graph*.py'
```

`graph_experiment_data.py` reuses `Aggregator` and the full-export integrity
checks. The input contains every modeled node, typed edge, independent weight
column, canonical row reference, external-symbol flag, physical file, lexical
module, semantic parent and Cargo target. Structural containment and declared
bindings are recorded as excluded model relations; they remain in the raw
graph. Imports/reexports are a distinct optional view, never use-token counts.
Self-loops remain in the data and are excluded explicitly from model fits.

The public corpus is read for selectors and split names only; neither fit nor
report code reads expectations or opens the holdout-answer file. Applicability
must be `ready` for every seed and pinned to the same `graph_id`. Preserve the
source revision and extraction coverage alongside predictions. Parameters and
per-case predictions are saved before architectural assessment.

The two families are:

- Directed Leiden with the RB configuration-model objective and explicit
  resolution parameters, implemented by `leidenalg`. Edge weights are positive
  typed-pair counts or distinct source-site counts within each typed pair.
- Directed, degree-corrected, independent-layer nested SBM, implemented by
  graph-tool 2.98. Every exact relation has its own edge layer. Hierarchy levels
  are projected back to item vertices; requested scales select the nearest
  nontrivial level by absolute log block-count ratio. Repeated selection of the
  same level is reported, not counted as independent scale evidence. Inference
  fails if the fitted description length exceeds an explicitly constructed
  one-block null. Description lengths from different views are not comparable.

The model papers and primary implementation documentation are
[Leiden](https://www.nature.com/articles/s41598-019-41695-z),
[leidenalg's objective](https://leidenalg.readthedocs.io/en/stable/reference.html#rbconfigurationvertexpartition),
[nested SBM](https://arxiv.org/abs/1310.4377), and
[graph-tool inference](https://graph-tool.skewed.de/static/docs/stable/demos/inference/inference.html).
The current graph-tool documentation describes a newer API; the environment
pins 2.98 and the script uses that version's `base_type/state_args/layers` API.
The private-workspace experiment rejected 3.9 after its returned state was worse than a
directly constructed null on the large layered graph. That observation does not
claim an isolated upstream root cause; the rejected runs and source remain in
the research data.

Each view declares its relations, weight, external-stub policy, optional target
indegree ablation, explicit forwarding whitelist, and random omission rate.
The omission RNG is independent of model initialization and removes typed
pairs in canonical input order. It does not simulate all possible extraction
failures. Repetitions of an omission view vary both its mask and model seed;
the report labels that joint sensitivity separately from fixed-input repeats.
Removing high-indegree incoming edges is an ablation, not evidence
that common contracts are unwanted. No shared-target consumer clique is built.

Forwarder entries require exact `(target, file, name)`, literal function source,
the unique resolved Call and a human explanation. An unresolved call or dispatch
rejects an entry. The analytical bypass replaces incoming Call targets with a
path through each wrapper, retaining both raw row references. Other relations,
wrapper nodes and the wrapper's own outgoing observations remain. This is a
partial call-path projection, not a deletion or a claim of direct A→B execution.

Fits can resume only with the same graph, configuration, forwarders, selectors,
implementation fingerprints and library versions. `--views` restricts work to
named views; independent disjoint view sets may run in separate processes.
Results retain complete memberships, active endpoint inventories, sparse block
matrices, per-case coassignments and fit times. Inactive/isolated nodes have no
inferred membership and are not grouped together.

The report compares repeated-run ARI on explicitly counted common endpoints and
changes relative to the typed baseline. It also compares physical-file, lexical
module and Cargo-target layouts on that same baseline input. An increased
internal-edge fraction or similarity to the layout is not architectural success.

Stable cores are unanimous membership intersections across seeds that retain at
least 80% of every parent group and have at least three members. Leiden cores
must remain connected in the original view; role cores may be disconnected and
report that fact. Tiny stable groups, uncertain boundaries and unobserved nodes
are separate outcomes. A one-block null never becomes a whole-workspace core.
These are conservative repeatability diagnostics, not posterior probabilities.
Different views/scales keep separate cores instead of hiding disagreements in
one consensus partition.

The retention threshold applies to each final connected core. A disconnected
intersection that retains 80% can split into components that individually fail
the threshold; those nontrivial components belong to the uncertain periphery.
They must not inherit the larger intersection's retention value. Regression
tests cover connected repeated parent blocks whose intersection splits into two
40% components, and a surviving 80% component split from a 90% intersection.
Reports produced before this correction can overstate component retention;
regenerate their cores from saved memberships without refitting the models.
The resolution-probe client imports the same corrected core rule.

`graph_resolution_probe.py INPUT CONFIG PROBE.json OUTPUT` adds separately
predeclared Leiden resolutions for named views when a concrete development case
remains unresolved. `PROBE.json` contains `reason`, `views` and `resolutions`.
It keeps the main runs intact, saves its context before fitting and emits the
same complete memberships and unscored reference predictions. Its repeatability
and core diagnostics are saved separately, so an added scale does not silently
change the main experiment's declared configuration.

Node profiles preserve exact incoming/outgoing relation counts, consumer targets
and unresolved-observation counts. `fan_in`, `fan_out`, `mixed` and `unobserved`
are descriptive degree profiles, using a declared 4:1 ratio and a denominator
floor of one; they are not domain roles. A separate semantic-owner profile rolls
ADT/trait methods and fields up for explanation. It retains all member ids,
physical files and lexical modules and does not alter any fitted partition or
attribute observed calls to a bodyless struct anchor.

Memory grows with the input and model state. On a private workspace (about 25k nodes, 77k
internal typed pairs), a single graph-tool layered fit used about 4 GiB. Control
concurrent fits separately from RA extraction, and never run multiple heavy RA
builders just to prepare these views.

## Explicit operation impact previews

`graph_operation_impact.py INPUT SPEC OUTPUT --sources FROZEN --current REPO`
reuses the same frozen input to inspect a proposed physical split/move/merge.
It does not fit a model or run RA. This is an offline research tool, not an MCP
endpoint or a Rust refactoring engine. The output is a location relabel preview;
it cannot establish compilation, privacy or the effects of an unimplemented API.

The JSON spec pins `graph_id` and `source_revision` and contains `name` and
`moves`. Each move requires `cargo_target`, `file`, exact qualified `names` and
`to_file`; optional `descendants` follows semantic parents within that same
file/target. Missing/ambiguous selectors, overlapping moves and foreign slices
fail. A compound spec may contain `scenarios`, each with its own name/moves;
OUTPUT is then a directory. Whole source files are compared with the current
tree when both source roots are supplied; changed sources are explicitly stale.

Every incident typed pair keeps its original endpoints, relation, independent
weights and raw evidence. The report counts newly crossing and internalized
file dependencies without deleting any observed relation. File identities
include Cargo target. Lexical module and semantic owner stay in the inventory,
and unresolved observations are reported. Observed imports remain separate and
are not automatically rewritten. Mutual usage file pairs mean direct two-way
dependencies only; this is not an SCC computation or a Cargo-cycle analysis.

A module-attributed impl-header reference may travel with its impl while the
module node stays put. An optional `source_edge_moves` entry pins the complete
raw `evidence` list, original `file`/`cargo_target`, destination `to_file` and an
explicit source-audit `reason`. The preview changes only the hypothetical source
file for that pair. Partial pair/site moves are rejected; they require raw-row
reaggregation. The raw consumer/module is preserved. New impl headers, import
rewrites, visibility changes and new contract edges still require a source patch
and subsequent graph extraction. Thus a preview never presents a synthetic
shortcut or an unchanged edge count as an observed post-refactoring graph.

## Held-out acceptance and explicit source plans

After saving predictions, `graph_holdout_check.py CORPUS ANSWERS PREDICTIONS
OUTPUT` compares every held-out `cohere`/`separate` pair across all saved views
and scales. Missing observations remain `unobserved`, rather than passing the
constraint. Counts are diagnostic mismatches, not an aggregate architecture
score: same-role membership need not mean the same file. The checker never fits
a model, selects a scale or proposes a source operation.

`graph_source_plan.py SOURCE PLAN.json PATCH` renders an explicitly audited text
plan. It never writes into SOURCE. Inputs are SHA-256-pinned UTF-8 files; regions
use unique start/end markers (end excluded). An edit removes named regions and
performs literal replacements with declared counts. A new file combines literal
text and those unchanged regions. Each region must move exactly once; stale
files, overlapping removals, ambiguous markers, repeated outputs and escaping
paths fail. The patch destination must be outside SOURCE and must not exist.
This is a text renderer, not a Rust parser or an automatic refactoring engine.
The caller must audit region ownership, module paths, imports and visibility.
Apply the patch to an isolated source copy with `git apply --check` and `git
apply`; then run compilation, relevant tests, rustdoc and new graph extraction.
`git apply --reverse --check` checks reversibility without changing the copy.

The private-workspace acceptance plan is `docs/GRAPH_RESPONSE_SOURCE_PLAN.json` in that
repository. It moves CSV readers/writer/header parsing into a child module and
explicitly updates `cfg(test)` callers omitted by graph extraction. Its new
graph preserves the 197 incident typed pairs predicted by the location preview,
but also has a new impl-header TypeRef and rewritten namespace bindings. Thus
successful physical relabeling alone is not source acceptance.

Environment locks for recreation with micromamba must begin with `@EXPLICIT`
and contain package URLs. A captured `micromamba list --explicit` display may
instead begin with a human-readable environment title; strip that title and
blank lines and prepend `@EXPLICIT` before using `create -f`. Preserve the
original display for provenance. The private-workspace acceptance data contains this
normalized lock and verifies the full Python suite in the recreated graph-tool
2.98 environment. Rust builds still require the project's Nix shell; Python
acceptance tools do not rebuild or deploy Rust binaries.
