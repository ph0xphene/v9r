# What v9r claims: the verification boundary

> Formerly `research/V9R_VERIFICATION_BOUNDARY_V0.md` (the v9r-review-v1.1 tree has it under that name). Body unchanged apart from document links.

*The final model document before any Phase 2 work. It defines what v9r
verifies and where that stops. It proposes no code change and does not
design any future layer.*

**Revision:** the code of `v9r-review-v1` (code commit `6a9c717`;
`kernel.rs` sha256 `85badb66…6177f`, unchanged since `bfebebe`).

**Sources.** Only measured results from these reports, plus the tests
they name:

| Short name | Report | What it contributes |
|---|---|---|
| **Kernel** | [INVARIANT_KERNEL_V0](archive/INVARIANT_KERNEL_V0.md) | the decision semantics; Verified ≠ Semantic ≠ Proposed by construction; unknown never collapses |
| **Causality** | [causality](causality.md) | identical S1 from an authorized and an unauthorized writer → identical decisions |
| **Self-ref** | [observer-boundary](observer-boundary.md) | X1: an observer that executes S1 is controlled by it; X4: a change after capture is not judged |
| **Cap-boundary** | [V9R_OBSERVER_CAPABILITY_BOUNDARY_V0](archive/V9R_OBSERVER_CAPABILITY_BOUNDARY_V0.md), and the archived reports it cites | which parts of observer independence reduce to authority (I-eff) and which do not (I-out) |
| Guarantees | [V9R_RELEASE_FREEZE_V1](archive/V9R_RELEASE_FREEZE_V1.md) §6, `claim_guarantees`, `content_addressed`, `scope_entries` | G1–G12, as narrowed in [V9R_CLAIM_REVISION_V0](archive/V9R_CLAIM_REVISION_V0.md) |

Two definitions come from the model documents rather than from a
report: [V9R_OBSERVER_INDEPENDENCE_MODEL_V0](archive/V9R_OBSERVER_INDEPENDENCE_MODEL_V0.md) (the four observer
properties and adequacy) and Cap-boundary (I-eff, I-out). They are used
as defined there.

**Labels:**

- **measured:** a named passing test or report result;
- **archived:** measured on the pre-debloat tree, reproducible at
  `v9r-archive-v0`, not part of the v1 claim;
- **code:** read in the frozen code, not run;
- **definition.**

---

## 1. The strongest claim v9r can make

### 1.1 Terms

*Definition.*

- **Observer.** A provider registered as `Trust::Attesting`, or a
  verifier deriving from one.
- **Attestation.** A value an observer returned for a key, in reply to a
  request v9r made, in the round v9r made it. A of a decision is the
  set of attestations it used.
- **Claim.** Anything else: the actor's statements, `ClaimsOnly`
  providers, volunteered or replayed answers, persisted records.
- **State.** A directory tree identified by its SHA-256 git tree id:
  names, content, symlink targets, the owner-execute bit.
- **Rules R.** Invariants deriving obligations over keys, in three
  forms: `Fact`, `Within`, `AtMost`.
- **Transition T.** It runs in four steps:
  1. a single-use authorization granted on S0;
  2. at the start of execution, a fresh observation of the watched keys;
  3. the actor's call;
  4. the after-snapshot S1, taken when that call returns, then the
     POST decision.

### 1.2 Formal claim

> **For one runtime process, a host-approved S0, rules R, and the
> registry's observers:**
>
> 1. **(Authorization)** Execution of T starts only if every PRE
>    obligation is Satisfied on a fresh observation at the start of
>    execution. An authorization is used at most once.
> 2. **(Acceptance)** S1 is accepted only if every POST obligation of
>    R(S0, S1) is Satisfied by A:
>    - a `Fact` only by exactly one distinct attested value, equal to
>      the required one;
>    - a `Within` or `AtMost` only by its local check.
> 3. **(Claims are inert)** No claim satisfies or violates any
>    obligation.
> 4. **(Unknown is not evidence)** A missing attestation, contradictory
>    attestations, or an unknown name possibly outside a scope give
>    **Blocked**, never Allow.
> 5. **(Determinism in the state)** The decision is a function of
>    (S0, S1, R, A, the order of the window). Two transitions that
>    agree on these get the same decision, up to counter values,
>    whoever wrote S1.
> 6. **(Hold)** After a refusal for drift or a rejected transition,
>    that runtime process authorizes nothing further.
> 7. **(Identity)** S0 and S1 are identified by the git tree id of the
>    captured tree. A claimed id is recomputed, not believed. An
>    unreadable entry, or a lost or altered stored object, makes the
>    identity incomplete, which gives Blocked.

