# Phase 1 Rewrite: Semantic Graph Environment + Strict Architectural Enforcement

Replaces the old Phase 1 in `long-plan.md`. Folds in the "codebase as
inhabitable semantic graph" framing and the "complexity belongs in
architecture, never in function bodies" axiom.

## Axioms (ground truth — not knobs to tune)

1. **Complexity belongs in architecture, not function bodies.** Functions
   are simple linear transformations between well-designed types.
   Branching becomes enum dispatch. Nesting becomes early returns + better
   types. Many parameters become a struct. Loops become iterator chains.
   *Use space instead of time.*

2. **The agent never bypasses guidelines.** Every violation is a
   missing-abstraction signal. The agent restructures, never suppresses.
   There is no `#[allow(complexity)]` agent-side override.

3. **The codebase is a semantic graph, not a directory tree.** The agent
   inhabits a navigable hypergraph. It never edits files directly. CRUD
   operations are semantic and auto-propagate (renames, signature changes,
   moves, deletes all update affected sites without agent intervention).

4. **Bodies hidden by default.** Signatures, types, and auto-generated
   descriptions are the working surface. `show_body` is an explicit move
   with a small reward cost.

5. **Boundaries are the only exception.** A small workspace-level allowlist
   marks items where guideline enforcement is suspended (FFI, proc-macro
   output, external trait impls). Humans curate this list. Agents read it
   but never modify it.

## State (what the agent perceives)

The agent has a **current location** in the hypergraph and a
**task-conditioned view** assembled by the engine:

```
Location: rmc_indexing::dir_hash
Kind: Function
Signature: pub(crate) fn dir_hash(dir: &Path) -> String
Description: computes a stable hash of a directory path for caching
Body: hidden (8 lines, no violations)
Callers (3):
  - rmc_indexing::project_paths::from_directory
  - rmc_server::project_paths::dir_hash      [DUPLICATE — sim 0.94]
  - rmc_server::project_paths::from_directory
Calls (2): sha2::Sha256::new, std::path::Path::to_string_lossy
Cluster: 2 items in "path hashing" (sim 0.94)
Hyperedges: re_export_chain (3 hops), trait_impl: none
Audit status: clean
Pending refusal: none
Available actions: goto | show_body | show_callers | show_cluster
                 | follow_trail | propose_dedupe | ...
```

The view filters surrounding noise based on the declared `task`. Same
location yields different views under different tasks.

## Action vocabulary (4 verb categories, ~25 typed operations)

The "intent enum" from the old Phase 1 is gone. The agent composes
primitives directly. The engine verifies and propagates.

### Navigate (5 ops)
- `goto(target)` — move to an item by qualified name
- `show_body(target)` — reveal body (small reward cost)
- `show_callers(target)` — pivot to callers list
- `show_cluster(target)` — view cluster members + similarity scores
- `follow_trail(hyperedge)` — walk a trait, reexport chain, co-change group

### Query (6 ops)
- `find_similar(target, substrate)` — DBSCAN/HDBSCAN over embeddings or
  graph-isomorphism over hypergraph
- `find_violations(scope)` — items with audit findings
- `find_dead(scope)` — dead pubs, unused items
- `find_cycles(scope)` — Tarjan SCC + bridges
- `simulate(operation)` — counterfactual; shows cascade without applying
- `search_by_description(text)` — natural-language retrieval over
  auto-descriptions

### Structural CRUD — items (5 ops)
- `add_item(kind, location, signature, body)` — body bounded by guideline
  enforcement
- `modify_signature(target, new_sig)` — auto-propagates to all call sites
  or returns structured refusal
- `modify_body(target, new_body)` — must pass audits inline
- `delete(target)` — refuses if used; engine may propose `inline`
- `move(target, dest)` — refuses on cycle; updates all imports

### Structural CRUD — refactor primitives (5 ops)
- `extract_function(body_span, new_name)`
- `extract_trait(items)` — requires compatible signatures
- `inline(target)` — reverse of extract
- `split_module(module, partition)` — creates child modules
- `merge_modules(modules)` — quotients into one

