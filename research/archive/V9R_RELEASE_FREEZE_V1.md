# v9r Release Freeze v1

*For a skeptical systems engineer who has only the repository,
README_REVIEWER.md and this document. Every number here was measured
on 2026-10-02 on one Linux host, with rustc 1.98.1 and cargo 1.98.0
(as reported by `rustc --version` and `cargo --version`). Re-measure it
with §5; do not take it on trust.*

## 0. Release notes: `v9r-review-v1.1`

*Added 2026-10-02. §1–§9 below describe the frozen baseline
`v9r-review-v1`, with the guarantee wording corrected for v1.1.*

| | |
|---|---|
| **frozen baseline** | `v9r-review-v1`, unchanged and not moved: code commit `6a9c717`, 86 passed, 0 failed, 4 ignored. The v1 documents as tagged are the reference the corrections are made against |
| **what v1.1 adds: X1/X4 measurement tests** | `crates/v9r-core/tests/self_referential_observers.rs`, 2 tests that assert the measured false ALLOWs (report V9R_SELF_REFERENTIAL_OBSERVERS_V0) |
| **what v1.1 adds: research documents** | V9R_SELF_REFERENTIAL_OBSERVERS_V0, V9R_ADVERSARIAL_REVIEW_V0 (with measured-status notes), V9R_CLAIM_REVISION_V0, V9R_OBSERVER_INDEPENDENCE_MODEL_V0, V9R_OBSERVER_CAPABILITY_BOUNDARY_V0, V9R_VERIFICATION_BOUNDARY_V0, V9R_RESEARCH_THESIS_V0, V9R_RELEASE_AUDIT_V0, V9R_RELEASE_AUDIT_V1_1, V9R_RELEASE_FREEZE_V1_1 |
| **what v1.1 corrects: release documentation** | wording only, as identified in V9R_RELEASE_AUDIT_V0: `README.md`, `README_REVIEWER.md`, this document, V9R_PHASE1_EXTERNAL_REVIEW_V0, V9R_REVIEW_CHECKLIST_V1, V9R_EXTERNAL_REVIEW_PACKAGE_V0 |
| **what v1.1 corrects: demo wording** | the **printed labels and doc comment** of `examples/v9r_demo.rs`, strings only: same agents, rules, observers and five verdicts, still asserted |
| **what v1.1 does not change: verifier implementation and kernel** | `git diff v9r-review-v1 v9r-review-v1.1 -- crates/v9r-core/src Cargo.toml Cargo.lock crates/v9r-core/Cargo.toml` is empty. `kernel.rs` sha256 `85badb66…6177f`, byte-identical to v1 |
| **`cargo test --workspace`** | **88 passed, 0 failed, 4 ignored** (86 + X1 + X4); rustc 1.98.1, cargo 1.98.0 |
| **the claim** | unchanged in substance. Its wording is narrowed where a measurement (X1, X4, G11) or a code fact (`fs_raw.rs:73`, `temporal.rs:526`) showed it said more than the code does. Formal statement: V9R_VERIFICATION_BOUNDARY_V0 §1 |
| **still predictions, not measured** | X2, X3, X5 and G′ of V9R_ADVERSARIAL_REVIEW_V0 |

Reproduce the v1.1 additions:

```sh
cargo test --offline --workspace
#  → 88 passed; 0 failed; 4 ignored
cargo test --offline -p v9r-core --test self_referential_observers -- --nocapture --test-threads=1
#  → X1: honest fix Allow, broken module Deny, sys.exit(0) Allow with greet incorrect
#  → X4 over 10 runs: ALLOW+accepted 10; write landed before completion 10; S1 stale 10
```

## 1. Release identity

| | |
|---|---|
| **tag** | `v9r-review-v1` (annotated) |
| **code commit** | `6a9c717695e3cf00b72e97b758283af3ab5e707e` |
| **tagged commit** | the documentation commit directly after it, which contains this file. A file cannot contain its own commit's hash: run `git rev-parse v9r-review-v1^{commit}`. That commit changes no code: `git diff 6a9c717 v9r-review-v1 -- crates Cargo.toml Cargo.lock` is empty |
| **branch** | `research/effect-runtime-v0` |
| **claim** | *v9r verifies states, not histories.* |
| **`cargo test --workspace`** | **86 passed, 0 failed, 4 ignored** |
| **kernel hash guard** | passed. `crates/v9r-core/src/kernel.rs` sha256 `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`, byte-identical to `v9r-review-v0` and unchanged since commit `bfebebe` |
| **demo** | ALLOW, DENY, BLOCKED, DENY, REFUSED |
| **State vs Causality** | complete decisions identical for an authorized writer and an unauthorized writer of identical S1 |
| **code** | 1 crate (`v9r-core`), 12 modules + `lib.rs`, **4,838** library lines (5,340 with unit tests); 4,813 lines of integration tests and demo |
| **library dependencies** | `serde`, `serde_json`, `sha2`, `libc` (20 external crates in the build graph, proc-macros included). Dev-only: `tokio`, `uuid` |
| **archive** | `v9r-archive-v0` → `da6d694`, the pre-debloat research tree (same commit as `v9r-review-v0`) |

