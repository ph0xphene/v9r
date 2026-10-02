# v9r Debloat Plan v0

*Analysis only. Nothing below has been implemented. Line counts are from
`wc -l` on the tree at `da6d694` (code lines = lines before
`#[cfg(test)]`).*

## 0. Verdict

The claim fits in about **1,000 lines**. The repository has about
**35,000** lines of Rust/Python, **9,000** lines of research prose, and
about **1 GB of committed build output**.

- The demo and `state_vs_causality`, which *are* the claim, import 7
  modules: `kernel`, `runtime`, `temporal`, `graph`, `snapshot`,
  `fs_raw` and `fs_provider`.
- `fs_provider` pulls in the legacy "transactional shell"
  (`effect` → `vfs` → `trusted`, `trace`, `task`, `manifest`, `state`).
  So the claim's dependency closure is most of `v9r-core`. That coupling
  is accidental: it serves one fact kind (`entries`).
- Five of the six workspace crates, and 27 of the 35 `v9r-core` modules,
  are not on the claim path.

Plan: **phase 1** deletes and gets to about 4,100 library lines without
rewriting kept code. **Phase 2** collapses what remains to about 1,000.

---

## 1. The irreducible kernel

The claim, as STATE_VS_CAUSALITY_V0 states it:

> an accepted state satisfies declared invariants relative to an
> approved prior state, on evidence from registered observers, within an
> ordered time window, under a fresh single-use authorization.

Remove any of the following and that sentence is false:

| # | Mechanism | Why it is irreducible |
|---|---|---|
| K1 | **Verified evidence that only trusted code can mint** (`Verified<T>`, crate-private constructor) | otherwise an agent's claim can satisfy a rule |
| K2 | **Three-valued evaluation**: Allow / Deny / Blocked, where missing or contradictory evidence gives Blocked | "unknown is never true or false" |
| K3 | **Two requirement forms**: `Fact` (exact value) and `Within` (names inside scopes, with `UnknownBelow`) | input identity, required checks and freshness are `Fact`; scope is `Within` |
| K4 | **Registered observers answering typed keys** (`Key`/`Term`, provider → attestation bound to provider and request) | "evidence from registered observers", not from the agent |
| K5 | **State identity**: a directory's content as a git-compatible SHA-256 tree id | "approved prior state" needs an exact id |
| K6 | **Snapshots before and after**, from one monotonic clock, each read twice | "relative to a prior state", "ordered time window", and no torn readings |
| K7 | **Single-use authorization**, bound to runtime and policy, re-checked against a fresh snapshot before the actor runs | "fresh single-use authorization" |
| K8 | **Hold**: after a transition that was not accepted, nothing more is authorized | "accepted state becomes the trusted state" means the alternative must stop |
| K9 | **Decision with per-rule reasons naming the observer** | "every decision explained" |

Everything else is either an experiment that measured a boundary, or
residue from the earlier product.

## 2. Components the claim requires

| Claim clause | Today | Minimal form |
|---|---|---|
| "declared invariants" | `kernel::{Obligation, Requirement, Invariant}`, `temporal::TransitionInvariant` | one `Invariant` trait: `watches`, `pre`, `post` |
| "on evidence" | `kernel::EvidenceBase` with 3 classes, `map`, `merge`, `forget` | a map from `Key` to a list of `Evidence`, with 2 classes (`Verified`, `Claimed`) |
| "from registered observers" | `graph::Registry` with trust levels, kind rules, verifiers, lineage, plans, discards | `Registry { providers }` plus `collect`, which drops answers that are unasked or came from another provider or request |
| "approved prior state" | `snapshot::FsSnapshot` as a `Verifier` over `fs_raw` keys, plus `ObjectStore` | one fs observer that walks with `openat(O_NOFOLLOW)` and emits `snapshot(dir)`, `entries(dir)` and `file(path)`; `ObjectStore` + `materialize` |
| "ordered time window" | `temporal::{Snapshot, tick, ORDERED}` | the same, concrete |
| "fresh single-use authorization" | `runtime::{Authorization, authorize, execute}`, generic over `EffectDomain` | the same logic, concrete over `Key`/`Term` |
| "accepted ⇒ trusted; else hold" | `runtime::settle`, `unaccepted` | the same |
| "explanation" | `Status` strings + `Provenance{observer,basis}` + `Lineage` + `provenance::explain` + `DecisionRecord` + `Journal` | `Status` strings naming the observer; `Vec<Decision>` as the journal |
| boundary ("not histories") | `tests/state_vs_causality.rs` | kept as is (ported) |
| demonstration | `examples/v9r_demo.rs` | kept as is (ported) |