**Support (measured):**

| Clause | Support |
|---|---|
| 1 | `artifact_changed_after_authorization_is_refused`, `external_modification_between_authorization_and_execution_refuses` |
| 2 | kernel unit tests; Kernel §Decision semantics |
| 3 | `claims_instead_of_evidence_block`, `volunteered_attestations_are_discarded`, `replayed_attestations_are_refused`, `evidence_claiming_an_earlier_state_is_refused`; Kernel: an oracle at 10000 bp cannot satisfy a hard obligation (archived) |
| 4 | `removing_any_observer_blocks_with_missing_evidence_never_denies`, `disagreeing_observers_block`, `observers_reading_different_versions_block` |
| 5 | Causality; `identical_s1_gets_identical_decisions_whoever_wrote_it`; Self-ref (X4's S1 = the honest control's S1, same decision) |
| 6 | `observer_unavailable_after_effect_blocks_and_holds`; Self-ref (`accepting after false` on Deny) |
| 7 | `a_snapshot_is_the_git_tree_of_the_same_content`, `a_claimed_snapshot_id_is_recomputed_not_believed`, `unreadable_file_makes_the_snapshot_incomplete`, `approved_snapshot_with_a_lost_or_altered_object_is_incomplete` |

### 1.3 What the claim is relative to

The formal claim is about **attestations**, not about the world. It is
**unconditionally** a statement about what v9r did with what its
observers said. It becomes a statement about S1's properties only
through the **validity conditions**:

> If every observer that contributed to A is **faithful, complete,
> current and independent** of S1 for its key, and every rule's key is
> **adequate** for the property its author means, then an accepted S1
> has the meant properties, **as captured at the after-snapshot**.

Each condition has a measured violation that gives a false Allow, and
v9r does not detect any of them alone:

| Condition | Measured violation |
|---|---|
| faithful | `a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness` |
| complete | `hidden_file_omitted_by_the_observer_blocks_only_with_an_independent_observer` |
| current | `stale_raw_content_attested_freshly_is_believed`, `stale_state_attested_freshly_is_believed_unless_independently_witnessed` |
| independent | X1 (Self-ref) |
| as captured | X4 (Self-ref): 40/40 accepted S1 stale at completion |

**This is the strongest claim:** sound relative to attestations, and
conditionally sound relative to the world.

## 2. The strongest claims v9r cannot make

Each row is a claim that a reader could reasonably draw from an Allow,
with the measurement that refutes it.

| Claim v9r cannot make | Refuted by | Measured outcome |
|---|---|---|
| "the authorized actor produced S1" | Causality | World A (actor) and World B (outsider) gave identical verdicts, findings, observers, receipts and journals; only counters differed |
| "the program is correct" / "the task was done" | Self-ref X1 | Allow with `greet` absent; independent check: incorrect |
| "the tests passed, so the code works" | Self-ref X1 + control | the same broken module was Denied without `sys.exit(0)` and Allowed with it. The judged code decided the report |
| "an attested fact is true" | `a_trusted_liar_…` | a lone liar is believed |
| "an attested fact is current" | `stale_…_attested_freshly_…` | a caching observer is believed |
| "the accepted state is the state when the decision completes" | Self-ref X4 | accepted S1 ≠ live tree, 40/40 |
| "the command succeeded" | Kernel (archived) | `touch made.txt no-such-dir/x`: **Allow although exit 1**. The verified effect met the postcondition. Verdicts are about states, not about program success |
| "a failed command means a rejected state" | Kernel (archived) | `true` expecting `report.txt`: **Deny although exit 0** |
| "the captured tree existed at one instant" | [ATOMIC_CAPTURE_V0](archive/ATOMIC_CAPTURE_V0.md) | detection strategies, alone and combined, accepted a false snapshot. Only the cgroup freezer gave 200/200, and only while writers could not leave |
| "observers are independent because they are isolated" | Cap-boundary | X1 uses no authority an isolated test runner would lack |
| "a rejected state stays rejected" | [CAPABILITY_ROOT_OF_TRUST_V0](archive/CAPABILITY_ROOT_OF_TRUST_V0.md) (archived) | a runtime restored from backup accepted an old ledger (Allow). v9r's hold is in-process only (code: `temporal.rs:526`) |

