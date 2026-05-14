**Title:** `rust-code-mcp` update: Rust agents now have to cite evidence before suggesting refactors

**Body:**

Follow-up to my [previous post](TODO: paste previous post link).

I got tired of agents ending Rust refactor suggestions with "this looks cleaner." If an agent wants to split a module, extract a trait, change visibility, move code between crates, or touch unsafe-adjacent code, I want it to show its work.

So the latest update is focused on evidence. `rust-code-mcp` exposes **45+ rust-analyzer-backed MCP tools** and ships **26 Claude Code skills**, all sitting on a persisted HIR-driven workspace hypergraph that gets reused across tool calls.

The goal: stop accepting

> "This seems cleaner."

and start expecting

> "This boundary is expensive because these imports and call edges cross it. Moving it introduces this crate edge. These three `pub` items can stay internal. The rename would touch 17 references across 4 files. Here is the exact preview."

## Concrete example

> "Can this module become its own crate?"

The agent can now check:

- `build_hypergraph` once, then reuse the fingerprinted snapshot.
- `get_imports` and `crate_edges` for what crosses the proposed boundary.
- `who_calls`, `calls_from`, `recursive_callers_count` for function-level blast radius.
- `dead_pub_report` for `pub` items that are not actually cross-crate API.
- `build_codemap` for a focused subgraph instead of dumping the whole repo into the prompt.
- `rename_symbol` as a dry-run probe when the change implies API renames or module moves.

## The theory layer

The part I am most interested in is `THEORY.md`. It names **15 structural principles** for agent-driven refactoring and maps each one to checks the server can run. The point is to make the agent's reasoning inspectable.

A few:

- **Boundary cost** — do not split code where imports and references across the boundary are dense and unstable. Check with `get_imports`, `crate_edges`, call graph tools.
- **Acyclicity** — do not pretend two containers are separate if they form a cycle. Check with `crate_edges`, `forbidden_dependency_check`.
- **Callsite-usage set** — if you extract a trait, the honest surface is the methods used at call sites, not every method the concrete type happens to have. Check with `who_uses`, `who_calls`, `function_signature`.
- **Visibility as projection** — `pub` is a structural claim, not style. Check with `dead_pub_report`.
- **Re-export transparency** — a `pub use` chain should not hide where the real API lives. Check with `re_export_chain`, `get_reexports`.

When the agent recommends a refactor, I want it to cite the principle it applied *and* the tool output that supports it. Much harder to fake than "this is cleaner."

## What changed (short version)

- **Safety audits** — `unsafe_audit`, `mut_static_audit`, `fn_body_audit`, `channel_capacity_audit`, `recursion_check`, `derive_audit`. Return exact files, spans, enclosing functions. Review triggers, not proofs — they flag `unsafe` blocks (and whether a nearby `SAFETY` comment exists), `static mut` / `LazyLock` / `OnceLock`, `unwrap` / `expect` / panic macros, self-recursion, lock-across-await, bounded vs unbounded channels.
- **Function-level call graph** — `who_calls`, `calls_from`, `call_graph`, `recursive_callers_count`. Import graphs are not enough when the real question is *which call paths reach this fn if I change its signature*.
- **Architecture checks** — crate-level instability metrics, forbidden-dependency rules, re-export chains, dead public API (`crate_dependency_metric`, `forbidden_dependency_check`, `dead_pub_report`, …).
- **Refactor probes** — `rename_symbol` is rust-analyzer rename exposed as a read-only preview: exact reference set, exact text edits, file moves. Useful even when you don't plan to rename — it answers "how big is the blast radius?"
- **Codemap** — `build_codemap` returns a task-conditioned subgraph (JSON / Mermaid / outline) so the agent reasons locally instead of pasting the whole workspace.
- **Semantic neighbors** — `similar_to_item` and `semantic_overlaps` find duplicate logic by embedding similarity, even when the candidates share no types, imports, or call edges.
- **26 skills** under `skills/` — small `SKILL.md` recipes with prerequisites, tool calls, expected evidence, and hand-offs.

Full list in `TOOLS.md`.

## What this is not

Not an auto-refactorer. Most tools are read-only, the rename tool previews instead of applies, semantic tools return candidates, and the safety audits are review triggers — not formal verification. The narrower goal: give coding agents better evidence before they recommend or perform changes in Rust projects.

## Known gaps

- Co-change locality needs git-log integration.
- Signature-rung diagnostic is missing.
- Some `THEORY.md` sections are still stubs, especially the worked walkthrough.
- Dynamic behavior, runtime perf, and team-ownership boundaries are out of scope for now.

## Links

- Website: https://rust-code-mcp.pages.dev/
- Repo: https://github.com/molaco/rust-code-mcp
- Discord: https://discord.com/invite/dENhfbtCa
- Full tool reference: `TOOLS.md`
- Principles: `THEORY.md`

Question for r/rust: before you trusted an agent to refactor a Rust workspace, what evidence would you want it to produce first?
