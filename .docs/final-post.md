A lot of people using ai, and particularly llms, talk about how llms enable us to code from a higher point of abstraction and not have to think about the code.

A lot of people also say that as they continue using llms in their codebases they start losing track of what is happening, leading to a bunch of ai slop.

These are two of the most common things i hear about coding with llms, and my experience using them for a while tells me there is an important point both are circling without naming. The point is this: the "higher abstraction" we get from ai is real, but it is much narrower than the phrase suggests, and the part it covers is the part we used to learn the codebase through — which is why the same people who gained the abstraction can end up losing track of the code.

Ai enabling us to take a higher point of abstraction does not really mean we can sit back and relax after writing "make a business with 1b$ return, no mistakes". And honestly, this higher point of abstraction is not really well specified anywhere. In practice, ai does basically only function bodies, and is missing pretty much all the other levels of abstraction — types, file/module/crate organization, trait and visibility design, dependency direction, what the public api of a crate even looks like.

If we cannot focus on the function/method code body, we usually lose all understanding of what the project is doing, because we are so accustomed to developing our understanding by doing exactly that — reading bodies, tracing branches, mentally executing a function until the shape of the thing clicks. Take that away and most of us are left looking at a codebase we cannot really see.

Putting it into a mathematical perspective: managing code without function bodies is similar to category/topos theory, while working at the level of function bodies is more similar to hott. You don't need to know the math — the punchline is below. There are a couple of theorems that reflect this particularly well: the yoneda lemma and the dependent yoneda lemma.

> "A thing is completely determined by all its relationships to other things."

In code: a module is fully specified by who imports it and what it exports. The body almost doesn't matter at this level — the edges do.

> "Knowing something at one point is the same as knowing a coherent family of values at every other point, varying consistently along the paths between them."

In code: knowing one type forces a whole family of conversions, traits, and call sites around it to line up; you cannot just "know" the type locally, the rest of the codebase has to agree with it.

Working in this framework is a very difficult adaptation and a complete change in paradigm, precisely because it asks you to give up the function-body habit and start understanding the project through edges, boundaries and shapes instead.

The consequence of taking this seriously is that codebases need to be super well organized. And instead of leveraging algorithms to do heavy computation with loops, there is a need to develop the code infrastructure to avoid that entirely and instead use space — precompute, type-encode, lay things out so the loop becomes a lookup. If a thing is determined by its boundaries, then getting the boundaries right *is* the work, not an afterthought you do once the loops compile.

For these reasons i have made rust-code-mcp — an mcp server that exposes a persisted HIR-derived hypergraph of a rust workspace as ~45+ read-only intelligence tools — for my personal coding, and today i share it with you so that you may give me your opinion and/or help me develop a better way of coding with llms. I needed something that made the non-function-body layers (imports, exports, boundaries, call graph, type overlap, public surface) as cheap to inspect as a function body is to read, and nothing else available was doing that.

what rust-code-mcp provides at the moment:

1. **crate / module skeletons** (`crate_skeleton`, `module_tree`) — read the shape of a crate without reading the bodies
2. **imports / exports / usage** (`get_imports`, `get_exports`, `get_reexports`, `who_imports`, `who_uses`, `who_uses_summary`) — the actual edge set, not your impression of it
3. **call graph** (`who_calls`, `calls_from`, `call_graph`, `callers_in_crate`, `recursive_callers_count`) — function-level dependency, both directions
4. **crate-level structure** (`crate_edges`, `crate_dependency_metric`, `module_dependencies`, `forbidden_dependency_check`) — boundary-level reasoning
5. **similarity / overlap** (`similar_to_item`, `semantic_overlaps`, `overlaps`, `get_similar_code`) — find existing infrastructure before re-implementing it
6. **audits** (`dead_pub_report`, `unsafe_audit`, `mut_static_audit`, `recursion_check`, `analyze_complexity`, `fn_body_audit`, `channel_capacity_audit`, `derive_audit`, `pub_use_pub_type_audit`) — enforce the guidelines
7. **rename preview** (`rename_symbol`) — returns the exact reference set and edits, never mutates the workspace

the mental model:

1. establish a series of guidelines for functions/methods and types — concrete ones, the kind you can check: low cyclomatic complexity, max input arity, max LOC per function, no `unsafe` outside designated modules, naming conventions, derive hygiene, no recursion unless explicitly justified.
2. establish guidelines for files/modules/directory/crate structure — max one level of directory nesting, dense internal edges stay inside a module, every module describable in one sentence, no cycles at the crate level, public surface as narrow as possible.
3. then use the tools to enforce them. The audit tools enforce the function/type guidelines (`fn_body_audit`, `analyze_complexity`, `unsafe_audit`, `recursion_check`, …). The dependency and boundary tools enforce the organization guidelines (`crate_edges`, `module_dependencies`, `who_imports`, `forbidden_dependency_check`, `dead_pub_report`, …). The agent does not get to claim "this is fine" — the tool either agrees or doesn't.

# particular useful stuff

after designing a plan or when tackling a problem, run the similarity/overlap tools against your proposed types and functions *first*. `semantic_overlaps` and `similar_to_item` against the plan steer the agent toward reusing the existing code infrastructure instead of redoing 15 types that are 80% the thing you already have three modules over. This is the single biggest source of ai slop in my experience and the cheapest one to prevent.

make the agent read the skeleton files instead of the actual files when it is thinking about structure. bodies pull in implementation noise that biases the agent toward editing-in-place instead of thinking about boundaries, and they eat context. skeletons preserve types, signatures, visibilities and module layout — which is everything you need to reason at the higher level — without the body context poisoning.

before any rename or move, run `who_uses_summary` on the target. the blast-radius read forces the agent to actually justify the scope of the change before it starts touching files, and it kills the "this looks like a small move" delusion early.

---

This is where i am right now. The tool is real, it is in daily use on my own projects, and it is open. I do not think the three-tools-and-a-mental-model above are the final shape — they are just what i ended up with after trying to make agentic coding feel like coding again, instead of like watching slop accumulate. If you have a sharper version of any of this, or you have hit a wall the tools should have caught and didn't, i want to hear about it. Repo: [github.com/molaco/rust-code-mcp](https://github.com/molaco/rust-code-mcp). Discord: [discord.com/invite/dENhfbtCa](https://discord.com/invite/dENhfbtCa).