## 2. Commit graph

```text
 bfebebe … (kernel frozen here)
    │
 da6d694  docs(research): milestone, overview, external review package   ◀── v9r-review-v0, v9r-archive-v0
    │                                                                         315 passed, 0 failed, 6 ignored
 7127a9a  chore: remove dead examples, committed build output, task.toml
    │                                                                         (no workspace member changed)
 e466b76  refactor!: Debloat Phase 1, extract the state transition verifier
    │                                                                         128 passed, 0 failed, 4 ignored
 6a9c717  chore!: move product-phase crates to the archive; remove dead APIs
    │                                                                         86 passed, 0 failed, 4 ignored
 <docs>   docs: Phase 1 research record, reviewer entrypoint, release freeze   ◀── v9r-review-v1
                                                                              (no code change)
```

- **Not rewritten:** history. The ~1 GB of build output removed in
  `7127a9a` is still in history, so a full clone is large.
- **Pushed:** nothing yet; the tags and the branch are local.

## 3. What was removed (relative to `v9r-review-v0`)

Everything listed here remains reproducible at `v9r-archive-v0`. The
reports that measured it are unchanged in `research/`.

| Removed | Was | Report(s) |
|---|---|---|
| `fs_provider` + `effect`, `facts`, `vfs`, `execution`, `trace`, `task`, `bundle`, `context`, `trusted`, `manifest`, `state`, `policy`, `guarded`, `adapter`; crate `v9r-cli` | the earlier "transactional shell": checkpoints, rollback, bundles, LLM clients, the first effect domain | Effect Runtime v0; Invariant Kernel v0 (its domain) |
| `git`, `git_provider`, `git_guard`; verifiers `GitObjects`, `ContentEquality`, `SnapshotCommitEquality`; SHA-1 ids | git as an evidence and effect domain | Git Evidence Domain v0, Effect Runtime v1, Evidence Graph v0, Verifiable Observers v0 |
| `capability`, `delegation`, `authority`, `object_identity` | manifest-built worlds, delegated and signed grants, object identity | Capability Manifest / Delegation / Root of Trust / Object Identity / Content Identity / Snapshot Boundary v0 |
| crates `v9r-vfs`, `v9r-cap`, `v9r-runtime`, `v9r-orchestrator`; `lib.rs` `VfsPath`, `VfsError`, `VfsResult`, `NodeId`, `Capability`, `CapabilityId` | product phase (`059ef29`): in-memory VFS, namespace views, Wasm agent host | none measured them ("Not involved", Effect Runtime v0) |
| `examples/*`, `agents/*.wasm`, `task.toml`, 5,318 tracked build files | product-phase artifacts | — |
| `snapshot::snapshot_from`, `snapshot::entries`, `SnapEntry`, `Runtime::add_semantic` | no caller | — |

**Guarantees the project no longer makes** (they were rows of the v0
overview): delegated authority, and exact snapshot grants. Both are
archived results, not properties of this release.

**How removal was made safe:**

- Every guarantee of the claim that was tested only in a suite slated
  for deletion was **ported first**: 28 tests in `claim_guarantees.rs`.
- The ported tests were run **alongside the originals** (345 passed)
  before anything was deleted.
- The one claim-relevant fact that `fs_provider` supplied (`entries`) was
  replaced by `snapshot_entries` from the snapshot's own walk. Over 18
  changes, it rejected everything the old source rejected.

Full record: V9R_DEBLOAT_PHASE1_REPORT_V0.

## 4. What remains

