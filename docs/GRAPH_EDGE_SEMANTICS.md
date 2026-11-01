# Edge semantics, schema v15

The production consumer extractor resolves identifiers through HIR, including
import aliases. It classifies the **site**, rather than inferring a call from
the target's kind. The historical target extractor remains available through
`RMC_USAGES_FROM_TARGET=1` for comparison of unchanged behavior; it emits
`Unknown` relations and is not a semantic oracle. The original independent
[51-site corpus](GRAPH_TRUTH_CORPUS.md) remains the source of expected edges.

`Usage.relation` is independent of `Usage.category` (Read/Write/Test/Other):

| Relation | Meaning |
| --- | --- |
| Call | A direct function/method call; a known concrete trait implementation can be the target |
| TraitDispatch | A call to a trait contract without a unique implementation established |
| FunctionReference | A function used as a value, including arguments, assignments and static initializers |
| TypeRef | A type, bound or qualified type dependency |
| Construct | A unit, tuple or record ADT/variant value constructor |
| Read / Write | A resolved field access; only the final assigned place is a Write |
| Use | Other resolved references, including constants and pattern references |
| Implements | An impl declaration or concrete impl reference also referring to its contract item |
| Unknown | Legacy extractor relation; never counted as a call |

Imports/reexports remain `Binding` records. `consumer_module` is the lexical
module of the expanded reference, reduced to its nearest non-block module.
`consumer_item` is the nearest declaring function, field, alias, const/static,
ADT or variant. `consumer_function` is populated only for a function **body**;
signature types do not become body references. Closures belong to their parent
function; nested functions and trait implementation methods own their bodies.
Trait implementation qualified names include `<Self as trait-path>` and differ
from the declaration. Inherent methods keep their existing semantic parent.
Schema v16 adds independent declaration modules, Cargo-target identities and
expansion provenance; see [identity and snapshot contract](GRAPH_IDENTITY_AND_SNAPSHOT.md).

`calls_from`, `who_calls`, recursive call queries and recursion checking consume
only Call/TraitDispatch. `who_uses` retains every resolved relation.
Codemap preserves relation kinds, including incoming edges. It is still a
bounded projection, and weight=1 is membership of (from, to, kind), not frequency.

`unresolved_calls_by_owner` stores observed call sites without a graph edge,
with expression, byte range, lexical owner/module and reason:
`IndirectCallable`, `TargetOutsideGraph`, or `Unresolved`. `calls_from` returns
all unresolved sites of its lexical owner separately from resolved-call paging.
The MCP view uses the response's caller as owner, omits raw ID arrays and
honors summary navigation-field omission; the stored graph retains the IDs.
`let f = leaf; f()` retains both the FunctionReference and an indirect call;
it does not synthesize a direct edge from the assignment. Known external
functions outside graph coverage also receive explicit records.

Both new tables (`usages_by_consumer_item`, `unresolved_calls_by_owner`) are
owned by the consumer crate and participate in snapshot comparison/deletion.
Usage IDs include relation and item owner, so different facts at one source
range do not overwrite each other. v14 snapshots are rejected before their
old bincode rows are read and must be rebuilt; graph IDs incorporate the schema.

## Reproducible corrections

All listed sites are in the unchanged handwritten corpus. The baseline is the
evidence collected before production fixes, described in GRAPH_TRUTH_CORPUS.md.

| Site / source | Before | v15 |
| --- | --- | --- |
| `renamed`: `use leaf as renamed; renamed()` | No usage | Call to leaf |
| `format_alias`: `{renamed_count}` | No usage | Use of COUNT, Read category |
| `reference_only`, `fn_value`, `local_fn_value` | Untyped usage; codemap calls leaf | FunctionReference; absent from calls_from/who_calls |
| `impl_body`, `concrete_trait`, `ufcs_trait` | No impl node/caller; contract only | Distinct impl node/caller and concrete target; contract edge is Implements |
| `generic_call`, `dyn_call` | Untyped contract reference | TraitDispatch, without an implementation claim |
| `nested_body`, `nested_call` | Missing caller and inner target | Nested function node, Call edges with exact callers |
| `inline_outer`, `inline_local` | Root consumer module | inline module |
| `field_type`, `alias_type`, `const_init`, `static_fn` | No structural owner | Field, alias, const/static owner; typed relation |
| `field_read`, `field_write`, three `variant_*` sites | Missing target edges | Field Read/Write and variant Construct |
| `macro_body` | Missing generated function body | Call owned by generated fn; exact source origin in macro definition |
| `feature_off`, `unit_test`, `bare_test` | Three false active-graph usages | No active usage |
| `indirect` | Silent absence | Explicit IndirectCallable record |

`tests/edge_semantics.rs` checks all 43 included sites (42 non-import + 1
import), seven loader/config exclusions, the indirect unknown and codemap/call
projections. It additionally tests parenthesized callees, chained field writes,
signature ownership, inactive fields and macro-written field targets.
The old parity test compares a declared subset of unchanged fixture owners;
alias, field, implementation and attribution corrections are not forced to
match the buggy extractor. Full versus per-crate and incremental equivalence
continue to compare **all** tables without this projection.

Run in the repository's required development environment:

```sh
cargo test --release -p rmc-graph --test edge_semantics -- --test-threads=1
cargo test --release -p rmc-graph --lib -- --test-threads=1
python3 -m unittest discover -s crates/rmc-graph/tests -p test_graph_truth.py
cargo check --workspace --all-targets --features migraphx
```

Use `truth_probe` and `mcp_truth_probe.py` as documented by the corpus to save
raw and persisted evidence and run the actual MCP projections.

## Limits

This is not points-to analysis. Indirect and unresolved calls are observations,
not proof that no target exists. Trait calls with unsupported const/lifetime
substitutions conservatively retain their contract: RA's public concrete-method
helper accepts only type substitutions. Generic and dyn call sites do not
enumerate possible runtime implementations.

The loader still uses all features, no test cfg, no proc-macro server and no
build-script output. Implicit operator/desugaring calls are not enumerated.
Impl items inside function bodies that RA's `Impl::all_in_crate` does not list
remain outside target coverage and calls to them are recorded as unresolved.
Tuple fields of tuple types, record shorthand's combined local/field semantics,
and generated nested/structural declaration bodies are not claimed complete.
The generated-function correction scans HIR function sources; it is not a
general scan of every macro-produced declaration.

Same-name lib/bin collisions, byte-offset-sensitive associated item IDs,
macro source-versus-expansion multiplicity and the reproduced load/fingerprint
race are corrected by [schema v16](GRAPH_IDENTITY_AND_SNAPSHOT.md). The strict
assessor checks target contexts, physical/semantic ownership and macro
multiplicity against the unchanged handwritten expectations.
