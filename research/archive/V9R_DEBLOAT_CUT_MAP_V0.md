# v9r Debloat Cut Map v0

*Analysis only, made before Phase 1 of V9R_DEBLOAT_PLAN_V0. No code was
modified and nothing was deleted. Dependency edges come from the
`crate::<module>` references in each file at `da6d694`. Test coverage
comes from the test names in each suite.*

## 0. Legend

**Participation** in the four functions the claim needs:

| Code | Function | Meaning |
|---|---|---|
| **E** | evidence creation | mints, collects, binds or derives facts |
| **D** | decision evaluation | turns obligations and evidence into a verdict |
| **S** | state identity | gives a state an exact, content-derived identity |
| **T** | transition lifecycle | authorize → re-check → act → observe → accept or hold |

Each cell is ● (yes, on the claim path), ○ (yes, but only for an archived
domain or experiment), or blank (no).

**Class:**

| Class | Meaning |
|---|---|
| **CORE** | required for the claim *"v9r verifies state transitions, not histories"* |
| **DEMO** | needed only by the demo or the boundary test, and replaceable |
| **HIST** | historical experiment; its result is recorded in a report |
| **LEGACY** | residue of the earlier "transactional shell" product; no research report relies on it |

**Removable in Phase 1:** yes, no, or **gated** (yes, but only after the
port in §4 has been done).

---

## 1. Corrections to V9R_DEBLOAT_PLAN_V0

Mapping every module found three errors in the plan. **They change
Phase 1.**

| # | The plan said | In fact | Consequence |
|---|---|---|---|
| C1 | "`kernel.rs` is untouched in Phase 1, so its hash guard still holds" | the guard `kernel_is_unchanged_and_knows_no_delegation` lives in **`src/delegation/tests.rs:810`**. Deleting `delegation` deletes the guard without a sound | move it first, to `tests/kernel_guard.rs` with `include_str!("../src/kernel.rs")`. Moving it into `kernel.rs` would change the hash it checks |
| C2 | "remove `snapshot::entries`/`SnapEntry` (capability experiments)" | `snapshot::entries(store, tree)` lists a stored tree and checks every object against its id. It is the **cheapest sound replacement** for `fs_provider`'s `entries` | keep it, and use it as the source for the scope invariant (§5.2) |
| C3 | "the demo's single `restrict` guards a kind that only one provider answers" | today **two** providers answer `fs_file`/`fs_link` (`raw` and `fs_provider`). `restrict(kind, ["raw"])` is what keeps the snapshot verifier reading only the raw observer | once `fs_provider` is gone the calls do nothing. Leave them in Phase 1 and remove them in Phase 2 |

There is a fourth finding, not a correction: **the claim's freshness and
anti-replay guarantees are tested only in suites that need git**
(`tests/temporal.rs`, `tests/graph.rs`, `tests/verifiable.rs`,
`tests/content_addressed.rs`). Deleting the git modules would leave
those guarantees untested. See §4.

---

## 2. Crates

| Crate | E | D | S | T | Class | Phase 1 | If removed: guarantee lost | Becomes archive-only |
|---|---|---|---|---|---|---|---|---|
| `v9r-core` | ● | ● | ● | ● | CORE (part) | no (shrinks) | — | — |
| `v9r-vfs` | | | | | LEGACY | yes | none. It is an in-memory VFS and write bus that no research report measures | Effect Runtime v0 background |
| `v9r-cap` | | | | | LEGACY | yes | none. Its `NamespaceView` mounts are over the in-memory VFS, not the capability worlds Capability Manifest v0 measured | — |
| `v9r-runtime` | | | | | LEGACY | yes | none. Wasm agent hosting is outside the claim | — |
| `v9r-orchestrator` | | | | | LEGACY | yes | none | — |
| `v9r-cli` | | | | | LEGACY | yes | **none for the research claim**; the `v9r` binary and REPL (product) go | — |
| `examples/hello-agent`, `llm-gateway`, `task-runtime` | | | | | LEGACY | yes | none | — |
| `agents/` (two copies of one `.wasm`) | | | | | LEGACY | yes | none | — |
| `examples/*/target/` (5,318 files, ~1.04 GB) | | | | | build output | yes (untrack) | none | — |

