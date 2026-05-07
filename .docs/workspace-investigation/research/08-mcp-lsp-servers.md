# MCP / LSP / Tower-Router Workspace Patterns

How transport-front, capability-back Rust services split crates, and what it means for our MCP code-search server.

## Examples found

- **rmcp** ([rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)) — two crates: `rmcp` (transport + service + model) and `rmcp-macros` (`#[tool]`, `#[tool_router]`, `#[tool_handler]`). Examples in `examples/servers/` are one binary with `src/common/` for shared service logic and one `.rs` per transport (`counter_stdio.rs`, `counter_streamhttp.rs`, …).
- **taplo** ([tamasfe/taplo](https://github.com/tamasfe/taplo)) — six crates: `taplo` (parser/AST), `taplo-common` (schema/config/IO), `taplo-lsp` (thin LSP front depending on `lsp-async-stub` + `taplo` + `taplo-common`), `taplo-cli`, `taplo-wasm`, plus the reusable `lsp-async-stub`.
- **tinymist** ([Myriad-Dreamin/tinymist](https://github.com/Myriad-Dreamin/tinymist)) — 25+ crates: `sync-ls` (transport/service framework, features `lsp`/`dap`/`server`/`web`), `tinymist-query` (analyzers — the real capability), `tinymist-project`, `tinymist-world`, `tinymist-vfs`, `tinymist-std`, and the `tinymist` binary that wires them.
- **async-lsp** ([crate](https://crates.io/crates/async-lsp)) — tower-style: `LspService` is a `tower::Service`, `MainLoop` drives it, middleware via `tower_layer`. Closest sibling to rmcp's `ToolRouter`.
- **tower-lsp-server** ([repo](https://github.com/tower-lsp-community/tower-lsp-server)) — single crate; user implements one trait, transport fixed.

## Common patterns

1. **Three-layer split**: transport/service framework -> capability crates -> binary. Taplo and tinymist follow it; rmcp folds layers 1+2 but its examples demonstrate the user-side split.
2. **Transport crate stays thin and reusable**. `lsp-async-stub`, `sync-ls`, `async-lsp` are general-purpose JSON-RPC dispatchers with no domain knowledge, gated by features (`lsp`, `dap`, `server`, `web`) to cover stdio, WASM, and DAP from one crate.
3. **Request/response types live with the protocol, not the transport**. `lsp-types` is upstream; rmcp's `model` is in the transport crate, feature-gated. Tinymist puts query I/O in `tinymist-query`, not in `sync-ls`.
4. **Capability crates are framework-agnostic**. `tinymist-query` returns plain values; the binary adapts them to LSP responses. `taplo` parses TOML with no LSP awareness.
5. **The binary is the wiring site**. tinymist's binary depends on ~15 sibling crates; rmcp examples compose `rmcp` features + `common/` service + transport-specific `main`.

## Tool/handler registration patterns

- **Macro-driven (rmcp)**: `#[tool]` annotates `&self` methods, `#[tool_router]` collects them into a `ToolRouter<Self>` field, `#[tool_handler]` plugs that router into `ServerHandler`. Workspace implication: handlers must live in a crate that can depend on `rmcp-macros` and `rmcp` together — you cannot put tool method bodies in a leaf "pure-logic" crate without re-exposing rmcp's macros there. This pulls handler code toward the binary or a thin `*-server` crate.
- **Trait-driven (tower-lsp, async-lsp)**: one big trait with a method per request; registration is implicit via trait impl. Handlers can sit in any crate that depends on `lsp-types`, but the impl block grows monolithic.
- **Explicit dispatch (sync-ls, lsp-async-stub)**: builder pattern registers `(method_name, handler_fn)` pairs at startup. Most flexible for splitting handlers across crates — each capability crate exposes free functions and the binary registers them.
- **Hybrid (tinymist)**: `tinymist-query` exposes pure analyzer functions; the binary owns a registration table that maps LSP method names to those functions. Macros only used for serialization.

State of the art for multi-capability servers (tinymist, taplo) is the **hybrid**: explicit registration in the binary, capability crates stay framework-agnostic, transport crate stays generic.

## Direct lessons for our project

1. **Keep the rmcp-facing crate thin and at the top of the dependency graph**. Mirror taplo: `code-mcp-server` (rmcp + macros + dispatch) -> `code-mcp-tools` (tool impls returning plain Rust types) -> capability crates (hypergraph, search, audits). The macro requirement forces tool methods into `code-mcp-server`, but each method should be a one-liner delegating to a capability crate — same pattern rmcp examples use with `src/common/`.
2. **Do not invent a "types" crate just for request/response DTOs**. Both LSP and rmcp ecosystems put protocol types next to the protocol. Put MCP-shaped DTOs in `code-mcp-server`; let capability crates own their own domain types.
3. **Tool registration via `#[tool_router]` makes the server crate the registration site by construction** — there is no clean way to register tools from a sibling crate without re-exporting macros. Plan for one server crate per transport surface, not per capability.
4. **Feature-gate transports inside the server crate** the way rmcp examples do (`transport-io`, `transport-streamable-http-server`) rather than splitting `code-mcp-stdio` / `code-mcp-http` crates. That keeps the workspace flat and matches how rmcp itself ships.
5. **Capability crates should compile without rmcp**. This is the tinymist-query lesson: it lets us reuse the analyzers from a CLI, tests, or a future LSP front without a transport detour.
