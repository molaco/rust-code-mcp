# Where this vendored copy comes from and what in it is ours

**Upstream:** https://github.com/Anush008/fastembed-rs
**Base:** branch `enomado:fixed-batch-shape`, commit `6bcef63` = upstream
`045d591` (tag 6.0.0) + our PR [#279](https://github.com/Anush008/fastembed-rs/pull/279).
Vendored as a FLAT copy (git archive, without `.git`), wired in via
`[patch.crates-io]` from the root `Cargo.toml`.

A copy rather than a submodule, on purpose: a submodule solves none of our problems,
but adds a ritual to every clone. The price of that decision: what we diverged from
cannot be derived from git, which is why this file exists.

## Our patches on top of the base

1. **Fixed batch shape** (`FixedBatchShape`, `real_rows`): already IN THE BASE,
   because the base is the PR branch itself. No need to apply it separately; once the PR is merged
   upstream, this line goes away together with the need for the branch.
2. **ONNX Runtime profiling.** A `profiling_file` field in `InitOptions` and
   `InitOptionsWithLength` + a `with_profiling` switch; the path goes into
   `SessionBuilder::with_profiling` inside `init_session_builder`
   (`src/common.rs`) BEFORE `commit_*`: after the commit the switch is gone. All five
   session-holding types (`TextEmbedding`, `TextRerank`, `SparseTextEmbedding`,
   `Bgem3Embedding`, `ImageEmbedding`) gained
   `end_profiling() -> Result<String>`: ORT appends a timestamp to the prefix,
   and the full file name is known only from there.
   This is needed for the oracle that the graph really runs on the EP (`rmc-engine`
   `embeddings/ep_census.rs`): the provider name of each node exists ONLY in the
   profile. Not sent upstream.
3. **Perf patch to `qwen3.rs`** (Candle): a fast `rms_norm` path for contiguous
   input and `softmax_last_dim` instead of the generic `softmax`. Came from the
   GPU optimization work (phase 8), not sent upstream.

## How to bump the version further

`git archive` of the desired branch/tag over an empty `vendor/fastembed`, then
re-apply patches 2 and 3 (patch 1 only while the PR is not merged) and update this file.
Move the `ort` pin in the root `Cargo.toml` IN SYNC with the pin inside the vendored crate:
two copies of ort in the graph will not build.