| Module | LOC | Role | Class |
|---|---|---|---|
| `kernel` | 604 | three-valued evaluation of `Fact`/`Within`/`AtMost` obligations; `Verified` evidence only the crate can mint. **Frozen** | claim |
| `runtime` | 684 | lifecycle: authorize, re-check on a fresh observation, execute, judge, accept or hold | claim |
| `temporal` | 529 | snapshots on one clock, each read twice; before/after rules; `transition.ordered` | claim |
| `graph` | 943 | evidence registry: binds each answer to its provider and request; kind rules; lineage | claim (+ explanations) |
| `verify` | 96 | protocol for derived facts (`FsSnapshot` is a verifier) | claim |
| `snapshot` | 726 | `FsSnapshot`: git-compatible SHA-256 tree id and `snapshot_entries` from one walk; content-addressed object store; `materialize` | claim |
| `fs_raw` | 222 | filesystem observer: `openat(O_NOFOLLOW)` per component, `getdents`, `fstatat`, `read`, `readlinkat` | claim |
| `verifiers`, `content` | 296 | hash-checked tree walking; the content-manifest definition used by `SnapshotObjects`/`MatchesSnapshot` | supporting |
| `provenance` | 275 | `explain`: resolves lineage, audits snapshot binding and definitions | supporting |
| `counter`, `fs_watch` | 445 | domain-neutrality falsifier (Effect Runtime v1); inotify capture strategy (Atomic Capture v0) | historical |
| `lib` | 18 | module list | — |

**Tests:**

| Suite | Tests |
|---|---|
| unit, `v9r-core` | 19 |
| doctests | 7 |
| `claim_guarantees` | 28 |
| `scope_entries` | 1 test, 18 cases |
| `content_addressed` | 10 |
| `state_vs_causality` | 2 |
| `kernel_guard` | 1 |
| demo | 1 |
| `counter` | 8 |
| `atomic_capture` | 9 (+3 ignored) |
| `controlled_domain` | 0 (+1 ignored) |

All 86 test `v9r-core`.

## 5. Reproduction

Requirements:

- Linux, Rust 1.98 (measured with rustc 1.98.1, cargo 1.98.0);
- `python3` (the demo's test runner);
- `git` (one identity test);
- network once for `cargo fetch`; everything else works `--offline`.

```sh
git checkout v9r-review-v1
cargo fetch

cargo test --workspace
#  → 86 passed; 0 failed; 4 ignored   (sum the "test result" lines)

cargo run -p v9r-core --example v9r_demo
#  → Summary: ALLOW, DENY, BLOCKED, DENY, REFUSED

cargo test -p v9r-core --test state_vs_causality -- --nocapture
#  → "measurements identical: true", every field "same"; 2 passed

cargo test -p v9r-core --test kernel_guard
sha256sum crates/v9r-core/src/kernel.rs
git diff v9r-review-v0 -- crates/v9r-core/src/kernel.rs
#  → 1 passed; 85badb669f50…6177f; empty diff

git diff 6a9c717 v9r-review-v1 -- crates Cargo.toml Cargo.lock
#  → empty: the tag adds documentation only
```

**Measured run times** (cache warm):

- demo from a fresh copy without `target/`: 8.5 s, including the build;
- full suite: ~1 min.

**The 4 ignored tests are measurements, not guarantees.** Two are
timing-dependent races, and two need a systemd user session with
cgroup delegation and/or unprivileged user namespaces. Run them with
`--ignored --nocapture`.

**Archived experiments:**

```sh
git worktree add ../v9r-archive v9r-archive-v0
cd ../v9r-archive
cargo test --offline -p v9r-core --test <capability_manifest|content_identity|snapshot_boundary|…> -- --nocapture
```

At that revision the full suite gave 315 passed, 0 failed, 6 ignored.

## 6. Guarantees

Each guarantee holds only if the trusted computing base below is correct
or faithful.

**Trusted computing base:**

- `kernel`, `runtime`, `temporal`;
- the `graph` registry and `verify`;
- `snapshot`, and `fs_raw` with the Linux filesystem: `openat`,
  `getdents`, `fstatat` and `read` semantics, plus directory link counts;
- **every registered observer**;
- **the rules**;
- **whoever holds the registry**;
- snapshot capture, against an undetected concurrent writer;
- `serde`, `serde_json`, `sha2`, `libc`, `rustc`.

**Not trusted:**

- the actor and its statements;
- stored object bytes (checked by hash);
- answers not bound to their request or not asked for.

| # | Guarantee | Condition | Test |
|---|---|---|---|
| G1 | Accepted only if every declared rule is satisfied by evidence **attested by registered observers** in reply to v9r's own requests; an actor's claims never satisfy a rule. An attestation is as true as its observer | observers faithful **and independent of the judged state**. Not met by the demo's test runner: X1 gives Allow with `greet` absent | kernel unit tests; `claims_instead_of_evidence_block`; demo runs 3–4. Limit: `x1_judged_program_decides_the_test_runners_report` (added in v1.1) |
| G2 | Missing or contradictory evidence → **Blocked**, never Allow | — | `removing_any_observer_blocks_with_missing_evidence_never_denies`, `disagreeing_observers_block`, `observer_disappearing_before_execution_blocks`, `observer_unavailable_after_effect_blocks_and_holds`, `observers_reading_different_versions_block` |
| G3 | Changes outside a declared scope, present when the after-snapshot is taken, are **denied and named**: names, file content, symlink targets, directories (empty ones too), and the **owner-execute bit** only (`fs_raw.rs:73`) | observer sees the whole scope. FIFO/socket/device → Blocked. Other metadata is not observed | `scope_entries` (18 cases); demo run 2 |
| G4 | S0 and S1 are identified by the SHA-256 git tree id of the captured tree (names, content, symlink targets, owner-execute bits); `git` computes the same id; a claimed id is recomputed | capture trusted | `a_snapshot_is_the_git_tree_of_the_same_content`, `a_claimed_snapshot_id_is_recomputed_not_believed` |
| G5 | An omitted subdirectory, an unreadable file, or a lost or altered stored object → identity incomplete (Blocked) | link counts kept by the filesystem. An omitted **file** needs a second observer | `hidden_directory_omitted_by_the_observer_blocks`, `hidden_file_omitted_by_the_observer_blocks_only_with_an_independent_observer`, `unreadable_file_makes_the_snapshot_incomplete`, `approved_snapshot_with_a_lost_or_altered_object_is_incomplete` |
| G6 | Authorization is single-use and **refused** if the watched state differs, at the start of execution, from the one it was granted on; drift **holds** that runtime process (in memory; a new runtime takes the current state as its baseline, `temporal.rs:526`) | change visible to the watched keys | `artifact_changed_after_authorization_is_refused`, `external_modification_between_authorization_and_execution_refuses`; demo run 5 |
| G7 | Replayed, forwarded, volunteered or anachronistic attestations are dropped | — | `volunteered_attestations_are_discarded`, `replayed_attestations_are_refused`, `replayed_raw_observation_fails_binding`, `before_snapshot_replayed_as_after_is_not_believed`, `evidence_claiming_an_earlier_state_is_refused` |
| G8 | Anything written between the fresh observation at the start of execution and the after-snapshot (taken when the actor's call returns) is attributed to the effect. Changes after the after-snapshot are not judged, even while the decision is still running | — | `external_modification_during_the_effect_is_attributed_to_it`. Limit: `x4_watched_state_changes_after_observation_before_completion` (added in v1.1; 40/40 accepted a stale S1) |
| G9 | Facts bind to the state they name. Binding is not independence: an observer that executes that state is controlled by it | rules name the tree | `test_success_for_another_tree_does_not_count`. Limit: X1 (added in v1.1) |
| G10 | Each rule's finding names its observer. Provider facts resolve to provider, request, round and snapshot | verifier facts name the verifier and the observers it trusted | `release_is_explained_by_the_lineage_of_its_evidence`, `snapshot_evidence_carries_its_snapshot_in_lineage` |
| **G11** | **Identical S1 from an authorized actor or an unauthorized writer → identical complete decisions** | by design: this is the claim's boundary | `state_vs_causality`, `identical_s1_gets_identical_decisions_whoever_wrote_it` (with a control world that differs) |
| G12 | Kernel unchanged since review | — | `kernel_guard` |

## 7. Limitations

**Trust:**

- **A registered observer is believed.** A liar or a caching observer
  alone gives a **false Allow**. An independent second observer turns
  that into Blocked, not into the truth. Kept as tests (§8).
- **Scope and identity rest on one filesystem observer** (`fs_raw`).
  They cannot disagree; they do not check each other.
- **Capture is trusted** against a same-uid concurrent writer. The
  double reading catches changes between its readings only.
- **Checks that execute the judged state are controlled by it.** The
  demo's test imports the code under test. Measured (X1): `sys.exit(0)`
  on import gives Allow with `greet` absent. The demo's label in v1,
  "tests pass on the result, run by v9r", overstated I3: it is the test
  command's exit status on S1. v1.1 relabels it.
- **The decision covers S1 as captured, not the state at completion.**
  Measured (X4): a process the actor left behind rewrote the protected
  test while v9r was deciding; the earlier S1 was accepted 40/40, and
  the runtime's trusted state no longer matched the workspace.

**What v9r does not do:**

- **No causal attribution** (G11), **no containment, no rollback, no
  freshness across restarts.** The runtime's state is in memory.

**Explanations:**

- They are parsed from kernel text. A basis containing `)` is cut, and a
  contradiction names no provenance.

**Code:**

- **Frozen-kernel residue:**
  - `Semantic`, `Strength::Soft` and `AtMost` have no producer on the
    claim path;
  - the `compile_fail` example on `EvidenceBase::map` is vacuous (it
    names a removed module); an equivalent one is on
    `graph::GraphEvidence`;
  - a doc comment names the removed `crate::policy`.
- **Generic runtime.** `EffectDomain` and its sum types make 45 public
  items purely structural. The reviewer-facing surface is 32 items
  (V9R_PUBLIC_SURFACE_AUDIT_V0).
- **Historical modules** `counter` and `fs_watch` remain, and `content`
  defines a second notion of "same content" beside the git tree id.
- **Measured on one Linux host.** Research code.

## 8. Known falsifications

Assumptions this project held, tested and found false. Each is either
still a passing test that demonstrates the failure, or an archived
measurement.

| Assumption | Finding | Where |
|---|---|---|
| a verified fact is true | a registered liar is believed alone | `a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness` |
| binding answers to requests keeps them current | a caching observer re-attests stale state: **false Allow** | `stale_raw_content_attested_freshly_is_believed`, `stale_state_attested_freshly_is_believed_unless_independently_witnessed` |
| a consistent explanation is a correct one | a stale provider gets a false Allow with a clean explanation | Evidence Provenance v0; `claimed_state_is_shown_as_a_claim_beside_the_established_round` |
| an observer sees everything it lists | an omitted file is invisible without a second observer | `hidden_file_omitted_by_the_observer_blocks_only_with_an_independent_observer` |
| a PRE fact can name the snapshot it was derived on | PRE is re-checked on a fresh snapshot; it must name `@current` | STATE_VS_CAUSALITY_V0, FA1 |
| snapshot or lineage ids tell writers apart | they are counters | STATE_VS_CAUSALITY_V0, FA2; G11 |
| scope entries without modes are enough | `chmod +x` outside the scope was **allowed** by the old source | V9R_DEBLOAT_PHASE1_REPORT_V0 §3.1 |
| a snapshot can be certified against a same-uid writer by detection | no detection strategy did; only the cgroup freezer (200/200), if writers cannot leave | Atomic Capture v0 |
| `(dev, ino)` or names identify an object | names wrong 3/6; inode reuse 500/500 on ext4 | Object Identity v0 (archived) |
| a live view checked against a tree id is the snapshot | it missed 2 changes; exact only when materialized (9/9) | Capability Content Identity v0 (archived) |
| a sealed snapshot view is safe from the host | the namespace owner (same uid) changed it: **detected, not prevented** | Snapshot Capability Boundary v0 (archived) |
| signed delegation is enough | a stolen key and a restored runtime still Allow | Capability Root of Trust v0 (archived) |
| tests that run agent code are evidence about the agent's change | they execute agent-controlled code | Agent Transition Runtime Design v0 (superseded) |
| a test command run by v9r on S1 is independent evidence about S1 | S1 that `sys.exit(0)`s on import: **false Allow**, `greet` absent | `x1_judged_program_decides_the_test_runners_report` (added in v1.1) |
| an accepted S1 is the state when the transition completes | a write after the after-snapshot, during the decision: **accepted S1 stale**, 40/40 | `x4_watched_state_changes_after_observation_before_completion` (added in v1.1) |
| removing a module only removes its tests | the kernel's `compile_fail` example went vacuous | V9R_DEBLOAT_PHASE1_REPORT_V0 §4 |

## 9. Open research questions

1. **Observation binding within one decision.** Different observers read
   one state at different instants. Is a wrong verdict constructible
   from two readings of one snapshot?
2. **Evidence independence.** How should a fact be typed when its
   producer executes the state being judged? Today it is `Verified` like
   any other, and X1 measured that such a fact can be decided by the
   judged state.
3. **Completeness of `snapshot_entries`.** What can change outside the
   scope and leave both the tree id and the entries unchanged? Hard
   links, xattrs, ACLs, ownership, timestamps, mounts inside the scope.
4. **Freshness across restarts.** The runtime's trusted state and hold
   are in memory.
5. **Compensation.** Where does "restore S0" belong, if anywhere?
6. **Structured provenance.** Explanations come from kernel text;
   fixing that touches the frozen kernel.
7. **Causal receipts (optional).** An isolating executor could supply
   evidence binding S1 to its writer, checked as an ordinary `Fact`.
   It is not built, and not needed for the claim.
8. **The kernel freeze.** Is it worth keeping a frozen kernel that
   carries features nothing produces any more? Phase 2 would have to
   answer this.
9. **The boundary itself.** Can rules over the existing observers tell
   World A from World B? If yes, G11 and the claim are wrong.