Dependencies between crates: `cli → core`, `orchestrator → {core, vfs,
cap, runtime}`, `runtime → {core, cap, vfs}`, `cap → {core, vfs}`, and
`vfs → core` (for `VfsPath`, `VfsError`, `NodeId` and `Capability` from
`v9r-core/src/lib.rs`). **Nothing in `v9r-core` depends on another
crate**, so all five can go in one step.

---

## 3. `v9r-core` modules

### 3.1 Claim path

| Module | Code lines | E | D | S | T | Class | Phase 1 | If removed: guarantee lost | Becomes archive-only |
|---|---|---|---|---|---|---|---|---|---|
| `kernel.rs` | 604 | ● `Verified` mintable only in the crate | ● `evaluate` | | | CORE | **no** | all: claims could satisfy rules; unknown could become Allow | Invariant Kernel v0 |
| `runtime.rs` | 695 | ● the `transitions_accepted` fact | ● calls `evaluate` | | ● | CORE | **no** | single-use authorization, re-check on fresh state, hold after rejection | Effect Runtime v1 |
| `temporal.rs` | 529 | ● clock fact `transition.ordered`; snapshot evidence | | ○ moments, not content | ● | CORE | **no** | before/after judgment, the ordered window, double-read consistency, `@current` basis | Temporal Evidence v0 |
| `graph.rs` (registry, `Key`/`Term`, `Attestor`) | ~575 | ● | | | | CORE | **no** | answers bound to provider and request; unasked or replayed answers dropped | Evidence Graph v0 |
| `graph.rs` (`Lineage`, `Method`, `lineage_*`, `define`, `plan`) | ~200 | ○ | | | | HIST | no (Phase 2) | explanation "down to request and round" | Evidence Provenance v0 |
| `graph.rs` (`GraphDomain`, `graph::runtime`) | ~150 | | | | ○ | HIST | no: inside a kept file (Phase 2) | none: `TemporalDomain` covers it | Evidence Graph v0 |
| `verify.rs` | 96 | ● the verifier protocol `FsSnapshot` runs on | | | | CORE in Phase 1, removable in Phase 2 | no | in Phase 1, the snapshot cannot be derived without it | Verifiable Observers v0 |
| `snapshot.rs` (`ObjectStore`, `FsSnapshot`, `entries`, `materialize`) | ~500 | ● | | ● git tree SHA-256 | | CORE | **no** | input and output identity; the demo's test runner (`materialize`) | Content-Addressed State v0 |
| `snapshot.rs` (`Capture{double_walk, recheck, watch}`, `fs_meta`/`fs_watch`/`fs_changes` inputs) | ~150 | ○ | | ○ | | HIST | no: inside a kept file (Phase 2). The default `Capture` is all-off; only `tests/atomic_capture.rs` turns it on | none: capture is trusted anyway | Atomic Capture v0 |
| `snapshot.rs` (`SnapshotObjects`, `MatchesSnapshot`, `SnapshotCommitEquality`, `snapshot_from`) | ~180 | ○ | | ○ | | HIST | no: inside a kept file (Phase 2) | "anyone can verify a snapshot *inside v9r*" (git still can) | Content-Addressed State v0, Content Identity v0 |
| `fs_raw.rs` | 222 | ● `fs_dir`, `fs_file`, `fs_link`, `fs_stat` | | ○ via snapshot | | CORE | **no** | the only trusted fs observation, with `O_NOFOLLOW` per component | Verifiable Observers v0 |
| `verifiers.rs` | 423 | ● only `observed`, `Walk`, `hex` (used by `FsSnapshot`) ○ the rest | | ○ | | CORE (≈60 lines) / HIST (rest) | **gated**: move the four helpers to `verify.rs`/`snapshot.rs` first | none once moved. `GitObjects`, `FsContent` and `ContentEquality` go | Verifiable Observers v0 |

### 3.2 Off the claim path, but in its import closure today

| Module | Code lines | E | D | S | T | Class | Phase 1 | If removed: guarantee lost | Becomes archive-only |
|---|---|---|---|---|---|---|---|---|---|
| `fs_provider.rs` | 186 | ● `entries`, also `fs_file`/`fs_link`/`dir_content` | | ○ second content identity | | **DEMO** | **gated** on §5.2 | none, if `entries` is re-sourced. It is the **only** edge from the claim into the legacy stack | Evidence Graph v0 (fs provider) |
| `content.rs` | 94 | ○ normal form `v9r-content-manifest/1` | | ○ | | HIST | yes, after `verifiers` | none: identity is the git tree id | Verifiable Observers v0 |
| `state.rs` | 396 | ○ legacy crawler, `ContentHash` | | ○ | | LEGACY | yes, after `verifiers`/`fs_provider` | none | Effect Runtime v0 |
| `effect.rs` | 519 | ○ | | ○ | ○ effect runtime **v0** | LEGACY/HIST | yes | none | Effect Runtime v0 |
| `trusted.rs` | 158 | | | | ○ | LEGACY | yes | none | — |

