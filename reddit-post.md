**Title:** `rust-code-mcp` now makes Rust agents show their work: call graphs, safety audits, rename previews

**Body:**

Follow-up to my previous `rust-code-mcp` post.

I got tired of agents ending Rust refactor suggestions with "this looks cleaner."

That is not enough. If an agent wants to split a module, extract a trait, make something `pub(crate)`, move code between crates, or touch unsafe-adjacent code, I want it to show its work.

So the latest update is focused on evidence. `rust-code-mcp` now exposes **45+ rust-analyzer-backed MCP tools**, ships **26 Claude Code skills**, and builds a persisted HIR-driven workspace hypergraph that can be reused across tool calls. The goal is to let an agent answer structural questions with actual references, imports, call edges, visibility, attributes, docs, and file spans.

Concrete example:

> "Can this module become its own crate?"

The agent can now check:

- `build_hypergraph` once, then reuse the fingerprinted snapshot.
- `get_imports` / `crate_edges` to see what crosses the proposed boundary.
- `who_calls`, `calls_from`, and `recursive_callers_count` to estimate function-level blast radius.
- `dead_pub_report` to find `pub` items that are not actually cross-crate API.
- `build_codemap` to return a small graph of the relevant symbols instead of dumping the whole repo.
- `rename_symbol` as a dry-run probe when the change implies API renames or module moves.

The answer should stop being:

> "This seems cleaner."

And start looking more like:

> "This boundary is expensive because these imports and call edges cross it. Moving it would introduce this crate edge. These three `pub` items can stay internal. This rename would touch 17 references across 4 files. Here is the exact preview."

That is the direction I am trying to push.

## What changed

**Safety and review audits**

New tools include `unsafe_audit`, `mut_static_audit`, `fn_body_audit`, `channel_capacity_audit`, `recursion_check`, `missing_docs_audit`, and `derive_audit`.

They find things like:

- `unsafe { ... }` blocks, including whether a nearby `SAFETY` comment exists.
- `static mut`, `LazyLock`, `OnceLock`, and `OnceCell`.
- `unwrap`, `expect`, panic macros, unchecked unwraps, self-recursion, suspicious loops, and lock-across-await patterns.
- bounded vs unbounded channel construction.
- public items missing docs or expected derives.

These are review triggers, not formal proofs. The useful part is that they return exact files, spans, and enclosing functions.

**Function-level call graph**

The new call graph layer includes `who_calls`, `calls_from`, `call_graph`, `callers_in_crate`, and `recursive_callers_count`.

Import graphs are not enough for refactoring. Sometimes the real question is not "which module imports this type?" but "which call paths reach this function if I change its signature?"

**Architecture and workspace checks**

There are new tools for crate-level instability metrics, forbidden dependency rules, name collisions, re-export chains, and dead public API:

- `crate_dependency_metric`
- `forbidden_dependency_check`
- `overlaps`
- `re_export_chain`
- `get_reexports`
- `get_declared_reexports`
- `pub_use_pub_type_audit`
- `dead_pub_in_crate`
- `dead_pub_report`

**Refactor probes**

`rename_symbol` is rust-analyzer rename exposed as a read-only preview. It returns the exact reference set, exact text edits, and any file moves rust-analyzer would perform. No files are modified.

That makes it useful even when you do not plan to rename anything. You can use it to ask whether a symbol is reachable, how big the blast radius is, whether macro-expanded references show up, or whether a module rename would move files.

**Codemap**

`build_codemap` creates a task-conditioned subgraph. Seed it with symbols or a task prompt, and it returns the relevant neighborhood as JSON, Mermaid, or outline: call edges, type uses, module hierarchy, and diagnostics.

The goal is to give an agent enough context to reason locally without pasting a whole workspace into the prompt.

**Semantic neighbors**

`similar_to_item` and `semantic_overlaps` are for duplicate-logic hunting. They use embeddings to find code that looks semantically similar even when it does not share a type, import, or call edge.

**Claude Code skills**

The repo now includes **26 skills** under `skills/`, including `/rmc-workspace-overview`, `/rmc-crate-audit`, `/rmc-unsafe-audit`, `/rmc-refactor-plan`, `/rmc-rename-symbol`, `/rmc-codemap`, `/rmc-architecture-rules`, `/rmc-call-graph`, and `/rmc-semantic-overlaps`.

Each skill is a small `SKILL.md` recipe: prerequisites, tool calls, expected evidence, and hand-offs to related skills.

## The theory layer

The part I am most interested in is the new `THEORY.md`.

It names **15 structural principles** for agent-driven refactoring and maps each one to checks the MCP server can run. The point is not to make the tool sound academic. The point is to make the agent's reasoning inspectable.

Examples:

- **Boundary cost**: do not split code across a boundary if imports and references across that boundary are dense and unstable. Check with `get_imports`, `crate_edges`, and call graph tools.
- **Acyclicity**: do not pretend two containers are separate if they form a cycle. Check with `crate_edges` and `forbidden_dependency_check`.
- **Callsite-usage set**: if you extract a trait, the honest trait surface is the set of methods actually used at call sites, not every method the concrete type happens to have. Check with `who_uses`, `who_calls`, and `function_signature`.
- **Visibility as projection**: `pub` is not style. It is a structural claim. Check with `dead_pub_report`.
- **Re-export transparency**: a `pub use` chain should not hide where the real API surface lives. Check with `re_export_chain` and `get_reexports`.

So when an agent suggests a refactor, I want it to cite both:

1. The principle it is applying.
2. The tool output that supports it.

That is much harder to fake than "this is cleaner."

## What this is not

It is not an auto-refactorer that should blindly change your workspace. Most of the interesting tools are read-only. The rename tool previews edits instead of applying them. The semantic tools return candidates, not verdicts. The safety audits are review triggers, not formal verification.

The design goal is narrower: give coding agents better evidence before they recommend or perform changes in Rust projects.

## Known gaps

- Co-change locality needs git-log integration.
- The signature-rung theory needs a one-shot diagnostic.
- Some `THEORY.md` sections are still stubs, especially the worked walkthrough.
- Dynamic behavior, runtime performance, and team ownership boundaries are intentionally out of scope for now.

## Links

- Website: https://rust-code-mcp.pages.dev/
- Repo: https://github.com/molaco/rust-code-mcp
- Discord: https://discord.com/invite/dENhfbtCa
- Full tool reference: `TOOLS.md`
- Principles: `THEORY.md`

I would especially like feedback on the evidence model: before you trusted an agent to refactor a Rust workspace, what would you want it to prove first?
