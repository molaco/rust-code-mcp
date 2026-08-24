# Plan: fix the shared-daemon transport before merge

Status: not started. Written against `/home/molaco/Documents/rust-code-mcp-pulls`
on 2026-08-31.

Commit stack this plan applies to:

- `f40be526` (bookmark `main`) — `feat(server): RMC_EMBEDDING_PROFILE picks the
  default embedding profile`.
- `976f36c1` (bookmark `pr-12`) — `feat(server): one server per project via a
  unix-socket daemon`, rebased onto `f40be526`. The rebase was clean; the two
  commits share only `README.md` and `crates/rust-code-mcp/src/main.rs`.

The daemon design is correct and needs no server-side change: `RuntimeState` is
`Clone` (`crates/rmc-server/src/mcp/runtime.rs:96`), `ServerRuntime::state()`
hands out a clone (`runtime.rs:299`), and `SearchTool::with_runtime_state`
already exists (`crates/rmc-server/src/tools/router.rs:67`). Sharing is real:
two clients (pids 700820, 700884) were both served by pid 700851.

What follows fixes the defects that were proven by probe, in the order that
keeps the diff small.

## Non-goals

- No migration to MCP `2026-07-28` or `rmcp` 3.x. The new revision keeps stdio
  as "one client-launched subprocess", so it does not remove the duplication
  this transport exists to remove. It does bless this design: custom transports
  over a byte stream SHOULD reuse the stdio framing, and the spec names unix
  domain sockets. This repo pins `rmcp` 0.8.1 (`Cargo.lock:7367`); the upgrade
  is a separate plan.
- No change to `rmc-server`, `rmc-engine`, or any tool endpoint.
- No formatting run. Do not call `cargo fmt`.

## Proven defects

| # | Defect | Anchor | Probe result |
|---|---|---|---|
| D1 | The key is the working directory, not the project | `daemon.rs:216` | cwd repo root → pid 701153; cwd `crates/` → pid 701230; two sockets for one repository |
| D2 | The key covers one env var, and the daemon keeps the first client's environment | `daemon.rs:70` | clients with `key-AAA`/concurrency 1 and `key-BBB`/concurrency 16 shared pid 701295, whose `/proc/701295/environ` held `key-AAA` and concurrency 1 |
| D3 | Stdin EOF discards pending responses | `daemon.rs:430-448` | piped `initialize` + `initialized` + `tools/call`: through the daemon **no** responses (`[]`); with `RMC_DAEMON=0` the initialize reply arrived (`[1]`) |
| D4 | `ensure_dir` chmods a directory the daemon does not own | `daemon.rs:203`, called at `:328` and `:463` | `--daemon --socket <dir>/mcp.sock` changed `<dir>` from `0o755` to `0o700` |
| D5 | A killed daemon looks like a clean stop, and its log is erased | `daemon.rs:377` | after `SIGKILL` the client exited **0**; `spawn_daemon` opens the log with `truncate(true)` |
| D6 | `/tmp` fallback directory follows symlinks | `daemon.rs:199-206` | code path; reached only when `XDG_RUNTIME_DIR` is unset |
| D7 | `idle_since` is published after the zero live count | `daemon.rs:514-515` | code path; effect is one early exit and one respawn |
| D8 | `--idle-secs` is dropped in client mode | `daemon.rs:145`, `:381` | the flag is parsed and discarded; `RMC_DAEMON_IDLE_SECS` still reaches the daemon by inheritance |
| D9 | Stale cleanup removes any file at the socket path | `daemon.rs:349` | needs an explicit wrong `--socket` |
| D10 | In client mode `RMC_EMBEDDING_PROFILE` is never validated | `main.rs:69-79` before `:94` | created by the rebase: the client returns from `main` before the profile check, so a typo exits 0 through the daemon and exits 2 with `RMC_DAEMON=0` |

## Phase 1 — Key on the project, not the directory

D1. `workspace_key` hashes `std::env::current_dir()`.