### 3.3 Historical experiments

| Module | Code lines | E | D | S | T | Phase 1 | If removed: guarantee lost | Becomes archive-only |
|---|---|---|---|---|---|---|---|---|
| `capability.rs` | 1,808 | ○ | | ○ | | yes | Overview row "snapshot grants exact" | Capability Manifest v0, Content Identity v0, Snapshot Boundary v0 |
| `delegation.rs` (+ `tests.rs` 820) | 297 | ○ compiles to kernel forms | ○ | | | **gated** on moving the kernel guard (C1) | Overview row "delegated authority attenuates" | Capability Delegation v0 |
| `authority.rs` (+ `tests.rs` 858) | 648 | ○ Ed25519 | | | | yes | the same row (signed grants) | Capability Root of Trust v0 |
| `object_identity.rs` | 256 | ○ | | ○ | | yes | none | Capability Object Identity v0 |
| `fs_watch.rs` | 150 | ○ inotify | | | | yes | none | Atomic Capture v0 |
| `provenance.rs` | 275 | ○ explains | | | | yes | **narrows** the Overview row "explained down to observer, request and snapshot" to "observer and snapshot" | Evidence Provenance v0 |
| `git.rs` | 657 | ○ | | ○ | | **gated** on §4 | none for the claim | Git Evidence Domain v0 |
| `git_provider.rs` | 209 | ○ | | | | **gated** on §4 | none for the claim | Evidence Graph v0, Temporal Evidence v0 |
| `git_guard.rs` | 740 | ○ | ○ | | ○ `GitDomain` + `Compensable` | **gated** on §4 | none for the claim | Effect Runtime v1, Git Evidence Domain v0 |
| `counter.rs` | 295 | | | | ○ `CounterDomain` | yes | "the lifecycle is domain-neutral" stops being *tested*. It stays *recorded* | Effect Runtime v1 |
| `world_probe.py` | 340 | | | | | yes | none | Capability Inventory v0 |

### 3.4 Legacy product

| Module | Code lines | E | D | S | T | Phase 1 | If removed: what goes |
|---|---|---|---|---|---|---|---|
| `guarded.rs` | 538 | ○ | ○ | | ○ `FsDomain` + `Compensable` | yes | fs effects with rollback; the only producer of `Semantic` evidence |
| `policy.rs` | 390 | | ○ legacy invariants | | | yes | the original invariant set (Invariant Kernel v0 domain) |
| `facts.rs` | 227 | ○ `RuntimeFact` | | | | yes | — |
| `vfs.rs` | 698 | | | ○ checkpoint seals | ○ | yes | checkpoints and rollback |
| `execution.rs` | 436 | | | | ○ | yes | command runner |
| `adapter.rs` | 917 | | | | | yes | LLM HTTP clients (the `reqwest` dependency) |
| `trace.rs` | 251 | | | | | yes | event log |
| `task.rs` | 115 | | | | | yes | — |
| `context.rs` | 243 | | | | | yes | — |
| `bundle.rs` | 292 | | | ○ artifact hashes | | yes | bundle export and import |
| `manifest.rs` | 67 | | | | | yes | task allow-lists |
| `lib.rs` (`VfsPath`, `VfsError`, `NodeId`, `Capability`, `CapabilityId`) | ~180 | | | | | yes, with the 5 crates | — (no `v9r-core` module uses them) |

---

## 4. Tests: what each protects, and what must be ported first

### 4.1 Suites that stay

