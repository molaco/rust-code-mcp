# THEORY.md

Reference for AI agents and architecture-aware tools operating on this Rust workspace (rust-code-mcp-final). Names the principles, operations, and diagnostics needed to make and justify structural decisions — splitting files, merging modules, extracting traits, moving code between crates, deciding boundaries.

## §1. Premise

This document is not a tutorial and not a derivation. Concepts are stated, given a short name, and pinned to a check the agent can perform using rmc tools.

**How to use this document.** An agent encountering a structural task (refactor, layout audit, boundary review) reads §4 to recall the principle space, §6 to choose a diagnostic, §5 to identify the operation, then jumps to the relevant rmc tool. Every principle has a name; cite the name when justifying a decision.

**Audience.** AI agents using rmc tools. Humans reading by accident should be able to follow, but prose density is tuned for retrieval-and-reuse, not pedagogy.

**Anchor.** Examples and tool invocations refer to this repo. Where something is specific to Rust idioms, it's labeled (Rust:).

**Vocabulary.** No math vocabulary as substance. Where a named theorem or external principle is relevant (min-cut, Tarjan SCC, Hyrum's Law), it appears as a parenthetical label, not as a derivation.

**Out of scope.**
- Aesthetics: file length, naming, style.
- Performance and runtime behavior.
- Human factors: team boundaries, ownership, Conway's law (treated as inputs, not derived).
- Protocol-layer composition: HTTP contracts, deployment graphs, service meshes.
- The lower half of the abstraction ladder (code → functions → types) — assumed handled.

**What this document refuses to opine on.** Whether a name is good. Whether a function is too long. Whether `Result<T, E>` is preferable to panics in some context. These are local-style decisions; this document operates at the structural level.

---

## §2. The ladder

Each level: the unit of analysis, the morphism connecting units, the rmc tool that returns the level's structure.

| Level | Unit | Morphism | rmc tool |
|---|---|---|---|
| Files | `.rs` source file | imports, uses | `get_imports`, `who_imports` |
| Modules | `mod` block | declared submodule, `use` | `module_tree` |
| Signatures | trait, supertrait chain | implements, supertrait-of | §3 |
| Crates | Cargo package | path dep, version dep | `crate_edges`, `get_dependencies` |
| Workspaces | workspace root | member crates | `workspace_stats` |

Level interactions:
- Files compose into modules; modules into crates; crates into workspaces.
- A morphism at one level is materialized at the level below: a crate dependency in Cargo.toml is implemented as one or more `use` statements in module files.
- The signature rung (§3) sits between types and files. It is the abstract shape a type or set of types realizes. Trait extraction and module-signature inference both live here.

Out of band:
- Below files: types, functions. Out of scope.
- Above workspaces: projects, organizations. Out of scope (protocol-layer).

**Notation.** *unit* = a thing at any level. *container* = the unit one level up that holds it (a file is a unit; its module is its container; the module's crate is the container above).

---

## §3. The signature rung

(Skeleton — to be filled after §4–§6 stabilize.)

The level between types and files. Traits, supertrait chains, module signatures, (Rust:) `impl` blocks viewed as morphisms.

A signature is the abstract shape a type or set of types realizes, independent of which file or module the implementations live in.

Key construction: **callsite-usage set** (P6). Given a concrete type `T` used at N call sites, the honest trait extracted from `T` has surface equal to the union of methods actually invoked. Methods present on `T` but never called at any seen call site do not belong on the extracted trait.

Pinned to: `who_uses`, `similar_to_item`, `find_references`, `function_signature`.

---

## §4. Named principles

The fifteen principles below are the substance of the theory. Cite by name when justifying a decision.

Format: name, statement, applies-when, how-to-check, failure mode. Parenthetical labels at the end reference established results.

### P1. Boundary cost

**Statement.** A boundary between two units is expensive in proportion to the density of morphisms crossing it and the instability of those morphisms.

**Applies.** Always. The load-bearing principle from which most others derive.

**Check.** Count edges crossing the boundary (`get_imports` per file, or `crate_edges` per crate). Inspect those edges for signature stability (have the referenced symbols' names or types churned recently — git log if available, otherwise treat as unknown).

**Failure mode.** Treating boundary cost as purely structural (edge count) without considering stability. Three edges to a churning external API can be more expensive than twenty edges to a stable interface.

(min-cut principle)

### P2. Density gradient

**Statement.** Within a healthy container, the density of morphisms among contained units exceeds the density of morphisms crossing the container's boundary.

**Applies.** Auditing whether a directory, module, or crate is a real container or a fictional one.

**Check.** For a directory D: count edges between files inside D, and edges from files in D to files outside D. If the second is comparable to or larger than the first, the boundary is fictional. Tools: `get_imports` per file, aggregated.

**Failure mode.** Both junk-drawer directories (many files, low internal coupling) and pseudo-boundaries (many external edges, few internal ones) fail this. The principle is silent on size — a single-file directory passes vacuously and that's fine.

### P3. Acyclicity condition

**Statement.** At the container level (modules, crates, workspaces), the dependency graph must be acyclic. Units that form a strongly connected component cannot be partitioned across containers.

**Applies.** Detecting impossible boundaries. If two crates depend on each other, no boundary between them is real — the SCC has not been broken.

**Check.** `crate_edges` for crate-level cycles; `forbidden_dependency_check` for assertion-style enforcement. For modules, check `module_tree` against `get_imports`.

**Failure mode.** Mutual dependencies disguised through trait objects, function pointers, or callback patterns may not appear in static dependency graphs but still violate the principle. Catch via `who_calls` / `calls_from` across the suspected boundary.

(Tarjan 1972, SCC detection)

### P4. Alignment

**Statement.** A unit belongs in the container where most of its morphisms terminate.

**Applies.** Deciding where to move a misplaced file or module.

**Check.** For file F: count edges to each candidate container directory. F belongs where its edges are densest, unless that placement would introduce a cycle (P3).

**Failure mode.** When edges tie or are evenly distributed across candidates, the unit is a shared interface and belongs in a third location (one level up, or in a `common`/`shared` sibling both candidates depend on). Don't force it into one of the two by majority.

### P5. Named-surface test

**Statement.** A container is real if and only if its public surface can be summarized in one sentence.

**Applies.** Auditing whether a directory, module, or crate has a coherent identity.

**Check.** List the public items of the container (`get_exports`, `dead_pub_in_crate`). Attempt to write one sentence describing what they collectively provide. If the sentence requires "and also" or "miscellaneous," the boundary is a coincidence, not a concept.

**Failure mode.** Some containers exist purely for build-system or visibility reasons (a crate exists to break a compile-time cycle, a module exists to satisfy a macro hygiene rule). Mark these explicitly with a one-sentence purpose ("breaks cycle between A and B") rather than failing the test silently.

### P6. Callsite-usage set

**Statement.** When extracting a trait from a concrete type, the honest trait surface is the union of methods invoked at observed call sites — not the full method surface of the underlying type.

**Applies.** Generalization operations: concrete type → trait, concrete module → signature, concrete crate → feature-flagged abstraction.

**Check.** For type T: gather call sites via `who_uses(target=T)`. For each site, record which methods/fields of T were accessed. The extracted trait's surface is the union.

**Failure mode.** Extracting the full type surface as a trait produces a trait that is a duplicate of the type, not an abstraction. Symptom: only one type ever implements it, and there is no plausible second implementer.

### P7. Cycle-breaking interface

**Statement.** When two containers depend on each other (P3 violation), the fix is to introduce a third unit that both depend on.

**Applies.** Resolving mutual dependencies at any level.

**Check.** Identify the symbols crossing both directions of the cycle. The smallest set of these is the cycle-breaking interface — extract it into a unit that lives one level up, or in a shared location both originals depend on. Verify post-extraction with `crate_edges`.

**Failure mode.** Moving the cycle to a different pair of containers without actually breaking it. The third unit must be acyclically depended on by both originals; if either original still re-imports something from the other through the new unit, the cycle survives.

### P8. Bridge unit

**Statement.** A unit whose removal would disconnect the dependency graph is a bridge. Bridges are the highest-leverage refactor targets.

**Applies.** Identifying where structural change has the most effect. Bridges typically encode the cycle-breaking interfaces (P7) that healthy architectures rely on.

**Check.** For each candidate unit, ask: does removing it disconnect the graph into components that previously only communicated through this unit? Bridge candidates often appear as small `common` / `shared` / `core` modules with high incoming fan-in. `who_imports` returning many distinct downstream containers is a signal.

**Failure mode.** Refactoring a bridge unit without understanding its role typically introduces cycles (P3) elsewhere when downstream containers reach around it. Treat bridges as load-bearing — change their internals freely, change their signatures carefully.

### P9. Forgetful operation

**Statement.** Most refactor operations are lossy: applying the operation and its inverse does not return the original state.

**Applies.** Reasoning about whether a refactor is safe to revert, or whether two refactor paths converge.

**Check.** For operation O, ask: can the pre-O state be reconstructed purely from the post-O state? If no, O is forgetful. Extract-then-inline loses boundary information. Generalize-then-specialize loses the callsite-usage set that justified the trait extraction.

**Failure mode.** Treating refactors as reversible. After extracting a helper, re-inlining it does not restore byte-identical text — comments, formatting, and adjacent edits may have changed in between. After generalizing to a trait and specializing back, the trait surface that was extracted is gone; the next extraction starts from scratch.

### P10. Hub / leaf / midstream

**Statement.** Each unit can be classified by in-degree/out-degree shape: hubs have high in-degree (called by many); leaves have low out-degree (terminal); midstream units forward.

**Applies.** Triaging which units bear refactor cost. Renaming a hub fans out to N callers; renaming a leaf affects nothing downstream.

**Check.** `recursive_callers_count` for hub-ness of a function; `who_imports` for hub-ness of a module or item. Combine with `call_graph` to inspect midstream depth.

**Failure mode.** Treating all units as equivalent in refactor cost. Hubs require migration plans (P9 forgetfulness compounds across callers); leaves usually do not. A "trivial rename" of a hub is not trivial.

### P11. Co-change locality

**Statement.** Units that always change together across container boundaries signal either a missing shared abstraction or a misplaced boundary.

**Applies.** Detecting boundaries that pass structural checks (P2, P3) but are functionally violated by always-coupled change.

**Check.** Inspect git history for files that co-occur in commits with high frequency relative to their individual change frequency. Not currently surfaced by rmc — requires `git log --name-only` plus post-processing.

**Failure mode.** Co-change is a lagging indicator. Newly-introduced coupling won't show up until the codebase has churned enough to produce signal. Use cautiously on young code, or on code whose change history was rewritten.

### P12. Surface stability

**Statement.** A boundary is cheap in proportion to the stability of the morphisms crossing it. A boundary with churning signatures is expensive regardless of edge count.

**Applies.** Audit decisions where edge counts are similar between options but signature volatility differs.

**Check.** Inspect change frequency of symbols on the boundary (git log on the files defining the boundary's public exports). High change frequency on public types or function signatures → unstable surface.

**Failure mode.** Interacts with P11. If both sides of a boundary change together when the surface changes, the surface isn't doing its job — it's a leak, not an interface.

(Hyrum's Law: every observable behavior eventually becomes a contract)

### P13. Visibility as projection

**Statement.** Visibility modifiers (`pub`, `pub(crate)`, `pub(super)`, `pub(in path)`) project internal morphisms onto the container's external surface. The correct visibility is the one that exposes exactly the morphisms callers need and no more.

**Applies.** Auditing public API. Downgrading over-exposed items. Promoting under-exposed items that siblings reach around.

**Check.** `dead_pub_in_crate` and `dead_pub_report` find `pub` items with no cross-crate consumer (candidates for `pub(crate)` downgrade). For the inverse, look for items used across module boundaries within a crate via workarounds (re-exports, `pub(super)` chains).

**Failure mode.** Treating visibility as a style decision. It is a structural decision: it determines which morphisms are part of the container's contract and which are implementation detail.

### P14. Re-export transparency

**Statement.** Re-export chains (`pub use a::b::c as d`) are syntactic redirection and do not change the morphism graph. Treat re-exported name and original definition as the same morphism target.

**Applies.** Reasoning about whether `crate::Foo` and `crate::internal::Foo` are duplicates or the same thing. Avoiding double-counting in audit metrics.

**Check.** `re_export_chain` traces redirection; `get_reexports` lists them per module.

**Failure mode.** Counting re-exports as duplicate definitions. They contribute to API ergonomics, not structural complexity. Conversely, ignoring re-export chains hides where the actual definition lives — `find_definition` plus `re_export_chain` is the safe combination.

### P15. Trait coherence as ordering

**Statement.** Supertrait hierarchies must form a partial order — no cycles, no contradictory bounds. The trait dependency graph at the signature level (§3) obeys the same acyclicity condition (P3) as containers.

**Applies.** Designing trait hierarchies during generalization (P6). Auditing existing trait sets.

**Check.** Inspect supertrait declarations across the workspace. (Rust:) the compiler catches outright cycles, but the design pathology — that a supertrait isn't actually a generalization of its subtraits — is invisible to the compiler. Cross-check via callsite-usage sets (P6): if a subtrait's callers never use the supertrait's methods, the supertrait is not actually generalizing.

**Failure mode.** Adding a method to a supertrait that conflicts with a subtrait's existing method, or that no subtrait implementer ever provides meaningfully. The result is a supertrait that exists for hierarchy-shape reasons but provides no abstraction.

---

## §5. Operations

(TBD. Catalog: split, merge, move, extract, inline, hoist, generalize, specialize, vendor. Each operation: name, precondition, postcondition, failure mode, rmc tools used to apply or verify.)

## §6. Diagnostics

(TBD. Five to seven questions to run against an existing layout. Each: question, metric, rmc tool call, threshold heuristic, common false positive. Maps loosely 1:1 to a subset of §4 principles.)

## §7. Composition

(TBD. What round-trips and what doesn't. Mostly negative results — extract-then-inline is lossy (P9), generalize-then-specialize forgets the trait extraction point (P9). Important because agents are tempted to revert operations and assume cleanliness.)

## §8. Worked walkthrough

(TBD. One end-to-end refactor decision on rust-code-mcp-final. Likely candidate: `src/graph/` or `src/tools/`. Run §6 diagnostics, decide §5 operation, justify each step against §4 principles.)

## §9. Gaps

(TBD. Known holes:
- Co-change (P11) requires git log; not exposed by rmc.
- Callsite-usage set (P6) needs method-resolution finer than `who_uses` provides.
- Module-signature inference (§3 construction) has no one-shot rmc tool.
- Surface stability (P12) requires temporal data not in the snapshot.)