### Structural CRUD — modules/crates (4 ops)
- `create_module(parent, name)`
- `move_module(src, dst)` — updates parent mods + all `use` paths
- `lift_to_crate(module)` — creates crate, moves files, updates Cargo.toml,
  updates dependents
- `lower_to_module(crate)` — inverse

### Meta (4 ops)
- `annotate(target, note)` — pin agent context to an item; persists
- `commit()` — atomic apply; runs compile + tests + audits
- `rollback()` — atomic undo via jj
- `declare_done()` — end episode

## Constraints (engine-enforced)

### Hard refusals (physical barriers — agent must restructure)

These are not reward penalties. The move does not apply.

- Cyclomatic complexity > threshold per function
- Parameter count > threshold
- Body LOC > threshold
- Nesting depth > threshold
- `unwrap()` / `panic!()` / `unreachable!()` in non-test code
- `unsafe` block outside boundary allowlist
- Cycle introduction (P3)
- Visibility widening (P13)
- Trait coherence violation (P15)
- Compile break
- Test failure on touched module

Refusal returns a structured reason the agent can act on:

```json
{
  "refused": true,
  "principle": "P3",
  "finding": "cycle introduced between rmc_engine and rmc_indexing",
  "suggested_actions": ["extract_trait", "move_target"]
}
```

### Soft penalties (gradient signals — move applies, reward reduced)

These shape behavior without blocking. The agent learns to avoid them but
can act through them when justified by downstream gain.

- Boundary cost regression (P1)
- Density gradient regression (P2)
- Bridge weakening (P8)
- Dead-pub growth (P13)
- Instability / abstractness regression (Robert Martin)
- Surface stability cost (P12)
- Hub blast-radius increase (P10)
- Per-step length penalty (small)

### Boundary allowlist

Workspace-level TOML, ~5-20 entries in a large workspace, curated by
humans (or a privileged higher-tier model — never the trained agent):

```toml
[[boundary]]
target = "ffi::cuda_init"
reason = "FFI to CUDA runtime — C signature fixed"

[[boundary]]
target = "build_script::generate_bindings"
reason = "build-time codegen"

[[boundary]]
target = "serde::ser::Serialize::serialize on EnumX"
reason = "manual serialization — derive not viable"
```

Within boundaries, audits suspended. Outside, strict. The agent reads the
list but cannot extend it.

## Infrastructure (four indices, not three)

| Index | Substrate | Built/maintained by |
|---|---|---|
| Hypergraph | LMDB | incremental extractor (Phase 0) |
| Embeddings | LanceDB | content-hash cache |
| BM25 | Tantivy | existing |
| **Descriptions** | new index | small sub-agent, merkle-keyed regen |

### Description layer (new)

- One-line description per item, generated by a small sub-model
  (Haiku-class)
- Merkle tree detects which items changed; only those regenerate
- Queryable via `search_by_description("normalizing paths")` — hits even
  when the code says `dir_hash`
- Descriptions are themselves a training signal:
  - **Stability**: did the description survive the next merkle update?
  - **Utility**: did description-search lead to a correct refactor?

### Counterfactual simulator

`simulate(operation)` runs the engine's effect computation without
applying. Returns predicted audit deltas, cascade scope, would-refuse
status. Lets the agent plan multi-step trajectories before committing.

### Atomic rollback (jj-backed)

- Every `commit` creates a jj operation
- `rollback()` is `jj op restore` wrapped as sub-second call
- Failed auto-propagation triggers automatic rollback before reward is
  computed — the agent never sees a partial state

### Engine audit log

Every operation, every refusal, every reason is logged and queryable.
Becomes the basis for "how did this code get into this state?" and
provides training data: every refusal is a learning datum.

## Episode shape

