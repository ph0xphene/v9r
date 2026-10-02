# v9r Debloat Phase 1 Report v0

*Phase 1 of V9R_DEBLOAT_PLAN_V0, carried out per
V9R_DEBLOAT_CUT_MAP_V0 under the rules of the Phase 1 request.
Phase 2 was not attempted. No history was rewritten, nothing was
committed, and nothing was pushed. All measurements below were taken on
2026-10-02 with `cargo 1.98.0` on one Linux host.*

## 0. Summary

| | Before (`da6d694`) | After |
|---|---|---|
| workspace suite | **315 passed, 0 failed, 6 ignored** | **127 passed, 0 failed, 4 ignored** |
| `kernel.rs` | sha256 `85badb66…6177f` | **unchanged** (`git diff` empty, same hash; guard passes) |
| demo | ALLOW, DENY, BLOCKED, DENY, REFUSED | **same five verdicts**, now asserted by `cargo test` |
| State vs Causality | every measured field identical between worlds | **reproduced**: every measured field identical (§6) |
| `v9r-core` modules | 35 | 13 |
| `v9r-core` library code (lines before `#[cfg(test)]`) | 15,370 | 5,079 |
| Rust/Python lines under `crates/` | 35,237 | 13,609 |
| workspace crates | 6 | 5 |
| `v9r-core` dependencies | 16 (+`libc`) | 5 (+`libc`); `tokio`, `uuid` moved to dev |
| tracked build output | 5,318 files, ~1.04 GB | 101 files, ~16.6 MB (§8) |

The archive point is the local tag **`v9r-archive-v0`** at `da6d694`.
Every removed file is there.

## 1. Removed

### 1.1 Modules (`crates/v9r-core/src`)

| Group | Modules | Why they could go |
|---|---|---|
| `fs_provider` chain | `fs_provider`, `effect`, `facts`, `vfs`, `execution`, `trace`, `task`, `bundle`, `context`, `trusted`, `manifest`, `state` | `fs_provider` was the claim's only edge into these. Their imports form one cycle, so they could only go together. Its one claim-relevant fact (`entries`) is replaced (§3) |
| effect product layers | `policy`, `guarded` (`FsDomain`), `adapter` (LLM HTTP client) | `guarded`/`policy` were the first effect domain, over `effect`/`facts`. `adapter` only served the CLI and imports the chain |
| git | `git`, `git_provider`, `git_guard` (`GitDomain`) | the git evidence domain; not used by the demo or the claim |
| capability | `capability`, `delegation` (+`tests.rs`), `authority` (+`tests.rs`), `object_identity`, `world_probe.py` | capability and delegation experiments |

Removed from **kept** files (all git- or chain-only):

- **`verifiers.rs`**: `GitObjects`, `FsContent` (read `fs_provider`'s
  `fs_listing`), `ContentEquality`, and SHA-1 object ids. The
  content-addressed helpers that `snapshot.rs` uses remain.
- **`snapshot.rs`**: `SnapshotCommitEquality` (snapshot ↔ git commit).
- **`content.rs`**: `from_fs_state`. `ContentHash` moved in from the
  deleted `state.rs`.
- **`runtime.rs`**: `Runtime::absorb`, a crate-private method whose only
  caller was `guarded.rs`. The phase left it dead, so it went. No other
  runtime change.

### 1.2 Crates and examples

| Removed | Why |
|---|---|
| `crates/v9r-cli` | uses `adapter`, `bundle`, `execution`, `manifest`, `task`, `trace`, `trusted`, `vfs`; cannot build without the chain |
| `examples/task-runtime` (incl. 5,217 tracked build files, ~1.02 GB) | uses `execution`, `manifest`, `task`, `trace`, `vfs` |

### 1.3 Dependencies

- **From `v9r-core`:** `anyhow`, `async-trait`, `bincode`, `chrono`,
  `reqwest`, `ring`, `sha1`, `tracing` (no remaining users). `tokio`
  and `uuid` are now dev-dependencies (tests only).
