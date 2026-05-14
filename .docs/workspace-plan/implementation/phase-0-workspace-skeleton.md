# Phase 0 — Workspace Skeleton & Policy Enforcement

**Authoritative reference:** `.docs/workspace-plan/DECISIONS.md`. If anything below conflicts with DECISIONS.md, DECISIONS.md wins.

**Goal:** convert the single-crate `file-search-mcp` repo into a virtual workspace where the legacy crate is one member and the eight future crates exist as compiling placeholders. Pin toolchain, wire `cargo-deny`, workspace lints, `cargo public-api`, and a custom `forbidden_dependency_check` script. **No runtime code moves in Phase 0.** `cargo build` still produces the legacy binary; the smoke checklist still passes.

Phase 0 is reversible by deleting the eight placeholder crate directories, restoring the original `Cargo.toml`, and removing `deny.toml` / xtask. No production state changes.

---

## Step 1 — Decide repo layout (in-place wrap)

**What to do.** Keep the existing repo. Move the current `Cargo.toml` + `src/` + `tests/` + `examples/` + `benches/` into `crates/file-search-mcp-legacy/` unchanged, then write a new virtual manifest at the repo root. This preserves git history (use `git mv`), keeps `Cargo.lock`, and lets every later phase carve a real crate out of the legacy member without a second relocation.

Justification (vs. fresh repo): the investigation under `.docs/workspace-investigation/` is wired to current paths; tests, fixtures, snapshots (`snapshots/`), and `storage/` layouts are coupled to the binary. A fresh repo would force a Phase 0 that's also a data migration. In-place wrap keeps Phase 0 a pure manifest operation.

**Files touched.**
- `git mv Cargo.toml crates/file-search-mcp-legacy/Cargo.toml`
- `git mv src crates/file-search-mcp-legacy/src`
- `git mv tests crates/file-search-mcp-legacy/tests`
- `git mv examples crates/file-search-mcp-legacy/examples`
- `git mv benches crates/file-search-mcp-legacy/benches`
- Leave `Cargo.lock`, `flake.nix`, `rust-toolchain.toml`, `snapshots/`, `storage/`, `assets/`, `README.md` at the repo root.
- Edit `crates/file-search-mcp-legacy/Cargo.toml`: add `[package] publish = false`, leave deps as inline literals for now (Step 2 hoists them).

**Acceptance.** `nix develop ../nix-devshells#code --command cargo build -p file-search-mcp` succeeds from the repo root after Step 2 lands the virtual manifest. The binary path becomes `target/debug/file-search-mcp`.

**Reversal.** `git mv` everything back; delete `crates/`. Single commit, single revert.

---

## Step 2 — Virtual workspace manifest

**What to do.** Write a virtual `Cargo.toml` at the repo root. Hoist every external dependency from the legacy crate into `[workspace.dependencies]`. Declare the eight future crates plus `xtask` plus the legacy crate as members. Wire workspace lints.

**Files touched.** `/Cargo.toml` (new, at repo root):

```toml
[workspace]
resolver = "3"
members = [
    "crates/file-search-mcp-legacy",
    "crates/rcm-paths",
    "crates/rcm-ra-syntax",
    "crates/rcm-ra-host",
    "crates/rcm-embedding",
    "crates/rcm-search",
    "crates/rcm-graph",
    "crates/rcm-ide",
    "crates/rcm-server",
    "crates/xtask",
]

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"
repository = "https://github.com/molaco/rust-code-mcp-final"
rust-version = "1.95"

[workspace.dependencies]
rmcp           = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main", features = ["server", "transport-io"] }
tokio          = { version = "1", features = ["macros", "rt", "rt-multi-thread", "io-std", "signal"] }
tantivy        = "0.22.0"
serde          = { version = "1.0.219", features = ["derive"] }
serde_bytes    = "0.11"
serde_json     = "1.0"
bincode        = "1.3"
uuid           = { version = "1.10", features = ["v4", "serde"] }
tracing        = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "std", "fmt"] }
notify         = "6"
sled           = "0.34"
sha2           = "0.10"
directories    = "5"
ra_ap_syntax       = "0.0.330"
ra_ap_ide          = "0.0.330"
ra_ap_ide_db       = "0.0.330"
"ra_ap_load-cargo" = "0.0.330"
ra_ap_project_model = "0.0.330"
ra_ap_vfs          = "0.0.330"
ra_ap_paths        = "0.0.330"
ra_ap_hir          = "0.0.330"
ra_ap_hir_def      = "0.0.330"
ra_ap_base_db      = "0.0.330"
heed           = "0.22.1"
text-splitter  = "0.13"
fastembed      = { git = "https://github.com/Anush008/fastembed-rs" }
ort            = { version = "=2.0.0-rc.10", features = ["cuda", "download-binaries", "ndarray", "std"] }
async-trait    = "0.1"
thiserror      = "1"
futures        = "0.3"
lancedb        = "0.15"
arrow-array    = "53"
arrow-schema   = "53"
walkdir        = "2"
anyhow         = "1"
regex          = "1"
glob           = "0.3"
rs_merkle      = "1.4"
sysinfo        = "0.30"
rayon          = "1.10"
num_cpus       = "1.16"
tempfile       = "3"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
unreachable_pub        = "warn"
rust_2024_compatibility = "warn"

[workspace.lints.clippy]
disallowed_methods = "warn"
pedantic           = { level = "warn", priority = -1 }
```