## 3. The boundary between the four layers

### 3.1 Definitions

*Definition.*

| Layer | Question | Object judged |
|---|---|---|
| **State verification** | does the captured S1, with S0, satisfy R on A? | (S0, S1, R, A) |
| **Observer verification** | is each attestation in A a valid report of the property its key means: faithful, complete, current, independent? | each observer's computation |
| **Execution provenance** | what ran to produce S1, and what ran to produce each attestation? | processes and their writes and reads |
| **Program correctness** | does the code in S1 do what was meant? | the meaning of S1's content |

### 3.2 What v9r establishes in each

| Layer | Established by v9r | Measured |
|---|---|---|
| **state verification** | **yes**: the formal claim (§1.2) | §1.2 table |
| **observer verification** | **binding only**: which registered observer answered which request about which state. Validity is established only for v9r's own content-addressed derivations (tree id recomputation, hash-checked objects) | binding: G7 tests. Validity: X1, liar, stale (not established). Recomputation: `a_claimed_snapshot_id_is_recomputed_not_believed` |
| **execution provenance** | **none**, for the actor or for observers | Causality (actor); Self-ref X1 (the runner's execution of S1 was recorded only by the test harness, never by v9r); X4 (the writer is not represented) |
| **program correctness** | **none directly.** Only as a consequence of state verification + observer validity + rule adequacy | X1 |

### 3.3 Each boundary is witnessed by a measured pair

Two cases that agree on one layer and differ on the other show that the
layers are separate:

| Boundary | Pair | Agrees on | Differs on | Decision followed |
|---|---|---|---|---|
| state ↔ provenance | Causality, World A vs B | S1, R, A | writer | **state**: identical |
| state ↔ program correctness | Self-ref, honest fix vs X1 | rule outcome (both Allow) | correctness (true vs false) | **attested value**, not correctness |
| observer validity ↔ program correctness | Self-ref, broken (no exit) vs X1 | correctness (both false) | runner's report (failed vs passed) | **observer** |
| captured state ↔ live state | Self-ref, X4 vs honest control | S1, A | live tree at completion | **captured S1** |
| command outcome ↔ state | Kernel (archived), `touch … no-such-dir/x` | postcondition state | exit status | **state** |

**Not yet witnessed by a pair:** observer validity varied **with S1
held fixed**. X1 moved observer validity and correctness together. The
four-cell experiment in [V9R_OBSERVER_INDEPENDENCE_MODEL_V0](archive/V9R_OBSERVER_INDEPENDENCE_MODEL_V0.md) §6.3 is the
smallest pair that would. It is defined there and not run.

### 3.4 How the layers depend on each other

```text
 program correctness of S1
        ▲   needs, all three:
        │
        ├── state verification          ← v9r (the formal claim)
        ├── observer validity           ← assumed; binding only is enforced
        └── rule adequacy (k means P)   ← assumed; the rule's author

 execution provenance                   ← not in v9r
   - of S1's writer:   orthogonal to every verdict (Causality)
   - of an observer:   I-eff reduces to authority (Cap-boundary);
                       I-out does not (X1)
```

- **Provenance is orthogonal to state verification.** Clause 5 is
  measured: the decision does not depend on it.
- **Provenance is partly upstream of observer validity.** An observer
  whose execution could change the judged state (I-eff) is excluded only
  by constraining its execution: construction and containment, measured
  for agent worlds, archived. An observer whose output the judged state
  controls (I-out) is not excluded by any provenance or authority
  constraint. It is excluded only by what the observer computes
  (Cap-boundary §1.3, §3.2).

## 4. Non-goals

| Non-goal | Meaning | Why it is outside, measured |
|---|---|---|
| causal attribution | which process wrote S1 | Causality: no collected evidence differs between writers |
| program correctness | S1's code does what is meant | X1: the rule's key is not the meant property |
| test adequacy | a test that passes is a good test | X1: the protected test is satisfied by exiting on import |
| observer validity | attestations are true, complete, current, independent | liar, omitted file, stale, X1 |
| actor containment | the actor cannot act outside the transition | X4: its leftover process wrote during the decision. Containment was measured as feasible only by construction ([CONTROLLED_DOMAIN_V0](archive/CONTROLLED_DOMAIN_V0.md), archived) and is not part of v9r |
| decision-time currency | the accepted state is the live state at completion | X4 |
| atomic capture | the captured tree existed at one instant | [ATOMIC_CAPTURE_V0](archive/ATOMIC_CAPTURE_V0.md) |
| persistence across restart | approval and hold survive the process | in-memory only (code); restore accepted stale state (ROOT_OF_TRUST, archived) |
| metadata integrity | ownership, group/other bits, xattrs, ACLs, timestamps | not in the identity (code: `fs_raw.rs:73`); measured for the owner-execute bit only, via `scope_entries` |
| same-uid interference | protection from processes with the verifier's uid | [SNAPSHOT_CAPABILITY_BOUNDARY_V0](archive/SNAPSHOT_CAPABILITY_BOUNDARY_V0.md) (archived): the namespace owner changed a sealed view, **detected, not prevented** |
| intent and reasoning | why the actor did what it did | claims are inert (clause 3) |
| rollback | restoring S0 after rejection | the runtime holds; nothing restores |

## 5. Minimal trusted computing base

Two tiers, because the formal claim and its validity conditions rest on
different things.

### 5.1 Tier 1: for the formal claim (§1.2)

What must be correct for "v9r decided correctly on what its observers
said":

| Component | Trusted for |
|---|---|
| `kernel.rs` | three-valued evaluation; `Verified` only mintable inside the crate (Kernel; compile-fail doctests; `kernel_guard`) |
| `runtime.rs`, `temporal.rs` | authorization, fresh re-check, window order, after-snapshot, hold |
| `graph.rs` registry, `verify.rs` | binding each answer to provider, request and round; `restrict`/`define`; claims kept as claims |
| `snapshot.rs` (`FsSnapshot`, `ObjectStore`) | tree id computation, recomputation of claimed ids, hash-checked objects |
| `rustc`, `serde`, `serde_json`, `sha2`, `libc` | compilation, serialization, hashing, FFI |
| the runtime process | not compromised; holds S0, the trusted snapshot and the hold in memory |

**Not in tier 1:**

- the actor and its statements;
- stored object bytes (hash-checked);
- unbound or volunteered answers;
- `provenance` (it explains, it does not decide);
- `counter` and `fs_watch` (historical);
- every observer's **truth**.

Tier 1 needs only that observers' answers are **bound**, not that they
are **right**.

### 5.2 Tier 2: for the validity conditions (§1.3)

What must additionally hold for "an accepted S1 has the meant
properties":

| Component | Trusted for | Measured failure if not |
|---|---|---|
| every registered observer | faithful, complete, current | liar; omitted file; stale |
| every observer that executes anything | independent of the judged state (I-out and I-eff) | X1; I-eff (G′) unmeasured |
| `fs_raw` + the Linux filesystem | true and complete raw observation; directory link counts | the `openat` correspondence defect, 3 in 200,000, fixed ([ATOMIC_CAPTURE_V0](archive/ATOMIC_CAPTURE_V0.md)) |
| snapshot capture | no undetected concurrent writer | [ATOMIC_CAPTURE_V0](archive/ATOMIC_CAPTURE_V0.md) |
| the rules and whoever holds the registry | adequacy: keys mean what the author means; which observers count | X1 |
| the host | nothing writes the watched state between the after-snapshot and completion; S0 approval and hold survive restarts if needed | X4; ROOT_OF_TRUST (archived) |

## 6. Remaining assumptions

| # | Assumption | Status |
|---|---|---|
| A1 | registered observers are faithful | **measured failing** when violated; detected only with a second independent observer |
| A2 | registered observers are complete | measured failing for omitted files; enforced for omitted directories (link counts) and unreadable entries |
| A3 | registered observers are current | measured failing (caching); binding is enforced, freshness is not |
| A4 | observers are independent of the judged state, output side (I-out) | **measured failing** (X1); not reducible to authority (Cap-boundary) |
| A5 | observers' execution does not change the judged state (I-eff) | **unmeasured** (G′); reducible to authority, with measured residue (Cap-boundary) |
| A6 | rules are adequate | measured failing (X1); author's responsibility |
| A7 | capture is not torn | measured unenforceable without a freezer whose writers cannot leave |
| A8 | nothing changes the watched state during the decision | **measured failing** (X4) |
| A9 | the runtime process is not compromised | stated; reachable from A4/A5, since an executing observer runs as the verifier's uid (code: the demo's runner, unsandboxed) |
| A10 | the approved S0 and the hold survive what they must | in-memory only (code); restore accepted stale state (archived) |
| A11 | the identity covers the properties that matter | names, content, symlink targets, owner-execute bit only (code) |
| A12 | the Linux filesystem reports truthfully | stated |

