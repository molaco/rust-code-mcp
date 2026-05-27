# Long-term Plan: rmc as RL Substrate for Rust-Coding Agents

## Vision

Build a framework that lets agents write better Rust code by lifting the action
space from text edits to high-level structural intents. The rmc tools are not
the deliverable — they are the **environment** and **reward signal** for RL
post-training of a small open model (target: future Qwen3.7 20B or equivalent)
to exceed frontier models on Rust coding tasks.

Game-development framing: the workspace is the game state, the legal moves are
restricted to a small set of typed intents, the score is multi-channel audit
deltas plus compile/test gates. The agent plays the refactor game; the engine
handles execution.

## Shape

Three layers, built in order. Layer 1 is ~70% present in current rmc; layers
2 and 3 do not exist yet.

```
Layer 1  Environment       deterministic snapshots, 4-tool API, fast reset
Layer 2  Data engine       perturbation + synthetic refactor pipeline
Layer 3  Training loop     SFT bootstrap, RL, eval, flywheel
```

Each layer depends on the previous one. The biggest risk lives in Layer 1
(latency). The biggest open design question lives in the API shape between
Layers 1 and 2.

---

## Phase 0 — Foundation

**What exists today**:

- HIR-driven hypergraph extractor (`build_hypergraph`)
- LMDB snapshot with workspace fingerprinting
- 30+ query and audit tools
- BM25 + vector hybrid search (Tantivy + LanceDB)
- Multiple embedding backends (Qwen3/Candle, BGE/ONNX, OpenRouter)
- jj-aware repository conventions (AGENTS.md)
- Theory documents (THEORY.md, THEORY_2.md, THEORY_3.md) defining principles
  P1-P15 and the three primitive operations (Move, Split/Merge, Lift/Lower)

**Gaps that must close before Phase 1 is meaningful**:

1. **Incremental hypergraph rebuild.** Current cold rebuild is 5-18s. RL needs
   millions of rollouts; this must drop to sub-second on small edits. Likely
   means per-crate incremental extraction plus diffing LMDB tables instead of
   full rewrite. This is the project's highest-risk engineering item.

2. **Deterministic snapshot keying.** Snapshots must be bit-identical for
   identical workspace states. Fingerprint exists; needs to be guaranteed and
   covered by tests for RL reproducibility.

3. **Rollback primitive.** Wrap `jj op restore` as a single sub-second call
   that returns the workspace to a named prior snapshot. This is the env's
   reset button.

4. **Benchmark workspace pool.** Curate ~50-100 real Rust crates of varying
   size and quality, with known structural problems. Mix of hand-selected
   crates and scraped from crates.io. This becomes both training input and
   eval substrate.

Sizing: ~2-3 months, one person.

---

## Phase 1 — The 4-Tool API

The agent's entire surface area:

```
observe(task, analysis?)    -> ContextView
propose(intent, evidence)   -> Preview | StructuredRefusal
implement(signature, plan)  -> Body                (graded inline by fn_body_audit)
commit()                    -> RewardVector        (compile + tests + audit deltas)
```

### Inside `observe`

A small typed vocabulary of analysis primitives, not 30+ tools. Target 6-8
primitives:

| Primitive       | Purpose                                              |
| --------------- | ---------------------------------------------------- |
| `cluster`       | DBSCAN/HDBSCAN over embeddings (item/fn/module/crate) |
| `community`     | Louvain/Leiden over call/import graph                |
| `centrality`    | PageRank / betweenness for hub detection             |
| `cycles`        | Tarjan SCC + bridges (P3, P8)                        |
| `anomaly`       | Statistical outliers on per-item features            |
| `similar`       | Semantic neighbors of a target item                  |
| `co_change`     | Git-history-based co-change (P11 — current gap)      |
| `distribution`  | Compare-to-workspace-median on any metric            |

The existing 30+ audits become **internal** composers of the standard
`ContextView` payload, or are accessible as parameterizations of these
primitives. The agent never directly calls `who_calls`, `dead_pub_report`,
etc.

### Inside `propose`

The principles (P1-P15) become structured constraints, not prose:

- **Hard fail** (engine refuses the move):
  - Cycle introduction (P3)
  - Visibility widening (P13)
  - Trait coherence violation (P15)
  - Compile break

- **Soft penalty** (move applies, reward is reduced):
  - Boundary-cost regression (P1, P2)
  - Bridge weakening (P8)
  - Dead-pub growth (P13)
  - Instability/abstractness regression (Robert Martin)
  - Surface stability cost (P12)

`intent` is a small typed enum, target ~10 values:

