---
name: rmc-call-graph
description: Rust fn-level call graphs.
argument-hint: "<fn-qualified-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust function-level call graphs

Layer 10 makes fn-body call edges first-class graph data. Five tools cover
incoming, outgoing, recursive descent, crate-scoped filter, and a
transitive-caller count. The older `get_call_graph` (parser, single-file)
is the within-file fallback. Scope: single fn or single file.

For single-symbol forensics including importer/usage data, use
`rmc-symbol-forensics`. For trait-dispatch-specific analysis across all
impls, use `rmc-trait-audit`.

## Scope — single fn or single file

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Layer 10 (workspace-wide) vs `get_call_graph` (within-file)

| Question | Tool |
|---|---|
| "Who calls fn Y, anywhere?" | `who_calls(target=Y)` |
| "What does fn Y call, anywhere?" | `calls_from(caller=Y)` |
| "What's reachable from Y up to depth N?" | `call_graph(root=Y, depth=N)` |
| "Who in crate X calls Y?" | `callers_in_crate(target=Y, krate=X)` |
| "How many distinct fns transitively call Y?" | `recursive_callers_count(target=Y, depth=N)` |
| "What does this single file look like internally?" | `get_call_graph(file_path=...)` (parser fallback) |

Layer 10 is HIR-resolved and includes calls through generic bounds and
`dyn T` receivers. The parser-driven `get_call_graph` only sees the AST
in one file and misses cross-file edges entirely.

### Step 1. Workspace-wide callers

```
who_calls(directory=..., target=<crate>::Y)
```

Every fn body containing a call to Y. Each row: `caller` (enclosing fn
qualified name), `file`, byte `start` / `end`, `category` (Read / Write /
Test / Other). References in const initializers, type aliases, and other
non-fn scopes are excluded — use `who_uses` for all reference sites. Calls
from closures attribute to the enclosing fn.

### Step 2. Workspace-wide callees

```
calls_from(directory=..., caller=<crate>::Y)
```

Every outgoing reference made from Y's body. Same row shape as
`who_calls` with `callee` instead of `caller`. Inventory Y's downstream
surface — "what does this fn touch?" — before extracting helpers.

### Step 3. Bounded recursive call tree

```
call_graph(directory=..., root=<crate>::Y, depth=3)
```

Bounded recursive descent over outgoing call edges. `depth` defaults to 3,
capped at 8. Each node carries `fn_qualified_name`, `crate_name`,
`callees`, `truncated_at_cycle` (fn already expanded earlier), and
`truncated_at_depth` (depth ran out with unvisited callees).

Depth budget:

| Depth | Use |
|---|---|
| 1 | Direct callees only (cycle/depth flags included) |
| 2 | Readable JSON for most fns |
| 3 | Default; balance of recall vs payload |
| ≥6 | Hub fns produce large outputs; use sparingly |
| 8 | Hard cap |

### Step 4. Crate-scoped audit

```
callers_in_crate(directory=..., target=<crate>::Y, krate=<other_crate>)
```

`who_calls(target=Y)` filtered to call sites whose *caller fn* lives in
the named crate. Use to verify a crate boundary holds — e.g. "no fn in
`domain` calls into `agent::orchestrator`" should return zero.

### Step 5. Blast-radius integer

```
recursive_callers_count(directory=..., target=<crate>::Y, depth=8)
```

Reverse BFS counting distinct transitive caller fns up to `depth` hops.
Returns `direct_callers`, `transitive_callers`, `depth_reached`,
`truncated_at_depth`. Counts *fns*, not call sites — a fn that calls Y
five times counts as 1 caller.

Single integer to weight refactor risk: a fn with `transitive_callers=200`
is a critical-path hub; signature changes have 200-fn fallout. Pair with
`rmc-unsafe-audit` and `rmc-mut-static-audit` to score which findings sit
on hot paths.

### Step 6. Within-file structure (parser fallback)

```
get_call_graph(file_path=<path>)
```

Parser-based, single-file: fn-to-fn edges within the AST. Use when:

- No hypergraph build available (analyzing a non-workspace drop, or a
  snapshot where `build_hypergraph` would take too long).
- You want fn-arity-style structural analysis on a single file (find
  dispatch hubs by out-degree).
- You want to corroborate Layer 10 results — parser sees the AST, Layer
  10 sees HIR-resolved.

Compose: `get_call_graph(file_path=...)` for internal structure +
`who_calls(target=<crate>::<fn>)` for external callers = full picture.

## Decision frames

| Goal | Tool |
|---|---|
| Workspace-wide caller list | `who_calls` |
| Workspace-wide callee list | `calls_from` |
| "What's reachable from here?" | `call_graph(depth=2 or 3)` |
| "Does this caller crate respect the rule?" | `callers_in_crate` |
| Single integer for refactor scoring | `recursive_callers_count` |
| Within-file structure (no hypergraph) | `get_call_graph` |
| Find leaf functions | `who_calls(target=Y)` returning empty |
| Find entry-point functions | `calls_from(caller=Y)` returning empty |

## Pattern reference

| Pattern | Invocation |
|---|---|
| List every caller of Y | `who_calls(target=Y)` |
| List every callee of Y | `calls_from(caller=Y)` |
| Reachability map up to depth 3 | `call_graph(root=Y, depth=3)` |
| Crate boundary check | `callers_in_crate(target=Y, krate=X)` |
| Refactor blast radius | `recursive_callers_count(target=Y, depth=8)` |
| File-internal structure | `get_call_graph(file_path=<file>)` |

## Output format

```
Function: <crate>::Y
Direct callers: <n>
Transitive callers (depth=8): <n>
Callees (direct): <n>
Reachable (depth=3): <n> distinct fns
Crate boundary check (krate=<X>): <PASS | n violations>
Verdict: <leaf | entry-point | hub | midstream>
Blast radius: <Low | Medium | High>
```

## Limitations

- Trait dispatch via dynamic calls is a static-resolution heuristic
  (Layer 10 follows HIR's resolved callee; runtime polymorphism through
  `dyn Trait` may be incomplete depending on how RA resolved the receiver).
- No enum-of-fn-pointers tracking — a `match` arm dispatching to one of N
  fn pointers reads as a load, not N call edges.
- Macro-expanded calls may not surface — `println!("{}", foo())` resolves
  inner `foo()`, but a custom macro whose expansion contains a call may
  be invisible if absent from the post-expansion HIR.
- `get_call_graph` is parser-only and misses cross-file calls entirely;
  use Layer 10 for workspace-wide questions.
