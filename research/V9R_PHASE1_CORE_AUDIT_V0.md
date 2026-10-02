# v9r Phase 1 Core Audit v0

*Post-extraction audit of the Phase 1 tree. It audits what remains; it
does not plan Phase 2. "LOC" is code lines before `#[cfg(test)]` (unit
tests excluded). Dependency edges come from `crate::<module>`
references; test edges from `v9r_core::<module>` imports. Measured on
2026-10-02.*

## 0. The claim, and what it needs

> An accepted state satisfies declared invariants relative to an
> approved prior state, on evidence from registered observers, within an
> ordered time window, under a fresh single-use authorization. Who
> produced the state is not observed. (*v9r verifies state transitions,
> not histories.*)

What the claim needs, and where each piece lives:

| Need | Where |
|---|---|
| a verdict that never turns "unknown" into true or false | `kernel` |
| evidence that only trusted code can mint | `kernel::Verified`, `graph::Attestor` |
| answers bound to the provider and request that produced them | `graph::Registry` |
| an exact identity for a state | `snapshot::FsSnapshot`, over `fs_raw` |
| the names that changed | `snapshot_entries`, same walk |
| before and after, on one clock, each read twice | `temporal` |
| one-shot authorization, re-checked fresh; hold after rejection | `runtime` |

The dependency path the demo and `state_vs_causality` exercise:

```text
 temporal ──▶ runtime ──▶ kernel
    │                       ▲
    └──▶ graph ──▶ runtime ─┘
           │  ▲
           ▼  │
          verify ◀── snapshot ──▶ verifiers ──▶ content
           ▲            │
           └── fs_raw ◀─┘ (observations, via the registry)

 off the path:  counter ──▶ runtime, kernel
                fs_watch ──▶ fs_raw, graph
                provenance ──▶ graph, kernel
```

No cycles. `kernel` depends on nothing in the crate. Its doc comment
still names `crate::policy` (removed); it is text only, and the kernel is
frozen.

## 1. Classification

The four classes:

- **E**: essential to the claim.
- **S**: supporting infrastructure, which tests, explains or
  demonstrates the claim.
- **H**: historical residue.
- **R**: removable now, because nothing uses it.

