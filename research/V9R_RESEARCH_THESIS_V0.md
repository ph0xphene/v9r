# v9r Research Thesis v0

*The final research thesis of v9r Phase 1. It states what was asked,
what was measured, and what follows. It is not an implementation plan.
It proposes no further phase, designs no architecture, and claims no
capability that has not been measured.*

**Revision:** the code of `v9r-review-v1` (code commit `6a9c717`;
`kernel.rs` sha256 `85badb66…6177f`, byte-identical since `bfebebe`).

**Sources, and only these:**

| Short name | Report |
|---|---|
| **Kernel** | INVARIANT_KERNEL_V0 |
| **Effect v0 / v1** | EFFECT_RUNTIME_V0, EFFECT_RUNTIME_V1 |
| **Causality** | STATE_VS_CAUSALITY_V0 |
| **Self-ref** | V9R_SELF_REFERENTIAL_OBSERVERS_V0 |
| **Cap-boundary** | V9R_OBSERVER_CAPABILITY_BOUNDARY_V0 |
| **Boundary** | V9R_VERIFICATION_BOUNDARY_V0 |

Results that Cap-boundary and Boundary cite from other reports are used
as they cite them.

**Archived results.** Kernel, Effect v0 and Effect v1 were measured on
earlier architectures. Most of their code is archived at
`v9r-archive-v0`. Their results are cited as measured **at that
revision**. Where a result carries over to v1, the v1 test is named.

---

## 1. The original problem

An LLM agent's work and its report about that work come from the same
unreliable process. The pre-v9r code made this concrete. Kernel found,
in the code it started from:

- **Claims recorded in the shape of observations.** On `<finish>`, the
  adapter appended a synthetic `CommandExecuted { exit_code: 0 }`. *"The
  model's self-reported success is indistinguishable from an exit
  status the runtime observed."*
- **Success on missing or stale evidence.** Validation failed only if a
  test *ran and failed*. It passed when the tests never ran, or ran
  before the agent's last change.
- **The policy was editable by the task.** It lived in the workspace
  under `allow_write = ["."]`.

Effect v0 found the same shape in the transactional code. Rollback
erased the execution history it was meant to protect, wrote outside the
workdir, and exported deleted secrets in bundles. Six defects, each
reproduced by a failing test first.

**The problem, as stated at the start:** *"'Tests pass' can mean the
code was fixed, or that the tests were changed. A pipeline that trusts
the report, or only re-runs checks the agent could influence, cannot
tell the difference."*
(V9R_EXTERNAL_REVIEW_PACKAGE_V0 §1.)

## 2. The hypothesis tested

**H: separate proposing from accepting.**

- An untrusted actor proposes and acts.
- A runtime that holds the authority observes the state before and
  after, and collects evidence itself.
- A small, domain-free kernel decides acceptance from declared rules
  over that evidence.
- The actor's statements never decide anything.

Four sub-hypotheses were tested:

| | Sub-hypothesis | Outcome |
|---|---|---|
| H1 | the decision fits in a small kernel that knows nothing about any domain | **supported** |
| H2 | one lifecycle serves unrelated domains | **supported, qualified** |
| H3 | an accepted transition tells you the authorized actor produced the state | **refuted** |
| H4 | evidence the runtime collects itself is independent of what it judges | **refuted** for observers that execute the judged state, and for changes after capture |

