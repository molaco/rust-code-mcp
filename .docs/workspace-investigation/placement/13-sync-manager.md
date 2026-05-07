# 13 — SyncManager placement

## Decision

**Split it.** Move the worker (`sync_directory`, `handle_sync_all`, the
`IncrementalIndexer` call site) into `code-search` as a `SyncWorker` /
`IncrementalSyncJob` value. Keep the lifecycle shell (the spawn site, the
`tracked_dirs` set, the public `track_directory` / `untrack_directory` /
`sync_now` API surface, and the `Arc<SyncManager>` that handlers register
against) in the `server` crate. The server orchestrates *when* and *what to
track*; `code-search` owns *how a sync runs*. The two meet at a single
trait — call it `SyncJob` — that takes `&Path` and returns
`Result<SyncStats>`.

## Rationale

`SyncManager` today fuses three responsibilities (see `src/mcp/sync.rs:20`,
`.docs/architecture/mcp.md`):

1. A registration set (`Arc<RwLock<HashSet<PathBuf>>>`) shared with `tools`
   so `query_tools::search` and `index_tool::index_codebase` can opt new
   workspaces into the watch loop (`tools.md` §Data flow #5).
2. A periodic loop driver (`tokio::time::interval`, warm-up sleep, error
   isolation per directory).
3. The actual work — building `ProjectPaths`, instantiating
   `IncrementalIndexer`, calling `index_with_change_detection` — which is a
   straight wrapper around the `indexing` pipeline (`indexing.md` §Data
   flow #2).

(1) and (2) belong with the transport-adjacent server: the registration
set is named in JSON-RPC requests, its lifetime is the MCP session, and
the spawn point is `main.rs`. (3) belongs with the ingest pipeline,
because every change to chunk format, embedding cadence, or
`IncrementalIndexer` constructor signature ripples into it. Putting the
whole struct in `code-search` would force that crate to import the MCP
session lifecycle; putting it in `server` would force `server` to depend
transitively on `IncrementalIndexer`, `ProjectPaths`, and `EMBEDDING_DIM`
just to spawn a timer. A "background tasks" utility crate is rejected:
there is no second consumer today and no shared abstraction beyond
`tokio::spawn(loop { ... })`, which is not worth a crate.

## Concrete shape

`code-search`:

```rust
pub trait SyncJob: Send + Sync + 'static {
    fn run<'a>(&'a self, dir: &'a Path)
        -> BoxFuture<'a, Result<SyncStats>>;
}
pub struct IncrementalSyncJob { /* embedding_dim, etc. */ }
impl SyncJob for IncrementalSyncJob { /* current sync_directory body */ }
```

`server`:

```rust
pub struct SyncManager<J: SyncJob> {
    tracked_dirs: Arc<RwLock<HashSet<PathBuf>>>,
    job: Arc<J>,
    interval: Duration,
    shutdown: CancellationToken,
}
```

`tools` continues to depend only on `server::SyncManager` for
`track_directory` — its registration contract is unchanged.

## Shutdown policy

Today: none — `run()` is an unbounded `loop` with no exit branch. Adopt
`tokio_util::sync::CancellationToken` owned by `SyncManager`. `run()`
becomes a `tokio::select!` between `interval.tick()` and
`shutdown.cancelled()`. `main.rs` holds the token, signals on SIGINT /
stdio EOF, then `join`s the spawned handle with a 30 s timeout. An
in-flight `sync_directory` is *not* interrupted (Tantivy/LanceDB writes
must finish cleanly); only the loop exits. If the timeout elapses, log
and abort the task — the next process start re-reads from disk anyway.

## Future graph background task

A periodic hypergraph rebuild (e.g. nightly `build_and_persist`) would
get its own `SyncJob` implementation in `code-graph` (or a graph crate),
*not* a second manager. `SyncManager` becomes generic over `J: SyncJob`
or holds `Vec<Arc<dyn SyncJob>>`; the registration set stays singular
because tracked directories are workspace-level, not job-level. Crates
bifurcate by *work*; the orchestrator does not.

## Top 3 risks

1. **Trait churn.** `SyncJob` must accommodate future inputs (force
   flag, partial-path scope) without breaking `code-search`'s impl.
   Mitigate by passing a `SyncRequest` struct, not bare `&Path`.
2. **Shutdown deadlock.** A long `index_with_change_detection` holding
   the Tantivy writer lock during shutdown can exceed the 30 s budget.
   Mitigate by checkpointing commits per batch (already true) and
   logging-but-aborting on timeout.
3. **Registration race on session restart.** `tracked_dirs` is in-memory;
   crashes lose the set. Future work: persist tracked roots to the XDG
   data dir on every `track_directory` write so the next session
   re-hydrates without waiting for a `search` call to re-register.