1. Add `fn project_root(start: &Path) -> PathBuf` to `daemon.rs`. Walk up from
   the canonicalized cwd. Take the highest directory that holds a `Cargo.toml`
   with a `[workspace]` table; if none has one, take the highest directory that
   holds any `Cargo.toml`; if none does, keep the cwd.
2. Do not call `cargo locate-project`: a spawn per startup on the client path
   costs more than reading two or three manifests, and it fails outside a Cargo
   project, which must stay usable.
3. Feed `project_root` into `key_from_parts` in place of the cwd. Keep the
   parameter name `cwd` out of the signature; call it `project`.
4. Unit tests, pure, on `key_from_parts` plus one on `project_root` over a temp
   tree: `<root>/Cargo.toml` with `[workspace]`, `<root>/crates/a/Cargo.toml`,
   and a plain subdirectory. All three must resolve to `<root>`.

Acceptance: two clients started from the repository root and from `crates/`
report the same serving pid, and the socket directory holds one `.sock`.

## Phase 2 — Key on every behaviour-changing variable

D2 and part of D10. A hand-kept list cannot hold: `KEYED_ENV` already misses
`RMC_EMBEDDING_PROFILE` from `main`, and `openrouter/config.rs:7-18` defines
ten more.

1. Replace `KEYED_ENV` with a prefix policy: collect every variable whose name
   starts with `RMC_` or `RUST_CODE_MCP_`, drop the daemon's own transport knobs
   (`RMC_DAEMON`, `RMC_DAEMON_DIR`, `RMC_DAEMON_IDLE_SECS`) because they select
   the daemon rather than change its answers, then sort by name.
2. Also key `OPENROUTER_API_KEY`: it has no shared prefix and it decides whose
   account pays.
3. Hash names and values as today. Never log a value: the set includes API keys.
   Log the count and the sorted names only.
4. Keep `key_from_parts` pure and pass the collected pairs in, so the tests stay
   free of `set_var`.
5. Unit tests: a new variable with a keyed prefix changes the key; a transport
   knob does not; ordering of the environment does not.

Acceptance: two clients that differ in `OPENROUTER_API_KEY`,
`RUST_CODE_MCP_OPENROUTER_CONCURRENCY`, or `RMC_EMBEDDING_PROFILE` report
different serving pids. Two clients that differ only in
`RMC_DAEMON_IDLE_SECS` report the same one.

## Phase 3 — Half-close instead of cancelling the reader

D3. `proxy` uses `select!`, so upstream EOF drops the downstream copy with the
responses still in flight.

1. Split the two directions into tasks. On stdin EOF, call
   `to_daemon.shutdown()` — the peer then sees EOF and finishes its reply — and
   keep copying downstream until the socket closes.
2. Keep one guard against the hang the current comment describes: if the
   downstream side is still open a bounded time after upstream EOF, stop. Use a
   named constant with the reason in one line.
3. Return an error when the socket closes while stdin is still open. That is the
   crash case, not a shutdown.

Acceptance: the piped one-shot returns the same response ids through the daemon
as with `RMC_DAEMON=0`, and no session hangs after the host closes stdin.

## Phase 4 — Touch only directories the daemon created

D4 and D6.

1. Split `ensure_dir` into two paths. If the directory exists, do not chmod it:
   check with `symlink_metadata` that it is a directory, not a symlink, and is
   owned by the current uid; refuse otherwise with a message naming the path.
2. If it does not exist, create it and then set `0o700`.
3. Apply the same check in `run_client` (`:328`) and `run_daemon` (`:463`).
4. Refuse a socket directory whose mode grants group or other access when the
   daemon created it earlier, so a later loosening is caught.

Acceptance: `--socket <existing 0755 dir>/x.sock` leaves the directory at
`0o755` and the daemon still binds. A symlinked socket directory is refused with
a named error, and the client falls back in-process.

## Phase 5 — Lifecycle and diagnostics

D5, D7, D8, D9. Four small edits.

1. `daemon.rs:514` — store `idle_since` before publishing the zero count.
2. `daemon.rs:381` — pass `--idle-secs` to the spawned daemon when the client
   parsed one, so `Mode::Client` must carry it. Update `USAGE` in the same edit.