| Module | LOC | Class | Why it exists | Guarantee / test that depends on it | Removing it changes the claim? |
|---|---|---|---|---|---|
| `kernel` | 604 | **E** | three-valued evaluation of `Fact`/`Within`/`AtMost` obligations; `Verified` is crate-private | every verdict; 13 unit tests; `kernel_guard` | **yes**: nothing decides |
| `runtime` | 689 | **E** | lifecycle: authorize, re-check on fresh observation, execute, judge, accept or hold; `transitions_accepted`, `authorization_basis_current` | single-use authorization, refusal on drift, hold; `claim_guarantees` (refusal, hold), demo run 5 | **yes** |
| `temporal` | 529 | **E** | snapshots on one monotonic clock, double reading, `transition.ordered`, `pin`/`changed`, `@current` basis | "within an ordered window", "relative to an approved prior state"; `state_vs_causality` (2), `scope_entries`, `claim_guarantees` (8), demo | **yes** |
| `graph`: `Key`, `Term`, `Registry`, `Attestor`, `Trust`, discards | ~575 | **E** | binds every answer to provider and request; drops replayed, unasked or foreign answers; registry of observers | "evidence from registered observers"; `claim_guarantees` (replay, volunteering, claims-only, disagreement, missing observers) | **yes**: an agent's claim could become evidence |
| `graph`: `restrict`, `define` (kind rules) | ~40 | S | host declares who may answer a kind and under which definition | `evidence_copied_between_domains_…`, `a_declared_definition_…`; `restrict` calls in demo/causality (now redundant) | no; it narrows the trusted set further |
| `graph`: `Lineage`, `lineage_*`, `assign_snapshot`, `plan` | ~170 | S | records who/which request/which round/which snapshot established a provider fact | `claim_guarantees` (5 provenance tests); `removing_any_observer_…` (`plan`) | no; it narrows "explained down to request and snapshot" |
| `graph`: `GraphDomain`, `graph::runtime` | ~150 | S | a declaration judged twice on fresh evidence, with no effect | harness for 18 ports in `claim_guarantees`; `content_addressed` | no; `temporal` is the claim's runtime |
| `verify` | 96 | **E** (as implemented) | the protocol `FsSnapshot` runs on: verifier asks for inputs, registry vouches | every `snapshot(…)` fact | **yes** as built: no snapshot can be derived |
| `snapshot`: `ObjectStore`, `FsSnapshot` (tree id and `snapshot_entries`), `materialize` | ~560 | **E** | state identity (git-compatible SHA-256 tree id); scope entries from the same walk; demo's test runner checks out trees | input and output identity, scope; `content_addressed`, `scope_entries`, causality, demo | **yes** |
| `snapshot`: `SnapshotObjects`, `MatchesSnapshot`, `IDENTITY` | ~110 | S | verify a stored snapshot from its objects alone; compare a live dir with an approved one | `content_addressed` (7 tests: claimed id recomputed, lost or altered object, two definitions, …) | no; it narrows "checkable afterwards by anyone" to "checkable by git" |
| `snapshot`: `Capture { double_walk, recheck, watch }`, `fs_meta`/`fs_watch` inputs | ~120 | H | Atomic Capture v0 strategies; off by default | `atomic_capture` (9 + 3 ignored) | no |
| `snapshot`: `snapshot_from`, `entries`, `SnapEntry` | ~90 | **R** | `snapshot_from` served capability worlds; `entries` was meant to source scope (option A) but `snapshot_entries` comes from the walk | **none** | no |
| `fs_raw`: `fs_dir`, `fs_stat`, `fs_file`, `fs_link` | ~200 | **E** | the one trusted filesystem observation; `openat(O_NOFOLLOW)` per component | everything filesystem | **yes** |
| `fs_raw`: `fs_meta` | ~15 | H | Atomic Capture `recheck`, object-identity experiments | `atomic_capture` | no |
| `verifiers`: `Walk`, `observed`, `hex` | ~45 | **E** | `FsSnapshot`'s view of vouched inputs | via `snapshot` | **yes** as built |
| `verifiers`: `object_at`, `tree_entries`, `walk_tree`, `digest_of`, `MANIFEST`, `NAMES` | ~150 | S | hash-checked tree walking for `SnapshotObjects`/`MatchesSnapshot` | `content_addressed` | no |
| `content` | 97 | S | the `v9r-content-manifest/1` normal form; `ContentHash` | `SnapshotObjects` (`snapshot_digest`), `MatchesSnapshot(MANIFEST)`; `content_addressed` (2 tests) | no |
| `provenance` | 275 | S | `explain`: resolves lineage, audits snapshot binding and definitions | `claim_guarantees` (lineage, snapshot audit, claimed state) | no; explanations only. Provenance v0 measured that it explains without making anything true |
| `counter` | 295 | H | in-memory domain that falsified "the runtime is domain-neutral" | `counter` (8 tests + 2 doctests); the runtime's generic `EffectDomain` | no |
| `fs_watch` | 150 | H | inotify change journal for capture | `atomic_capture` | no |
| `lib.rs`: module list | ~15 | E | crate root | all | — |
| `lib.rs`: `VfsPath`, `VfsError`, `NodeId`, `Capability`, `CapabilityId` | ~150 | H | types of the in-memory VFS and capability crates | `v9r-vfs`, `v9r-cap`, `v9r-runtime`, `v9r-orchestrator`; 5 unit tests | no; these exist only for the archived crates (§2) |

**Totals:**

- `v9r-core`: 5,079 LOC.
- **E: ~3,560.** Kernel, runtime, temporal, the registry part of graph,
  verify, the claim part of snapshot, fs_raw, three verifier helpers.
- **S: ~1,130.**
- **H: ~730.**
- **R: ~90.** Some modules split across classes, so the totals
  overlap a little.

### 1.1 Dead or idle API inside kept modules (not removed: rule 3)

| Item | State | Note |
|---|---|---|
| `snapshot::snapshot_from`, `snapshot::entries`, `SnapEntry` | no caller | **R** |
| `runtime::Runtime::add_semantic`; `kernel` `Semantic`, `Strength::Soft` | nothing produces semantic evidence since `guarded` went | kernel part frozen |
| `runtime::Compensable`, `compensate`, `Stage::Compensated` | no implementor; referenced only by `counter`'s `compile_fail` doctest (proving the counter *cannot* compensate) | H |
| `kernel::Requirement::AtMost` | no obligation uses it on the claim path (`counter` does) | frozen |
| `kernel` `compile_fail` example on `EvidenceBase::map` | vacuous since `facts` was removed; replaced on `graph::GraphEvidence` | frozen |

## 2. Remaining crates and artifacts

### 2.1 Decisions