## 7. What a future execution layer would and would not solve

"Execution layer" means only: **a mechanism outside v9r that constrains
and reports what processes ran during a transition**. Its form is not
designed here. Each row follows from a measured result; where only an
expectation exists, it says so.

### 7.1 Would address

| Problem | Why, from measurement | Status |
|---|---|---|
| **X4**: actor processes writing during the decision | process-tree writers, including a delayed writer after its parent exits, were contained, frozen and killed from L1. Self-migration was closed at L2 ([CONTROLLED_DOMAIN_V0](archive/CONTROLLED_DOMAIN_V0.md)) | measured for agent worlds, archived; **not measured in a v9r transition** |
| **I-eff (A5)**: an executing observer changing the judged state | an observer is an actor for authority. Construction from a manifest (9/9), a materialized read-only view (9/9), and IPC closure (11/11 at L7–L8) bound its writes | components measured, archived; **never assembled for an observer** |
| **A7**: torn capture | the freezer gave 200/200 true snapshots while writers could not leave | measured, archived |
| **causal attribution** | Causality: World B "could be distinguished *with*" an isolating executor | **expectation, not measurement** (Causality, "What this does not show") |

**Expression.** Evidence from such a layer would enter as an ordinary
`Fact` or `Within`. Causality decided "not C": the architecture can
express it, and lacks the producer. The archived capability work
expressed containment and identity with `kernel.rs` unchanged.