**Not required by the claim:** delegation, signed authority, object
identity, capability worlds, OS containment, atomic capture, git as a
domain, verifiers and re-derivation, lineage, compensation, semantic
evidence, numeric bounds, domain-neutrality, the CLI and REPL, LLM
adapters, Wasm, the VFS, bundles, traces and checkpoints.

## 3. What goes to the archive

**Mechanism:** tag `v9r-archive-v0` on the current head, then delete.
The archive is git history, not a directory. An `archive/` directory of
code that no longer compiles would rot, and it would be counted as
v9r. Research reports move to `research/archive/` (prose doesn't rot)
and keep citing the tag.

### 3.1 Whole crates and trees (delete)

| Path | Lines | What it is |
|---|---|---|
| `crates/v9r-vfs` | 827 | in-memory Plan-9 VFS and write bus (earlier product) |
| `crates/v9r-cap` | 363 | namespace views over the VFS |
| `crates/v9r-runtime` | 1,161 | Wasmtime agent host |
| `crates/v9r-orchestrator` | 965 | VFS-event handler dispatch |
| `crates/v9r-cli` | 789 | "transactional shell" CLI and REPL, built on the legacy core |
| `examples/hello-agent`, `examples/llm-gateway`, `examples/task-runtime` | — | earlier product demos |
| `agents/` | — | a committed `.wasm` binary, **stored twice** (`agents/llm_gateway.wasm`, `agents/llm_gateway/module.wasm`) |
| `examples/*/target/**` | **5,318 files, ~1.04 GB** | **committed build output.** Delete it whatever else is decided: `git rm -r --cached examples/*/target`. It is already ignored by `target/` in `.gitignore` but was committed before that rule |
| `task.toml` (repo root) | 10 | legacy shell config |

### 3.2 `v9r-core` modules: experiments (archive)

| Module(s) | Code lines | Experiment | Why it is not kernel |
|---|---|---|---|
| `capability.rs`, `delegation.rs`, `authority.rs`, `object_identity.rs` | 3,009 | Capability Manifest / Delegation / Root of Trust / Object Identity / Content Identity / Snapshot Boundary | about *who may act*. The claim is about *what state resulted*. Removes `ring` |
| `fs_watch.rs` | 150 | Atomic Capture v0 (inotify) | measured that capture must be trusted; the result is a sentence in the limitations, not code |
| `provenance.rs`, `graph.rs` lineage (~200) | ~475 | Evidence Provenance v0 | measured that provenance **does not** make evidence true. Kept: the observer name in each reason |
| `verify.rs`, `verifiers.rs`, `snapshot.rs` verifiers (`SnapshotObjects`, `MatchesSnapshot`, `SnapshotCommitEquality`) | ~900 | Verifiable Observers v0, Content-Addressed State v0 | re-derivation framework. Tree ids are git-compatible, so `git` itself is the independent checker. Keep one git-compatibility test |
| `git.rs`, `git_provider.rs`, `git_guard.rs` | 1,606 | Git Evidence Domain, Effect Runtime v1 | second domain, built to prove the lifecycle generic. Proven; no longer needed. Removes `sha1` |
| `counter.rs` | 295 | Effect Runtime v1 (domain-neutrality falsifier) | same |
| `graph.rs` `GraphDomain` + `graph::runtime` | ~150 | Evidence Graph v0 | a second `EffectDomain` with no snapshots: `TemporalDomain` with every key `@now` |
| `world_probe.py`, `tests/capability_probe.py` | 845 | Capability Inventory, Controlled Domain | host probes |

### 3.3 `v9r-core` modules: legacy product residue (delete)

The README still describes this product ("Snapshot / Execute / Validate /
Rollback / Export"). It is not v9r as the research defines it.

| Module | Code lines |
|---|---|
| `adapter.rs` (LLM HTTP clients; pulls in `reqwest`) | 917 |
| `vfs.rs` (task checkpoints, seals) | 698 |
| `guarded.rs` (`FsDomain`, the first effect domain) | 538 |
| `effect.rs` (Effect runtime **v0**, superseded) | 519 |
| `execution.rs` (command runner) | 436 |
| `state.rs` (third fs crawler) | 396 |
| `policy.rs` (legacy invariants) | 390 |
| `bundle.rs` (export/import) | 292 |
| `trace.rs` (event log) | 251 |
| `context.rs` | 243 |
| `facts.rs` (`RuntimeFact` vocabulary) | 227 |
| `fs_provider.rs` (fourth fs view; the only path from the claim into the above) | 186 |
| `trusted.rs` | 158 |
| `task.rs` | 115 |
| `content.rs` (second content identity) | 94 |
| `manifest.rs` (task allow-lists) | 67 |
| `lib.rs`: `VfsPath`, `VfsError`, `NodeId`, `Capability`, `CapabilityId` | ~180 |

### 3.4 Tests

| Keep (port) | Archive |
|---|---|
| `state_vs_causality.rs` (the boundary) | `atomic_capture`, `capability_manifest`, `content_identity`, `controlled_domain`, `object_identity`, `snapshot_boundary` |
| `examples/v9r_demo.rs`, also as a test that asserts the 5 verdicts | `git`, `guarded`, `effects`, `counter`, `perf` |
| kernel unit tests (~240 lines, trimmed to 2 classes and 2 forms) | `graph`, `provenance`, `verifiable` |
| 1 test from `content_addressed.rs`: tree id equals `git write-tree` under `--object-format=sha256` | the rest of `content_addressed`, and `temporal.rs` except replay/drift (fold 2 cases into the causality file) |
| | `authority/tests.rs`, `delegation/tests.rs` (1,678 lines) |
| | the five source-scanning "depends on no domain module" guards and the kernel hash guard (see §5.11) |

### 3.5 Research documents

| Keep in `research/` | Move to `research/archive/` |
|---|---|
| `V9R_ARCHITECTURE_OVERVIEW_V0` (rewritten to the reduced guarantees, §7) | the 11 capability, containment and identity reports |
| `STATE_VS_CAUSALITY_V0` | `EFFECT_RUNTIME_V0`, `EFFECT_RUNTIME_V1`, both baselines |
| `INVARIANT_KERNEL_V0` | `EVIDENCE_GRAPH_V0`, `EVIDENCE_PROVENANCE_V0`, `VERIFIABLE_OBSERVERS_V0`, `GIT_EVIDENCE_DOMAIN_V0`, `ATOMIC_CAPTURE_V0` |
| `TEMPORAL_EVIDENCE_V0` | `AGENT_TRANSITION_*` |
| `CONTENT_ADDRESSED_STATE_V0` | `V9R_RESEARCH_MILESTONE_V0`, `V9R_EXTERNAL_REVIEW_PACKAGE_V0`, `V9R_REVIEW_FREEZE_CHECKLIST_V0`, `freeze.sh` (each restates the overview) |
| `V9R_DEMO_IMPLEMENTATION_V0` (merge the design doc into it) | `V9R_DEMO_DESIGN_V0` |
| this plan | |

That leaves 7 documents in `research/`, down from 31.

## 4. APIs to remove

### 4.1 Kernel (`kernel.rs`)

| Remove | Evidence it is unused or not claim-bearing |
|---|---|
| `Semantic<T>`, `Evidence::Semantic`, `EvidenceClass::Semantic`, `add_semantic` | constructed only by `guarded.rs` (legacy) |
| `Strength`, `Strength::Soft` | `Soft {` appears in no file outside `kernel.rs`. With `Soft` gone, `Hard` is the only value, so the field goes too |
| `Requirement::AtMost` | used only by `policy`, `git_guard`, `delegation`, `counter` and `provenance` tests, all archived. A bound can be a verified `Bool` fact |
| `EvidenceBase::map` | exists only to relabel across the `RtSubject`/`TKey` sum types (§5.6) |
| `EvidenceBase::forget`, `merge` | `forget`: used once, for double-read inconsistency; build the snapshot without those keys instead. `merge`: becomes an internal loop |
| `Invariant<C,S,V>` trait | superseded by `TransitionInvariant`; only legacy `policy` implements it |
| `DecisionRecord`, `FindingRecord`, `Decision::record` | a lossy copy of `Decision`; keep `Decision` itself in the journal |
| `EvidenceClass` | derivable from the enum variant; used only by `temporal::same_readings` |
| `Phase` | each `Invariant` already has separate `pre` and `post` methods, so the phase is known where the obligation is made |
| generics `<S, V>` on every type | one subject/value space remains: `Key` and `Term` |

### 4.2 Runtime (`runtime.rs`)

| Remove | Why |
|---|---|
| `EffectDomain` (8 associated types), `Stage`, `Concluded`, `Concluded::durable` | one domain remains. The generic was the Effect Runtime v1 experiment; the experiment is finished |
| `Compensable`, `compensate`, `Stage::Compensated` | the temporal domain implements no compensation; it is a non-goal ("a rejected transition holds") |
| `Journal` trait, `MemoryJournal` | journal = `Vec<Decision>` |
| `RuntimeSubject`, `RuntimeValue`, `RtSubject`, `RtValue`, `lift`, all `Rt*`/`Domain*` type aliases | a sum type that exists only because the runtime is generic. Runtime facts become ordinary keys: `runtime(accepted)` |
| `add_semantic`, `absorb`, `durable` evidence | semantic is gone; nothing in the claim path produces durable evidence |
| `RuntimeFault` | one variant; becomes an error string |
| `Authorize::decision`, `Authorization::decision` accessors | keep the verdict and the decision only in `Authorize` |

### 4.3 Evidence (`graph.rs`, `verify.rs`)

| Remove | Why |
|---|---|
| `Trust::{Attesting, ClaimsOnly}` | a claims-only source returns `Claimed`; the registry needs no mode |
| `Registry::{define, restrict, plan, remove, discarded, lineage, assign_snapshot, query, add_verifier}`, `KindRules`, `Plan` | kind rules: the demo's single `restrict` guards a kind that only one provider answers. Discards: make them a `Vec<String>` the registry returns, or drop them; no claim depends on inspecting them. `query` is a test helper |
| `Method`, `Attested::{observed, depends_on}`, `Lineage`, `lineage_token`, `lineage_ids` | provider claims about method and dependencies, which the registry cannot check (graph.rs says so itself) |
| `Verifier`, `Step`, `Basis`, `Candidate`, `Inputs`, `vouched`, the recursive `collect_at`/`run_verifier` loop (depth 8, 1024 steps) | the snapshot observer computes the tree id directly. Ingestion is trusted anyway (Atomic Capture v0) |
| `Term::{Digest, Text}` (fold into `Id`), `Term::Absent` (a pin with no value is an `Option`) | `Term = Bool \| Id \| Map \| Bytes` |
| `GraphDomain`, `graph::runtime`, `GraphError`, `GraphRuntime` | §3.2 |

### 4.4 Temporal (`temporal.rs`)

| Remove | Why |
|---|---|
| `At::Now` vs `At::Current` vs `At::Snapshot` as a separate `TKey` type | keep three moments, but as a field of `Key` (`at: At`), so the kernel needs no second key type |
| `TemporalDomain`, `TemporalRuntime`, `Acted`, `relabel` | merged into the concrete runtime |
| `async` on `Actor::act` and on everything else | no caller awaits real I/O. Removing it removes `tokio` from the library |

### 4.5 Snapshot / fs

| Remove | Why |
|---|---|
| `fs_raw` kinds `fs_stat`, `fs_meta`, and the reading-tag argument | `fs_meta` served object-identity experiments; `fs_stat` (link-count cross-check) becomes an internal check of the walk; the tag exists for verifier double-reads |
| `snapshot_from`, `Capture`, `SnapEntry` (`entries` over a tree) | capability experiments; `entries` is emitted by the same walk |
| `FilesystemEvidenceProvider` | replaced by `entries(dir)` from the snapshot walk: one walk gives one identity |

### 4.6 Dependencies

Remove from `v9r-core`: `anyhow`, `async-trait`, `bincode`, `chrono`,
`reqwest`, `ring`, `sha1`, `slotmap`, `thiserror`, `tokio`, `tracing`,
`uuid`. Remove from the workspace: `wasmtime`, `wasmtime-wasi`, `wat`,
`rustyline`, `colored`, `toml`, `tracing-subscriber`, `bytes`.

**What remains:** `sha2`, `libc`. `serde` stays only if decisions must be
serialized. The claim needs them *recorded*, not serialized, so drop it.

## 5. Duplicated concepts

| # | Concept | Copies | Keep |
|---|---|---|---|
| 5.1 | **Effect lifecycle** | `EffectDomain` impls: `FsDomain` (guarded), `GitDomain` (git_guard), `CounterDomain`, `GraphDomain`, `TemporalDomain`; plus `effect.rs` (runtime v0) and `execution::run_observed_step` | one concrete transition runtime |
| 5.2 | **Looking at a directory** | `state.rs` crawler, `effect::Observation`, `fs_provider`, `fs_raw`, `fs_watch`, `vfs.rs` checkpoints, `context::TaskSnapshot`, the `v9r-vfs` crate, `capability.rs` worlds | one `openat`-based walk |
| 5.3 | **Content identity** | `content.rs` manifest (`v9r-content-manifest/1`), `verifiers::FsContent` (`MANIFEST` and `NAMES` definitions), `snapshot.rs` git tree SHA-256, `state::ContentHash`/`FsState::digest`, `bundle::ArtifactHash`, `vfs::CheckpointSeal` | **git tree SHA-256** only |
| 5.4 | **Git** | `git.rs` observer, `git_provider`, `verifiers::GitObjects` (SHA-1 and SHA-256), `SnapshotCommitEquality`, `ContentEquality` | none (the snapshot is git-compatible; git is not a domain) |
| 5.5 | **"Manifest" / authority** | `lib.rs` `Capability`, `v9r-cap::NamespaceView`, `capability.rs` world manifests, `manifest.rs` task allow-lists, `v9r-orchestrator/manifest.rs`, `agents/*/manifest.toml`, `delegation` grants, `authority` signed grants | none: the claim's only authority is `Authorization` |
| 5.6 | **Fact vocabulary** | `facts::RuntimeFact`, `graph::{Key, Term}`, `runtime::{RtSubject, RtValue}`, `temporal::TKey` | `Key { kind, args, at }`, `Term` |
| 5.7 | **"Is this trusted?"** | `Evidence::{Verified, Semantic, Proposed}`, `Trust::{Attesting, ClaimsOnly}`, `verify::Basis::{SelfCertified, VouchedBy, Unvouched}`, `Candidate::vouched_by`, `Strength::{Hard, Soft}` | `Evidence::{Verified, Claimed}` |
| 5.8 | **Explanation / record** | `Status` reason strings, `kernel::Provenance`, `graph::Lineage`, lineage tokens parsed back out of reason strings, `provenance::explain`, `DecisionRecord`/`FindingRecord`, `Journal`/`MemoryJournal`, `trace::TraceLogger` | `Status(String)` naming the observer, and `Vec<Decision>` |
| 5.9 | **Process-global counters** | `CLOCK` (temporal), `NEXT_LINEAGE`, `NEXT_REQUEST`, `NEXT_ROUND` (graph), `NEXT_RUNTIME` (runtime), the snapshot store counter | one clock (`tick`); request ids derive from it |
| 5.10 | **Agent hosts** | `v9r-runtime` (Wasm), `adapter.rs` (LLM HTTP), `v9r-orchestrator`, the CLI/REPL, three example crates | none: the actor is a closure |
| 5.11 | **Architecture guards** | source-scanning "depends on no domain module" tests in `kernel`, `runtime`, `graph`, `temporal`, `verify`, `provenance`, plus the kernel sha256 guard | none: a 1,000-line crate with 2 dependencies **is** the guard. Re-pin a hash only if the external review asks for one |
| 5.12 | **Status documents** | Architecture Overview, Research Milestone, External Review Package, Freeze Checklist: four restatements of claim, guarantees and limits | the Overview |

## 6. A 1,000-line v9r

One crate, `v9r`, with dependencies `sha2` and `libc`. Synchronous.

```text
src/
  lib.rs        ~20   re-exports
  kernel.rs    ~200   Key, Term, Evidence, Requirement, Status, Verdict, Decision, evaluate
  evidence.rs  ~150   Provider, Attestor, Registry::collect
  runtime.rs   ~330   Snapshot, Invariant, Authorization, Runtime::{new, authorize, execute}
  fs.rs        ~300   FsObserver (walk → snapshot/entries/file), ObjectStore, materialize
tests/
  kernel.rs        semantics (unknown blocks, contradiction blocks, claims never satisfy)
  causality.rs     state_vs_causality, ported; plus replay and drift cases
  git_compat.rs    tree id == git's (skipped when git is absent)
examples/
  demo.rs          the five runs; asserts ALLOW, DENY, BLOCKED, DENY, REFUSED
```

### 6.1 Kernel (~200)

```rust
pub enum At { Now, Current, Snap(u64) }
pub struct Key { pub kind: String, pub args: Vec<String>, pub at: At }
pub enum Term { Bool(bool), Id(String), Map(BTreeMap<String,String>), Bytes(Vec<u8>) }

pub struct Verified(Key, Term, String /* observer */);   // pub(crate) constructor only
pub enum Evidence { Verified { value: Term, observer: String }, Claimed { value: Term, source: String } }
pub struct EvidenceBase(BTreeMap<Key, Vec<Evidence>>);    // insert is idempotent

pub enum Name { Known(String), UnknownBelow(String) }
pub enum Requirement { Fact { key: Key, value: Term }, Within { names: Vec<Name>, scopes: Vec<String> } }
pub struct Obligation { pub invariant: String, pub requirement: Requirement }

pub enum Status { Satisfied(String), Violated(String), Undetermined(String) }
pub enum Verdict { Allow, Deny, Blocked }
pub struct Decision { pub verdict: Verdict, pub findings: Vec<(Obligation, Status)> }

pub fn evaluate(obligations: &[Obligation], evidence: &EvidenceBase) -> Decision;
```

The semantics are unchanged from `kernel.rs` minus `Soft`/`Semantic`/`AtMost`:
any Violated gives Deny, else any Undetermined gives Blocked, else Allow.
More than one distinct verified value gives Undetermined. Claims are
listed in the reason and never satisfy.

### 6.2 Evidence (~150)

```rust
pub trait Provider { fn id(&self) -> &str; fn answers(&self, k: &Key) -> bool;
                     fn provide(&self, keys: &[&Key], a: &Attestor) -> Vec<Answer>; }
pub struct Attestor { provider: String, request: u64 }      // attest() -> Attested (opaque)
pub enum Answer { Verified(Attested), Claimed { key: Key, value: Term } }
pub struct Registry { providers: Vec<Box<dyn Provider>> }
impl Registry { pub fn register(..); pub fn collect(&self, keys: &[&Key]) -> EvidenceBase; }
```

`collect` keeps exactly today's binding: an answer counts only for the
provider and request it was attested in, and only for a key that
provider was asked. Everything else is dropped.

### 6.3 Runtime (~330)

```rust
pub trait Invariant<P> {
    fn id(&self) -> &str;
    fn watches(&self) -> Vec<Key>;
    fn pre(&self, p: &P, before: &Snapshot) -> Vec<Obligation> { vec![] }
    fn post(&self, p: &P, before: &Snapshot, after: Option<&Snapshot>) -> Vec<Obligation>;
}
pub fn pin(inv: &str, s: &Snapshot, k: &Key) -> (Obligation, Option<&Term>);
pub fn changed(prefix: &str, a: Option<&Term>, b: Option<&Term>) -> Vec<Name>;

pub struct Snapshot { id: u64, facts: BTreeMap<Key, Term>, inconsistent: Vec<Key> } // read twice
pub struct Authorization<P> { runtime: u64, policy: String, proposal: P, pre: Vec<Obligation> } // !Clone
pub enum Authorize<P> { Allowed(Authorization<P>), Denied(Decision), Blocked(Decision) }
pub struct Report<P> { pub executed: bool, pub accepted: bool, pub decision: Decision, pub output: Option<P> }

pub struct Runtime<P> { id, registry, invariants, trusted: Snapshot, held: Option<String>, pub journal: Vec<Decision> }
impl<P> Runtime<P> {
    pub fn new(registry: Registry, invariants: Vec<Box<dyn Invariant<P>>>) -> Self; // baseline snapshot now
    pub fn authorize(&mut self, p: P) -> Authorize<P>;           // PRE + runtime(accepted) + basis@current
    pub fn execute(&mut self, a: Authorization<P>, act: impl FnOnce(&P) -> Result<(), String>) -> Report<P>;
}
```

Built-in obligations, unchanged from today:

- `transitions_accepted`;
- `authorization_basis_current` (every watched key's verified value
  `@current`);
- `transition.ordered` (`before < done < after` on the clock).

Drift handling, the stale-authorization refusal and the hold behave as
`runtime.rs` does now.

### 6.4 Filesystem (~300)

`FsObserver::new(id, root, store)` answers three kinds:

| Kind | Value | How |
|---|---|---|
| `snapshot(dir)` | `Id(tree sha256)` | walk with `openat(O_NOFOLLOW)` per component (from `fs_raw`), cross-checking link counts (from `snapshot`); build git blob and tree objects into `ObjectStore` |
| `entries(dir)` | `Map(path → mode:blobid)` | from the same walk, so scope and identity can no longer disagree |
| `file(path)` | `Bytes` | `openat` + `read` |

`materialize(store, tree, dest)` stays for the demo's test runner.

### 6.5 What the budget leaves out, deliberately

- **Re-derivation by anyone, inside v9r.** Replaced by git compatibility:
  `git fsck` on the exported objects is the independent check.
- **Lineage (request, round, snapshot) in explanations.** The reason
  names the observer and the snapshot id (in the key). Provenance v0
  showed that the rest explains without adding truth.
- **Domain-neutrality.** The runtime is concrete. Neutrality was proven
  (Effect Runtime v1, counter) and is recorded; a second domain is
  restored from the archive tag if one is ever needed.
- **Compensation and rollback.** Already a non-goal.

## 7. Effect on the published guarantees

Rows in V9R_ARCHITECTURE_OVERVIEW_V0 "Guarantees":

| Row | After debloat |
|---|---|
| accepted only if every rule satisfied; claims never satisfy | **kept** |
| out-of-scope changes denied and named | **kept** |
| exact content identities (git-compatible SHA-256) | **kept**; "checkable afterwards" is now via git, not via in-tree verifiers |
| authorization single-use; refused on change | **kept** |
| missing or contradictory evidence blocks | **kept** |
| explained "down to the observer, request and snapshot" | **narrowed** to "observer and snapshot" |
| delegated authority attenuates… | **removed**: archived result, not a v9r guarantee |
| snapshot grants exact… | **removed**: archived result |
| kernel ~550 lines, unchanged since `bfebebe` | **replaced** by "kernel ~200 lines". The old hash guard no longer applies; see §8 |

The non-goals and limitations stay as written, minus the rows about
delegation and ledgers.

## 8. Order of work and risks

**Phase 0: hygiene, no semantic change.**

- Untrack `examples/*/target` (~1 GB) and the duplicated `.wasm`.
- Tag `v9r-archive-v0`.
- Note: the history still holds the gigabyte. Shrinking the clone needs a
  history rewrite, which is your decision and is not part of this plan.

**Phase 1: deletion only (target ~4,100 library lines).**

1. Delete §3.1 and §3.3, and §3.2 except `snapshot`/`fs_raw`.
2. Cut the single accidental edge: make the demo and the causality test
   take `entries` from the snapshot walk instead of
   `FilesystemEvidenceProvider`. Move `digest_of`/`hex`/`walk_tree`
   from `verifiers.rs` into `snapshot.rs`.
3. Re-run the demo and `state_vs_causality`. **Acceptance:** same five
   verdicts, same "every measured field identical" table.
   `kernel.rs` is untouched in this phase, so its hash guard still holds.

**Phase 2: collapse (target ≤1,000 library lines).**

4. Remove the APIs in §4 and merge `temporal` into a concrete runtime,
   kernel first.
5. Same acceptance as step 3, plus the git-compatibility test.
6. Rewrite the README to the claim. Today it advertises the deleted
   transactional shell.

**Risks**

- **The kernel freeze is spent.** "Byte-identical since `bfebebe`" is a
  credibility claim the external-review package relies on. Phase 2
  breaks it on purpose. Do phase 1, run the external review on it, then
  phase 2. Or accept that the review should look at the 1,000-line
  version instead. That trade-off is yours to decide.
- **The causality test's evidence changes** in step 2: `entries` comes
  from a different observer. The result is expected to hold, since both
  observers read the same bytes, but it must be re-measured, not assumed.
- **The demo's I3 runs workspace code** (the test imports `src/`). This
  is a known limitation, not created by the debloat. A smaller v9r makes
  it more visible, which is fine.
- **Discarded-answer visibility.** Dropping `Registry::discarded` loses
  a debugging aid used by 6 test files. If replay rejection should stay
  observable, keep it as a returned `Vec<String>`, about 10 lines.

## 9. Numbers

| | Now | Phase 1 | Phase 2 |
|---|---|---|---|
| workspace crates | 6 | 1 | 1 |
| `v9r-core` modules | 35 | 8 | 4 |
| library lines (code, no tests) | ~15,400 (core) + ~4,100 (other crates) | ~4,100 | **~1,000** |
| test + example lines | ~13,600 | ~1,300 | ~800 |
| direct dependencies (core) | 16 | 4 | 2 |
| research documents | 31 | 7 (+ archive) | 7 |
| tracked build output | ~1.04 GB | 0 | 0 |