```
loop:
  observation = engine.view(agent.location, task)
  action = agent.choose(observation)

  if action ∈ navigate / query:
    reward = small_observation_cost
    update view, location

  elif action ∈ structural CRUD:
    if engine.refuses(action):
      reward = small_negative
      observation += structured_refusal_reason

    else:
      engine.apply(action)
      run compile + tests + audits
      reward = shaped(audit_deltas, principle_violations)

  elif action = commit:
    reward = full_reward_vector

  elif action = declare_done:
    reward = episode_end_summary
    break

  if step_budget exhausted:
    reward = episode_end_summary
    break
```

Many-step trajectories. Per-action reward plus episode-end summary.
Multi-step credit assignment is natural — every action gets feedback,
episode outcome shapes long-term strategy.

## Sizing (~3-4 months for the full Phase 1)

| Component | Weeks |
|---|---|
| Description sub-system | 3 |
| Navigation + state representation | 3 |
| Query primitives (6 typed analyses) | 4 |
| Structural CRUD with auto-propagation | 6 |
| Guideline enforcement + boundary allowlist | 2 |
| Counterfactual simulator | 2 |
| Episode runner + reward computation | 2 |

Structural CRUD is the largest piece. Every operation needs a verified,
principle-respecting execution path, including auto-propagation under
multiple call-site patterns.

## Open decisions before kickoff

1. **Description sub-model.** Separate Haiku-class model generating
   descriptions, or main agent? **Lean: separate.** Descriptions generate
   continuously; main-agent compute too valuable to spend on them.

2. **Auto-propagation when engine can't auto-fix.** E.g.
   `modify_signature` that adds a required parameter — engine cannot
   invent the new arg at call sites. Three options:
   - (a) refuse outright
   - (b) insert `todo!()` at call sites with required-follow-up flag
   - (c) require a `callsite_strategy` parameter (e.g. "for callers in
     crate X, pass `Ctx::default()`")

   **Lean: (c) for nontrivial cases, (a) as default for simple
   modify_signature.** This is the trickiest practical problem; shapes how
   clean the abstraction stays.

3. **Multi-window navigation.** Single current location vs multiple open
   windows. **Lean: start single, add multi only if needed.**

4. **Soft-penalty reward weights.** Grid-searched per phase, or learned
   jointly with the policy. **Lean: fix per phase** — joint learning risks
   coupled instability.

5. **Step budget per episode.** 50? 100? **Lean: 50-100, empirically tuned
   after first SFT runs.**

## What this replaces from old Phase 1

- 4-tool API (`observe` / `propose` / `implement` / `commit`) →
  4 verb categories with ~25 typed operations, no top-level wrapping tool
- `intent` enum (~10 high-level labels, engine plans the decomposition) →
  direct primitive composition (agent composes, engine verifies)
- Loose hard-vs-soft split → strict categorization: physical barriers vs
  gradient signals, nothing in between
- `implement` as separate body-writing tool → bodies are bounded
  parameters to `add_item` / `modify_body`, no special tool
- New: description layer as fourth index and training signal
- New: counterfactual `simulate` for plan-before-commit
- New: boundary allowlist for the only legitimate exceptions
- New: navigation as first-class action with persistent location state

## Why this is RL-tractable

Recapping the structural advantages, now sharper:

- **Action space is discrete and small** (~25 ops with typed parameters)
- **Observations are structured** (location + signature + description +
  edges + cluster + audit + available actions)
- **Reward is dense and multi-channel** (per-action + commit-time +
  episode-end, across compile / tests / audits / principle violations)
- **State is reproducible** (deterministic snapshots + jj rollback +
  workspace fingerprinting)
- **No degenerate strategies** (no bypass mechanism; agent cannot learn
  "suppress when stuck")
- **Forcing function is permanent** (every refusal pushes toward
  architectural decomposition, the actual skill we want)
- **Trajectories have natural structure** (navigate → query → propose →
  commit, repeating until audits clear)

This is genuinely a game, with discrete moves, a graded score, a
navigable world, and learnable skills. The "gamify code development"
framing is not metaphor — it is the design's literal shape.
