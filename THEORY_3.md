# THEORY_3.md

Step-by-step operational theory for AI agents working on Rust projects with
`rust-code-mcp`.

This document is the concrete workflow version of `THEORY.md` and
`THEORY_2.md`. It starts from the way the work is actually done:

1. Starting project: move from fast agentic discovery to a real module/crate
   layout.
2. Semi-mature project: implement a feature without letting scope destroy the
   agent.
3. Mature project: change the design while preserving compatibility.

The theory is attached to each step. Category theory explains the boundary and
layout moves. HoTT/type theory explains the type/function redesign moves.

## 0. The Small Vocabulary

There are only three primitive operations.

### Operation 1: Move

Move an existing thing without changing its level.

Examples:

- file to another directory,
- function to another file,
- type to another module,
- module directory to another crate,
- re-export to another facade.

Category-theory meaning:

- An object is reattached to a different container.
- The goal is to turn external morphisms into internal morphisms.
- The graph changes by changing where edges cross boundaries.

HoTT/type meaning:

- The moved type/function should remain equivalent for callers.
- If the public path changes, the path itself may be part of the contract.
- Re-exports and adapters can witness that old callers still reach the same
  thing.

### Operation 2: Split / Merge

Change the number of things at the same level.

Examples:

- split file,
- merge files,
- split module,
- merge one-file modules,
- split crate,
- merge tiny crate into module,
- split type,
- merge types,
- split function,
- merge duplicated helpers.

Category-theory meaning:

- Split refines one object into several objects.
- Merge quotients several objects into one object.
- The correct split keeps dense edges internal to each new object.

HoTT/type meaning:

- Struct split is product decomposition.
- Enum split is sum decomposition.
- Function split factors a transformation through named intermediate shapes.
- Merge says two shapes were always one shape for the callers that matter.

### Operation 3: Lift / Lower

Change the level of a thing.

Examples:

- file to module,
- module to file,
- module to crate,
- crate to module,
- concrete type to trait,
- concrete type to wrapper/supertype,
- field group to child type,
- broad type to smaller types.

Category-theory meaning:

- Lift maps objects and morphisms into a higher-level category.
- Lower maps a weak higher-level object back down.
- A lifted object must have a real public surface.

HoTT/type meaning:

- Concrete to trait: lift a specific implementation to the contract callers use.
- Wrapper/supertype: embed a value into a broader shape.
- Child type: make an implicit invariant explicit.
- Lowering specializes when the abstraction is not used.

## 1. Theory Dictionary

Use this dictionary while following the workflows.

| Practical phrase | Category-theory reading | HoTT/type-theory reading |
|---|---|---|
| File, module, crate | object | context containing terms/types |
| Import, reference, call | morphism | dependency on a term/type |
| Directory/module boundary | partition or quotient | context boundary |
| Public API | projection of internal graph | exposed contract |
| Re-export | aliasing morphism | same term through another path |
| Move | reattach object to another container | preserve caller equivalence |
| Split | refine object into several objects | product/sum/function decomposition |
| Merge | quotient several objects into one | identify shapes used as one |
| Module to crate | lift to higher category | expose stronger contract |
| Crate to module | lower to weaker category | specialize the abstraction |
| Trait extraction | project used behavior | contract from callsite usage |
| Adapter | commuting path during migration | witness between old and new shapes |

Do not use theory words as decoration. Every theory claim must imply an action,
tool check, or exit condition.

## 2. Global Workflow Rules

These rules apply to all three workflows.

### Rule 1: Start with version-control state

Action:

- Run `jj status`.
- If `jj` is not available, run `git status`.
- Identify unrelated dirty files before editing.

Theory meaning:

- The working copy is the current object being transformed.
- Unrelated dirty files are external constraints, not material for the current
  operation.

Exit condition:

- The agent knows which files it may touch.
- Unrelated changes are not reverted.

### Rule 2: Structure before local cleanup

Action:

- Do not start by polishing functions while the directory/module/crate graph is
  chaotic.
- First make boundaries understandable enough that local cleanup can be scoped.

Theory meaning:

- Category-theory boundary work comes before HoTT/type-function refinement when
  the graph is unstable.
- A function cannot be judged well if the context around it is fictional.

Exit condition:

- The agent can name the containing module/crate in one sentence.

### Rule 3: Shallow first

