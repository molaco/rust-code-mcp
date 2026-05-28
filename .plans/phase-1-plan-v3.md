# Phase 1 Plan (v3)

Supersedes `phase-1-rewrit-2.md` and the Phase 1 section of `long-plan.md`.
This version folds in everything settled since v2:

- the ML / analysis layer, reframed as **perception (vision)** rather than
  passive duplication detection
- the **is/ought line** as the governing principle for every analysis tool
- **composition over labels** — primitive-first architecture, the intent enum
  demoted to a training-time teacher
- **right-sized rigor** — the relaxed stance on determinism / latency /
  Goodhart, recorded so the over-strict version is not re-derived
- **Candle, not Burn** (one GPU tensor runtime, already linked via fastembed)

The whole point: leverage infrastructure and architecture. Use **space instead
of time**. Keep **functions simple**. Push divide-and-conquer out of function
bodies and into types, files, and memory.

---

## Governing Principles

1. **Complexity belongs in architecture, not function bodies.** Functions are
   simple linear transformations between well-designed types. Every body-level
   violation is a missing-abstraction signal: branching → enum dispatch,
   params → struct, length → more types, nesting → early returns + better
   types.

2. **The agent never bypasses guidelines.** No agent-facing override. A refused
   move means "restructure," not "suppress." The only exception is the
   human-curated boundary allowlist (FFI, proc-macro output, external trait
   impls).

3. **The codebase is an inhabited semantic graph.** The agent has a location
   and a view. It navigates; it never edits files directly. CRUD operations are
   semantic and auto-propagate.

4. **Bodies are hidden by default.** Signatures, types, and descriptions are
   the working surface. `show_body` is an explicit, costed move.

5. **The is/ought line.** The engine **renders perception** (what *is* — facts
   about the current graph). The agent **forms intention** (what *ought to be*
   — the target state). The is→ought leap is the skill being trained, so it
   must live in the policy. Every analysis tool sits on the *is* side: it shows
   structure, it never concludes a refactor.

6. **Composition over labels.** The agent emits primitive operations and
   sequences them itself; the engine only validates. The architectural
   reasoning lives in the policy, because you cannot exceed a competence you
   delegate to a hardcoded decomposition. (See "Composition vs the Enum.")

7. **Clustering is vision, not a to-do list.** Clustering is how the agent
   *sees* the codebase — its perceptual mesoscale — not a passive audit that
   emits "dedup these." Deduplication, trait extraction, and module splits all
   become *consequences* of good vision.

---

## Right-Sized Rigor (read this before adding constraints)

This project is a research prototype, not a production RL system. Earlier
design passes over-applied production-grade rigor. The corrected stance:

- **Determinism.** RL tolerates noisy observations — clustering jitter in what
  the agent *sees* is harmless, even a mild regularizer. Do **not** chase
  deterministic GPU kernels. Set one global seed per run so experiments are
  reproducible, log it, move on. Only the dominant reward terms must be stable,
  and the gates (compile / tests / audit deltas) are deterministic for free.

- **Latency.** The one lethal constraint is the **per-step hypergraph rebuild**
  — it must be fast (sub-second incremental). Global analysis (clustering,
  graph metrics) runs **once per episode, cached**; a few seconds there is in
  the noise, since each step is dominated by the policy's own forward pass.
  Early experiments need thousands of rollouts, not millions — do not optimize
  for millions yet.

- **Goodhart.** Reward-hacking the graph metrics is a late-game problem. Ship
  the simple reward; add regularization when you *observe* gaming, not
  preemptively.

- **The three-audience split is a habit, not machinery.** It reduces to one
  rule: run expensive global stuff less often than cheap local stuff. No
  elaborate enforcement layer.

The rigor comes later, driven by observed failures, not designed in upfront.

---

## State and Perception

The agent has a **current location** in the graph and a task-conditioned
**view**. Navigation is multi-scale, like a map with zoom levels:

```
Continents  = crates              (structural, given)
Countries   = modules             (structural, given)
Cities      = clusters            (semantic mesoscale — only exists via clustering)
Streets     = items               (functions, types, methods)
Buildings   = bodies              (hidden by default)
```

The "city" level is the missing middle of every code map today: semantically
coherent groups that do **not** line up with module boundaries. Clustering
generates it. The agent's location can be a *cluster* — it can zoom in to
members or out to neighboring clusters in cluster-space.

Example view at the item level:

```
Location: rmc_indexing::dir_hash
Kind: Function
Signature: pub(crate) fn dir_hash(dir: &Path) -> String
Description: computes a stable hash of a directory path for caching
Body: hidden (8 lines, no violations)
Callers (3): [..., rmc_server::project_paths::dir_hash  (latent-affinity 0.94)]
Calls (2): sha2::Sha256::new, Path::to_string_lossy
City: "path hashing" (2 members, spans 2 crates)
Salience: this concept is fragmented across crates
Audit status: clean
Available: goto | zoom_out | show_body | show_callers | analyze | <CRUD ops>
```