| Intent                          | Decomposes to                              |
| ------------------------------- | ------------------------------------------ |
| `extract_shared_module`         | Split + Move + Merge (cycle-aware)         |
| `lift_module_to_crate`          | Lift + Move (visibility-aware)             |
| `lower_crate_to_module`         | Lower + Merge                              |
| `split_module`                  | Split                                      |
| `merge_modules`                 | Merge                                      |
| `extract_trait_from_callsites`  | Lift (signature rung — THEORY.md §3 op)    |
| `move_item`                     | Move                                       |
| `narrow_visibility`             | Project (visibility tightening)            |
| `dedupe`                        | Merge + Move                               |
| `extract_function_body`         | Split (function-level)                     |

The engine owns the decomposition. Every `intent` value has a verified,
audit-aware execution path that respects the principle constraints.

### Inside `implement`

The only place text is written. Structured I/O:

- Input: function signature (set up by a prior `propose`), plan outline
- Output: function body
- Inline grading: `fn_body_audit` runs on output; bodies failing audit
  (unwrap, panic, lock-across-await, deep nesting, etc.) are returned to the
  agent with the violation, not committed.

No free-form file editing exists. The agent cannot author text outside this
tool.

### Inside `commit`

Atomic apply. Returns a multi-channel reward vector:

```json
{
  "compile": true,
  "tests_passed": 0.98,
  "audit_delta": {
    "cycles": 0,
    "dead_pub": -3,
    "instability_change": -0.04,
    "complexity_change": -12,
    "unsafe_change": 0,
    ...
  },
  "principle_violations": [
    { "principle": "P12", "severity": "soft", "weight": 0.1 }
  ],
  "episode_length": 7
}
```

The reward function that combines these into a scalar is itself a tunable
design parameter. Expect to revisit it across phases.

Sizing: ~3 months. The hardest piece is `propose`'s intent-decomposer —
every intent value needs a verified, principle-respecting execution path.

---

## Phase 2 — Synthetic Data and SFT Bootstrap

### Perturbation engine

Inverse of the audits. Take a clean repo, apply a known structural disorder.
Each perturbation has an obvious golden refactor (its inverse).

| Perturbation             | Inverse intent                |
| ------------------------ | ----------------------------- |
| inject cross-crate cycle | `dedupe` or `move_item`       |
| clone module across crate | `extract_shared_module`       |
| promote `pub(crate)` to `pub` | `narrow_visibility`       |
| scatter `unwrap()` calls | `extract_function_body` w/ audit |
| inline 5 modules into one | `split_module`               |
| shadow name across crates | `move_item` or rename         |
| widen function signature | `extract_function_body`       |

Each perturbation is **invertible by construction**, giving a clean
supervised signal: `(perturbed_state, golden_trace, target_audit_deltas)`.

Generate ~1-5M synthetic episodes. Each trace is a sequence of 4-tool calls
in the API format.

### SFT bootstrap

Fine-tune the strongest available open coder model (current best: Qwen3-Coder
or equivalent; design to swap when 3.7-20B lands) on synthetic traces.

Target: ~30-50% perturbation-solve rate before any RL.

### Eval benchmark — built in parallel

- ~500-1000 held-out perturbations across difficulty tiers
- ~50 hand-curated real refactor tasks
- The rmc-refactor repo itself as ultimate dogfood case
- Difficulty levels: single-intent, multi-intent, cross-cutting

This benchmark does not exist for Rust today and is itself a deliverable.

Sizing: ~2-3 months. Benchmark curation is the slowest part and the most
important for end-to-end credibility.

---

## Phase 3 — RL

GRPO or PPO from the SFT'd checkpoint. Reward shaped from the `commit` vector.

### Reward shaping (initial proposal, will iterate)

- Compile fail or test fail: 0 or large negative (hard gate)
- Audit delta improvement: positive, weighted per audit category
- Principle violation: soft negative proportional to weight
- Episode length penalty: small, discourages thrashing

### Two-step credit assignment

The agent's choice of `analysis` in `observe` is part of the trajectory.
Explicitly reward the meta-skill of choosing the right diagnostic for the
state — this is exactly what frontier coding agents are worst at, and where
specialization can produce real differentiation.

### Bootstrapping options

- Teacher distillation: frontier model emits structured trajectories on
  perturbed envs, student SFTs on these before RL. Probably necessary.
- Self-play later: model A perturbs, model B repairs, swap roles. Useful
  for hard-distribution generation once base policy is competent.

Sizing: ~3 months including ablations. Most likely phase to need iteration.

---

## Phase 4 — Flywheel