All four crates come from commit `059ef29` ("implement transactional
runtime", 2026-05-11). That is the project's first product phase, before
the research branch. EFFECT_RUNTIME_V0 recorded at the time that they are
"Not involved in the host-filesystem transaction path". No research
report measured them.

| Item | Size | Belongs to | Part of the claim? | Decision | Done now? |
|---|---|---|---|---|---|
| `crates/v9r-vfs` | 827 lines, 7 tests | product phase: in-memory VFS (slotmap arena) and write bus | no. v9r judges the host filesystem through `fs_raw` | **ARCHIVE** | no: rule 3 allows removing only obvious residue. Recommended as its own commit |
| `crates/v9r-cap` | 363 lines, 11 tests | product phase: Plan-9-style namespace views over `v9r-vfs` | no. Not the capability *worlds* of Capability Manifest v0 (those were `capability.rs`, removed) | **ARCHIVE** | no (`tracing` dependency removed: unused) |
| `crates/v9r-runtime` | 1,161 lines, 8 tests | product phase: Wasmtime agent host | no. Agents are opaque `Actor`s to the verifier | **ARCHIVE** | no |
| `crates/v9r-orchestrator` | 965 lines, 11 tests | product phase: VFS write events → agent handlers | no | **ARCHIVE** | no |
| `examples/hello-agent` | 3 source files, 13 tracked build files | product phase: Wasm agent for `v9r-runtime` | no. Not a workspace member; no test loads it (runtime tests use inline WAT) | **DELETE** (dead example) | **yes** |
| `examples/llm-gateway` | 4 source files, 88 tracked build files | product phase: Wasm LLM gateway agent | no. Same as above | **DELETE** (dead example) | **yes** |
| `agents/llm_gateway.wasm`, `agents/llm_gateway/{module.wasm, manifest.toml}` | 2 byte-identical 136 KB binaries | product phase: compiled `llm-gateway`, deployed for the orchestrator | no. Nothing loads them (orchestrator tests use inline TOML) | **DELETE** (dead build artifact) | **yes** |
| tracked `examples/*/target/**` | 101 files, ~16.6 MB (was 5,318, ~1.04 GB before Phase 1) | build output committed before `.gitignore` covered it | no | **DELETE** | **yes**: none remain tracked |
| `task.toml` (repo root) | 10 lines | the removed CLI's default manifest | no | **DELETE** (dead artifact) | **yes** |
| `README.md` | 275 lines | described the removed CLI/REPL/bundles/providers | — | **REWRITE** (stale references) | **yes**: now describes the verifier |
| `research/freeze.sh` | one-shot script | the `v9r-review-v0` freeze | no. It now stops at its own precondition (the tag exists) | KEEP as historical record | no |
| `.gitignore` entries `trace.log`, `*.bundle`, `.v9r-workdir`, `.v9r/`, `task.txt`, `*.jsonl` | — | the removed CLI's outputs | no; harmless | KEEP | no |
| repository history | `.git` 301 MB | the gigabyte of build output is still in history | — | out of scope (no history rewrite) | no |

### 2.2 What would change if the four crates were archived

- `lib.rs` would lose `VfsPath`, `VfsError`, `NodeId`, `Capability` and
  `CapabilityId` (~150 lines, 5 tests).
- `v9r-core` would lose `slotmap` and `thiserror`.
- The workspace would lose `wasmtime`, `wasmtime-wasi`, `wat`, `reqwest`,
  `toml`, `bytes`, `async-trait`, `anyhow` and `tracing`.
- **Claim:** unchanged.
- **Tests:** 37 tests (11 + 11 + 8 + 7) leave with them.

## 3. Residue removed in this step

| Removed | Why it is not part of the core claim | Historical origin |
|---|---|---|
| `examples/hello-agent`, `examples/llm-gateway` (incl. 101 tracked build files) | Wasm agents for the product-phase agent host; not built, not loaded | product phase, `059ef29` |
| `agents/` (two copies of one `.wasm` + manifest) | compiled output of `llm-gateway`; not loaded | product phase, `059ef29`; last touched `221994d` |
| `task.toml` | input of the removed CLI | product phase |
| `tracing` in `v9r-cap` | no use in its source | product phase |
| stale README | described the removed product | product phase |

Nothing in `crates/v9r-core/src` changed in this step. `kernel.rs` is
byte-identical (sha256 `85badb66…6177f`).

## 4. Evidence added in this step

`state_vs_causality.rs::identical_s1_gets_identical_decisions_whoever_wrote_it`:

- **Setup:** S0, the authorized agent writes S1 (World A); the agent
  writes nothing while an unauthorized thread writes byte-identical S1
  (World B).
- **What it compares:** the **complete serialized decisions**: PRE, POST
  and the journal, every finding and reason. Counters are replaced by
  their role: the two snapshot ids, the effect's clock reading and
  lineage ids. The test also asserts that no raw snapshot id survives
  normalization.
- **Result:** A = B exactly.
- **Not trivial:**
  - the raw decisions differ (A ≠ B before normalization);
  - a control World C, whose outsider writes *different* bytes, gets the
    same PRE but a different POST (Deny).
- **Stability:** passed 3 runs in a row.

## 5. Counts

| | |
|---|---|
| `v9r-core` LOC | 5,079 (5,623 with unit tests) |
| `v9r-core` tests and example | 4,813 lines |
| other crates (to archive) | 3,316 lines |
| all Rust under `crates/` | 13,752 lines |
| tracked files | 72 |
| workspace tests | **128 passed, 0 failed, 4 ignored** |
