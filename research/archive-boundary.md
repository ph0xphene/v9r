# What happened to the old runtime: archive boundary

> Formerly `research/V9R_ARCHIVE_BOUNDARY_V0.md` (the v9r-review-v1.1 tree has it under that name). Body unchanged apart from document links.

> **Outcome.** This document was written as a recommendation. It was
> carried out in Debloat Phase 1 (commits `e466b76`, `6a9c717`):
>
> - the four product-phase crates (§1) left the tree. They remain,
>   byte-identical, at the annotated tag `v9r-archive-v0` (=
>   `v9r-review-v0`, commit `da6d694`);
> - the three dead APIs (§3) were removed; `counter`, `fs_watch` and
>   `content` were kept (§2);
> - §5 Option A was taken.
>
> §5's statements about which tags exist where describe the repository
> when this was written. As of 2026-10-02 the archive tags are not
> published on GitHub; a clone without `v9r-archive-v0` cannot run the
> archived experiments.

*Decides what belongs to the Phase 1 verifier and what belongs to the
archive. Nothing was deleted or changed while writing this.
Recommendations only. Facts as of 2026-10-02.*

Possible actions:

- **KEEP**: stays in the release candidate.
- **MOVE TO ARCHIVE TAG**: removed from the tree; it stays reachable at
  `v9r-archive-v0` = `v9r-review-v0` = `da6d694`, where it is
  byte-identical.
- **DELETE**: removed, with nothing worth pointing to.

The test for each item is the claim, *"v9r verifies state transitions,
not histories"*: does removing the item change any guarantee in
[V9R_PHASE1_EXTERNAL_REVIEW_V0](archive/V9R_PHASE1_EXTERNAL_REVIEW_V0.md) §5?

## 1. The four product-phase crates

| | `v9r-vfs` | `v9r-cap` | `v9r-runtime` | `v9r-orchestrator` |
|---|---|---|---|---|
| what it is | in-memory VFS (slotmap arena) and write-event bus | Plan-9-style namespace views over that VFS | Wasmtime agent host | dispatch of VFS write events to agents |
| size | 827 lines, 7 tests | 363 lines, 11 tests | 1,161 lines, 8 tests | 965 lines, 11 tests |
| milestone | product phase: commit `059ef29` "implement transactional runtime with atomic rollbacks" (2026-05-11), before the research branch | same | same | same |
| last change to its source | `059ef29` | `059ef29` (Phase 1 removed an unused `tracing` dependency) | `059ef29` | `059ef29` |
| measured by a research report? | no. [EFFECT_RUNTIME_V0](archive/EFFECT_RUNTIME_V0.md): "**Not involved** in the host-filesystem transaction path" | no. [EFFECT_RUNTIME_V0](archive/EFFECT_RUNTIME_V0.md): "Not reused. `v9r-cap` governs the in-memory VFS" | no | no |

**Why they are not part of the verifier claim:**

- The verifier observes the **host** filesystem through `fs_raw` and
  judges transitions in `runtime`/`temporal`.
- None of the four crates is imported by `v9r-core`. The dependency runs
  the other way: they use `v9r-core`'s `lib.rs` types (`VfsPath`,
  `VfsError`, `NodeId`, `Capability`, `CapabilityId`).
- An agent is an opaque `temporal::Actor` to the verifier. How it is
  hosted (Wasm or otherwise) is outside the claim.
- Removing the four crates changes no guarantee and no test of
  `v9r-core`.

**Does keeping them harm reviewer understanding? Yes, in four concrete
ways:**

1. `cargo test --workspace` reports 128 tests, of which **37 belong to
   these crates**. A reviewer counting evidence for the claim is
   misled by 29%.
2. **`v9r-core/src/lib.rs` exports `Capability` and `CapabilityId`**
   (namespace-mount ids). This collides with the research vocabulary,
   where "capability" means manifest-built worlds and delegated grants
   (Capability Manifest/Delegation v0). Those are archived and unrelated.
3. The workspace pulls in Wasmtime, reqwest and tokio's process and
   filesystem features. A TCB reading of `Cargo.lock` has to separate
   them from the verifier by hand.
4. `crates/` lists five crates for a verifier that is one.