Action:

- Prefer `src/dir/file.rs`.
- Avoid deeper nesting until the nested group earns a name and has internal
  density.

Theory meaning:

- A directory is a quotient of a graph, not a visual bucket.
- A second directory level is another quotient. It must earn the extra boundary
  cost.

Exit condition:

- Each directory has a named surface and most edges stay inside it.

### Rule 4: Guidelines after boundaries

Action:

- After structural boundaries are stable, enforce project guidelines crate by
  crate, module by module, then function by function.
- Use `.docs/rust-guidelines-final.md` when present.
- Respect local instructions. In this repo, do not run formatting commands.

Theory meaning:

- Guideline cleanup is HoTT/type-function work.
- It refines transformations and contracts inside already useful contexts.

Exit condition:

- Local code is simpler without changing the structural decomposition.

## 3. Workflow A: Starting Project

Goal:

Move from fast AI-generated discovery code to a shallow, understandable module
layout, and then to workspace crates only if the project size earns them.

Use this when:

- the project was created by fast agentic iteration,
- features and dependencies are still being discovered,
- the directory layout is bad,
- one directory has too many files,
- many directories have one file,
- boundaries are unclear.

### A0. Build fast to discover the app

Action:

- Use agentic coding to implement rough features quickly.
- Let the first layout be imperfect.
- Optimize for learning the app behavior, dependencies, user workflows, and
  failure cases.

Theory meaning:

- This phase generates the initial graph.
- The graph is allowed to be noisy because its purpose is discovery.
- In HoTT terms, the early code samples the domain shapes before the final type
  structure is known.

Tools/evidence:

- user workflows,
- smoke tests if available,
- dependency list,
- feature notes,
- examples or manual runs.

Exit condition:

- The agent can describe what the app must do.
- The major external dependencies are known.
- The rough domain nouns and workflows are visible in code.

Common failure:

- Trying to impose final architecture before the app behavior is understood.

### A1. Stop feature expansion and freeze the current behavior

Action:

- Pause new feature work.
- Record the current expected behavior.
- Add or identify smoke tests if they already exist.
- If tests are missing, at least write down the workflows the refactor must
  preserve.

Theory meaning:

- Before changing the graph, record what equivalence means.
- HoTT reading: tests and workflows are witnesses that old and new code inhabit
  the same behavior for important paths.

Tools/evidence:

- existing tests,
- examples,
- CLI commands,
- screenshots or manual workflows where relevant.

Exit condition:

- The refactor has a behavioral baseline.

Common failure:

- Refactoring structure without knowing what behavior must remain equivalent.

### A2. Inventory the current structural graph

Action:

- List files.
- Map modules.
- Inspect imports and references.
- Identify obvious hubs and junk drawers.

Theory meaning:

- Build the free category generated by the current file/module graph.
- Objects are files/modules.
- Morphisms are imports, uses, calls, and references.

Tools/evidence:

- `rg --files`
- `module_tree(directory=..., krate=...)`
- `get_dependencies(file_path=...)`
- `get_imports(directory=..., module=...)`
- `who_imports(directory=..., target=...)`
- `who_uses_summary(directory=..., target=...)`

Exit condition:

- The agent has a list of candidate file/module clusters.
- The agent knows which files are hubs.

Common failure:

- Grouping by filename or vibes instead of observed edges.

### A3. Identify dense groups

Action:

- Group files that import or call each other heavily.
- Separate files that only happen to live nearby.
- Mark bridge files/modules that many others use.
- Mark leaf files/modules that can move cheaply.

Theory meaning:

- A good directory is a quotient of a dense subgraph.
- Dense internal morphisms should become internal edges.
- Sparse external morphisms become the public surface.

Tools/evidence:

- `get_dependencies(file_path=...)` per candidate file,
- `get_imports(directory=..., module=...)` per candidate module,
- `who_imports` for likely shared items,
- `recursive_callers_count` for high-fan-in functions,
- `crate_edges` only if multiple crates already exist.

Exit condition:

- Each candidate group has a reason based on edge density.
- Hubs and bridge units are not accidentally buried.

Common failure:

- Creating one huge directory plus many one-file directories.

### A4. Name each candidate group

Action:

- For every proposed directory/module, write one sentence:
  "This module provides X for Y."
- Reject groups that require "and also" unless they are intentionally a facade.

