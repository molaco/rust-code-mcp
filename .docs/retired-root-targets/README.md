# Retired Root Targets

Phase 12 converted the repository root into a virtual Cargo workspace and
removed the temporary `file-search-mcp` compatibility crate. The files under
this directory are the old root-only manual examples and probes that are no
longer Cargo targets.

Active integration tests now live under the workspace crates that own the APIs
they exercise.