- **From `[workspace.dependencies]`:** `chrono`, `bincode`,
  `tracing-subscriber`, `rustyline`, `colored`, `ring`, `sha1` (no crate
  uses them).
- **`Cargo.lock`:** −249 lines.

### 1.4 Test suites

| Removed suite | Tests | Its claim-relevant cases now live in |
|---|---|---|
| `tests/graph.rs` | 12 | `claim_guarantees.rs` (11 ported) |
| `tests/temporal.rs` | 14 | `claim_guarantees.rs` (8 ported); "external write inside the scope is indistinguishable" is State vs Causality itself |
| `tests/verifiable.rs` | 12 | `claim_guarantees.rs` (3 ported) |
| `tests/provenance.rs` | 7 | `claim_guarantees.rs` (6 ported) |
| `tests/effects.rs`, `guarded.rs`, `perf.rs` | 41 (+2 ignored) | none: legacy fs effect domain |
| `tests/git.rs` | 20 | none: git domain |
| `tests/capability_manifest.rs`, `content_identity.rs`, `object_identity.rs`, `snapshot_boundary.rs` | 7 | none: capability experiments |
| unit tests of the removed modules | 101 | the kernel guard moved to `tests/kernel_guard.rs`; the rest are domain or product tests |
| doctests in `effect.rs`, `guarded.rs` | 7 | none |

Several ported tests cover more than one original, so the per-suite
counts overlap. The mapping is in each test's `// Ports …` comment. The
cases not ported are domain-specific: git refs, commits and SHA-1
mirrors; `FsContent` and `ContentEquality` definitions; the legacy fs
domain's checkpoints, rollback and traces; capability worlds. Their
results remain in their reports and at the tag.

## 2. Kept

| Module | Lines (code) | Why |
|---|---|---|
| `kernel` | 604 | the claim. **Unchanged** |
| `runtime` | 689 | lifecycle (−`absorb`) |
| `temporal` | 529 | transitions, snapshots, clock |
| `graph` | 943 | registry, attestation binding (+ a doctest, §4) |
| `verify` | 96 | protocol `FsSnapshot` runs on |
| `snapshot` | 818 | state identity, and now scope entries (§3) |
| `fs_raw` | 222 | the trusted filesystem observer |
| `verifiers` | 199 | content-addressed helpers for `snapshot` |
| `content` | 97 | content-manifest definition used by `SnapshotObjects`/`MatchesSnapshot` |
| `counter`, `fs_watch`, `provenance` | 720 | not in the removal set (rule 5). Their suites (`counter.rs`, `atomic_capture.rs`) or ported tests still cover them |
| `lib.rs` (`VfsPath` & co.) | 162 | used by the kept crates `v9r-vfs`/`v9r-cap` (§8) |

The claim path (kernel through `fs_raw`, plus `verifiers`/`content`) is
4,197 lines. The Phase 1 target in the cut map was ~3,900; the
difference is the two helper files the cut map planned to dissolve.

## 3. Changed trust boundary

### 3.1 Scope evidence: from a second observer to the snapshot

| | Before | After |
|---|---|---|
| fact | `entries(ws)`: `Map` path → `file:<sha256>` \| `dir` \| `symlink:<target>` \| `other` | `snapshot_entries(ws)`: `Map` path → `<mode> <blob id>` \| `40000` |
| produced by | `fs_provider` ("fs"), an **attesting provider** with its own walk (`state.rs` crawler) | `FsSnapshot` (`verifier:fs-snapshot`), from **the same walk** that yields `snapshot(ws)`, over `RawFsObserver` inputs |
| trusted observers for scope | `fs_provider` | `RawFsObserver` (already trusted for identity) |
| `restrict(fs_file/fs_link, [raw])` | needed: two providers answered those kinds | now redundant (one observer). Kept per rule 3, for Phase 2 |
| explanation of a scope fact | provider lineage (`provider:fs`, `lineage:Lnn`: request, round, snapshot) | a layer (`verifier:fs-snapshot`, "trusts provider:raw"). **No lineage chain** |

