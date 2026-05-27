```
                  rust-code-mcp — the workspace from its boundaries
            45 crates · 319 modules · 2,811 items · 6,364 bindings


                              ┌────────────────────────┐
                              │      rmc_server        │
                              │   tool / MCP surface   │
                              │  465 items   I = 0.33  │
                              └────┬──────┬──────┬─────┘
                                   │      │      │
                            112 refs│  22 refs   │ 36 refs
                                   ▼      ▼      │
                         ┌──────────────┐  ┌──────────────┐
                         │  rmc_graph   │  │ rmc_indexing │
                         │  hypergraph  │  │  indexer +   │
                         │   + audits   │  │  embeddings  │
                         │  802 items   │  │  352 items   │
                         │  I = 0.08    │  │  I = 0.13    │
                         └──────┬───────┘  └──┬──────┬────┘
                                │             │      │
                           8 refs│       49 refs  10 refs
                                │             │      ▼
                                │             │  ┌────────────┐
                                │             │  │ rmc_config │
                                │             │  │  55 items  │
                                │             │  │  I = 0.33  │
                                │             │  └──────┬─────┘
                                ▼             ▼         │
                         ┌──────────────────────────┐   │
                         │        rmc_engine        │◄──┘  4 refs
                         │   HIR extraction + IR    │
                         │   562 items   I = 0.07   │
                         │   (the foundation)       │
                         └──────────────────────────┘


           consumer ──refs──▶ producer      I = Ce / (Ce + Ca)
                                            (0 = stable foundation,
                                             1 = volatile leaf)

         ↑ none of this is visible from function bodies.
           this is what AI doesn't see by default.
```