Theory meaning:

- A nameable public surface means the quotient has semantic content.
- The name is not aesthetics. It is a claim about the projected interface.

Tools/evidence:

- `get_exports(directory=..., module=..., consumer=...)`
- `get_declared_reexports(directory=..., module=...)`
- source inspection for public items,
- `dead_pub_report(directory=...)` as a rot signal.

Exit condition:

- Every proposed module has a one-sentence surface.

Common failure:

- Naming a junk drawer after the broadest noun in it.

### A5. Propose a shallow module layout

Action:

- Produce a concrete target tree.
- Prefer max one directory depth:

```text
src/indexing/file_processor.rs
src/indexing/indexer_core.rs
src/search/bm25.rs
src/search/resilient.rs
```

- Use deeper nesting only when the nested group has its own density and public
  surface.

Theory meaning:

- This is the proposed quotient map from files to modules.
- Each directory is a container object.
- The parent module projects a small surface out of the internal graph.

Tools/evidence:

- proposed tree,
- per-directory one-sentence purpose,
- list of expected public items,
- list of expected internal-only files.

Exit condition:

- The target layout can be implemented with Move and Split/Merge operations.

Common failure:

- Designing a deep taxonomy instead of a working quotient of the current graph.

### A6. Implement the module refactor

Action:

- Move files into the proposed directories.
- Split large mixed files only when the split follows dense internal groups.
- Merge tiny fictional modules when they have no independent surface.
- Update `mod` declarations and imports.
- Keep visibility narrow.

Theory meaning:

- Move reattaches objects to better containers.
- Split refines overloaded objects.
- Merge quotients fake boundaries.
- Visibility is projection; it should not be widened just to make compilation
  easier.

Tools/evidence:

- `get_dependencies(file_path=...)` after moves,
- `get_imports(directory=..., module=...)` after moves,
- `get_exports(directory=..., module=..., consumer=...)`,
- `who_imports(directory=..., target=...)`,
- normal compile/test commands if appropriate for the project.

Exit condition:

- The project compiles or reaches the expected mechanical blockers.
- The new module graph is shallower and more nameable.
- Public surface did not widen accidentally.

Common failure:

- Fixing imports by making everything `pub`.

### A7. Verify the new boundary graph

Action:

- Re-run the structural checks.
- Compare the new graph to the proposal.
- Look for edges that still cross heavily between sibling directories.

Theory meaning:

- Verify that the quotient actually reduced boundary cost.
- If most morphisms still cross between two modules, they are not separate
  objects at this level.

Tools/evidence:

- `module_tree(directory=..., krate=...)`
- `get_imports(directory=..., module=...)`
- `get_dependencies(file_path=...)`
- `who_imports(directory=..., target=...)`
- `dead_pub_report(directory=...)`

Exit condition:

- Each module boundary earns its existence.
- Remaining cross-boundary edges are intentional.

Common failure:

- Declaring the layout done because the files moved, without checking the new
  edges.

### A8. Decide whether modules should become crates

Action:

- Only lift a module to a crate if the module is big enough, stable enough, and
  independently meaningful.
- Prefer crate shape:

```text
workspace/crate1/src/dir1/file1.rs
workspace/crate1/src/dir2/file1.rs
```

- Keep each crate internally shallow until deeper nesting earns itself.

Theory meaning:

- Module-to-crate is Lift.
- The module object is moved into a higher category.
- The public API becomes a stronger contract because other crates can now depend
  on it.

Tools/evidence:

- `get_exports(directory=..., module=..., consumer=...)`
- `who_imports(directory=..., target=...)`
- `who_uses_summary(directory=..., target=...)`
- `crate_edges(directory=...)` after extraction,
- Cargo metadata if package dependency structure is the question.

Exit condition:

- The candidate crate has a named public surface.
- It does not depend back on its callers.
- `crate_edges` has no forbidden cycle.

Common failure:

- Creating crates because directories are large, not because boundaries are
  stable.

### A9. Enforce local guidelines after boundaries stabilize

Action:

- Work crate by crate.
- Inside each crate, work module by module.
- Inside each module, work file/function/type by file/function/type.
- Enforce `.docs/rust-guidelines-final.md` when present.

Examples:

- no unjustified unsafe code,
- low cyclomatic complexity,
- minimal function/method LOC,
- limited input count,
- bounded loops,
- no accidental recursion,
- clear visibility,
- docs and derives for public surface.