Note the framing: the engine reports the *fact* ("this concept is fragmented
across crates," "latent-affinity 0.94") — it does **not** say "dedup these."
The agent draws that conclusion. That is the is/ought line in the view itself.

---

## Action Vocabulary

Five verb categories. The agent composes primitives directly; the engine
validates.

### Navigate
- `goto(target)` / `zoom_in` / `zoom_out` — move location across map scales
- `show_body(target)` — reveal a hidden body (costed)
- `show_callers` / `show_cluster` — expand the view
- `follow_trail(hyperedge)` — walk a hyperedge (trait impls, re-export chain,
  cluster) like a corridor

### Analyze (perception — the vision layer)
Whole-workspace surveys that shape what the agent sees. Run once per episode,
cached. Pure *is*: they render structure, they conclude nothing.
- `cluster(substrate, algo)` — substrate ∈ {embedding, structure, both}
- `outliers(feature?)` — perceptual salience
- `affinity(target)` — latent pull between items (link-prediction *features*,
  not edge predictions)
- `co_change(scope)` — temporal coupling (P11)

### Query (pointwise lookups)
- `find_similar(target, substrate)` — nearest-neighbor to a seed
- `find_violations(scope)` / `find_dead(scope)` / `find_cycles(scope)`
- `search_by_description(text)` — retrieve via the description index
- `simulate(operation)` — counterfactual: predicted deltas + cascade +
  would-refuse, without applying

### Structural CRUD (the primitives — the alphabet)
Items:
- `add_item(kind, location, definition)`
- `modify_signature(target, new_sig, callsite_strategy?)`
- `modify_body(target, new_body)` — graded inline
- `delete(target)` / `move(target, dest)`

Refactor primitives:
- `extract_function(body_span)` / `extract_trait(items)` / `inline(target)`
- `split_module(module, partition)` / `merge_modules(items)`

Modules / crates:
- `create_module` / `move_module` / `lift_to_crate` / `lower_to_module`

### Meta
- `annotate(target, note)` — agent-curated persistent note
- `commit()` — atomic apply, returns the reward vector
- `rollback()` — return to prior snapshot (wraps `jj op restore`)
- `declare_done()` — episode end

---

## The is/ought Discipline Applied

The single line that governs the entire Analyze layer:

> The engine may show the agent what **is** (descriptive facts about the
> graph). It must never tell the agent what **ought to be** (a refactor target).

Worked through the two cases that tempt the boundary:

**Clustering.** "These items are near each other / this concept spans two
crates" — *is*, allowed; it is the agent's vision. "Therefore extract a shared
module" — *ought*, forbidden; the agent must leap to it. This is exactly why
clustering is framed as **vision** and not duplication-detection:
duplication-detection smuggles the *ought* into the engine; vision keeps
clustering on the *is* side and leaves every conclusion to the policy.

**Link prediction.** "A and B have high latent affinity — shared callees,
embedding-similar, no edge between them" — *is*, allowed as an `affinity`
feature in the view. "Therefore add a shared helper" — *ought*, forbidden. So
link prediction survives as **perceived affinity**, never as a predicted edge
to create. It lands in two clean places and one forbidden one:

1. Curriculum (offline pipeline) — use it to *choose where to point the agent*
   during training. Clean.
2. Observation features — surface latent affinity; agent concludes. Clean.
3. Primary reward — forbidden: caps the agent at the predictor's taste, which
   is fatal when the goal is to *exceed* frontier models.

Consequence for reward design: reward "audit deltas improved," **never** "acted
on a salient/flagged item." Otherwise even pure perception gets *interpreted*
prescriptively and the audit-as-to-do behavior creeps back. Salience only pays
off when the agent's *conclusion* about it was right.

---

## Composition vs the Enum

The action space is the primitives (the **alphabet**), not a fixed menu of
labeled intents (a **phrasebook**). The old `intent` enum — `extract_shared_module`,
`lift_module_to_crate`, … (~10 values, each a frozen multi-step plan the engine
decomposed) — is **not** in the deployed action space.

Why composition for the end state:
- Composition **contains** the enum: a trained composing policy re-grows
  "extract_shared_module" as a *learned chunk* it reaches for when the
  situation is typical — and it can deviate when the situation isn't, which the
  frozen enum cannot. The relationship is containment, not symmetry.
- The architectural judgment (e.g. P7: where to put a shared helper so neither
  crate cycles) lives in the policy, not in a hardcoded decomposition. You
  cannot exceed a competence you delegate.

Honest credit to the enum (it is not "the dumb option"):
- It guarantees a **floor** — the agent can always emit a valid, rewarded
  action. Composition is higher-ceiling **and higher-variance**: a small base
  model under sparse reward over 5-step sequences can flail and form no chunks.

The resolution — an asymmetry, not a pole:
- Build the architecture **primitive-first**, unconditionally.
- Use enum-shaped scaffolding **only as a training-time teacher**: it generates
  demonstration data (label → primitive-sequence expansions for SFT) and
  **anneals away** as the policy's own composition reward takes over. Never a
  permanent action in the deployed model.
- Justification (one-way door): you can always add a phrasebook over an
  alphabet; you can never factor an alphabet out of a phrasebook. So the
  architecture decision is forced even while the training decision stays
  flexible.

---

## Constraints (engine-enforced)

### Hard refusals — physical barriers, not penalties
The agent cannot perform these; it must restructure. Returns a structured
reason it can learn from.
- Cyclomatic / param count / body LOC / nesting depth over threshold
- `unwrap()` / `panic!()` / `unreachable!()` outside tests
- `unsafe` outside the boundary allowlist
- Cycle introduction (P3) · visibility widening (P13) · trait coherence (P15)
- Compile break · test failure on touched module

```json
{ "refused": true, "principle": "P3",
  "finding": "move would create rmc_server -> rmc_indexing -> rmc_server cycle",
  "suggested_actions": ["extract_to_third_unit", "move_target_instead"] }
```

### Soft penalties — gradient signals, move applies
- Boundary cost (P1) · density gradient (P2) · bridge weakening (P8)
- Dead-pub growth (P13) · instability/abstractness regression
- Surface stability (P12) · hub blast-radius increase (P10)
- Per-step length penalty

### Boundary allowlist — the only exception
Workspace-level TOML, ~5-20 items, human-curated. The agent reads it but
cannot modify it.
```toml
[[boundary]]
target = "ffi::cuda_init"
reason = "FFI to CUDA runtime; C signature is fixed"
```

---

## Infrastructure

### Four indices
| Index | Substrate | Maintained by |
|---|---|---|
| Hypergraph | LMDB | incremental extractor (Phase 0) — **must be fast** |
| Embeddings | LanceDB | content-hash cache (Candle) |
| BM25 | Tantivy | existing |
| Descriptions | new | small sub-model, merkle-keyed regen |

**Descriptions** are the fourth retrieval axis: "what is *described as* doing
X," hits even when the code says `dir_hash` but the intent is "normalize a
path." Description quality is itself a training signal (stability across merkle
updates; utility toward correct refactors). Paired with clusters, each cluster
gets a generated concept label — the agent navigates *concepts*, not items.

