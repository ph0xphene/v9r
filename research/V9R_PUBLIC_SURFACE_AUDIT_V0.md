# v9r Public Surface Audit v0

*Every public top-level item of `v9r-core` (125), classified by who needs
it. Nothing was removed. Measured on the Phase 1 tree, 2026-10-02.*

## 0. Method

**What was scanned:**

- `pub` items before `#[cfg(test)]` in each module of
  `crates/v9r-core/src`. `pub(crate)` items are not public.
- For each item, the files that name it outside comments: the demo
  (`examples/v9r_demo.rs`), State vs Causality
  (`tests/state_vs_causality.rs`), the guarantee tests
  (`claim_guarantees`, `scope_entries`, `content_addressed`,
  `kernel_guard`), and the historical consumers (`counter`,
  `atomic_capture`, `controlled_domain`, the four product-phase crates).
- Then a manual pass for items that no consumer names but that appear
  in a needed item's signature: associated types, return types, trait
  bounds.

**Classes**, in priority order (the first that applies is listed first;
the others follow):

| Class | Meaning |
|---|---|
| **D** | named by the demo |
| **C** | named by State vs Causality |
| **G** | named by (or reached only through) a guarantee test |
| **S** | structural: no consumer names it, but a D/C/G item cannot be used without it (signature, associated type, bound, the verdict function itself) |
| **H** | needed only for historical compatibility (archived experiments, product-phase crates, frozen kernel features nothing produces) |
| **X** | dead: no caller anywhere |

## 1. Result

| Class | Items |
|---|---|
| D (demo) | 31 |
| C only (State vs Causality) | 1 (`Verdict`; the rest of what SvC names is also in the demo) |
| G | 24 |
| S | 45 |
| H | 21 |
| X | 3 (`snapshot_from`, `snapshot::entries`, `SnapEntry`) |
| **total** | **125** |

### 1.1 The smallest public API for the claim

**State vs Causality alone needs 22 named items:**

```text
fs_raw::RawFsObserver
graph::{Key, Term, Registry, Trust}
snapshot::{FsSnapshot, ObjectStore}
kernel::{Obligation, Phase, Requirement, Strength, Status, Verdict}
runtime::Authorize
temporal::{runtime, Actor, TransitionInvariant, Transition, Snapshot, TKey, At,
           TObligation, require, pin, changed}
```

**The demo adds 10 more:**

- a third-party observer, for the test runner:
  `graph::{EvidenceProvider, Attestor, Answer}`;
- the agent's claim: `kernel::{Proposed, Fact}`;
- rendering: `kernel::Decision`;
- checking out a tree: `snapshot::materialize`.

**Methods they call:**

- `Registry::{new, register, restrict, add_verifier, query}`;
- `Runtime::{authorize, execute, add_proposed, is_accepting, journal, domain}`;
- `Snapshot::{id, verified}`;
- `Authorize::{verdict, decision}`;
- `MemoryJournal::records`;
- the fields of `Report` and `TransitionReceipt`.

So the **reviewer-facing surface is 32 items**. It rests on 45
structural ones: chiefly the generic runtime (`EffectDomain`,
`Rt*`/`Domain*` aliases, `Stage`, `Concluded`, `Journal`), the
verifier protocol (`Verifier`, `Step`, `Basis`, `Candidate`, `Inputs`)
and the kernel's evidence types. Most of the structural set exists
because the runtime is generic over domains. That is a Phase 2
question, not removed here.

### 1.2 What the 24 guarantee-only items are

| Group | Items | Guarantee |
|---|---|---|
| declaration harness | `graph::{GraphDomain, GraphRuntime, runtime}`, `kernel::Invariant` | G2, G6, G7, G9 ports judged as declarations |
| registry and kind rules | `graph::{Attested, Discarded, Plan, Method}` | G7 (binding), host declarations |
| explanations | `graph::{Lineage, lineage_token, lineage_ids}`, `provenance::{explain, Explanation, Explained, Source}` | G10 |
| content-addressed verification | `snapshot::{SnapshotObjects, MatchesSnapshot, IDENTITY}`, `verifiers::MANIFEST`, `content::{ContentHash, DEFINITION, Item, ItemKind, digest}` | G4, G5 |

### 1.3 Historical only (21)

| Items | Belongs to |
|---|---|
| `counter::*` (9) | Effect Runtime v1 |
| `fs_watch::WatchObserver`, `snapshot::Capture` | Atomic Capture v0 |
| `lib.rs` `VfsPath`, `VfsError`, `VfsResult`, `Capability`, `CapabilityId` | product-phase crates |
| `runtime::{Compensable, ACCEPTED, BASIS}` | Effect Runtime v1 |
| `kernel::Semantic` | Invariant Kernel v0; frozen |
| `verifiers::NAMES` | Verifiable Observers v0 |

(`NodeId` is declared by a macro and so is not in the scan. It belongs
with the `lib.rs` types.)

### 1.4 Dead (3)