H3 and H4 were never written as goals. They are what the demo's labels
invited a reader to conclude ("honest fix → ALLOW", "tests pass on the
result, run by v9r"). Phase 1 tested them because a reader would.

## 3. The strongest supported claim

> **For one runtime process, from a host-approved state S0:**
>
> - v9r refuses to start a transition whose watched state no longer
>   matches its single-use authorization;
> - it accepts the state S1 captured when the actor's call returns, and
>   only if every declared rule is satisfied by attestations its
>   registered observers gave in reply to its own requests;
> - claims never satisfy a rule; missing or contradictory attestations
>   block; otherwise it denies or blocks, and that process then accepts
>   nothing further.
>
> The decision is a function of S0, S1, the rules, the attestations and
> the order of the window. **v9r verifies states, not histories.**
>
> An accepted S1 has the property a rule means only if each
> contributing observer is faithful, complete, current and independent
> of S1, and the rule's key is adequate for that property.

This is Boundary §1.2–§1.3. Each clause is backed by a named v1 test.
Each validity condition has a measured violation that gives a false
Allow.

## 4. The experiments that define the boundary

Each experiment fixed one side of the boundary. Together they define
it.

### 4.1 Kernel: the decision can be small and typed

- **Size.** 554 non-test lines, `std` + `serde` only, enforced by a
  test. It knows nothing about files. Today: 604 lines, unchanged since
  `bfebebe`, guarded by `kernel_guard`.
- **Forms.** Three requirement forms (`Fact`, `Within`, `AtMost`), no
  quantifiers, no user code.
- **Verdicts.** Three-valued (Allow, Deny, Blocked). Deny dominates.
  Unknown never collapses to true or false.
- **Evidence classes by construction, not convention.** `Verified` is
  crate-private and not `Deserialize`. A serialized receipt ingests as
  `Proposed`. An oracle at 10000 bp cannot satisfy a hard obligation:
  **Blocked**.
- **Verdicts are about states, not program outcomes.**
  - `touch made.txt no-such-dir/x`: **Allow although exit 1**;
  - `true` expecting `report.txt`: **Deny although exit 0**.

*Established:* the decision procedure can be small, domain-free and
typed so that a claim cannot pass as evidence.

### 4.2 Effect v0 and v1: one lifecycle, measured effects

- **Effect v0.** The receipt is defined as the comparison of two live
  scans by the runtime, never from stored or oracle data. Ten behaviours
  (A–J) held deterministically. **Its non-guarantees were written down
  at the time:** causation (*"the effects are those of the observation
  window"*; *"changes made after the post-scan by lingering children
  land in a later receipt or in none"*), atomicity, metadata, semantic
  satisfaction.
- **Effect v1.** Filesystem, git and an in-memory counter shared one
  runtime.
  - Adapters contained 0 lifecycle logic.
  - The kernel was byte-identical, and no new requirement form was
    needed.
  - Divergences between the former guards went from 9 to 0. The toy
    counter found a lifecycle defect (D10) both production guards
    shared.
  - The cost: +51% code, a domain contract of eight items rather than
    three, and domains that can only be written inside the trusted
    crate.

*Established:* authorization, freshness, acceptance and hold are one
lifecycle, independent of the domain. The decision is a judgement of
two observed states, and Effect v0 already said what that excludes.

### 4.3 Causality: the decision does not see the writer

World A, the authorized actor, and World B, an unauthorized outsider
writing byte-identical content inside the window:

- identical verdicts, findings, observers, receipts, journals, S0 and
  S1;
- only counter values differed;
- re-measured after Debloat Phase 1, still identical
  (`identical_s1_gets_identical_decisions_whoever_wrote_it`).

*Established:* execution provenance is not an input to the decision.
No rule over the collected evidence can recover it. **The architecture
can express provenance, and it lacks the producer.**

### 4.4 Self-ref: the decision sees only what its observers say, as captured

On the demo's own setup, with `src/` unchanged:

- **X1.** An agent whose `greet.py` is `sys.exit(0)` got **Allow**,
  `tests(S1) = passed, verified by provider:test-runner`, with `greet`
  absent.
  - The same module without the exit: **Deny**.
  - The honest fix: **Allow**.
  - The judged code decided the runner's report.
- **X4.** A process the actor left behind rewrote the protected test
  in the live workspace while v9r was still deciding.
  - The earlier S1 was accepted in **40/40** transitions.
  - The live tree differed from the accepted S1 in 40/40.

*Established:* a faithful observer can still be controlled by the state
it judges. The decision covers S1 as captured, not as left. **The
failure in X1 is the one the original problem names: re-running a
check the agent could influence.**

### 4.5 Cap-boundary: independence is only half an authority property

- **I-eff** ("the observer does not change what is judged") is an
  authority property, `watched ∩ mutable(O) = ∅`.
  - It needs objects as file handles or materialized content, a
    writable set built by construction and closed over delegation, and
    the whole decision window.
  - Each component was measured, archived:
    - a world constructed from a manifest (9/9);
    - object identity by file handle (6/6);
    - a view materialized from the snapshot (9/9);
    - containment of process-tree and delegated writers (11/11).
  - Residue that stays: the same-uid namespace owner (detected, not
    prevented), supplementary groups, harness tools.
- **I-out** ("the judged state does not control the observer's output")
  is not an authority property.
  - X1 used only the authority any test runner needs: read the
    checkout, run the interpreter.
  - In the trusted, the capability-scoped and the isolated observer
    models alike, X1 is Allow.

*Established:* the distinction between **authority** (what an observer
can touch) and **influence** (what controls its output). Capability
checks bound the first only.

### 4.6 Boundary: four layers, separated by measured pairs

| Layer | v9r establishes | Separated from state verification by |
|---|---|---|
| state verification | yes | — |
| observer verification | binding only; validity only for its own recomputed tree ids | X1 vs the broken-module control: same correctness, different report, different verdict |
| execution provenance | nothing | Causality A vs B: different writer, same decision |
| program correctness | nothing directly | X1 vs the honest fix: same verdict, different correctness |

One pair is defined and not run: observer validity varied **with S1
fixed** (V9R_OBSERVER_INDEPENDENCE_MODEL_V0 §6.3). Until it runs, the
separation of observer validity from correctness rests on X1, where the
two moved together.

## 5. The final trust model

### 5.1 Three kinds of source

| Source | Registered as | Treatment | Enforced? |
|---|---|---|---|
| **untrusted:** the actor, `ClaimsOnly` providers, persisted records, unrequested answers | — / `ClaimsOnly` | recorded as claims; never satisfy or violate a rule | **yes** (`claims_instead_of_evidence_block`; G7 tests; Kernel) |
| **trusted observer** | `Attesting` | its bound answers are evidence | binding yes; truth no |
| **self-referential observer:** a trusted observer that executes the judged state | `Attesting` | identical to a trusted observer; nothing records the difference | **no** (X1) |

### 5.2 Two tiers of trust

**Tier 1, for the decision being correct on what observers said:**

- `kernel`;
- `runtime` and `temporal`;
- the `graph` registry with `verify`;
- `snapshot` (tree ids, recomputation, hash-checked objects);
- `rustc`, `serde`, `serde_json`, `sha2`, `libc`;
- an uncompromised runtime process.

**Tier 2, for an accepted state having the meant property,** adds:

- every observer's faithfulness, completeness and currency;
- the independence of every observer that executes anything;
- `fs_raw` and the Linux filesystem;
- untorn capture;
- the adequacy of each rule;
- a host that does not change the watched state between capture and
  completion.

### 5.3 The assumptions, with their measured status

| Assumption | Measured when violated |
|---|---|
| observers are faithful | false Allow alone; Blocked with an independent witness |
| observers are complete | omitted file: false Allow alone |
| observers are current | caching observer: false Allow |
| observers do not let the judged state control their output | X1: false Allow |
| observers do not change the judged state | not measured |
| rules are adequate | X1: false Allow |
| capture is not torn | without a freezer, not certifiable against a same-uid writer |
| nothing writes during the decision | X4: 40/40 stale |
| approval and hold persist | in-memory only; a restored runtime accepted stale state (archived) |
| the identity covers what matters | names, content, symlink targets, owner-execute bit only |

## 6. What v9r solves

Each item is measured, and each is a defect class found in the code v9r
started from (§1), or a guarantee that held under attack.

| Solved | Measured |
|---|---|
| **an actor's claim of success counts as evidence** | claims never satisfy a rule; a serialized receipt ingests as `Proposed`; an oracle at 10000 bp blocks (Kernel; `claims_instead_of_evidence_block`) |
| **success on missing evidence** | missing evidence blocks, never allows (`removing_any_observer_blocks_with_missing_evidence_never_denies`) |
| **success on evidence about another version** | facts are keyed by the tree they name (`test_success_for_another_tree_does_not_count`). Replayed, forwarded and anachronistic answers are dropped (G7 tests). *Not* solved: an observer that re-attests a stale value is believed (§5.3, §7) |
| **contradiction resolved by picking a side** | contradictory attestations block (`disagreeing_observers_block`, `observers_reading_different_versions_block`) |
| **acting on a state that changed after approval** | refused and held (`artifact_changed_after_authorization_is_refused`, `external_modification_between_authorization_and_execution_refuses`) |
| **changes outside the declared scope** | denied and named, for names, content, symlinks, empty directories and the owner-execute bit (`scope_entries`, 18 cases) |
| **a claimed state identity** | recomputed, not believed; git computes the same id (`a_claimed_snapshot_id_is_recomputed_not_believed`, `a_snapshot_is_the_git_tree_of_the_same_content`) |
| **continuing after a rejection** | that process authorizes nothing further (`observer_unavailable_after_effect_blocks_and_holds`) |
| **lifecycle logic duplicated per domain** | one lifecycle, 0 lifecycle sites in adapters, 9 divergences → 0 (Effect v1) |
| **an unexplained verdict** | each finding names its observer, request and snapshot (`release_is_explained_by_the_lineage_of_its_evidence`) |

## 7. What v9r deliberately does not solve

"Deliberately" means one of two things. Either the property is outside
the decision's inputs (§3), so no rule could establish it. Or it was
measured, found to need something other than a verifier, and left
outside the claim.

| Not solved | Why it is outside | Measured |
|---|---|---|
| **who produced the state** | the writer is not an input to the decision | Causality |
| **whether the program is correct** | a rule names a key, not a meaning; correctness needs valid observers and adequate rules | X1 |
| **whether a test is adequate** | the same | X1 |
| **whether observers are truthful, complete, current** | observers are the source of evidence; v9r binds them, it cannot re-derive them, except for its own content-addressed recomputation | liar, omitted file, stale |
| **whether an observer is independent of the judged state** | output-side independence is not an authority property | X1; Cap-boundary |
| **what happens after capture** | the decision judges the captured S1 | X4; Effect v0 non-guarantees |
| **containing the actor** | containment needs construction of the actor's world, outside the verifier | X4; archived Controlled Domain |
| **atomic capture** | needs quiescence across a privilege boundary | archived Atomic Capture |
| **persistence across restart** | v9r's approval and hold are process state | archived Root of Trust |
| **metadata beyond the identity** | the identity is git's | `fs_raw.rs:73` |
| **intent, reasoning** | statements are claims | Kernel |
| **rollback** | the runtime holds; it restores nothing | — |

## 8. Why the boundary is a result, not a failure

**1. The boundary follows from the claim's own form.** The decision is
a function of (S0, S1, R, A, window order). Causality measured that the
writer is not among these inputs: identical inputs gave identical
decisions. Self-ref measured that the live state after capture is not
among them: the same S1 gave the same decision whether or not the
workspace changed afterwards. A verifier whose decision depends only on
these inputs cannot establish what lies outside them. Showing that the
inputs are exactly these is a positive result about the verifier, not a
gap in it.

**2. Every limit was found without changing the kernel.**

- X1, X4, the liar, the caching observer and the omitted file all gave
  false Allows with `kernel.rs` byte-identical.
- In each case the kernel did what its typed semantics say.
- Each failure was in what the evidence meant (observers, rules), or in
  what the wording promised.
- So the boundary separates a core whose behaviour is fully specified
  from the assumptions it depends on. That separation is the thing a
  verifier must offer, and it is now measured, not asserted.

**3. Each limit is reproducible and kept.** The false Allows are
passing tests that **assert** them:

- `a_trusted_liar_is_believed_alone_…`;
- `stale_…_attested_freshly_is_believed`;
- `hidden_file_…_only_with_an_independent_observer`;
- `x1_judged_program_decides_the_test_runners_report`;
- `x4_watched_state_changes_after_observation_before_completion`.

A reader does not have to trust the boundary's description. They can
run it. A limit that is a passing test cannot quietly disappear from
the claim.

**4. The negative results corrected the claim, not the code.**

- The released wording said "verified evidence", "tests pass on the
  result, run by v9r", "every decision came from what v9r observed".
- The measurements showed where each says more than the code does. The
  wording was narrowed to match.
- The code at `v9r-review-v1` was not changed by any of it.
- A system whose claims can be narrowed by measurement, with no code
  change, had claims and code that were separable to begin with.

**5. The boundary splits the remaining problem along a measured
line.**

- Execution provenance is **orthogonal** to the decision (Causality).
- Observer independence divides into an **authority** half, whose
  components were measured as constructible, and an **influence** half,
  which no authority model measured here reaches (Cap-boundary).
- Program correctness is **downstream** of both, through rule adequacy
  (X1).
- These are three different problems. Before Phase 1 they were one
  word: "verified".

**6. The original problem is answered precisely, in both directions.**

- *"A pipeline that trusts the report … cannot tell the difference."*
  v9r measurably does not trust the report: claims never decide.
- *"… or only re-runs checks the agent could influence."* v9r's own
  demo did exactly that, and X1 measured the consequence.
- The thesis therefore holds as stated and no further:
  - **separating proposing from accepting removes the actor's report
    from the decision;**
  - **it does not remove the actor's influence from the evidence.**

  That second sentence is the boundary, and it is the result.