| Suite | Protects | Phase 1 |
|---|---|---|
| `kernel.rs` unit tests (10) | K1, K2, K3: unknown → Blocked, verified contradiction → Blocked, semantic and proposed never satisfy, `Within` is three-valued, determinism | keep |
| `temporal.rs` unit test `pairing_must_follow_the_clock` | ordered window | keep |
| the `runtime`, `graph`, `temporal`, `verify` and `provenance` source guards | the module boundaries | keep (`provenance`'s goes with its module) |
| `tests/state_vs_causality.rs` | the boundary result | keep, then **re-measure** after §5.2 |
| `examples/v9r_demo.rs` | ALLOW, DENY (scope), BLOCKED (claim only), DENY, REFUSED (drift) | keep. It is **not a test**: nothing asserts its verdicts in `cargo test` |

### 4.2 Claim guarantees whose only coverage is in suites Phase 1 would delete

| Guarantee | Only covered by | Needs git? | Action before deletion |
|---|---|---|---|
| kernel byte-identical (sha256 `85badb66…`) | `delegation/tests.rs::kernel_is_unchanged_and_knows_no_delegation` | no | **move** to `tests/kernel_guard.rs` (C1) |
| volunteered or unasked attestations are discarded | `graph.rs::volunteered_attestations_are_discarded` | yes (fixture) | port to an fs fixture |
| a replayed raw observation fails temporal binding | `verifiable.rs::replayed_raw_observation_fails_temporal_binding` | no direct git in the test body; registry fixture uses git | port |
| the before snapshot replayed as after is not believed | `temporal.rs::before_snapshot_replayed_as_after_is_not_believed` | yes | port |
| disagreeing verified providers → Blocked | `graph.rs::disagreeing_providers_block`, `temporal.rs::providers_observing_different_versions_block` | yes | port one |
| a missing provider → Blocked, never Deny (at runtime level) | `graph.rs::removing_any_provider_blocks_with_missing_evidence_never_denies` | yes | port |
| post-state unobservable → Blocked, and hold | `temporal.rs::provider_unavailable_after_effect_blocks_and_holds` | yes | port |
| change between authorization and execution → refused | `temporal.rs::external_modification_between_authorization_and_execution_refuses`; demo run 5 (unasserted) | yes / no | turn the demo into a test that asserts all 5 verdicts |
| snapshot id equals git's tree id | `content_addressed.rs::a_snapshot_is_the_git_tree_of_the_same_content` | git binary (external), registry fixture uses `GitObjects` | port with a fixture that has no `GitObjects` |
| a claimed snapshot id is recomputed, not believed | `content_addressed.rs::a_claimed_snapshot_id_is_recomputed_not_believed` | fixture | port |
| unreadable file → snapshot incomplete (Blocked) | `content_addressed.rs::unreadable_file_makes_the_snapshot_incomplete` | fixture | port |
| a hidden directory omitted by the observer → Blocked (link-count check) | `content_addressed.rs::hidden_directory_omitted_by_the_observer_blocks` | fixture | port |

These are 12 guarantees. One is a move, one turns the demo into a
test, and 10 are ports onto one shared fs-only fixture (`RawFsObserver` +
`FsSnapshot` + `ObjectStore`), roughly 400–500 test lines in total.
**Without these ports, Phase 1 removes tests, not just code.**

### 4.3 Suites that become archive-only

`atomic_capture`, `capability_manifest`, `content_identity`,
`controlled_domain`, `object_identity`, `snapshot_boundary`, `counter`,
`effects`, `git`, `guarded`, `perf`, `provenance`, `authority/tests.rs`,
`delegation/tests.rs` (after C1), and the parts of `graph`, `temporal`,
`verifiable` and `content_addressed` not ported in §4.2.

---

## 5. Dependency graphs

### 5.1 Today: the claim's import closure

The demo and the causality test import 7 modules. Following
`crate::` references from those 7 reaches **21 modules**:

```text
 demo / state_vs_causality
   ├── kernel
   ├── runtime ─────────────▶ kernel
   ├── temporal ────────────▶ kernel, runtime, graph
   ├── graph ───────────────▶ kernel, runtime, verify
   ├── verify ──────────────▶ graph
   ├── fs_raw ──────────────▶ graph
   ├── snapshot ────────────▶ graph, verify, verifiers
   │                                          │
   │                          verifiers ──────┴──▶ content ──▶ state ──▶ trusted
   │
   └── fs_provider ═════════▶ content, state, graph,
                     ║        effect ──▶ facts, manifest, state, vfs
                     ║                    vfs ──▶ effect, execution, state, trace, trusted
                     ║                    facts ──▶ content, effect, state, trace, vfs
                     ║                    execution ──▶ effect, manifest, state, task, trace, vfs
                     ║                    trace ──▶ effect, manifest, runtime, state, task, trusted, vfs
                     ║                    task ──▶ bundle, context, manifest, trace, trusted
                     ║                    bundle ──▶ manifest, task, trace, trusted, vfs
                     ║                    context ──▶ manifest, task, trace
                     ╚═ the one accidental edge (it serves only `entries(ws)`)
```

The legacy modules form one cycle (`effect ↔ vfs ↔ trace ↔ task ↔
bundle …`). They can only be removed together.

### 5.2 The cut: re-sourcing `entries`

The scope invariant (I2) needs the names that changed between before
and after. Options:

| Option | Change | Soundness | Size |
|---|---|---|---|
| **A (recommended)** | a provider `tree_entries(tree)` over `ObjectStore`, using the existing `snapshot::entries`. I2 pins `snapshot(ws)` before and after, then reads `tree_entries` of both tree ids | content-addressed: every object is checked against its id, and **scope and identity come from the same walk** | ~40 lines in `snapshot.rs`; both test files change their I2 |
| B | I2 calls `snapshot::entries(store, id)` directly on the pinned ids | sound, but the names are not evidence and the explanation names no observer | ~0 library lines |
| C | keep `fs_provider` and rewrite it over `fs_raw` | sound | ~150 lines, and a second fs walk remains |

With A, the measured `entries` values in STATE_VS_CAUSALITY_V0 change
form (`file:<sha256>` becomes git modes and blob ids). The report's
conclusion must be **re-measured**, not assumed.

### 5.3 Phase 1 target

```text
 tests: kernel_guard, kernel (unit), fs fixture ports (§4.2),
        state_vs_causality, demo-as-test
 example: v9r_demo
   │
   ▼
 temporal ──────────▶ runtime ──▶ kernel
    │                                ▲
    └──▶ graph ─────▶ runtime ───────┘
           │   ▲
           ▼   │
         verify          (Verifier protocol, + observed/Walk/hex from verifiers)
           ▲
           │
 snapshot ─┴──▶ graph    (ObjectStore, FsSnapshot, entries, materialize,
                          + tree_entries provider [option A])
 fs_raw ──────▶ graph    (fs_dir, fs_file, fs_link, fs_stat)
```

| | Today (closure) | Phase 1 |
|---|---|---|
| modules | 21 of 35 reachable | **7** (`kernel`, `runtime`, `temporal`, `graph`, `verify`, `snapshot`, `fs_raw`) |
| library code lines | ~8,000 reachable | ~3,900 (+ ~100 moved/added) |
| crates | 6 | 1 |
| `v9r-core` dependencies used | 16 declared | `serde`, `sha2`, `libc`, plus `tokio` for the async runtime. The others can be removed from `Cargo.toml` |
| cycles | legacy cycle | none |
| claim guarantees tested | 12 listed in §4.2 and the kernel tests | the same 12, on fs fixtures |

Phase 1 keeps three things inside its kept files and leaves them for
Phase 2: `GraphDomain`, `Lineage`, and the archived verifiers and
capture options in `snapshot.rs`.

---

## 6. Phase 1 order, with gates

| Step | Action | Gate (must pass before the next step) |
|---|---|---|
| 0 | tag `v9r-archive-v0`; untrack `examples/*/target` and duplicate `.wasm` | `git status` shows only removals |
| 1 | move the kernel guard to `tests/kernel_guard.rs` (C1) | guard passes with the same hash |
| 2 | make the demo assert its 5 verdicts (as a test) | passes |
| 3 | build the fs fixture and port the 10 cases in §4.2 | all pass **while the git suites still exist** (same verdicts on both) |
| 4 | add `tree_entries` (option A); switch I2 in the demo and causality test | demo: same 5 verdicts. Causality: "every measured field identical" still holds. Record the new values |
| 5 | move `observed`/`Walk`/`hex` (and `digest_of`, if `SnapshotObjects` stays for now) out of `verifiers.rs` | builds |
| 6 | delete the 5 crates, `examples/*`, `agents/`, `task.toml` | `cargo build` |
| 7 | delete §3.4, §3.3, `fs_provider`, `content`, `state`, `effect`, `trusted`, `verifiers` and their suites (§4.3); remove `VfsPath` & co. from `lib.rs` | `cargo test --workspace`: kernel guard, ports, demo, causality all pass |
| 8 | move archived reports to `research/archive/`; record the new counts in the Overview | — |

`kernel.rs` is not modified at any step.