- Stronger policy generates harder synthetic perturbations
- Stronger policy generates better golden traces (becomes its own teacher)
- Real refactor PRs from dogfooded repos feed the eval set continuously
- Benchmark grows with each iteration

The endgame is a small specialized model that exceeds frontier models on
Rust structural refactoring, then generalizes outward.

---

## Open Design Decisions

To pin before kicking off Phase 1:

1. **Bootstrap base model.** Qwen3-Coder-32B today vs Kimi-K2.5 path vs
   waiting for Qwen3.7-20B. Affects SFT compute estimate by ~3x.

2. **`intent` enum scope.** ~10 values is the working guess. Right way to
   lock it: 2-week study of real refactor PRs across ~100 Rust repos to see
   what shapes actually occur in the wild.

3. **Rust-native vs Python for `observe`'s ML primitives.** Recommend
   Rust-native (`linfa` + `petgraph` + `nalgebra`). Slower to build, faster
   at rollout, deterministic — all three matter for RL.

4. **Reward weights.** Grid-search on eval set per phase, or learn jointly
   with the policy. Recommend fix-per-phase to avoid coupled instability.

5. **Hard vs soft constraints in `propose`.** Initial proposal:
   - *Hard*: cycle introduction, visibility widening, trait coherence,
     compile break.
   - *Soft*: boundary cost, bridge weakening, dead-pub growth, instability.
   The decision shapes what the model generalizes — soft constraints with
   penalty produce richer learning than hard refusals.

6. **`implement` granularity.** Keep as a single tool with structured I/O,
   or add a sub-vocabulary for body construction? Recommend single tool with
   schema enforcement, free-form body text within. Revisit if generated
   bodies are consistently malformed.

7. **Mid-episode vs episode-start analysis.** Should the agent re-cluster
   after each commit, or survey at start and plan? Affects latency budget
   by ~100x. Recommend aggressive caching + mid-episode allowed; incremental
   recomputation only on affected subgraphs.

---

## Risks, Ordered by Lethality

1. **Hypergraph rebuild latency.** Sub-second incremental is non-negotiable.
   Without it, the rest of the project is infeasible.

2. **4-tool abstraction leaks.** Some refactors won't fit the typed `intent`
   vocab. Discover empirically; either expand vocab or accept the loss.

3. **Synthetic distribution gap.** Perturbations may not span the real
   refactor distribution. The hand-curated real-task benchmark helps but
   doesn't eliminate this.

4. **SFT-to-RL lift smaller than hoped.** If SFT lands at 50% and RL only
   pushes to 70%, that's not "exceed Composer 2.5" territory. Mitigation:
   teacher distillation, better synthetic curriculum, more compute.

5. **Tool-call format learning dominates RL gradient.** Common failure mode
   for tool-use RL on small models. Mitigation: heavier SFT before RL,
   smaller initial learning rates.

---

## What to Build First

Two tracks in parallel:

1. **Phase 0 incremental hypergraph rebuild** — sub-second on small edits.
   Highest-risk item; needs validation before everything else.

2. **Phase 1 `observe` with analysis primitives** — typed vocabulary,
   `linfa`/`petgraph` integration, caching layer. Validates the API shape
   independently of `propose`/`implement`/`commit`.

`propose`, `implement`, `commit` come after the perturbation engine starts
producing real episodes. Let the observed shape of episodes inform the API,
not the other way around.

---

## Comparison Reference: Cursor Composer 2.5

Cursor's Composer 2.5 (Kimi K2.5 base + RL with synthetic data) is the
closest public reference point. It's "decent" on general code editing.

Three structural advantages this project has over that approach:

1. **Domain specialization** — Rust only, not all languages.
2. **Lifted action space** — 4 semantic tools, not character-level edits.
3. **Dense verifier** — 30+ audit signals, not just "tests passed."

The wedge is the combination of all three. Each alone is incremental;
together they should produce qualitatively different learning dynamics.

---

## Success Criteria

- Phase 0: incremental rebuild benchmarked at <500ms on 10-line edit in
  100k-LOC workspace.
- Phase 1: 4-tool API can complete the rmc-refactor repo's known cleanup
  tasks (project_paths dedupe, dead-pub downgrade, query module split) end
  to end via tool calls from a frontier model.
- Phase 2: SFT'd model solves 30%+ of held-out perturbations.
- Phase 3: RL'd model solves 70%+ of held-out perturbations and matches or
  exceeds frontier on the hand-curated real-task benchmark.
- Phase 4: model meets or exceeds frontier on at least one externally
  validated Rust coding benchmark, with the framework documented enough that
  others can reproduce the training loop.