Theory meaning:

- This is HoTT/type-function refinement.
- Types are adjusted to encode invariants.
- Functions are simplified as transformations between clearer shapes.
- Tests witness behavioral equivalence.

Tools/evidence:

- `unsafe_audit(directory=...)`
- `fn_body_audit(directory=...)`
- `recursion_check(directory=...)`
- `channel_capacity_audit(directory=...)`
- `derive_audit(directory=..., required_derives=[...])`
- `missing_docs_audit(directory=...)`
- `analyze_complexity(file_path=...)`
- `function_signature(directory=..., target=...)`

Exit condition:

- Guideline violations are resolved or explicitly justified.
- Local cleanup did not destabilize module/crate boundaries.

Common failure:

- Starting this step before the structural graph is stable.

## 4. Workflow B: Semi-Mature Project

Goal:

Implement a feature in a project that already has mostly useful boundaries,
without letting the feature become too large for the agent to execute.

Use this when:

- a feature request is too big,
- the agent loses track of the plan,
- implementation touches too many modules,
- existing boundaries are mostly good but the feature does not fit cleanly.

### B0. State the feature as a behavior change

Action:

- Write the feature in concrete behavior terms.
- Identify inputs, outputs, user-visible changes, and failure cases.
- Avoid starting with file edits.

Theory meaning:

- HoTT/type reading: define the desired new inhabitant of the program behavior.
- Category reading: do not choose graph operations until the target behavior is
  known.

Tools/evidence:

- issue/request text,
- examples,
- expected tests,
- affected commands or APIs.

Exit condition:

- The feature can be tested or manually verified.

Common failure:

- Beginning with a guessed implementation location.

### B1. Find the smallest high-leverage implementation target

Action:

- Search for the module, crate, file, or type cluster that can absorb most of
  the feature.
- Prefer one target.
- Accept two targets only if their boundary is already stable.

Best targets:

- one independent crate,
- one coherent module,
- one new file inside a coherent module,
- one local type/function cluster.

Theory meaning:

- Category-theory reading: find the smallest subgraph whose outgoing boundary is
  narrow enough for the feature.
- HoTT reading: find the smallest type/context where the new invariant belongs.

Tools/evidence:

- `module_tree(directory=..., krate=...)`
- `who_uses_summary(directory=..., target=...)`
- `who_imports(directory=..., target=...)`
- `calls_from(directory=..., caller=...)`
- `function_signature(directory=..., target=...)`
- `semantic_overlaps(directory=...)` when duplicate logic is suspected.

Exit condition:

- The agent can say: "The feature belongs primarily in X."

Common failure:

- Choosing a target because it is familiar rather than because it has the
  densest relevant edges.

### B2. If no target exists, stop and redesign

Action:

- If the feature must modify many unrelated modules at once, do not implement
  directly.
- First create or repair the boundary that would make the feature local.

Theory meaning:

- Category-theory diagnosis: the required object does not exist yet.
- The current quotient is fictional for this feature.
- HoTT diagnosis: the required type shape or invariant is missing.

Redesign options:

- Move code to where most references already point.
- Split a module whose public surface says "and also."
- Merge modules that always change together.
- Wrap a type into a supertype.
- Add a child/member type for a hidden invariant.
- Split one type into two independent types.
- Extract a trait from observed callsite usage.

Tools/evidence:

- same as B1,
- plus `get_exports(directory=..., module=..., consumer=...)`,
- `get_declared_reexports(directory=..., module=...)`,
- `dead_pub_report(directory=...)`,
- `crate_edges(directory=...)` if crate boundaries are involved.

Exit condition:

- The redesign creates a smaller implementation target.

Common failure:

- Treating "feature too big" as an effort problem instead of a boundary problem.

### B3. Choose the primitive operation

Action:

- Classify the redesign or feature work as Move, Split/Merge, Lift/Lower, or a
  short composition.

Decision table:

| Symptom | Operation |
|---|---|
| Most references point elsewhere | Move |
| One module has multiple surfaces | Split |
| Two modules always work together | Merge |
| A module has become independently meaningful | Lift module to crate |
| A crate has no independent job | Lower crate to module |
| Callers need only part of a concrete type | Lift concrete type to trait |
| One type carries two invariants | Split type |
| Function has many unrelated inputs | Create child type or split function |