What moved:

- **Trust base: one filesystem observer fewer.** Scope and identity now
  rest on the same observations, so they can no longer disagree about
  the tree they describe. They are also no longer two independent
  readings. Before, they were never cross-checked either (different
  keys), so no check was lost.
- **Negative cases, measured** before `fs_provider` was removed, both
  sources side by side, 18 changes, fresh workspace each. The table
  is in the header of `tests/scope_entries.rs`. The new source rejected
  everything the old one rejected and allowed everything in scope.
  Differences:
  - `chmod +x` outside the scope: **Allow → Deny**. The old source
    ignored permission bits: a **false Allow, now closed**.
  - FIFO outside the scope: **Deny → Blocked**. A snapshot cannot
    represent a FIFO, so the identity is unknown. It is still rejected,
    but as "cannot judge" instead of "violated".
  - Empty directories (added or deleted, inside or outside) are judged
    as before. `snapshot_entries` lists them although the git tree id
    omits them; it is built from the walk, not from the stored tree, for
    exactly this reason.
- **Explanation narrows for scope facts.** The Overview row "explained
  down to the observer, request and snapshot" now holds for
  provider facts (e.g. `fs_file`, `tests`). For verifier-derived facts
  (`snapshot`, `snapshot_entries`) it gives the verifier and the
  observers it trusted. This was already true for `snapshot`.

### 3.2 Verification that no longer exists in-tree

- **Git objects** (`GitObjects`, SHA-1 or SHA-256) and
  **snapshot ↔ commit equality** are gone. The snapshot's id is still
  git's own tree id (SHA-256), and the test that `git` computes the same
  id stays. So git remains an external checker, but v9r no longer checks
  git data itself.
- `object_at` accepts only 64-hex (SHA-256) ids.

### 3.3 Guarantees no longer made (archived)

| Overview row | Status |
|---|---|
| "Delegated authority attenuates, is authentic, live and unexpired…" | **removed** with `delegation`/`authority`: Capability Delegation v0 and Root of Trust v0 are archive-only |
| "A read-only snapshot grant gives exactly the granted content…" | **removed** with `capability`/`object_identity`: Content Identity v0 and Snapshot Boundary v0 are archive-only |

Archive-only experiments (code at `v9r-archive-v0`, reports unchanged):

- Effect Runtime v0, v1 (the git and fs domains; the counter domain
  stays);
- Git Evidence Domain v0;
- Evidence Graph v0 (git, fs and CI composition; the fs-only port stays);
- Verifiable Observers v0 (git half);
- Capability Manifest / Delegation / Root of Trust / Object Identity /
  Content Identity / Snapshot Boundary v0;
- Controlled Domain v0. Its suite still compiles; its single test was
  and remains `#[ignore]`.

## 4. Preserved guarantees

