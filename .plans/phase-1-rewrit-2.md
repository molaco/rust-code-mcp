# Phase 1 Rewrite (v2)

Supersedes the Phase 1 section of `long-plan.md`. This version is built on a
firmer design philosophy: complexity lives in architecture, not function
bodies; the agent never bypasses guidelines; the codebase is a navigable
semantic graph the agent inhabits rather than a tree of text files.

## Design Philosophy

The whole point is to leverage infrastructure and architecture — use **space
instead of time**, and keep **functions simple**.

No one should do a divide-and-conquer strategy inside a function. They should
do it in types, files, and memory. A function that violates guidelines is a
signal the architecture is wrong, not that the guidelines are too strict.

Concretely, every internal complexity violation reduces to a missing
abstraction:

| Symptom                | Missing abstraction                          |
| ---------------------- | -------------------------------------------- |
| Cyclomatic > threshold | enum + match dispatch, or strategy type      |
| Many params            | a struct                                     |
| Long body              | several functions; logic belongs in types    |
| Deep nesting           | early returns + better types                 |
| Branching logic        | trait dispatch or table lookup               |

"Use space instead of time" restated: pay memory/structure to buy simpler code
paths. Lookup tables over branches. Newtypes to make impossible states
unrepresentable. The type system as a compile-time proof, not the function
body as a runtime check.

## Axioms (ground truth, not knobs)

1. **Complexity belongs in architecture, not function bodies.** Functions are
   simple linear transformations between well-designed types.
2. **The agent never bypasses guidelines.** Every violation is a
   missing-abstraction signal. The agent restructures, never suppresses.
   There is no agent-facing override.
3. **The codebase is a semantic graph.** The agent inhabits a navigable
   hypergraph. It never edits files directly. CRUD operations are semantic and
   auto-propagate.
4. **Bodies are hidden by default.** Signatures, types, and descriptions are
   the working surface. `show_body` is an explicit, costed move.
5. **Boundaries are the only exception.** A workspace-level allowlist (FFI,
   proc-macro output, external trait impls) suspends audits for a tiny set of
   curated items. Humans curate it. Agents cannot modify it.

This is consistent with THEORY.md §1, which explicitly places local style out
of scope — the entire theory operates at the structural level. The
"complexity in architecture" axiom is the design's true center.

## State (what the agent perceives)

The agent has a **current location** in the hypergraph and a
**task-conditioned view**:

```
Location: rmc_indexing::dir_hash
Kind: Function
Signature: pub(crate) fn dir_hash(dir: &Path) -> String
Description: computes a stable hash of a directory path for caching
Body: hidden (8 lines, no violations)
Callers (3):
  - rmc_indexing::project_paths::from_directory
  - rmc_server::project_paths::dir_hash      [DUPLICATE sim 0.94]
  - rmc_server::project_paths::from_directory
Calls (2): sha2::Sha256::new, std::path::Path::to_string_lossy
Cluster: 2 items in "path hashing"
Hyperedges: re_export_chain (3 hops)
Audit status: clean
Available: goto | show_body | show_callers | show_cluster | propose_dedupe | ...
```

The view is the observation. Navigation changes location and therefore changes
the view. This is the game-state.

## Action Vocabulary

Four verb categories, ~25 typed operations. The "intent enum" from the old
design is gone — the agent composes primitives directly, the engine verifies.

**Navigate**
- `goto(target)` — move current location
- `show_body(target)` — reveal hidden body (costed)
- `show_callers(target)` / `show_cluster(target)` — expand view
- `follow_trail(hyperedge)` — walk a hyperedge (trait impls, re-export chain,
  cluster) as if it were a corridor

**Query**
- `find_similar(target, substrate)` — substrate ∈ {embedding, structure, both}
- `find_violations(scope)` — guideline violations
- `find_dead(scope)` — dead pub / unreferenced items
- `find_cycles(scope)` — SCCs and bridges
- `search_by_description(text)` — retrieve by generated descriptions
- `simulate(operation)` — counterfactual: predicted deltas + cascade +
  would-refuse, without applying

**Structural CRUD — items**
- `add_item(kind, location, definition)`
- `modify_signature(target, new_sig, callsite_strategy?)` — auto-updates
  callers or refuses