The legacy crate's `Cargo.toml` keeps its current deps inline for Phase 0 (rewriting it to use `dep.workspace = true` is Phase 1 work). New placeholder crates use `dep = { workspace = true }`.

**Acceptance.** `nix develop ../nix-devshells#code --command cargo metadata --format-version 1 --no-deps` lists 10 workspace members. `cargo build --workspace` is green.

**Reversal.** Delete the root `Cargo.toml`; restore the legacy `Cargo.toml` to the root via `git mv`.

---

## Step 3 — Pin toolchain

**What to do.** The current `rust-toolchain.toml` says `channel = "nightly"`, which contradicts DECISIONS §15 ("`rust-toolchain.toml` pinned"). Pin to a released stable that supports edition 2024 and `[workspace.lints]` (both stabilized by 1.85; current pin: 1.95; resolver = "3" requires 1.84).

**Files touched.** `/rust-toolchain.toml`:

```toml
[toolchain]
channel    = "1.95.0"
components = ["rustfmt", "clippy", "rust-src"]
profile    = "minimal"
```

`rust-src` is needed by `cargo public-api` (it builds against `rustdoc` JSON, which requires the source component on stable). If the nix devshell pins a different toolchain, sync the `flake.nix` rust attribute to `1.95.0` in the same commit.

**Acceptance.** `nix develop ../nix-devshells#code --command rustc --version` reports `1.95.0`. `nix develop ../nix-devshells#code --command cargo build --workspace` passes.

**Reversal.** Restore previous `rust-toolchain.toml`.

---

## Step 4 — Empty placeholder crates

**What to do.** For each of the eight future crates create a directory with a minimal `Cargo.toml` and a `src/lib.rs` that contains crate-root docs and the per-crate lint attributes mandated by DECISIONS §15. Code is inert — just enough to compile and exercise lints/policy.

Per DECISIONS §15, `#![warn(missing_docs)]` goes on `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-paths`, `rcm-embedding` only. **Not** on `rcm-server`, `rcm-ra-syntax`, `rcm-ra-host`, `xtask`.

**Files touched.** Eight pairs. Template for a strict-tier crate (`crates/rcm-paths/Cargo.toml`):

```toml
[package]
name        = "rcm-paths"
version     = "0.0.0"
edition.workspace      = true
license.workspace      = true
repository.workspace   = true
rust-version.workspace = true
publish     = false

[lints]
workspace = true
```

`crates/rcm-paths/src/lib.rs`:

```rust
//! `rcm-paths` — storage path resolution (workspace fingerprint hashing).
//!
//! Phase 0 placeholder. Real implementation lands in Phase 1.
//! See `.docs/workspace-plan/DECISIONS.md` for the frozen contract.
#![warn(missing_docs)]
#![forbid(unsafe_code)]
```

The exempt-tier infra leaves (`rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`) record their API-leak exemption in the crate-root doc:

```rust
//! `rcm-ra-syntax` — narrow re-export of `ra_ap_syntax`.
//!
//! **API-leak exemption (documented):** this crate exists *to* expose
//! `ra_ap_syntax` types. See DECISIONS.md §"Two-tier API leak rule".
//!
//! Phase 0 placeholder.
#![forbid(unsafe_code)]
```

`rcm-server` (binary + lib) has `src/lib.rs` and `src/main.rs`:

```rust
// crates/rcm-server/src/main.rs
fn main() {
    // Phase 0 placeholder. Real composition root lands in Phase 6.
}
```

`rcm-server`'s `Cargo.toml` adds `[[bin]] name = "rcm-server"` and `path = "src/main.rs"`. Its `[lints]` inherits workspace; **no** `missing_docs`.

**Acceptance.** `cargo build --workspace` compiles all 10 members. `cargo clippy --workspace -- -D warnings` is clean. `cargo doc --workspace --no-deps` builds without `missing_docs` warnings on the five strict crates.

**Reversal.** `rm -rf crates/rcm-*` (legacy crate untouched); drop the corresponding `members = […]` entries.

---

## Step 5 — `deny.toml`

**What to do.** Write `deny.toml` at the repo root following DECISIONS §15 (advisories, license allow-list, duplicate detection, ban list). Match `cargo-deny` 0.16 schema.

**Files touched.** `/deny.toml`:

```toml
[advisories]
version             = 2
db-path             = "~/.cargo/advisory-db"
db-urls             = ["https://github.com/rustsec/advisory-db"]
yanked              = "deny"
ignore              = []

[licenses]
version             = 2
allow               = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
    "Unicode-3.0",
    "Zlib",
    "MPL-2.0",
    "CC0-1.0",
]
confidence-threshold = 0.8

[bans]
multiple-versions    = "warn"
wildcards            = "deny"
highlight            = "all"
deny                 = [
    # rcm-server is the only crate allowed to depend on rmcp;
    # full enforcement lives in forbidden_dependency_check (Step 6).
    # Listed here for visibility only.
]
skip-tree            = []

[sources]
unknown-registry = "deny"
unknown-git      = "warn"
allow-git        = [
    "https://github.com/modelcontextprotocol/rust-sdk",
    "https://github.com/Anush008/fastembed-rs",
]
```

`multiple-versions = "warn"` (not `"deny"`) because the legacy crate transitively pulls `arrow` 53 + `arrow-array` siblings via `lancedb` and several `tokio-*` minor pairs via `rmcp`; a hard deny here blocks Phase 0 on issues unrelated to architecture.

**Acceptance.** `nix develop ../nix-devshells#code --command cargo deny check` exits 0 (warnings allowed; `errors == 0`). RustSec scan completes.

**Reversal.** Delete `deny.toml`.

---

## Step 6 — CI policy gates

**What to do.** Add a single GitHub Actions workflow that runs the four gates on every push and PR. Add the `forbidden_dependency_check` script as a real, runnable Rust binary under `xtask` (Step 7) so it can be invoked locally too.

**Files touched.** `/.github/workflows/ci.yml`:

```yaml
name: ci
on:
  push:    { branches: [main] }
  pull_request:
jobs:
  policy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.95.0
        with: { components: "rustfmt, clippy, rust-src" }
      - uses: Swatinem/rust-cache@v2
      - name: Build
        run: cargo build --workspace --locked
      - name: Clippy (deny warnings, lib+bins only — examples/benches retired separately before Phase 1)
        run: cargo clippy --workspace --lib --bins -- -D warnings
      - name: cargo-deny
        uses: EmbarkStudios/cargo-deny-action@v2
        with: { command: check }
      - name: Install cargo public-api
        run: cargo install --locked cargo-public-api@0.38
      - name: Public API leak check (strict tier)
        run: |
          for c in rcm-paths rcm-search rcm-graph rcm-ide; do
            cargo public-api --simplified -p "$c" \
              --diff-git-checkouts main HEAD \
              --deny added \
              --deny changed \
              || true   # Phase 0: placeholder crates have no public surface, baseline is empty.
          done
      - name: Forbidden dependency edges
        run: cargo run -p xtask -- forbidden-deps
```

**Forbidden dependency script** (lives in `crates/xtask/src/forbidden_deps.rs`, invoked above):