Theory meaning:

- The operation is the proof obligation.
- The agent must explain why that operation preserves behavior and improves the
  graph/type shape.

Tools/evidence:

- operation-specific evidence from B1/B2,
- source inspection for invariants,
- tests or manual workflows for equivalence.

Exit condition:

- The planned operation is small enough to execute in one agent pass.

Common failure:

- Planning a broad rewrite instead of one primitive operation.

### B4. Implement the smallest slice

Action:

- Implement only the smallest feature slice that proves the design.
- Keep edits inside the chosen target unless the boundary explicitly requires an
  adapter or re-export.
- Do not opportunistically refactor unrelated modules.

Theory meaning:

- Category reading: minimize new crossing morphisms.
- HoTT reading: construct one path from old behavior to new behavior and verify
  it.

Tools/evidence:

- targeted file reads,
- targeted tests,
- `who_uses_summary` when public symbols move,
- `function_signature` when signatures change.

Exit condition:

- One coherent slice works.
- The next slice is clearer than before.

Common failure:

- Expanding the slice until the agent is back in unbounded feature work.

### B5. Verify boundary impact

Action:

- Check whether the feature widened APIs, introduced cycles, or created new
  coupling.
- Check whether any new public items are actually needed.

Theory meaning:

- Public visibility is projection.
- Adding a public symbol changes the external contract.
- New morphisms across boundaries are future maintenance cost.

Tools/evidence:

- `get_exports(directory=..., module=..., consumer=...)`
- `get_declared_reexports(directory=..., module=...)`
- `who_imports(directory=..., target=...)`
- `who_uses_summary(directory=..., target=...)`
- `dead_pub_report(directory=...)`
- `crate_edges(directory=...)`
- `forbidden_dependency_check(directory=..., rules=[...])` when rules exist.

Exit condition:

- Any new boundary edge is intentional.
- Public API growth is justified.

Common failure:

- Accepting wider visibility because it made implementation easier.

### B6. Verify type/function shape

Action:

- Check complexity, input arity, unsafe, recursion, and invariant placement.
- Split or wrap types only when it removes real coupling or encodes a real
  invariant.

Theory meaning:

- HoTT reading: refine the local type space.
- Functions should transform clear shapes, not compensate for missing shapes.

Tools/evidence:

- `function_signature(directory=..., target=...)`
- `fn_body_audit(directory=...)`
- `recursion_check(directory=...)`
- `unsafe_audit(directory=...)`
- `analyze_complexity(file_path=...)`
- source inspection.

Exit condition:

- The feature is local, tested, and does not make the type/function layer worse.

Common failure:

- Adding wrapper types that do not encode an invariant.

### B7. Repeat by slices

Action:

- Repeat B1-B6 for the next smallest slice.
- If a later slice no longer fits the target, return to B2 and redesign.

Theory meaning:

- The implementation is a path through a sequence of equivalent or intentionally
  refined program states.
- Each slice should commute with the boundary graph: callers still reach the
  behavior they need.

Exit condition:

- The full feature is done through a series of local changes.

Common failure:

- Keeping the original plan after the graph shows the feature belongs elsewhere.

## 5. Workflow C: Mature Project

Goal:

Change the design in a mature project without breaking callers, stored data,
public APIs, tests, downstream crates, or user workflows.

Use this when:

- public APIs exist,
- downstream crates or users may depend on behavior,
- serialized data or migrations matter,
- a hub symbol must change,
- feature work requires a redesign but direct replacement is too risky.

This workflow is stricter than Workflow B. The mature-project default is not
"replace old with new." The default is "introduce new, adapt old, migrate,
remove only when safe."

### C0. Identify compatibility boundaries

Action:

- List all public or semi-public surfaces affected by the change.
- Include public Rust APIs, CLI behavior, config files, serialized data,
  database schema, feature flags, and documented workflows.

Theory meaning:

- Category reading: external morphisms into the project must keep landing
  somewhere valid.
- HoTT reading: old and new shapes need an equivalence, adapter, or deliberate
  refinement.

Tools/evidence:

- `get_exports(directory=..., module=..., consumer=...)`
- `get_declared_reexports(directory=..., module=...)`
- `who_imports(directory=..., target=...)`
- `who_uses_summary(directory=..., target=...)`
- `re_export_chain(directory=..., target=...)`
- docs/config/schema inspection.

