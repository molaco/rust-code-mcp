# Target identity and coherent snapshots, schema v16

The [independent corpus](GRAPH_TRUTH_CORPUS.md) supplies the expected semantics.
`tests/identity_snapshot.rs` adds shared physical source compiled by both a lib
and a bin, nested macro multiplication, and comment insertion before every
declaration. Whole/per-target and incremental/full table comparisons remain
separate persistence checks, not semantic oracles.

## Identities and ownership

Cargo targets are independent crate nodes/write units. Their key is
`name[kind]@workspace-relative-root`; root path distinguishes equal names in
different packages and kind distinguishes targets sharing even a root file.
The crate node's `file` is that root; `crate_target_kind` comes from RA's actual
crate graph. Display names and Rust qualified names remain Rust names.
Ambiguous lookup fails explicitly. `app[lib]::helper` and `app[bin]::helper`
select target kinds; a bare unique path continues to work. The full key
`app[bin]@src/main.rs::helper` additionally selects the target root. Crate-edge,
metric and module-dependency labels use full target keys when a Rust crate name
is shared, so projections cannot fold distinct targets back together. A crate and its root
module share a path: lookup consistently selects the module.

Module ids derive from the target id and module path; ordinary item ids from
the module id, kind and name. Associated item ids use the declaration module,
kind/name and the impl's tokenized header (or trait declaration name).
Whitespace/comments and body contents are excluded. Trait arguments, self
type, generic parameters, where clauses and attributes distinguish specialized
impls. Enum variants and fields derive from the parent id and name. Block
items derive from their enclosing declaration, kind/name and lexical block
ordinal; adding a preceding block changes that ordinal, while inserting
comments, whitespace or non-block statements does not. Moving/renaming an item
or changing its impl header can change its id; no fuzzy match is asserted.
These are semantic keys within a declared slice, not identities across arbitrary
refactorings. Same-id nodes can be joined across snapshots after offset shifts.

`parent_id` remains the semantic containing item (an inherent method's type,
trait member's trait, field's ADT/variant). `declaration_module` is independently
the lexical Rust module. `file`/`span` are physical source coordinates; modules
also have source locations, and expanded module declarations use invocation
coordinates. Module dependency projection uses the declaration module;
containment still retains semantic ownership. This does not assign a business
domain owner: that is an architectural research decision.

Files shared by targets are walked once **per target**, using
`Semantics::new_for_crate` in the RA fork. The old default resolves syntax in
the first matching crate and loses one target's calls. The new context uses
the selected target for files it contains, and dependency contexts for other
files. Its caches belong to that Semantics instance. This follows rustc's
separate [Compiler Session/Config and input](https://doc.rust-lang.org/nightly/nightly-rustc/src/rustc_interface/interface.rs.html),
without changing type inference rules. The focused RA fixture checks both
contexts and dependency-source fallback.

Per-crate row ownership is unchanged: nodes belong to their declaring Cargo
target, bindings/usages to the consumer target, shared stubs to all naming
targets. New ids do not merge lib/bin rows. Target keys, not display names,
index the incremental manifest; dependencies are visited before dependents.

## Macro observations and counts

`Usage.expansion` records the outer invocation file, an outer-to-inner chain
of invocation ranges, and the final token range in expanded syntax. Inner
ranges are **expansion coordinates**, not byte offsets in the physical file.
No per-load RA macro ids enter persisted provenance. Physical source origin,
relation, target and lexical owner remain on the original usage record.

`twice!(leaf)` has one physical source site and two expanded Call observations;
two nested invocations of `twice!` produce four observations at one source site.
Usage identity includes expansion provenance. Discovering the *same expanded
occurrence* through multiple scans is deduplicated before both raw export and
persistence; distinct expanded tokens remain separate. Expanded provenance is
also exposed by non-summary `who_uses` and call-site MCP responses.

The manifest and `build_hypergraph.usage_counts` name their units:

| Field | Unit |
| --- | --- |
| `observations` | Distinct persisted target/relation/owner/site observations; equal to `usage_count` and usages_by_id length |
| `source_sites` | Distinct lexical module + physical file/start/end, with Cargo context retained in the module id |
| `expanded_observations` | Observations with expansion provenance; a subset of observations |

These measure static observations, not execution frequency. Codemap is still a
bounded membership projection; its weights do not become these counts.

## Snapshot transaction

One workspace build lock covers reuse, input capture, RA loading, staging,
publication and pruning. Content hashes identify reusable contents; file and
directory change stamps protect one attempt. The stamps include file identity,
length, modification time and, on Unix, ctime with nanoseconds. They catch
same-content replacement, edit-and-revert and temporary source creation/removal.
Walk errors fail the build instead of silently shrinking its fingerprint.

The declared workspace slice is captured before loading RA, checked immediately
after loading and again before the manifest is committed/published. A changed
slice discards both RA and staging and retries, at most three fresh attempts;
continuous edits return an explicit error without publishing. Missing
`Cargo.lock` creation therefore causes a reload, not a new hash attached to an
old RA database. Hashing itself is bracketed by stamp checks.
Requesting a member directory indexes that declared subtree; Cargo membership
does not silently include sibling targets whose sources are outside its
fingerprint. Symlinked Rust files are hashed by their source contents too.

An existing snapshot directory is never rewritten, including a forced rebuild
of identical contents. Such a rebuild gets a new opaque graph id; `fingerprint`
remains its content identity. Reuse prefers the compatible CURRENT instance,
then the content-addressed fallback. Failed staging is removed; CURRENT and
open readers retain their earlier immutable instances. Superseded instances
are pruned by the existing reader-aware policy. No rollback requires rebuilding
the previous reader's data.

v15 snapshots are rejected before bincode rows are read and require a full
rebuild: their ids, expansion records and even load/fingerprint coherence can
be wrong. Old source evidence remains valid historical evidence and is not
rewritten as v16 output.

## Coverage limits

The declared slice is the existing workspace input policy: Rust files,
manifests/locks and root Cargo/toolchain configuration, excluding target/.git/
.skeleton. Directory stamps conservatively reject structural changes even
outside Rust files. External path/registry dependencies, environment variables
and tool binaries are **not** a hermetic content-addressed input closure;
changes there need an explicit forced build. Proc-macro/build-output/test target
coverage remains as described in [edge semantics](GRAPH_EDGE_SEMANTICS.md#limits).
Several modules including the same file within one target are still subject to
RA's module ambiguity; target-context selection does not solve that separate
case. Timestamp/identity guards assume ordinary filesystem operations, not an
adversary capable of forging filesystem change metadata.