```rust
//! Reject any cross-crate edge not listed in the allow-table from DECISIONS.md.

use std::process::{Command, ExitCode};
use std::collections::{BTreeMap, BTreeSet};
use serde::Deserialize;

#[derive(Deserialize)]
struct Metadata { packages: Vec<Pkg>, workspace_members: Vec<String> }
#[derive(Deserialize)]
struct Pkg { name: String, id: String, dependencies: Vec<Dep> }
#[derive(Deserialize)]
struct Dep { name: String, kind: Option<String> }

const ALLOWED: &[(&str, &[&str])] = &[
    ("rcm-server",  &["rcm-search","rcm-graph","rcm-ide","rcm-paths","rcm-embedding"]),
    ("rcm-search",  &["rcm-ra-syntax","rcm-embedding","rcm-paths"]),
    ("rcm-graph",   &["rcm-ra-host","rcm-embedding","rcm-paths"]),
    ("rcm-ide",     &["rcm-ra-host","rcm-paths"]),
    ("rcm-ra-host", &["rcm-ra-syntax"]),
    ("rcm-paths",   &[]),
    ("rcm-ra-syntax", &[]),
    ("rcm-embedding", &[]),
    // legacy crate is unconstrained for the duration of the migration;
    // it shrinks crate-by-crate in later phases.
    ("file-search-mcp-legacy", &["*"]),
];

pub fn run() -> ExitCode {
    let out = Command::new("cargo").args(["metadata","--format-version","1","--no-deps"])
        .output().expect("cargo metadata");
    let md: Metadata = serde_json::from_slice(&out.stdout).expect("parse metadata");
    let allow: BTreeMap<&str, BTreeSet<&str>> = ALLOWED.iter()
        .map(|(k, v)| (*k, v.iter().copied().collect())).collect();
    let mut violations = Vec::new();
    let ws_names: BTreeSet<&str> = md.packages.iter()
        .filter(|p| md.workspace_members.iter().any(|m| m.contains(&p.id)))
        .map(|p| p.name.as_str()).collect();
    for p in &md.packages {
        if p.name == "xtask" { continue; }                 // excluded per DECISIONS
        let Some(allowed) = allow.get(p.name.as_str()) else { continue; };
        if allowed.contains("*") { continue; }
        for d in &p.dependencies {
            if !ws_names.contains(d.name.as_str()) { continue; } // external deps are out of scope
            if d.name == p.name { continue; }
            if !allowed.contains(d.name.as_str()) {
                violations.push(format!("{} -> {}", p.name, d.name));
            }
        }
    }
    if violations.is_empty() { return ExitCode::SUCCESS; }
    eprintln!("forbidden cross-crate edges:");
    for v in &violations { eprintln!("  {v}"); }
    ExitCode::FAILURE
}
```

**Acceptance.** All four gate commands pass locally:
- `nix develop ../nix-devshells#code --command cargo build --workspace --locked`
- `nix develop ../nix-devshells#code --command cargo clippy --workspace --lib --bins -- -D warnings` (NOT `--all-targets` — legacy `examples/`/`benches/` are retired separately before Phase 1; widening to `--all-targets` is a Phase 1 acceptance criterion, not a Phase 0 one)
- `nix develop ../nix-devshells#code --command cargo deny check`
- `nix develop ../nix-devshells#code --command cargo run -p xtask -- forbidden-deps`

CI runs the same four on PR.

**Reversal.** Delete `.github/workflows/ci.yml` and the xtask command.

---

## Step 7 — Wire xtask

**What to do.** Create a minimal binary crate `crates/xtask/` that dispatches subcommands. Phase 0 gives it two: `forbidden-deps` (Step 6) and `policy` (a meta-command running `cargo deny check && forbidden-deps`).

**Files touched.** `crates/xtask/Cargo.toml`:

```toml
[package]
name        = "xtask"
version     = "0.0.0"
edition.workspace      = true
license.workspace      = true
publish     = false

[dependencies]
serde      = { workspace = true }
serde_json = { workspace = true }
anyhow     = { workspace = true }

[lints]
workspace = true
```

`crates/xtask/src/main.rs`:

```rust
mod forbidden_deps;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        "forbidden-deps" => forbidden_deps::run(),
        "policy" => {
            let s1 = Command::new("cargo").args(["deny","check"]).status().expect("cargo deny");
            let s2 = forbidden_deps::run();
            if s1.success() && matches!(s2, ExitCode::SUCCESS) { ExitCode::SUCCESS }
            else { ExitCode::FAILURE }
        }
        _ => { eprintln!("usage: xtask {{forbidden-deps|policy}}"); ExitCode::FAILURE }
    }
}
```

**Why xtask is excluded from the policy itself.** xtask is a workspace tool, not runtime; per DECISIONS, it's allowed to depend on any workspace crate (e.g., it'll later import `rcm-search` to drive the storage-v2 migration in Phase 7). Including it in `forbidden_dependency_check` would require listing every crate as a permitted edge, which would defeat the check. Documented at the top of `crates/xtask/src/main.rs`.