Exit condition:

- The agent knows what must not break.

Common failure:

- Treating mature code like internal-only code.

### C1. Measure blast radius

Action:

- Identify hubs, bridge units, and downstream callers before changing anything.
- Treat high fan-in symbols as migration projects.

Theory meaning:

- Category reading: high in-degree means many morphisms land on the object.
- Replacing that object directly breaks many composed paths.

Tools/evidence:

- `who_imports(directory=..., target=...)`
- `who_uses_summary(directory=..., target=...)`
- `recursive_callers_count(directory=..., target=...)`
- `crate_edges(directory=...)`
- `call_graph(directory=..., root=...)`

Exit condition:

- The change is classified as low, medium, or high blast radius.

Common failure:

- Calling a hub rename or signature change "mechanical."

### C2. Design the new shape beside the old shape

Action:

- Add the new type/module/API beside the old one.
- Do not remove the old surface yet.
- Keep old callers working.

Theory meaning:

- Category reading: create a new object without deleting the old target of
  external morphisms.
- HoTT reading: introduce the refined type shape while preserving a path from
  old shape to new shape.

Possible operations:

- Lift module to crate while keeping old facade.
- Split type while keeping old wrapper.
- Extract trait while keeping concrete API.
- Move implementation under old re-export.
- Add adapter between old and new structures.

Tools/evidence:

- `function_signature(directory=..., target=...)` for old and new callables,
- `get_exports(directory=..., module=..., consumer=...)`,
- `get_declared_reexports(directory=..., module=...)`,
- source inspection for invariant preservation.

Exit condition:

- Old API still compiles.
- New API exists and can be used by at least one caller.

Common failure:

- Big-bang replacement.

### C3. Write adapters as witnesses

Action:

- Add conversions, facades, compatibility functions, or re-exports that connect
  old callers to the new implementation.
- Keep adapters small and explicit.

Theory meaning:

- Category reading: adapters make old paths commute through the new graph.
- HoTT reading: adapters witness equivalence or refinement between old and new
  types.

Adapter examples:

- `From<Old> for New`,
- `TryFrom<Old> for New`,
- old function delegates to new function,
- old module re-exports new type,
- old config parses into new config.

Tools/evidence:

- `who_calls(directory=..., target=...)` to ensure old functions delegate as intended,
- `calls_from(directory=..., caller=...)` for adapter internals,
- tests comparing old and new behavior,
- `re_export_chain(directory=..., target=...)` for facade paths.

Exit condition:

- Existing callers work through adapters.
- New callers can use the new shape directly.

Common failure:

- Adapter becomes a second implementation instead of a delegation path.

### C4. Migrate callers incrementally

Action:

- Move callers from old API to new API in small batches.
- Prefer leaf callers first.
- Save hubs and public facades for later.

Theory meaning:

- Category reading: gradually redirect morphisms from old object to new object.
- HoTT reading: each migrated caller follows a path from old shape to equivalent
  new shape.

Tools/evidence:

- `who_uses_summary(directory=..., target=...)` before and after each batch,
- `who_imports(directory=..., target=...)` before and after each batch,
- `recursive_callers_count(directory=..., target=...)` for migration order,
- tests after each batch.

Exit condition:

- The old API has fewer callers after each batch.
- No batch requires unrelated redesign.

Common failure:

- Migrating all callers at once and losing the ability to localize failures.

### C5. Deprecate or narrow the old surface

Action:

- Once most callers are migrated, mark old API as deprecated if public.
- If internal, narrow visibility.
- Remove re-exports only when no caller depends on the facade.

Theory meaning:

- Category reading: reduce the projection of the old object.
- HoTT reading: old shape is no longer the canonical contract.

Tools/evidence:

- `who_imports(directory=..., target=...)`
- `who_uses(directory=..., target=...)`
- `dead_pub_in_crate(directory=..., krate=...)`
- `dead_pub_report(directory=...)`
- `get_declared_reexports(directory=..., module=...)`

Exit condition:

- Old surface has no unsupported callers.
- Any remaining old surface is intentionally compatible API.

Common failure:

- Removing public aliases before checking actual import paths.

### C6. Remove the old implementation only when it is dead

Action:

