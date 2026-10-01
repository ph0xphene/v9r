# Effect Runtime v1: one lifecycle, three domains

Research question: **is v9r a true invariant runtime, or a collection of
domain-specific guards that share a kernel?**

Method: extract a domain-neutral runtime that owns the whole lifecycle,
turn the filesystem and git guards into adapters, then add a third,
structurally unrelated domain (an in-memory counter) and see what
breaks.

Short answer:

- **The lifecycle generalizes.** Three domains now run through one
  `Runtime`, and the adapters contain no authorization, freshness,
  acceptance or rollback logic.
- **The kernel did not change.** `kernel.rs` is byte-identical, and no
  new requirement forms were needed.
- **Complexity relocated rather than shrank.** Total code grew by about
  half.
- **The domain boundary is wider than hypothesized.** A domain supplies
  eight things, not three.
- **The runtime is general, but the platform is not.** Domains can only
  be written inside the trusted crate, and composing two domains is
  still hand-written.

Verdict: **A, qualified** (see *Decision*).

## Baseline (Phase 1)

Recorded in `EFFECT_RUNTIME_V1_BASELINE.md` before any change:

- Commit `bfebebe`: **199 tests passing, 2 ignored**.
- `kernel.rs` sha256 `85badb66…6177f`, 842 lines.
- Two guards, with about 300 lines of lifecycle each.
- **Nine divergences (D1–D9)** between what were supposed to be copies
  of the same lifecycle. One of them (D2) was a latent defect.
- **Runtime facts in a domain vocabulary.** `Transitions` (acceptance
  history) was a variant of the *filesystem* `Subject` enum, and the git
  guard reached it through `CrossSubject::Fs(Subject::Transitions)`.

Toolchain note: the host's rustup toolchain cannot execute (NixOS);
everything ran under `nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc`
(cargo 1.98.0).

## Architecture

```text
                         ┌──────────────────────── runtime.rs (std + serde + kernel only) ────────────────────────┐
  Proposal (plain data)  │                                                                                         │
  ─────────────────────▶ │ authorize:  PRE obligations = domain.obligations(Pre)                                    │
                         │                             + transitions_accepted      (runtime fact)                   │
                         │                             + authorization_basis_current (domain.basis(trusted))        │
                         │             evaluate(Pre) on evidence from the TRUSTED observation ──▶ Authorization      │
                         │                                                                                         │
  Authorization ───────▶ │ execute:    observe() ─▶ re-evaluate the authorization's obligations on FRESH evidence   │
                         │             stale ─▶ refuse;  hold only if reality ≠ trusted state (drift)               │
                         │             domain.execute() ─▶ observe() ─▶ domain.conclude() ─▶ receipt, output        │
                         │             POST = domain.obligations(Post) ─▶ evaluate(Post)                            │
                         │             Allow ∧ observable ─▶ trusted := after, release output                       │
                         │             otherwise          ─▶ hold: transitions_accepted fails every PRE             │
                         │                                                                                         │
  (Compensable only) ──▶ │ compensate: observe() ─▶ domain.compensate() ─▶ observe() ─▶ conclude ─▶ evaluate(Post)  │
                         │             Allow ─▶ release the hold                                                   │
                         │                                                                                         │
                         │ owns: authorization minting/consumption, trusted observation, acceptance state,          │
                         │       durable + semantic + proposed evidence, decision journal, RtSubject/RtValue        │
                         └───────────────▲──────────────────────────────▲───────────────────────────▲──────────────┘
                                         │ EffectDomain                 │ Journal                   │ kernel::evaluate
            ┌────────────────────────────┼───────────────┐              │                           │
            │                            │               │       TraceLogger (sealed)     kernel.rs (unchanged)
    FsDomain (guarded.rs)      GitDomain (git_guard.rs)  CounterDomain (counter.rs)       MemoryJournal
      Observation                GitWorld{fs, refs}        u64
      basis: Workspace digest    basis: fs digest +        basis: Value
                                        every repo's refs
      Compensable (rollback)     Compensable (rollback)    not Compensable
            └──────────┬──────────────────┘
              Workspace (shared effect machinery: trace, checkpoint,
              run command, rollback, receipts, workspace evidence)
```

### The domain contract (`EffectDomain`)