**Acceptance.** `cargo run -p xtask -- policy` runs both gates locally and exits 0.

**Reversal.** Delete `crates/xtask/`; remove from workspace `members`.

---

## Step 8 — Smoke checklist preservation

**What to do.** Before merging Phase 0, run every smoke from DECISIONS §"Smoke checklist" against the legacy binary, now invoked through the workspace. The binary path is `target/debug/file-search-mcp` (unchanged); only the `cargo` invocations change.

**Smoke commands.** From the repo root, all under the devshell:

```bash
nix develop ../nix-devshells#code --command bash -c '
  cargo build -p file-search-mcp --release &&
  ./target/release/file-search-mcp &  echo $! > /tmp/fsmcp.pid
'
```

Then issue the canonical MCP calls (`index_codebase`, `search`, `find_definition`, `find_references`, `build_hypergraph`, `who_calls`, `who_imports`, `workspace_stats`, `get_dependencies`, `get_call_graph`, `analyze_complexity`, `semantic_overlaps`, `similar_to_item`, `clear_cache` followed by `search`) against a fixture workspace, e.g. `tests/fixtures/sample_workspace/`. Each must return a non-error JSON-RPC response.

The existing integration tests under `crates/file-search-mcp-legacy/tests/` already cover most of this; gate the phase on:

```bash
nix develop ../nix-devshells#code --command cargo test -p file-search-mcp --tests
```

(Per project memory, only run when explicitly verifying the gate; `cargo check --lib` is the routine fast path.)

**Acceptance.** Every smoke tool returns success against the fixture; integration test suite green.

**Reversal.** Skipping smokes is not a reversal — it's a phase failure. If smokes fail, revert Steps 1-7 and re-plan.

---

## Step 9 — Documentation

**What to do.** Add `architecture.md` at the repo root pointing readers to the migration plan.

**Files touched.** `/architecture.md`:

```markdown
# Architecture

This workspace is **mid-migration** from a single crate to an 8-crate workspace.

- Migration plan: [`.docs/workspace-plan/DECISIONS.md`](.docs/workspace-plan/DECISIONS.md)
- Phase docs: [`.docs/workspace-plan/implementation/`](.docs/workspace-plan/implementation/)
- Pre-migration architecture: [`.docs/ARCHITECTURE.md`](.docs/ARCHITECTURE.md)

The legacy implementation lives in `crates/file-search-mcp-legacy/`. Real
crates land one at a time per the phase plan; until they do, the placeholder
`rcm-*` crates are intentionally empty.
```

**Acceptance.** File exists and links resolve.

**Reversal.** `git rm architecture.md`.

---

## Acceptance gate for Phase 0 completion

Phase 1 may not start until **all** of the following hold on `main`:

- [ ] `cargo build --workspace --locked` is green.
- [ ] `cargo clippy --workspace --lib --bins -- -D warnings` is clean. (NOT `--all-targets` — the legacy crate ships ~10 stale `examples/` / `benches/` that pre-date the workspace migration. Phase 0 narrows the gate to library + binary targets; a separate Step "retire stale examples" runs after Phase 0 to clean the example tree before Phase 1 widens the gate.)
- [ ] `cargo deny check` exits 0 (warnings permitted; errors zero).
- [ ] `cargo run -p xtask -- policy` exits 0.
- [ ] `cargo public-api -p rcm-paths` (and the other strict-tier placeholders) reports an empty public surface — establishes the baseline for Phase 1 diffs.
- [ ] `rust-toolchain.toml` pins `1.95.0` with `rustfmt`, `clippy`, `rust-src`.
- [ ] `deny.toml` exists at repo root with the schema in Step 5.
- [ ] All ten workspace members compile, including the legacy crate at its new path.
- [ ] Every smoke MCP tool from DECISIONS §"Smoke checklist" passes against a fixture workspace.
- [ ] `architecture.md` at repo root linking to DECISIONS.md.
- [ ] `.github/workflows/ci.yml` runs the four gates on PRs.

Phase 0 is reversible by `git revert`-ing the Phase 0 merge commit: deleting the eight placeholder crates, the root `Cargo.toml`, `deny.toml`, the workflow, and `crates/xtask/`, and `git mv`-ing the legacy crate back to the repo root restores the pre-Phase-0 state byte-for-byte. No data, snapshots, or storage layouts are touched in Phase 0.