3. `daemon.rs:349` — check `FileTypeExt::is_socket()` before removing.
4. `daemon.rs:377` — open the daemon log with `append(true)` instead of
   `truncate(true)`, and write one header line with the pid and the time, so a
   new daemon does not erase the crash of the last one.

Acceptance: the existing eight `daemon.rs` unit tests still pass, and
`--idle-secs 5` through the normal client path produces a daemon that exits
about five seconds after its last client.

## Phase 6 — Make the profile check reach every mode

D10.

1. Move the profile validation from `main.rs:94-105` to before the mode dispatch
   at `main.rs:43`. It is a pure check plus one `OnceLock` install, so it costs
   nothing on the client path and it makes one input give one outcome.
2. Keep the install where it is if the borrow checker objects; only the
   validation has to move.
3. Leave the startup log line where it is.

Acceptance: `RMC_EMBEDDING_PROFILE=typo` exits 2 with the same message in all
three modes: through the daemon, with `RMC_DAEMON=0`, and on the in-process
fallback.

## Phase 7 — Tests

Add to `crates/rust-code-mcp/tests/test_daemon_shared_runtime.rs` unless a test
needs its own process, and reuse the `Session` helper.

1. **Fallback** (missing today, and it is the claim that makes this safe):
   `RMC_DAEMON_DIR` pointing at an uncreatable path, then assert
   `serving_pid == child.id()`.
2. **One project, two directories**: two sessions with different `current_dir`
   inside this repository report one serving pid.
3. **Configuration splits daemons**: two sessions differing only in
   `OPENROUTER_API_KEY` report different serving pids.
4. **Piped one-shot**: the response ids through the daemon equal the ids with
   `RMC_DAEMON=0`.
5. **Permissions**: a pre-existing `0o755` socket directory keeps its mode.
6. **Profile refusal in client mode**: exit code 2 with a bad
   `RMC_EMBEDDING_PROFILE`, daemon path enabled.

Every test sets `RMC_DAEMON_DIR` to its own temp directory and
`RMC_DAEMON_IDLE_SECS` to a small value, as the existing tests already do, so a
failure cannot leave a daemon holding memory.

## Phase 8 — Documentation

1. `README.md`: the key covers the *project root* and *every* `RMC_*` /
   `RUST_CODE_MCP_*` variable plus `OPENROUTER_API_KEY`. The current text
   promises both and delivers neither.
2. `README.md`: name the log file, and say that it is appended, not replaced.
3. `daemon.rs` module docs: replace the `KEYED_ENV` maintenance warning with the
   prefix policy, since there is no list left to extend.
4. `USAGE`: state that `--client` overrides `RMC_DAEMON=0`, and that
   `--idle-secs` now reaches a client-spawned daemon.
5. `.docs/configure-models-guide.md`: one line that a shared daemon is keyed by
   these variables, so two differently configured sessions do not share one
   server.

## Verification

Run everything through the dev shell, one crate or one target at a time, with a
timeout:

```
nix develop ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --bin rust-code-mcp
nix develop ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test test_daemon_shared_runtime
nix develop ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test test_default_profile_env
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib
```

Queue them in pueue as sequential tasks. One caveat from this session: a queued
task once sat in `Running` with an empty log and no `cargo`, `rustc`, or `nix`
child process. If that happens, kill the task and run the command directly; the
three-target run takes about four minutes with a warm `target/`.

Baseline to compare against, measured on `976f36c1` before the fixes:

- daemon unit tests 8 passed; shared-runtime tests 3 passed.
- `rmc-server --lib` 98 passed, 3 ignored.
- `test_mcp_stdio_transport.rs:71` is `#[ignore]` (it needs an embedding model),
  so the `RMC_DAEMON=0` line the daemon commit adds there is not exercised by a
  normal run. Run it once with `-- --ignored` after Phase 6.

## Order and commits

One commit per phase, on top of `976f36c1`, then squash Phases 1 to 6 into the
daemon commit if the branch is to be sent upstream as one change. Phases 1 to 4
are the merge blockers; Phases 5 to 8 are cheap and should not be deferred,
because each one is a few lines.