| Item | Kind | fs | git | counter |
|---|---|---|---|---|
| `Subject`, `Value` | vocabulary | `facts::Subject/Value` | `CrossSubject/CrossValue` (fs ⊕ git) | `CounterSubject`, `u64` |
| `Proposal` | plain data | `ActionProposal` | `GitProposal` | `Increment` |
| `Observation` | snapshot | workspace scan | scan + ref observation | the value |
| `Effect`, `Receipt`, `Output` | effect plumbing | `WorkspaceEffect`, `WorkspaceReceipt`, bundle bytes | same + `Release`, `(Oid, digest)` | `()`, `CounterReceipt`, `()` |
| `observe()` | reality | scan | scan + refs | read |
| `basis(obs)` | freshness identity | `Workspace = D` | `Workspace = D`, `Refs(r) = Dᵣ` | `Value = v` |
| `obligations(stage)` | invariants | `Policy` (I1, I3, I4, budget, export, expectations) | G1–G3, I3, I4 | C1, C2 |
| `evidence(subjects, obs, receipt)` | attestation | observation, receipt, seals | fs + git observer | read |
| `execute(proposal, before)` | effect | run / bundle | run / declare | add |
| `conclude(effect, before, after)` | account | receipt into the trace, test outcome (durable) | same | receipt |
| `label`, `policy_digest` | bookkeeping | | | |
| `Compensable::compensate` | optional | checkpoint rollback | checkpoint rollback | — |

The runtime adds two obligations of its own, in its own vocabulary:

- `transitions_accepted`: a runtime fact, `runtime.Transitions`.
- `authorization_basis_current`: the domain's basis facts.

Domain facts and runtime facts meet in one kernel decision under
`RtSubject<S> = Runtime(..) | Domain(S)`. The domain evidence is lifted
with the crate-private `EvidenceBase::map` that the git milestone added.

A test (`runtime_depends_on_no_domain_module`) pins two things:

- `runtime.rs` imports only `std`, `serde` and `crate::kernel`;
- its code (comments excluded) contains none of `path`, `file`,
  `commit`, `git`, `workspace`, `ref` or `refs`.

## Before / after

### Structure

| | Before (`bfebebe`) | After (`ddd0b2b`) |
|---|---|---|
| lifecycle implementations | 2 (one per guard) | 1 (`runtime.rs`) |
| `evaluate(` calls in adapters | 9 | **0** |
| `unaccepted` / hold-state handling in adapters | 17 sites | **0** |
| freshness/basis logic in adapters | 9 sites | **0** (a domain only states its basis facts) |
| authorization types | 2 hand-written | 1 generic (`Authorization<D>`) |
| runtime facts | in the fs vocabulary | `RuntimeSubject` in the runtime |
| domains | 2 | 3 |
| `kernel.rs` | sha256 `85badb66…` | **identical** |
| tests | 199 pass, 2 ignored | **211 pass, 2 ignored** |

### Code lines

Counted without blanks, comments or `#[cfg(test)]`:

| File | Before | After | Notes |
|---|---|---|---|
| `runtime.rs` | — | 505 | 49 vocabulary, 94 trait, 59 authority types, 295 runtime |
| `guarded.rs` | 359 | 411 | 171 shared `Workspace` machinery, 103 domain, 77 facade |
| `git_guard.rs` | 665 | 635 | 364 vocabulary/policy/invariants (mostly unchanged), 181 domain, 90 facade |
| `counter.rs` | — | 225 | including about 60 lines of fault-injection fixture |
| `policy.rs` / `facts.rs` | 326 / 195 | 320 / 179 | acceptance moved to the runtime |
| `execution.rs` | 340 | 361 | `run_observed_from` split into run / observe / record |
| **guards + runtime** | **1024** | **1551** | **+51%** |

### Behaviour

Every D1–D9 divergence now has one rule:

| # | Before | After |
|---|---|---|
| D1/D2 | fs: basis = last accepted state. git: basis = fresh scan, which absorbed out-of-band changes between steps | **last accepted state**, for every domain. New test `out_of_band_change_between_steps_is_not_absorbed`; it fails on the old git guard |
| D3 | fs re-checked only the basis at execute; git re-checked everything | everything (PRE + basis) is re-evaluated on fresh evidence |
| D4 | fs held on any stale authorization; git only on a violated basis | hold only on **drift** (reality ≠ trusted state); see D10 |
| D5 | fs held on an unobservable result; git ignored it | hold ("post-state unobservable"); counter test `unobservable_result_blocks_and_holds` |
| D6 | the hold reason listed invariants (fs) or only the verdict (git) | invariants, everywhere |
| D7 | git rollback accepted only if `rollback()` returned `Ok` (fs used the kernel) | kernel only; the git domain now also requires `I3.rollback_restores_checkpoint`, which subsumes the return-value check |
| D8 | durable evidence (test outcomes, oracle answers) was fs-only | runtime-owned; any domain can return `durable` facts |
| D9 | a git release was judged by an execute-time PRE re-evaluation, logged twice | a release is an effect with no side effects; G1/G3 are re-judged as POST obligations; output released only on acceptance |
| **D10** (new) | *both* guards held the task when an authorization was superseded by an **accepted** step | stale ≠ drift: refused, not held. Found by the counter domain (`authorization_superseded_by_an_accepted_step_is_stale_but_not_drift`); a mutation reverting the rule fails it |

Test changes to pre-existing suites, both reviewed:

1. **`guarded.rs`, one line.** The invariant id `workspace_accepted` is
   renamed `runtime::ACCEPTED` (`transitions_accepted`), because the
   runtime must not name workspaces.
2. **`git.rs`, `graft_file_does_not_forge_ancestry`.** The test used to
   write the graft file out of band, between two guarded steps. That is
   exactly the D2 hole, and it is now refused. The file is now written
   by a mediated `git show --output=grafts`. The test still asserts that
   plain git is fooled and that the observer is not.

### Performance

Release build, medians of 7 steps, `tests/perf.rs`. Times in ms.

| workspace | authorize (before → after) | execute | guarded total | no-op rollback |
|---|---|---|---|---|
| 10 × 1 KiB | 0.06 → 0.04 | 2.09 → 1.73 | 2.14 → 1.77 | 0.50 → 0.46 |
| 1 000 × 4 KiB | 0.11 → 0.11 | 24.8 → 24.0 | 24.9 → 24.1 | 34.3 → 32.3 |
| 10 000 × 1 KiB | 0.72 → 0.72 | 196.4 → 194.2 | 197.1 → 194.9 | 288.7 → 282.2 |
| 100 × 1 MiB | 0.07 → 0.08 | 112.8 → 110.9 | 112.9 → 111.0 | 165.8 → 163.1 |

- **No measurable change.** Every difference is within run-to-run noise.
- **Kernel cost.** Evaluation is still about 0.9 µs per decision.
- **Scan count.** Still two scans per step: the runtime's
  post-observation *is* the receipt's post-scan, which is why
  `conclude` takes `after` instead of scanning.
- **Trace size.** It grows from 13 to 15 KiB in the bench, because every
  PRE decision now records its basis finding.
- **Not measured.**
  - Export and git release now observe their result: one extra scan,
    and for git also the ref observation (≈ 7.6 ms per the git report).
  - Execute re-checks the trusted-state seals: O(trace), negligible at
    these sizes.

## Third domain (Phase 4)

`CounterDomain` guards one `u64` under `counter <= limit`. It has:

- no files, processes, trace or checkpoints;
- no `Compensable` implementation, deliberately.

| Scenario | Result |
|---|---|
| ALLOW: 0, +5, limit 10 | PRE Allow, POST Allow, trusted = 5 |
| DENY: 8, +5, limit 10 | PRE Deny (`C1`: 13 > 10); nothing runs; the runtime keeps accepting |
| stale authorization (two grants on 0, the first executed) | the second is refused (`authorization_basis_current` violated); **not held** (D10) |
| state changed between authorization and execution (out-of-band `set(7)`) | refused, **held**; the next PRE is denied by `transitions_accepted` |
| observed result contradicts the proposal (actuator adds 1 extra) | POST Deny (`C2.observed_matches_proposal`: 3 ≠ 2); held **for good**, since there is nothing to compensate |
| actuator overshoots the limit (8, +2, skew 1) | PRE Allow (10 ≤ 10), POST Deny on C1 and C2 |
| result unreadable after the increment | POST Blocked, held |

A compile-fail doctest pins that `Runtime<CounterDomain, _>` has no
`compensate()`. The error is `CounterDomain: Compensable` not satisfied;
this was checked against the actual compiler output.