- Delete old implementation after callers are gone or after the compatibility
  window has ended.
- Keep compatibility tests if old external behavior must still be preserved.

Theory meaning:

- Category reading: remove an object only when no required morphisms target it.
- HoTT reading: remove old shape only after the migration path no longer needs
  it as a witness.

Tools/evidence:

- `who_uses(directory=..., target=...)`
- `who_imports(directory=..., target=...)`
- `find_references(directory=..., symbol_name=...)`
- `dead_pub_report(directory=...)`
- test suite or compatibility tests.

Exit condition:

- The old implementation is unreachable or intentionally replaced.

Common failure:

- Trusting one tool only. Deletion should combine graph tools with textual or RA
  references.

### C7. Re-check architectural rules

Action:

- After migration, check that the new graph is not worse.
- Look for new cycles, new broad public surfaces, and accidental dependency
  inversions.

Theory meaning:

- Category reading: migration is a functor-like mapping from old graph to new
  graph. The new graph must preserve or improve the intended ordering.
- HoTT reading: the new type shapes should be the canonical contracts, not just
  extra wrappers around the old shapes.

Tools/evidence:

- `crate_edges(directory=...)`
- `forbidden_dependency_check(directory=..., rules=[...])`
- `get_exports(directory=..., module=..., consumer=...)`
- `dead_pub_report(directory=...)`
- `semantic_overlaps(directory=...)` for duplicate old/new logic.

Exit condition:

- New architecture is simpler or more explicit than the old one.
- Compatibility scaffolding is either removed or deliberately retained.

Common failure:

- Ending with both old and new systems permanently active.

## 6. Which Workflow Should The Agent Use?

Use this decision table.

| Situation | Workflow |
|---|---|
| Fast prototype, bad layout, unclear boundaries | Workflow A |
| Feature request too big but project has useful modules | Workflow B |
| Public API or downstream compatibility matters | Workflow C |
| One directory has 80 files and many one-file dirs exist | Workflow A |
| Agent cannot keep the feature plan in context | Workflow B |
| A hub type/function must change | Workflow C |
| Need to enforce guidelines after rough generation | Workflow A, step A9 |
| Need to add a feature to a coherent crate | Workflow B |
| Need to replace a public type | Workflow C |

## 7. Required Agent Output

For Workflow A:

```text
Workflow: Starting project
Current structural problem:
Evidence:
Proposed shallow layout:
Operations:
Theory meaning:
Verification:
```

For Workflow B:

```text
Workflow: Semi-mature feature
Feature behavior:
Smallest target:
If no target, redesign needed:
Primitive operation:
Theory meaning:
Verification:
```

For Workflow C:

```text
Workflow: Mature migration
Compatibility surfaces:
Blast radius:
New shape:
Adapter/witness:
Migration batches:
Deprecation/removal condition:
Verification:
```

The agent should always name the primitive operation and explain the theory in
plain operational terms:

- Category theory: what happens to objects, morphisms, boundaries, quotients,
  projections, or lifts.
- HoTT/type theory: what happens to types, functions, contracts, equivalences,
  paths, products, sums, or refinements.

## 8. Short Version

Starting project:

1. Build fast to discover behavior.
2. Freeze behavior enough to refactor.
3. Map the file/module graph.
4. Find dense groups.
5. Name each group.
6. Propose shallow `src/dir/file.rs` modules.
7. Move/split/merge into that layout.
8. Lift modules to crates only when boundaries are stable.
9. Enforce guidelines after boundaries stabilize.

Semi-mature project:

1. State the feature as behavior.
2. Find the smallest high-leverage target.
3. If no target exists, redesign first.
4. Choose Move, Split/Merge, Lift/Lower, or a short composition.
5. Implement the smallest slice.
6. Verify boundary impact.
7. Verify type/function shape.
8. Repeat by slices.

Mature project:

1. Identify compatibility boundaries.
2. Measure blast radius.
3. Add the new shape beside the old shape.
4. Write adapters as witnesses.
5. Migrate callers incrementally.
6. Deprecate or narrow the old surface.
7. Remove old implementation only when dead.
8. Re-check architecture.

The whole method:

- Category theory decides where code lives.
- HoTT/type theory decides what shape code has.
- Move, Split/Merge, and Lift/Lower are the only primitive operations.
- Project maturity decides how aggressively the operations can be applied.
