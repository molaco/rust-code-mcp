# Phase 0 + Phase 1 — Actual Implementation Plan

Code-explicit, step-by-step, phase-by-phase plan synthesized from 10 parallel
subagent designs against the source plan at
`.plans/phase-1-implementation.md` and the existing crate surface in
`.skeleton/*.rs`.

Read **Section Z first** for the project-wide map (crate inventory, milestone
Gantt, file-tree diff, cross-slice integration tests, issue register, open
decisions, risk-reduction order). Sections A–J are the vertical slices in
critical-path order — each is self-contained and can be implemented from its
own text without further design rounds.

Devshell: every shell command runs under `nix develop ../nix-devshells#cuda-code --command <cmd>`.

---

## Table of contents

- [Errata (post-review revisions) — canonical resolutions](#errata-post-review-revisions--canonical-resolutions) — **READ FIRST**
- [Canonical Reconciliation — Single Source of Truth](#canonical-reconciliation--single-source-of-truth) — **HIGHEST PRECEDENCE**
- [Section Z — Integration, Milestones, Crate Inventory](#section-z--integration-milestones-crate-inventory)
- [Section A — P0.1 Determinism + P0.4 Benchmark Pool](#section-a--p01-determinism--p04-benchmark-pool)
- [Section B — M0 Contracts (D1–D4) + Feasibility Spikes](#section-b--m0-contracts-d1d4--feasibility-spikes)
- [Section C — P0.2 Warm-Host Incremental Writer + P0.3 jj Rollback](#section-c--p02-warm-host-incremental-writer--p03-jj-rollback)
- [Section D — P1.1 Read View / Navigate](#section-d--p11-read-view--navigate)
- [Section E — P1.2 Description Index](#section-e--p12-description-index)
- [Section F — P1.3 Analyze / Vision Layer](#section-f--p13-analyze--vision-layer)
- [Section G — P1.5a modify_body + P1.5b move / delete](#section-g--p15a-modify_body--p15b-move--delete)
- [Section H — P1.5c modify_signature + P1.5d extract/inline + P1.5e module ops](#section-h--p15c-modify_signature--p15d-extractinline--p15e-module-ops)
- [Section I — P1.4 Counterfactual Simulator + P1.6 Write-Time Gates](#section-i--p14-counterfactual-simulator--p16-write-time-gates)
- [Section J — P1.7 Commit/Reward + P1.8 Episode Runner](#section-j--p17-commitreward--p18-episode-runner)

---