- `modify_body(target, new_body)` — graded inline by guideline enforcement
- `delete(target)` — checks refs; refuses or proposes cascade
- `move(target, dest)` — updates imports; refuses on cycle

**Structural CRUD — refactor primitives**
- `extract_function(body_span)` — creates fn, replaces span with call
- `extract_trait(items)` — requires compatible signatures (callsite-usage set)
- `inline(target)` — reverse of extract
- `split_module(module, partition)` — creates child modules
- `merge_modules(items)`

**Structural CRUD — modules/crates**
- `create_module`, `move_module`
- `lift_to_crate(module)` / `lower_to_module(crate)`

**Meta**
- `annotate(target, note)` — agent-curated persistent note
- `commit()` — atomic apply, full reward vector
- `rollback()` — return to prior snapshot
- `declare_done()` — episode end

Every operation has typed I/O, principle-aware preconditions, and structured
refusal reasons. This is still far smaller than the current 45-tool rmc.

## Constraints (engine-enforced)

### Hard refusals — physical barriers, not penalties

The agent cannot perform these. It must restructure. There is no override
(except the boundary allowlist, below).

- Cyclomatic > threshold
- Param count > threshold
- Body LOC > threshold
- Nesting depth > threshold
- `unwrap()` / `panic!()` / `unreachable!()` outside tests
- `unsafe` outside boundary allowlist
- Cycle introduction (P3)
- Visibility widening (P13)
- Trait coherence violation (P15)
- Compile break
- Test failure on touched module

Returns a structured reason the agent can learn from:

```json
{
  "refused": true,
  "principle": "P3",
  "finding": "move would create rmc_server -> rmc_indexing -> rmc_server cycle",
  "suggested_actions": ["extract_shared_module", "move_target_instead"]
}
```

The agent gets gradient on *why*, not just *that*.

### Soft penalties — gradient signals, move applies

- Boundary cost regression (P1)
- Density gradient regression (P2)
- Bridge weakening (P8)
- Dead-pub growth (P13)
- Instability / abstractness regression (Robert Martin)
- Surface stability cost (P12)
- Hub blast-radius increase (P10)
- Per-step length penalty

### Boundary allowlist — the only exception

Workspace-level TOML, ~5-20 items in a large workspace, human-curated. The
agent reads it (sees flagged items) but cannot modify it.

```toml
[[boundary]]
target = "ffi::cuda_init"
reason = "FFI to CUDA runtime; C signature is fixed"

[[boundary]]
target = "codec::manual_deserialize"
reason = "hand-rolled deserialization; not derivable"
```

Legitimate boundary cases: FFI declarations, proc-macro / build.rs output,
external-trait impls with fixed signatures, hand-rolled deserialization.
Within a boundary, anything goes; outside, strict.

## Infrastructure — four indices, not three

| Index        | Substrate | Built / maintained by                  |
| ------------ | --------- | -------------------------------------- |
| Hypergraph   | LMDB      | incremental extractor (Phase 0)        |
| Embeddings   | LanceDB   | content-hash cache                     |
| BM25         | Tantivy   | existing                               |
| Descriptions | new       | small sub-agent, merkle-keyed regen    |

### Description layer (new)

One-line description per item, generated by a Haiku-class sub-model when an
item changes. The merkle tree detects what changed; only those items
regenerate. Queryable via `search_by_description`.

Why it matters — it is a fourth retrieval axis:

- Hypergraph: "what calls what"
- Embeddings: "what is vector-similar"
- BM25: "what mentions 'parse'"
- Descriptions: "what is *described as* doing X" — hits even when the code
  says `dir_hash` but the intent is "normalize a path"

Description quality is itself a training signal: graded on stability (did it
survive the next merkle update?) and utility (did description-search lead to a
correct refactor?).

### Counterfactual simulator (new)

`simulate(operation)` runs the engine's effect-computation without applying.
Returns predicted audit deltas, the propagation cascade, and would-refuse
status. Lets the agent plan without committing — cheap lookahead.

### Atomic rollback

Every `commit` creates a jj operation. `rollback()` wraps `jj op restore` as a
sub-second call. The agent never observes partial state; a propagation that
breaks compilation fully reverts.