### 7.2 Would not address

| Problem | Why, from measurement |
|---|---|
| **I-out (X1)** | X1 uses only the authority an isolated test runner must hold. The judged code decides the report whatever the runner's boundary (Cap-boundary §4) |
| **program correctness, test adequacy (A6)** | these are about what the rule's key means. No constraint on execution changes what a key means |
| **observer faithfulness, completeness, currency (A1–A3)** | the layer's own reports are attestations, and the same conditions apply to it. World W: code running before a reporter controlled its report ([CAPABILITY_MANIFEST_V0](archive/CAPABILITY_MANIFEST_V0.md)) |
| **same-uid namespace owner** | changed a sealed view: detected, not prevented ([SNAPSHOT_CAPABILITY_BOUNDARY_V0](archive/SNAPSHOT_CAPABILITY_BOUNDARY_V0.md)) |
| **supplementary groups, harness tools** | not removable unprivileged, or not visible to the OS ([CAPABILITY_INVENTORY_V0](archive/CAPABILITY_INVENTORY_V0.md)) |
| **restart freshness (A10)** | a runtime restored from backup accepted a stale ledger; no rollback-protected counter is available unprivileged ([CAPABILITY_ROOT_OF_TRUST_V0](archive/CAPABILITY_ROOT_OF_TRUST_V0.md)) |
| **metadata outside the identity (A11)** | a property of the identity, not of execution |
| **the trust itself** | it moves trust from "observers are honest" to "the layer and its reporter are honest". It does not remove it |

### 7.3 Where the layer would sit relative to the boundary

- It would add evidence to **execution provenance**.
- It would partly support **observer verification**, for I-eff only.
- It would leave **state verification**, the formal claim, unchanged.
- It would leave **program correctness** exactly where it is: a
  consequence of valid observers and adequate rules, never established
  directly.

## 8. Summary

- **v9r verifies state transitions relative to attestations.** It
  accepts a captured S1 only if declared rules are satisfied by
  answers its registered observers gave to its own requests. Claims and
  unknowns never count, and nothing about who wrote S1 enters the
  decision.
- **That is all it verifies.** It does not verify:
  - its observers, beyond binding;
  - execution, of the actor or of observers;
  - correctness.

  Each of the three has a measured false Allow.
- **The three layers beyond state verification are separate**, each
  boundary witnessed by a measured pair, except observer validity with
  S1 fixed, whose separating experiment is defined and not run.
- **An execution layer is relevant to two of them, partially:**
  provenance, and the authority half of observer independence. It does
  not reach output-side independence or correctness. Those remain
  assumptions about observers and rules, outside any mechanism measured
  so far.