The counter needed nothing the runtime did not already offer. It also
exposed a lifecycle defect (D10) that both production guards shared.

## Evaluation

### 1. Can three domains share one lifecycle?

**Yes, for these three.**

- Filesystem, git-over-filesystem and an in-memory counter all run
  through `Runtime<D, J>`.
- No adapter contains authorization, freshness, acceptance or rollback
  policy.
- Pre-existing behaviour is preserved except where it was divergent or
  wrong (D1–D10). That is shown by the 20 fs tests and 19 git tests,
  with the two reviewed edits above.

The domains do share one important assumption: **reality can be
observed synchronously, right after the effect, as one consistent
snapshot.** None of the three challenges it (see *Where the abstraction
breaks*).

### 2. Does `kernel.rs` remain unchanged?

**Yes.** It is byte-identical to `bfebebe`: same sha256, empty diff.

The runtime depends on two crate-private kernel affordances:

- `Verified::attest`, for the runtime's own `Transitions` fact;
- `EvidenceBase::map`, which the git milestone added for domain
  composition, to lift domain evidence into `RtSubject`.

`map` thus turned out to be the general mechanism for layering
vocabularies, not a git-specific patch.

### 3. Were new requirement types necessary?

**No.** The counter uses `AtMost` and hard `Fact`; the runtime uses hard
`Fact`.

One strain is worth recording, though. `C1` on the *observed* value is
`AtMost { value: <observed>, limit }`, so the observation is copied into
the obligation as a literal. `AtMost` and `Within` are local checks with
no evidence class: the kernel cannot tell an observed quantity from a
predicted or claimed one. fs already had the same property, with I1's
touched names coming from receipts.

Correctness therefore depends on obligation derivation feeding only
attested values into local checks. That is a convention, not a type. An
evidence-backed bound (`Fact`-style subject with an upper limit) would
close it. It was not added, because nothing failed without it.

### 4. Did runtime complexity decrease?

**Not in size. It decreased in kinds of logic per domain.**

| Measure | Result |
|---|---|
| total code, guards + runtime | **+51%** (1024 → 1551 lines) |
| `guarded.rs` | grew (359 → 411) |
| `git_guard.rs` | shrank by only 30 lines |
| lifecycle implementations | 2 → 1 |
| lifecycle sites in adapters | dozens → 0 |
| semantic divergences | 9 → 0 |
| cost of a new domain | 225 lines, 0 of them lifecycle; it inherits freshness, holds, journaling, durable evidence and single-use authority for free |

With two domains the runtime does not pay for itself in lines. It pays
for itself in **consistency**: the duplicated copies had already drifted
nine ways, one of which was a hole. It probably breaks even in lines
around the fourth domain.

Some of the new lines are compatibility cost:

- the facades: 77 + 90 lines that keep the pre-runtime API so the old
  tests run unchanged;
- generic `Debug` impls and type aliases.

### 5. Where does the abstraction break?

Nothing broke outright. These are the places it strained, ordered by
how much they limit the "general runtime" claim.

1. **Closed-world attestation.**
   - Every domain must live inside `v9r-core`, including the toy
     counter, because only that crate can construct `Verified`.
   - The runtime is domain-neutral, but domain *registration* is not
     open. This is the largest gap between "general runtime" and
     "general platform".
2. **Composition is still hand-written.**
   - `GitDomain` composes fs and git by hand: a sum vocabulary
     (`CrossSubject`), evidence routing, and a composite observation.
   - The runtime layers *runtime ⊕ domain* generically (`RtSubject`),
     but not *domain ⊕ domain*.
   - The git report's conclusion still stands: the bottleneck is the
     composition layer.
3. **Trusted-store integrity is not the runtime's.**
   - I4 (trace seal plus checkpoint seals) is still a filesystem-domain
     fact.
   - The runtime owns its journal but never checks that journal's
     integrity; `MemoryJournal` has no notion of it.
   - A runtime that owns the trace should own its seal as well. The
     checkpoint seals belong with compensation, in the domain.
4. **The domain contract is eight methods and eight associated types,
   not three.** Two are irreducible:
   - `basis` (what "the same state" means). The runtime cannot derive
     it, and in composite domains it inherits the grain conflict from
     the git report: `.git/index` stat data makes fs versions move when
     git state does not.
   - `conclude` (the receipt needs *before* and *after*). The runtime
     owns *when* to observe; the domain owns what the difference means.