| Guarantee (Overview) | Tested now by |
|---|---|
| accepted only if every rule is satisfied by verified evidence; claims never satisfy | kernel unit tests (13); `claim_guarantees`: `claims_instead_of_evidence_block`; demo run 3 |
| out-of-scope changes denied and named | `scope_entries` (18 cases); demo run 2; `external_modification_during_the_effect_is_attributed_to_it` |
| exact content identity (git-compatible SHA-256) | `content_addressed`: `a_snapshot_is_the_git_tree_of_the_same_content` (against the `git` binary), `a_claimed_snapshot_id_is_recomputed_not_believed`, `unreadable_file_…`, `approved_snapshot_with_a_lost_or_altered_object_…`, `hidden_directory_…`, `hidden_file_…` |
| authorization single-use; refused if the state changed | `artifact_changed_after_authorization_is_refused`, `external_modification_between_authorization_and_execution_refuses` (incl. hold on drift); demo run 5; runtime `Authorization` has no `Clone` (compile-level) |
| missing or contradictory evidence blocks, never allows | `removing_any_observer_blocks_…`, `disagreeing_observers_block`, `observer_disappearing_before_execution_blocks`, `observer_unavailable_after_effect_blocks_and_holds`, `observers_reading_different_versions_block` |
| answers bound to provider and request (replay, volunteering) | `volunteered_attestations_are_discarded`, `replayed_attestations_are_refused`, `replayed_raw_observation_fails_binding`, `before_snapshot_replayed_as_after_is_not_believed`, `evidence_claiming_an_earlier_state_is_refused` |
| keys bind to the state they name | `test_success_for_another_tree_does_not_count` |
| declared observers and definitions | `evidence_copied_between_domains_is_caught_only_by_declared_observers` (what `restrict` is for), `a_declared_definition_refuses_foreign_answers` |
| explanations (observer, lineage, snapshot audit) | `release_is_explained_by_the_lineage_of_its_evidence`, `snapshot_evidence_carries_its_snapshot_in_lineage`, `claimed_state_is_shown_as_a_claim_…`, `lineage_tokens_round_trip_…` |
| **limits**, kept as tests: a trusted liar and a caching observer are believed | `a_trusted_liar_is_believed_alone_…`, `stale_raw_content_attested_freshly_is_believed`, `stale_state_attested_freshly_is_believed_unless_independently_witnessed` |
| kernel frozen | `tests/kernel_guard.rs` (moved unchanged from `delegation/tests.rs`, so the guard no longer depends on a module it guards against) |

**Found during the phase: the kernel's `compile_fail` example on
`EvidenceBase::map` went vacuous.**

- **Where:** it names `v9r_core::facts::RuntimeEvidence`, and `facts`
  was removed.
- **Effect:** it now fails to compile because the module is missing,
  not because `map` is crate-private. So it still "passes" while proving
  nothing.
- **Fix:** `kernel.rs` is frozen, so an equivalent example was added on
  `graph::GraphEvidence`, with a compiling control beside it. A mutation
  check confirmed it is sensitive: replacing `map` with the public `get`
  makes the `compile_fail` test fail.
- **Still stale in `kernel.rs`:** the vacuous example and a doc
  reference to `crate::policy`. Both stay until the kernel is next
  allowed to change.

**Order of work (the gates):**

1. All 28 ports and the side-by-side scope comparison passed **while
   the original git and `fs_provider` suites still existed**: full
   workspace 345 passed, 0 failed, 6 ignored.
2. Only then were modules deleted.
3. The ported suite also passed three times in a row.

## 5. Test count before and after

| Binary | Before | After |
|---|---|---|
| `v9r-core` unit | 125 | 24 (kernel 13, lib 5, temporal 2, runtime/graph/verify/provenance 1 each) |
| `v9r-core` doctests | 12 | 7 (−7 effect/guarded, +2 graph) |
| `atomic_capture` | 9 (+3 ignored) | 9 (+3 ignored) |
| `content_addressed` | 10 | 10 (git half of one test removed) |
| `controlled_domain` | 0 (+1 ignored) | 0 (+1 ignored) |
| `counter` | 8 | 8 |
| `state_vs_causality` | 1 | 1 |
| `claim_guarantees` | — | **28** (new, ported) |
| `scope_entries` | — | **1** (18 cases) |
| `kernel_guard` | — | **1** (moved) |
| `v9r_demo` (example, `test = true`) | — | **1** |
| removed suites (§1.4) | 113 (+2 ignored) | — |
| `v9r-cap` / `v9r-orchestrator` / `v9r-runtime` / `v9r-vfs` | 11 / 11 / 8 / 7 | 11 / 11 / 8 / 7 |
| `v9r-cli` | 0 | — |
| **total** | **315 passed, 6 ignored** | **127 passed, 4 ignored** |