`snapshot::snapshot_from`, `snapshot::entries`, `snapshot::SnapEntry`:
no caller in the workspace. The methods with no caller outside the
crate, `Runtime::add_semantic`, `Authorization::proposal` and
`Decision::undetermined`, are listed in V9R_ARCHIVE_BOUNDARY_V0 §3.

## 2. Full table

| module | item | kind | class | note |
|---|---|---|---|---|
| `content` | `DEFINITION` | const | G (via SnapshotObjects) | content-manifest normal form |
| `content` | `ItemKind` | enum | G (via SnapshotObjects) |  |
| `content` | `Item` | struct | G (via SnapshotObjects) |  |
| `content` | `digest` | fn | G (via SnapshotObjects) |  |
| `content` | `ContentHash` | struct | G | `content_addressed` names it |
| `counter` | `Fault` | enum | H | Effect Runtime v1 |
| `counter` | `CounterCell` | struct | H | Effect Runtime v1 |
| `counter` | `CounterSubject` | enum | H | associated type of `CounterDomain` |
| `counter` | `Increment` | struct | H | Effect Runtime v1 |
| `counter` | `CounterReceipt` | struct | H | associated type of `CounterDomain` |
| `counter` | `CounterError` | enum | H | associated type of `CounterDomain` |
| `counter` | `CounterDomain` | struct | H | Effect Runtime v1 |
| `counter` | `CounterRuntime` | type | H | Effect Runtime v1 |
| `counter` | `runtime` | fn | H | Effect Runtime v1 |
| `fs_raw` | `RawFsObserver` | struct | D C G H |  |
| `fs_watch` | `WatchObserver` | struct | H | Atomic Capture v0 |
| `graph` | `Key` | struct | D C G H |  |
| `graph` | `Term` | enum | D C G H |  |
| `graph` | `GraphFact` | type | S | `Attested::fact` returns it |
| `graph` | `GraphEvidence` | type | S | `Registry::collect` returns it; carries the `map` doctest |
| `graph` | `EvidenceProvider` | trait | D G H |  |
| `graph` | `Method` | struct | G (S) | `Attestor::attest` takes `impl Into<Method>` |
| `graph` | `LineageId` | type | S (G) | field type of `Lineage` |
| `graph` | `Lineage` | struct | G |  |
| `graph` | `lineage_token` | fn | G |  |
| `graph` | `lineage_ids` | fn | G |  |
| `graph` | `Attestor` | struct | D G H |  |
| `graph` | `Attested` | struct | G (S) | `Answer::Verified` holds it; tests build replaying providers from it |
| `graph` | `Answer` | enum | D G H |  |
| `graph` | `Trust` | enum | D C G H |  |
| `graph` | `Discarded` | struct | G (S) | `Registry::discarded` returns it; 22 assertions |
| `graph` | `Plan` | struct | G | `Registry::plan`; one test |
| `graph` | `Registry` | struct | D C G H |  |
| `graph` | `GraphDomain` | struct | G |  |
| `graph` | `GraphError` | enum | S | error type of the temporal and graph runtimes; demo/SvC `.unwrap()` it |
| `graph` | `GraphRuntime` | type | G |  |
| `graph` | `runtime` | fn | G |  |
| `kernel` | `Verified` | struct | S | only the crate can mint it: the core of G1 |
| `kernel` | `Semantic` | struct | H | no producer since `guarded` (Invariant Kernel v0); kernel frozen |
| `kernel` | `Proposed` | struct | D | the agent's claim in run 3/4 |
| `kernel` | `EvidenceClass` | enum | S | `Evidence::class`; used by temporal double-read |
| `kernel` | `Fact` | struct | D |  |
| `kernel` | `Provenance` | struct | S | observer and basis of every verified fact |
| `kernel` | `Evidence` | enum | S (G) |  |
| `kernel` | `EvidenceBase` | struct | S | evidence container |
| `kernel` | `Strength` | enum | D | every `Fact` names `Hard`; `Soft` is idle |
| `kernel` | `Name` | enum | S (G) | `changed` returns it |
| `kernel` | `Requirement` | enum | D C G |  |
| `kernel` | `Phase` | enum | D C G H |  |
| `kernel` | `Obligation` | struct | D C G |  |
| `kernel` | `Invariant` | trait | G | `graph::runtime` takes it (declaration harness) |
| `kernel` | `Status` | enum | D C G H |  |
| `kernel` | `Verdict` | enum | C |  |
| `kernel` | `Finding` | struct | S (D) | demo iterates `decision.findings` |
| `kernel` | `Decision` | struct | D G H |  |
| `kernel` | `DecisionRecord` | struct | S (C) | journal entries; SvC compares the journal |
| `kernel` | `FindingRecord` | struct | S (C) | field of `DecisionRecord` |
| `kernel` | `evaluate` | fn | S | the verdict function; called by `runtime` |
| `kernel` | `name_within` | fn | S | scope containment; used by `check_within` |
| `lib` | `VfsError` | enum | H | product-phase crates only |
| `lib` | `VfsResult` | type | H | product-phase crates only |
| `lib` | `VfsPath` | struct | H | product-phase crates only |
| `lib` | `CapabilityId` | struct | H | product-phase crates only |
| `lib` | `Capability` | struct | H | product-phase crates only |
| `provenance` | `Source` | enum | G |  |
| `provenance` | `Explained` | struct | G (S) | field of `Explanation` |
| `provenance` | `Explanation` | struct | G |  |
| `provenance` | `explain` | fn | G |  |
| `runtime` | `RuntimeSubject` | enum | S | runtime facts in every decision |
| `runtime` | `RuntimeValue` | enum | S |  |
| `runtime` | `RtSubject` | enum | S (G) | subject type of every decision |
| `runtime` | `RtValue` | enum | S |  |
| `runtime` | `RtObligation` | type | S |  |
| `runtime` | `RtDecision` | type | S (G) | type of every decision |
| `runtime` | `RtEvidence` | type | S |  |
| `runtime` | `DomainEvidence` | type | S | `EffectDomain` signatures |
| `runtime` | `DomainFact` | type | S |  |
| `runtime` | `DomainObligation` | type | S |  |
| `runtime` | `ACCEPTED` | const | H | named only by `tests/counter.rs`; demo uses the string |
| `runtime` | `BASIS` | const | H | same |
| `runtime` | `Stage` | enum | S | `EffectDomain::obligations` argument |
| `runtime` | `Concluded` | struct | S |  |
| `runtime` | `EffectDomain` | trait | S | `TemporalDomain` implements it |
| `runtime` | `Compensable` | trait | H | no implementor; `counter` doctest only |
| `runtime` | `Journal` | trait | S | bound of `Runtime<D, J>` |
| `runtime` | `MemoryJournal` | struct | S (C) | SvC reads `journal().records()` |
| `runtime` | `RuntimeFault` | enum | S | `From` bound of domain errors |
| `runtime` | `Authorization` | struct | S (G) | held in `Authorize::Allowed`; consumed by `execute` |
| `runtime` | `Authorize` | enum | D C G H |  |
| `runtime` | `Report` | struct | S (G) | returned by `execute`; demo/SvC read its fields |
| `runtime` | `Runtime` | struct | S | `TemporalRuntime` is `Runtime<…>` |
| `snapshot` | `IDENTITY` | const | G |  |
| `snapshot` | `ObjectStore` | struct | D C G H |  |
| `snapshot` | `snapshot_from` | fn | X | no caller |
| `snapshot` | `SnapEntry` | enum | X | no caller |
| `snapshot` | `entries` | fn | X | no caller |
| `snapshot` | `materialize` | fn | D |  |
| `snapshot` | `Capture` | struct | H | Atomic Capture v0 |
| `snapshot` | `FsSnapshot` | struct | D C G H |  |
| `snapshot` | `SnapshotObjects` | struct | G |  |
| `snapshot` | `MatchesSnapshot` | struct | G |  |
| `temporal` | `SnapshotId` | type | S | `Snapshot::id` |
| `temporal` | `At` | enum | D C G |  |
| `temporal` | `TKey` | struct | D C G |  |
| `temporal` | `TObligation` | type | D C G |  |
| `temporal` | `Snapshot` | struct | D C G |  |
| `temporal` | `Transition` | struct | D C G |  |
| `temporal` | `TransitionInvariant` | trait | D C G |  |
| `temporal` | `require` | fn | D C G |  |
| `temporal` | `pin` | fn | D C G H |  |
| `temporal` | `changed` | fn | D C G H |  |
| `temporal` | `Actor` | trait | D C G |  |
| `temporal` | `Acted` | struct | S | associated type of `TemporalDomain` |
| `temporal` | `TransitionReceipt` | struct | S (C) | SvC reads `receipt.before/after/done/result` |
| `temporal` | `TemporalDomain` | struct | S (G) |  |
| `temporal` | `TemporalRuntime` | type | S (G) |  |
| `temporal` | `runtime` | fn | D C G |  |
| `verifiers` | `MANIFEST` | const | G |  |
| `verifiers` | `NAMES` | const | H | Verifiable Observers v0; no test asks for it |
| `verify` | `Candidate` | struct | S | `FsSnapshot` is a `Verifier`; demo/SvC register it |
| `verify` | `Inputs` | type | S |  |
| `verify` | `Basis` | enum | S |  |
| `verify` | `Step` | enum | S |  |
| `verify` | `Verifier` | trait | S | `Registry::add_verifier` takes it |
| `verify` | `vouched` | fn | S | used by `verifiers::observed` |


## 3. Caveats

- **The scan is textual.** A name in a string literal counts as a use.
  The manual pass corrected the cases found (e.g. `Verified`).
- **Some methods are public but cannot be used from outside.**
  `EvidenceBase::add_verified` takes `Verified`, which only the crate can
  construct. Such methods are counted under their type's class.
- **"Dead" means no caller in this workspace,** not that no downstream
  crate could call it. There are no published downstream crates.