### Engine audit log

Every operation, refusal, and reason is logged and queryable. Two uses: "how
did this code reach this state?" and — crucially — every refusal becomes
training data for what not to propose.

## Episode Shape

```
loop:
  obs = engine.view(agent.location, task)
  action = agent.choose(obs)

  if action in {navigate, query}:
      reward = small_observation_cost          # budget pressure

  if action in {structural CRUD}:
      if engine.refuses(action):
          reward = small_negative + structured_reason
      else:
          apply(action)
          reward = shaped(compile, tests, audit_deltas, soft_penalties)

  if action == commit:
      reward = full_reward_vector

  if action == declare_done:
      reward = episode_end_summary
      break

  if step_budget_exhausted:
      break
```

Many-step trajectories. Per-action reward plus episode-end summary. Multi-step
credit assignment is natural — every operation has its own signal and
contributes to the outcome.

Because guideline enforcement is a hard barrier, most episodes naturally
become "agent attempts a body, gets blocked, must restructure first." The
dominant trajectory shape is architecture work, not body-writing — which is
exactly the skill we want to train.

## Reward Vector (from commit)

```json
{
  "compile": true,
  "tests_passed": 0.98,
  "audit_delta": {
    "cycles": 0,
    "dead_pub": -3,
    "instability_change": -0.04,
    "complexity_change": -12,
    "unsafe_change": 0
  },
  "principle_violations": [
    { "principle": "P12", "severity": "soft", "weight": 0.1 }
  ],
  "episode_length": 7
}
```

The scalarization function is a tunable design parameter, fixed per training
phase.

## Sizing (~3-4 months)

| Component                                   | Weeks |
| ------------------------------------------- | ----- |
| Description sub-system                       | 3     |
| Navigation + state representation           | 3     |
| Query primitives (6 typed analyses)         | 4     |
| Structural CRUD with auto-propagation       | 6     |
| Guideline enforcement + boundary allowlist  | 2     |
| Counterfactual simulator                    | 2     |
| Episode runner + reward computation         | 2     |

## Open Decisions Before Kickoff

1. **Description sub-model**: separate Haiku-class model or main agent?
   Recommend separate — descriptions generate continuously; main-agent compute
   is too valuable to spend on them.

2. **Auto-propagation when the engine can't auto-fix** (e.g. `modify_signature`
   adds a param the engine can't synthesize at call sites):
   - (a) refuse outright
   - (b) insert `todo!()` with a required-follow-up flag
   - (c) require a `callsite_strategy` parameter (e.g. "callers in crate::foo
     pass `Ctx::default()`")
   Recommend (c) for nontrivial cases, (a) as the default. This is the single
   biggest "how complete is the abstraction" decision.

3. **Multi-window navigation**: start single-location; add multi-window only if
   comparing two clusters side-by-side proves necessary.

4. **Soft-penalty reward weights**: fix per phase; do not learn jointly with
   the policy (avoids coupled instability).

5. **Step budget per episode**: 50-100, empirically tuned.

## What This Replaces From the Old Phase 1

| Old                                          | New                                                   |
| -------------------------------------------- | ----------------------------------------------------- |
| 4-tool API (observe/propose/implement/commit)| 4 verb categories, ~25 typed ops                      |
| `intent` enum (engine plans)                 | direct primitive composition (agent plans, engine verifies) |
| loose hard-vs-soft split                     | strict: physical barriers vs gradient signals         |
| `implement` as a separate tool              | bodies are bounded params to add/modify ops           |
| three indices                                | four indices (+ descriptions)                         |
| —                                            | counterfactual simulator                              |
| —                                            | boundary allowlist                                    |
| —                                            | navigation as first-class action with location state  |

## Why This Is Genuinely a Game

The system has discrete moves, a graded score, a navigable world, and
learnable skills. "Gamify code development" is not a metaphor here — the agent
literally traverses a world (the hypergraph), makes legal moves (typed
operations the engine validates), and earns a score (audit deltas + gates).
The forcing function of hard guideline enforcement means the only winning
strategy is to push complexity out of bodies and into architecture — which is
the entire thesis.