Reconciliation: 315 − 221 removed (101 unit incl. the moved guard, 113
integration, 7 doc) + 33 added (28 + 1 + 1 + 1 + 2 doc) = 127.

```
cargo test --offline --workspace --no-fail-fast
```

## 6. State vs Causality: reproduced

```
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
```

| Measurement | Before (report) | After | Same between worlds? |
|---|---|---|---|
| S0 | `b69da1dd…1497` | `b69da1dd…1497` | yes |
| S1 | `8348b34c…fd5e` | `8348b34c…fd5e` | yes |
| PRE / POST verdict | Allow / Allow | Allow / Allow | yes |
| accepted, accepting after | true, true | true, true | yes |
| POST findings | 6 × satisfied | 6 × satisfied | yes |
| scope fact | `entries(ws)` `{3 entries, #4922c1b7…}` → `{3 entries, #9d4b3894…}`, by `provider:fs` | `snapshot_entries(ws)` `{3 entries, #2c30f65d…}` → `{3 entries, #4bb22669…}`, by `verifier:fs-snapshot` | yes |
| observers named | `provider:fs`, `provider:raw`, `temporal-clock`, `verifier:fs-snapshot` | `provider:raw`, `temporal-clock`, `verifier:fs-snapshot` | yes |
| receipt, ordering, journal | `Ok("")`, true, `[(Task, Allow) ×2]` | same | yes |
| final workspace bytes | identical | identical | — |

**The result reproduced: every measured field is identical between
World A (authorized agent) and World B (unauthorized writer).**

- What changed is the *form* of the scope fact (new values, new
  observer).
- `provider:fs` no longer exists, so it appears in neither world.
- The decision, A: v9r verifies states, not histories, stands.
- The run was repeated after the deletions, and its output matches the
  run made right after the switch, with `fs_provider` still present.

## 7. Demo

```
cargo run --offline -p v9r-core --example v9r_demo
```

- **Verdicts:** ALLOW, DENY, BLOCKED, DENY, REFUSED, as before.
- **Output change:** run 5's explanation now names `snapshot(ws)@current`
  where it named `entries(ws)@current`. Both basis facts fail; the
  renderer shows the first.
- **Determinism:** two runs after the deletions gave identical output
  (lineage numbers normalized).
- **Now a test:** `scenarios()` and `play()` were extracted, so
  `cargo test` runs the same five runs and asserts the verdicts.

## 8. Deferred (not in this phase's removal set)

| Item | Why kept now |
|---|---|
| `crates/v9r-vfs`, `v9r-cap`, `v9r-runtime`, `v9r-orchestrator`; `lib.rs` `VfsPath`/`VfsError`/`NodeId`/`Capability` | not part of the `fs_provider` chain, git, effect or capability *modules*; they still build and their 37 tests pass |
| `examples/hello-agent`, `examples/llm-gateway`, `agents/` (one `.wasm`, stored twice) | Wasm agents for `v9r-runtime` |
| 101 tracked build files (~16.6 MB) under those two examples' `target/` | belong to the deferred examples; `.gitignore` already ignores them |
| `counter`, `fs_watch`, `provenance`, `content`, `verifiers` | not in the removal set; `content`/`verifiers` are needed by `snapshot` |
| `restrict` calls in the demo and `state_vs_causality` | rule 3; redundant now (§3.1) |
| `GraphDomain`, `Lineage`, `Capture` options, `SnapshotObjects`/`MatchesSnapshot` | inside kept files; Phase 2 |
| README still describes the removed CLI ("transactional shell") | documentation; not code |

## 9. Reproducing

```
cargo test --offline --workspace --no-fail-fast                    # 127 passed, 4 ignored
cargo test --offline -p v9r-core --test kernel_guard               # kernel unchanged
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
cargo run  --offline -p v9r-core --example v9r_demo                # needs python3
git diff v9r-archive-v0 -- crates/v9r-core/src/kernel.rs           # empty
```