**Recommendation: MOVE TO ARCHIVE TAG**, in the release-candidate commit
(tag layout: §5). The `lib.rs` VFS and
capability types (~150 lines, 5 tests) and the dependencies only they use
go with them: `slotmap`, `thiserror` in `v9r-core`; `wasmtime`,
`wasmtime-wasi`, `wat`, `reqwest`, `toml`, `bytes`, `async-trait`,
`anyhow`, `tracing` in the workspace.

## 2. Kept `v9r-core` modules outside the claim path

| Module | LOC | Milestone | Why it is not the claim | What depends on it | Does it harm review? | Recommendation |
|---|---|---|---|---|---|---|
| `counter` | 295 | Effect Runtime v1: the in-memory domain that falsified "the runtime is domain-neutral" and found a defect shared by both real guards | a second `EffectDomain`; the claim runs on `temporal` | `tests/counter.rs` (8 tests), 2 doctests (one proves a non-`Compensable` domain cannot `compensate`) | **mildly helpful while `runtime` stays generic**: it is the only domain besides `temporal`/`graph`, and it shows what `EffectDomain` abstracts | **KEEP** for the release candidate. MOVE TO ARCHIVE TAG when the runtime becomes concrete (Phase 2) |
| `fs_watch` | 150 | Atomic Capture v0: inotify change journal as a capture strategy | capture is *trusted* in the claim; this strategy is off by default (`Capture::default()`) | `tests/atomic_capture.rs` (watch scenarios); `snapshot.rs` asks for `fs_watch`/`fs_changes` keys when `Capture.watch` is set | low: isolated, off by default | **KEEP** for the release candidate (removing it means editing `snapshot.rs`'s capture code). MOVE TO ARCHIVE TAG with `Capture` in Phase 2 |
| `provenance` | 275 | Evidence Provenance v0 | explains decisions; does not change any verdict | 5 tests in `claim_guarantees` (G10: lineage, snapshot audit, claimed state shown as a claim) | **helpful**: it backs the "explained per rule" guarantee and its measured limit (a stale provider gets a consistent explanation) | **KEEP** |
| `content` | 97 | Verifiable Observers v0 / Content-Addressed State v0: the `v9r-content-manifest/1` normal form | a **second definition of "same content"** beside the git tree id; the claim uses only the tree id | `verifiers::digest_of` → `SnapshotObjects` (`snapshot_digest`) and `MatchesSnapshot(MANIFEST)`; `content_addressed` (2 tests, incl. "two definitions of content that disagree block") | **moderate**: a reviewer meets two content identities and must work out which one the claim uses | **KEEP** for the release candidate, with this note. Phase 2 candidate together with `SnapshotObjects`/`MatchesSnapshot` |

## 3. Unused and idle APIs (Phase 1 audit §1.1)

| Item | Where | Status | Milestone | Recommendation |
|---|---|---|---|---|
| `snapshot_from` | `snapshot.rs` | **no caller** | Snapshot Boundary / Content Identity v0 (snapshot from a transcript made inside a world) | **DELETE** at the next code commit. Dead; nothing to point to beyond the tag |
| `snapshot::entries`, `SnapEntry` | `snapshot.rs` | **no caller** | Content Identity v0 (listing a materialized grant); considered for scope in the cut map, not used | **DELETE** at the next code commit |
| `Runtime::add_semantic` | `runtime.rs` | no caller; nothing produces semantic evidence since `guarded` went | Invariant Kernel v0 (soft requirements) | **DELETE** at the next code commit. It is in `runtime`, not the frozen kernel |
| `Authorization::proposal`, `Decision::undetermined` | `runtime.rs`, `kernel.rs` | no caller outside the crate | — | KEEP (one-liners; `kernel.rs` is frozen) |
| `Compensable`, `Runtime::compensate`, `Stage::Compensated` | `runtime.rs` | no implementor; only `counter`'s `compile_fail` doctest refers to it | Effect Runtime v1 (fs/git rollback) | KEEP while `counter` stays; MOVE TO ARCHIVE TAG with it |
| `Semantic`, `Strength::Soft`, `EvidenceClass::Semantic`, `Requirement::AtMost` | `kernel.rs` | no producer on the claim path | Invariant Kernel v0 | **KEEP**: the kernel is frozen. Removing them is a kernel change and needs a new freeze |
| the `compile_fail` example on `EvidenceBase::map` | `kernel.rs` | vacuous (names the removed `facts`) | — | KEEP (frozen); its replacement is on `graph::GraphEvidence` |
| `verifiers::NAMES` | `verifiers.rs` | reachable only if a caller asks for `snapshot_digest(root, "names/1")`; no test does | Verifiable Observers v0 (a deliberately weaker definition) | KEEP for now; Phase 2 with `content` |
| `snapshot::Capture`, `FsSnapshot::with`, `fs_raw`'s `fs_meta` | `snapshot.rs`, `fs_raw.rs` | used only by `atomic_capture` | Atomic Capture v0 | KEEP for the release candidate; Phase 2 with `fs_watch` |
| `GraphDomain`, `graph::runtime` | `graph.rs` | no claim-path use; harness for 18 of the 28 ported guarantee tests and for `content_addressed` | Evidence Graph v0 | KEEP |
| `lib.rs` `VfsPath`, `VfsError`, `VfsResult`, `NodeId`, `Capability`, `CapabilityId` | `lib.rs` | used only by the four crates | product phase | MOVE TO ARCHIVE TAG **with the crates** (§1) |

## 4. Summary

| Item | Decision |
|---|---|
| `v9r-vfs`, `v9r-cap`, `v9r-runtime`, `v9r-orchestrator` (+ `lib.rs` types, their dependencies) | **MOVE TO ARCHIVE TAG** (in the release candidate) |
| `counter`, `fs_watch`, `content` | **KEEP** for the release candidate; archive in Phase 2 |
| `provenance` | **KEEP** |
| `snapshot_from`, `snapshot::entries`/`SnapEntry`, `Runtime::add_semantic` | **DELETE** at the next code commit |
| kernel's idle items | **KEEP** (frozen) |

After the recommended moves and deletions, about **4,830** library lines
and **86** tests would remain, every test about `v9r-core`. That is an
estimate from current counts:

- library: 5,079 − ~150 (`lib.rs` types) − ~100 (the three dead APIs);
- tests: 128 − 37 (the four crates) − 5 (`lib.rs` unit tests).

## 5. Tag layout

Facts:

- `v9r-review-v0` is an **annotated** tag on `da6d694` ("315 passed,
  kernel guard passed").
- `v9r-archive-v0` is a **lightweight** tag on the same commit, created
  locally during Phase 1.
- The research branch `research/effect-runtime-v0` and both tags exist
  **only locally**; `origin` has `main` and two other branches.
- Phase 1 documents cite `v9r-archive-v0` in four places: README,
  `verifiers.rs`, `content_addressed.rs`, the Phase 1 report.

### Option A: two tags

```text
v9r-archive-v0  ──▶ da6d694   pre-debloat research tree (all experiments)
v9r-review-v1   ──▶ <RC>      the verifier only
```

**For:**

- tags are immutable, and a reviewer cites exact bytes;
- archived experiments stay reproducible exactly as measured
  (`git worktree add ../v9r-archive v9r-archive-v0`);
- the archive's role is explicit in its name. A reviewer should not have
  to know that "review-v0" also means "everything we removed".

**Against:**

- `v9r-archive-v0` and `v9r-review-v0` name the same commit: two names,
  one tree. That is redundant but harmless if both are annotated and say
  so.
- Fixes to archived code are impossible without a new tag, which is the
  intended property for an archive.

### Option B: one review tag, plus an archive branch

```text
v9r-review-v1          ──▶ <RC>
archive/pre-debloat    ──▶ da6d694 (branch)
```

**For:**

- one release name;
- the archived experiments can keep evolving (e.g. re-measure Atomic
  Capture), and history stays connected.

**Against:**

- a branch moves. "The archived experiment" becomes ambiguous unless
  every citation also names a commit;
- an archive branch invites maintenance of code the verifier no longer
  depends on, which is the drift Phase 1 removed;
- reviewers must be told not to review it.

### Recommendation: Option A

1. Make `v9r-archive-v0` an **annotated** tag on `da6d694`, with a
   message stating that it equals `v9r-review-v0` and is the archive of
   everything removed in Phase 1. It is local and unpublished, so
   re-creating it rewrites nothing shared.
2. Tag the release-candidate commit `v9r-review-v1`. It contains only the
   verifier: Phase 1, with the four crates moved to the archive tag (§1).
3. Keep `v9r-review-v0` unchanged. It is the revision the earlier review
   package names.
4. Push the branch and the three tags together, so citations resolve for
   the reviewer.