5. **Irreversibility is all-or-nothing.** For a domain without
   `Compensable`, one rejected transition ends the runtime's life. There
   is no notion of accepting a residual state through a privileged path.
   That may be the right default, but it is a policy the runtime
   hard-codes.
6. **Async contract without `Send`.**
   - `async fn` in traits currently yields futures with unknown `Send`
     bounds.
   - That is fine for today's single-threaded callers. A multi-threaded
     host would need a different trait signature.

## Failed assumptions

| Assumption | Outcome |
|---|---|
| "A domain provides effect, observation and evidence" | **False.** It also provides its invariants, its freshness identity (`basis`), the account of an effect (`conclude`) and a policy digest |
| "prepare / execute / observe" is the right trait | **False.** `prepare` became `obligations(Stage)`, and `basis`, `evidence` and `conclude` were needed besides |
| "The two guards already share one lifecycle" | **False.** They had nine divergences, including a hole (D2) |
| "Runtime facts can live in the first domain's vocabulary" | **False.** The runtime needed its own `RuntimeSubject` and a sum type |
| "Extracting the runtime reduces complexity" | **False by size** (+51%); true by kinds of logic per domain |
| "The runtime owns trusted-state handling" | **Partly.** It owns the trusted observation, acceptance and durable evidence, but not the integrity of persistent stores (I4) |
| "A toy third domain can only confirm, not find bugs" | **False.** The counter found D10 in both production guards |
| "Anyone can add a domain" | **False.** Attestation is crate-private |

## Decision

**A, qualified: v9r is becoming a general invariant runtime. It is not
yet a general invariant platform.**

- **Why not B** ("the kernel generalizes but the runtime remains
  domain-specific"):
  - B predicts that a third domain would need its own guard. It did
    not: the counter reused the runtime unchanged.
  - The fs and git guards lost all lifecycle code.
- **Why not C** ("the abstraction boundary is wrong"):
  - The boundary held for three structurally different domains without
    forcing: no kernel change, no new forms, no domain words in the
    runtime.
  - It is *wider* than hypothesized (eight items, not three), and two of
    those items (`basis`, `conclude`) are genuinely domain knowledge.
    That is a correction to the boundary, not a refutation.
- **Why "qualified":**
  - The claim rests on three domains that all observe synchronously and
    all live in the trusted crate.
  - Composition of domains (2. above) is exactly where a wrong boundary
    would show next, and it has not been tested generically.

## Next falsification experiment

**Generic domain composition.** Implement `Compose<A, B>` once, in the
runtime layer:

- vocabulary `A::Subject ⊕ B::Subject`;
- observation `(A::Observation, B::Observation)`;
- basis is the union of both bases;
- evidence routed by tag;
- effects delegated to a designated acting domain.

Then rebuild git as `Compose<FsDomain, GitRefsDomain>`. G1–G3 become a
policy over the composite, with no hand-written `CrossSubject` and no
evidence routing in the adapter.

- **Supports A:** `GitDomain`'s glue disappears. The 19 git tests and
  the D2 regression test pass, and the only git-specific code left is
  the observer, its vocabulary and the G-invariants.
- **Falsifies A (→ C):** the composite needs per-pair glue that cannot
  be stated as policy. For example:
  - touched refs need a git diff of observations that `Compose` cannot
    express generically;
  - the fs/git basis grain conflict cannot be resolved without
    domain-specific exclusions;
  - `EvidenceBase::map` has to become public.

Secondary, if composition holds:

- a domain whose result is **not immediately observable** (an
  eventually consistent store), to test the runtime's one shared
  physical assumption;
- moving the **journal seal** into the runtime.

CI-history invariants stay deferred until composition is answered, as
planned.

## Commits

| Commit | Content |
|---|---|
| `cd92c00` | Phase 1 baseline note |
| `95c09fc` | `runtime.rs`, `Journal` for `TraceLogger`, run/observe/record split |
| `6622cbe` | fs and git guards become adapters; acceptance moves to the runtime |
| `ae5d967` | D2 regression test |
| `0988bab` | counter domain and tests |
| `ddd0b2b` | rustfmt |