### One GPU runtime: Candle
The project already links Candle/CUDA via fastembed. The analysis algorithms
reuse it — no second tensor framework (Burn dropped to avoid two runtimes
competing for VRAM and two CUDA surfaces to align). Candle covers
PCA/K-means/GMM/spectral/autoencoder/GNN forward passes. Combinatorial graph
algorithms stay CPU via `petgraph` (O(V+E), cheap at workspace scale).

### Counterfactual simulator
`simulate(op)` computes effects without applying — predicted deltas, cascade,
would-refuse. Cheap lookahead for planning.

### Atomic rollback + audit log
Every `commit` is a jj operation; `rollback()` wraps `jj op restore`. The agent
never sees partial state. Every operation, refusal, and reason is logged —
"how did this reach this state?" and every refusal is training data.

---

## The Statistical Algorithm Menu (by audience)

The audience split is a habit (run expensive global stuff less often), not
enforced machinery.

### Perception — once per episode, cached, feeds the view
| Algo | Substrate | Renders |
|---|---|---|
| GMM (soft clustering) | embeddings, Candle | overlapping concepts (a fn that's "parse AND cache") |
| Spectral | graph Laplacian, Candle | structural clusters — fuses embedding + graph |
| Agglomerative | both | nested structure (dendrogram = candidate module tree) |
| LOF / Mahalanobis | per-item features | multivariate / local-norm outliers (salience) |
| affinity (node2vec features) | graph | latent pull between unconnected items |
| co-change (Apriori + lift) | git history | temporal coupling (P11) |

### Reward — per-commit, delta-computable, Goodhart-deferred
| Metric | Scores |
|---|---|
| clustering coefficient | P2 density gradient, as a number |
| conductance / normalized cut | P1 boundary cost |
| betweenness | P8 bridge-ness |
| modularity (resolution-corrected) | partition quality for split/merge |

These score state *after* a move; the agent does not read them as targets.

### Pipeline — offline, frozen, never in rollout
| Method | Use |
|---|---|
| GNN node embeddings | learned structure+content fusion (frozen feature extractor) |
| contrastive (code ↔ description) | sharpen the embedding space |
| survival analysis / churn | P12 surface stability from history |
| link prediction | curriculum: where to point the agent during training |

Learned methods are **frozen** — train once, version the weights, treat as a
fixed extractor during rollouts (avoids training nondeterminism fighting
reproducibility).

---

## Reward Vector (from commit)

```json
{
  "compile": true,
  "tests_passed": 0.98,
  "audit_delta": { "cycles": 0, "dead_pub": -3,
                   "instability_change": -0.04, "complexity_change": -12 },
  "graph_metrics_delta": { "modularity": +0.03, "conductance": -0.05 },
  "principle_violations": [ { "principle": "P12", "severity": "soft", "weight": 0.1 } ],
  "episode_length": 7
}
```

- **Gates clean and stable**: compile + tests drive the gradient hardest and
  are deterministic for free. Hold the line here, relax elsewhere.
- **Graph metrics as reward, not objective**: shaped in, Goodhart-hardened only
  when gaming is observed.
- **Reward deltas improving, never acting-on-a-flag** (keeps perception honest).
- Scalarization is fixed per training phase, not learned jointly.

---

## Episode Shape

```
loop:
  obs = engine.view(location, task)        # includes cached per-episode analysis
  action = policy.choose(obs)
  navigate/analyze/query  -> small observation cost (budget pressure)
  structural CRUD         -> refuse(+reason) | apply -> shaped reward
  commit                  -> full reward vector
  declare_done            -> episode-end summary; break
  step budget exhausted   -> break
```

Many-step trajectories, per-action reward + episode-end summary. Because
guideline enforcement is a hard barrier, most episodes naturally become "agent
attempts a body, gets blocked, must restructure first" — the dominant
trajectory shape is architecture work, which is the skill being trained.

---

## Sizing (~3-4 months)

| Component | Weeks |
|---|---|
| Description sub-system | 3 |
| Navigation + multi-scale map / state representation | 3 |
| Analyze layer (clustering / outliers / affinity, Candle + petgraph) | 4 |
| Query primitives | 2 |
| Structural CRUD with auto-propagation | 6 |
| Guideline enforcement + boundary allowlist | 2 |
| Counterfactual simulator | 2 |
| Reward computation (audit + graph-metric deltas) | 2 |
| Episode runner | 2 |

(Phase 0 incremental hypergraph rebuild is the prerequisite and is sized
separately in `long-plan.md` — it is the one lethal-risk item.)

---

## Open Decisions

1. **Vision: perceptual or literal?**
   - *Perceptual* (assumed here): the map is structured **text** — cluster
     labels, membership, inter-cluster edges — read like everything else.
   - *Literal*: generate a 2D layout (UMAP / force-directed), render an
     **image**, feed a vision-capable policy. Novel, "gamify like never
     before," but a multimodal bet riding on layout quality + VLM spatial
     reasoning. Changes whether the 4th index emits text or pixels and whether
     the policy is multimodal.

2. **Auto-propagation when the engine can't auto-fix** (e.g. `modify_signature`
   adds a param it cannot synthesize at call sites): (a) refuse, (b) `todo!()`
   + required-follow-up, (c) require a `callsite_strategy`. Lean (c) for
   nontrivial cases, (a) as default. The biggest "how complete is the
   abstraction" call.

3. **Description sub-model**: separate small model vs main agent. Lean separate
   — descriptions generate continuously; main-agent compute is too valuable.

4. **Multi-window navigation**: start single-location; add multi-window only if
   side-by-side cluster comparison proves necessary.

5. **Step budget per episode**: 50-100, empirically tuned.

### Resolved since v2
- GPU runtime: **Candle**, not Burn (one runtime, already linked).
- Action space: **composition / primitive-first**; enum demoted to
  training-time teacher.
- Clustering role: **vision (perception)**, not duplication detection.
- Rigor level: **right-sized** (see the section above).

---

## What This Replaces From v2

| v2 | v3 |
|---|---|
| 4 verb categories | 5 — adds **Analyze** (the vision layer) |
| clustering implied under `find_similar` | first-class perception; clusters define map locations |
| "intent enum is gone" (flat) | composition-first + enum-as-teacher, with the asymmetry argument |
| Burn-GPU / petgraph-CPU split | **Candle**-GPU / petgraph-CPU (Burn dropped) |
| strict determinism / latency caveats | **right-sized rigor** section |
| reward vector (audits only) | + graph-metric deltas; is/ought reward discipline |
| — | the is/ought line as an explicit governing principle |
| — | mesoscale map / zoom-level navigation model |
```
