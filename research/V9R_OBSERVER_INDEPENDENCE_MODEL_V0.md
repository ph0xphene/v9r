# v9r Observer Independence Model v0

*A model document. It formalizes the trust boundary that
V9R_SELF_REFERENTIAL_OBSERVERS_V0 (X1, X4) measured. It proposes no
architecture change, and nothing here was built or run for it.*

**Revision:** the code of `v9r-review-v1` (code commit `6a9c717`,
`kernel.rs` sha256 `85badb66…6177f`).

**Evidence labels:**

- **measured:** a passing test, named, and the report it belongs to;
- **code:** a fact about the frozen code, read at the cited lines, not
  run;
- **definition:** introduced by this document;
- **open:** not measured by any existing report.

## 1. The model

### 1.1 Terms

*Definition.*

- **S:** a judged state (S0 or S1), identified by its git tree id.
- **k:** a key a rule names, e.g. `tests(S1)`.
- **P:** the property the rule's author *means* by k, e.g. "`greet` is
  correct on S1". P is never written down. The rule names only k.
- **O:** the observer the registry lets answer k. O(S, E) is what it
  reports for state S, run in environment E: the host, its uid, the
  live workspace, time.
- **Attested value:** the value v9r receives from O, bound to v9r's
  request (G7).

The kernel decides on attested values for k. The reader of an Allow
concludes P. **Evidence is valid** when the attested value equals P(S).
This document is about the distance between those two.

### 1.2 Four observer properties

*Definition.* Each property is stated for O, a key k and a judged state S.

| Property | Statement | Violated means |
|---|---|---|
| **faithful** | O reports the value it actually observed or computed | O lies |
| **complete** | O's observation covers everything P(S) depends on | O omits something P depends on |
| **current** | O's observation is of S at the instant k names, and S is still the state the decision is about when the decision completes | O re-attests an old value, or the state has moved on before the decision completes |
| **independent** | (I-out) S affects O's output only through P(S). (I-eff) O's execution does not change S, or any state the decision watches | S can choose O's output; or observing S changes what is being judged |

"Current" has two parts. **Observer currency**: O read the state named,
not a cached one. **Decision currency**: the state named is still the
state when the transition completes. X4 measured a violation of the
second part, not of independence (§2.4).

**Valid evidence for P** needs all four, plus one property of the rule
rather than the observer:

- **adequacy:** k, as O computes it, equals P on every state the other
  rules admit.

Adequacy is where X1 lands. §2.1 shows why X1 is also an independence
failure.

## 2. What X1 and X4 measured, in these terms

### 2.1 X1: independence (I-out), with O faithful

*Measured:* `x1_judged_program_decides_the_test_runners_report`,
V9R_SELF_REFERENTIAL_OBSERVERS_V0 §4.

The three S1 had identical I1/I2 status and differed only in
`src/greet.py`:

| S1 | O(S1) (runner) | P(S1) (independent check) | verdict |
|---|---|---|---|
| honest fix | passed | true | Allow |
| broken, no exit | failed | false | Deny |
| broken + `sys.exit(0)` | **passed** | **false** | **Allow** |

- **Faithful:** yes. The runner reported the exit status it saw.
- **Complete:** yes. The checkout held the judged `greet.py`
  byte-for-byte.
- **Observer-current:** yes. The value is keyed by S1's tree id.
- **Independent (I-out):** **no**. Within the states I1 and I2 admit
  (anything under `ws/src`), S1 chose O's output: two S1 with the same
  P (false) gave opposite O. O's output depends on S1 through an
  arbitrary program, not through P.

So the X1 false Allow needs no unfaithful observer. **Faithful +
complete + current is not enough.** This is the boundary X1 found.

*Definition, from the measurement:* O is **controlled by S on k**
when, for a target value v, the states the other rules admit include
an S with O(S) = v and P(S) ≠ v. X1 is a witness for the runner and
v = `passed`.

### 2.2 X1 and adequacy are the same failure seen twice

An observer that executes S cannot be adequate on its own. Adequacy
needs O(S) = P(S) on every admitted S. If S controls O, an admitted S
exists where they differ (§2.1). The broken-module control shows the
converse: the same runner gives a **true negative**. **A self-referential
observer can be relied on for Deny, not for Allow.**

### 2.3 I-eff (the observer changes the world): open

The adversarial review's G′, where S1's code, run by the observer,
rewrites the live workspace, **was not measured**. No existing report
measures I-eff for any observer.

### 2.4 X4: decision currency, not independence

*Measured:* `x4_watched_state_changes_after_observation_before_completion`,
40/40.

- The writer was a process the **actor** left behind, not the observer.
  The runner's checkout was used only as a timing signal.
- Every observer was faithful, complete, observer-current and, for
  this purpose, independent. S1 was identical to the honest control's
  S1, and so was the decision.
- What failed is **decision currency**: the live tree differed from
  the accepted S1 before `execute` returned (40/40).
- *Code:* the after-snapshot is taken at `runtime.rs:485`, POST
  evidence is collected at `runtime.rs:510`, and on Allow the trusted
  state is set to the after-snapshot (`runtime.rs:517`). Nothing
  re-observes in between.

X4 belongs in this model because the decision's duration includes
observer execution time. A decision that runs observers holds its
"current" window open for as long as they run.

## 3. Which properties v9r enforces

"Enforces" means: when the property fails, the decision is Blocked or
Deny, not Allow, **with no second observer required**.

| Property | Enforced? | What v9r does | Evidence |
|---|---|---|---|
| **faithful** | **only for content-addressed derivations** | a claimed snapshot id is recomputed, not believed; an altered stored object makes the identity incomplete (Blocked) | measured: `a_claimed_snapshot_id_is_recomputed_not_believed`, `approved_snapshot_with_a_lost_or_altered_object_is_incomplete` (CONTENT_ADDRESSED_STATE_V0) |
| | **not otherwise** | a lone liar is believed | measured: `a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness` |
| **complete** | **partly** | an omitted subdirectory (via link counts) or an unreadable file blocks | measured: `hidden_directory_omitted_by_the_observer_blocks`, `unreadable_file_makes_the_snapshot_incomplete` |
| | **not for files** | an omitted file goes unnoticed alone | measured: `hidden_file_omitted_by_the_observer_blocks_only_with_an_independent_observer` |
| **current** (observer) | **binding only** | an answer counts only for the request and provider it was minted for; replayed, forwarded, volunteered and anachronistic answers are dropped | measured: `replayed_attestations_are_refused`, `volunteered_attestations_are_discarded`, `before_snapshot_replayed_as_after_is_not_believed`, `evidence_claiming_an_earlier_state_is_refused` |
| | **not freshness** | a caching observer that re-attests stale values is believed | measured: `stale_raw_content_attested_freshly_is_believed`, `stale_state_attested_freshly_is_believed_unless_independently_witnessed` |
| | **visible change between two readings of one snapshot** | Blocked | measured: `observers_reading_different_versions_block`; limits: ATOMIC_CAPTURE_V0 |
| **current** (decision) | **at the start of execution only** | a watched key that differs from the authorization's snapshot refuses execution | measured: `external_modification_between_authorization_and_execution_refuses` |
| | **not at completion** | a change after the after-snapshot is not judged | measured: X4, 40/40 |
| **independent** | **no** | no field, check or record of it exists | measured: X1. *Code:* `Requirement::Fact` has no observer field; `check_fact` uses provenance only to format the reason (`kernel.rs:491–521`) |

**What v9r does enforce, which bounds every property above:**

- **Who may answer.** Only registered observers attest, and an
  answer is bound to its provider and request (G7).
  - `Registry::restrict(kind, observers)` limits which providers may
    answer a kind.
  - `define` refuses answers under a foreign definition (measured:
    `a_declared_definition_refuses_foreign_answers`,
    `evidence_copied_between_domains_is_caught_only_by_declared_observers`).
- **What is not evidence.** Claims, and `ClaimsOnly` providers, never
  satisfy a rule (measured: `claims_instead_of_evidence_block`).
- **Disagreement.** Two registered observers that disagree on a key
  give Blocked (measured: `disagreeing_observers_block`). This turns a
  failure of any one property into Blocked **if a second observer that
  has the property exists**. It never turns it into the truth.

All of these constrain **which** observer answered. None constrains
**how** it computed its answer.

## 4. Which properties are assumptions

| Property | Status | Who must uphold it |
|---|---|---|
| faithful | assumption, except content-addressed derivations | whoever registers the observer |
| complete | assumption, except omitted directories and unreadable files | the observer, and the filesystem's link counts |
| current: observer freshness | assumption | the observer |
| current: decision currency at completion | assumption (X4 measured it failing) | the host: nothing writes the watched state during a decision |
| independent (I-out) | assumption (X1 measured it failing) | whoever registers the observer and writes the rule |
| independent (I-eff) | assumption, unmeasured | the observer's execution environment |
| adequacy (k means P) | assumption | the rule's author |

**The trust boundary, stated once:**

> v9r establishes **which** registered observer answered **which**
> request about **which** state. It does not establish that the answer
> is a faithful, complete, current or independent function of that
> state, except for faithfulness and completeness of content-addressed
> derivations. The decision is valid evidence of P(S) only under those
> assumptions. X1 shows that faithfulness, completeness and currency
> together do not imply independence.

## 5. Can independence be a `Fact` or `Within` obligation?

**Answer: not with the existing evidence.** Each form can carry the
statement, but no registered observer can supply the evidence for it.

### 5.1 As a condition on an existing `Fact`: no

- *Code:* `Requirement::Fact { subject, value, strength }` has no
  observer or provenance field (`kernel.rs:292`). `check_fact`
  (`kernel.rs:491`) satisfies a Hard fact with any single `Verified`
  value equal to the required one. It reads `provenance` only to
  format the reason.
- *Code:* verified values are de-duplicated by value. Two observers
  that agree count as one. Agreement between two self-referential
  observers adds nothing the kernel can see.
- So the rule `tests(S1) = passed` cannot say "and its producer did not
  execute S1". The kernel's form cannot hold it, and the kernel is
  frozen.

### 5.2 As a separate `Fact`, e.g. `independent(O, k) = true`: expressible, not established

- The kernel would accept such a fact like any other. But it would
  itself be attested by some observer O′, and is valid only if O′ is
  faithful, current and independent of O's execution. **The regress
  ends in an assumption**: a registration-time judgement, the same
  place it sits today.
- The kernel does not link two findings. It does not know that
  `independent(O, k)` is about the producer of the `tests(S1)` finding.
  The only link is naming: the key names O, and `restrict` makes O the
  only allowed answerer. That pins **who** answered `tests(S1)`, which
  v9r already enforces (§3). It adds nothing about **how**.
- *Measured analogue:* G11 shows the same structure for the writer of
  S1. The decision is identical whoever wrote the state
  (`identical_s1_gets_identical_decisions_whoever_wrote_it`). The
  observer's execution is to its report what the writer is to S1, and
  is equally invisible.

### 5.3 As `Within`: the form fits, the evidence does not exist

- *Code:* `Within { names, scopes }` judges names the obligation
  supplies (`kernel.rs:299`, `check_within` at `kernel.rs:569`). It
  reads no evidence.
- Both halves of independence have a natural `Within` shape over an
  **execution record of O**:
  - **I-eff:** the names O wrote during the decision ⊆ ∅ (or ⊆ its
    scratch area);
  - **I-out:** the files O executed ∩ the judged tree = ∅. For a test
    runner, by intent, that ∩ is non-empty. The runner is
    self-referential by construction, and `Within` would correctly Deny
    it.
- No registered observer reports O's write set or executed files. That
  record is **execution provenance**.
- *Code:* POST obligations are derived (`runtime.rs:504`) **before**
  POST evidence is collected (`runtime.rs:510`). An invariant cannot
  read the runner's provenance to build its `names`. The names would
  have to arrive as a verified fact about a key, from an observer of
  O's execution.

**Conclusion of §5:** independence reduces to execution provenance.
Expressing it needs no new kernel form. `Within` over an execution
record already has the right shape. Establishing it needs evidence
about an execution, and v9r has none, for the actor (G11) or for its
observers (X1). This is one gap, not two.

## 6. The smallest experiment that separates the three

### 6.1 The three, defined

*Definition:*

- **State validity:** P(S1). Does the judged state have the meant
  property? Ground truth comes from outside v9r (the X1 test's
  independent `python3` check).
- **Observer validity:** O(S1) = P(S1). Is the evidence true of the
  state?
- **Execution provenance:** what ran to produce S1 (the writer), and
  what ran to produce O's report (the observer's execution).

### 6.2 What is already separated, by measurement

| Measured pair | Held fixed | Varied | Decision followed | Report |
|---|---|---|---|---|
| honest fix vs broken, no exit | O (runner, valid on both), writer | **state validity** | state | SELF_REFERENTIAL_OBSERVERS_V0 |
| honest fix vs X1 | O, writer | state validity **and** observer validity, together | **observer**, not state | SELF_REFERENTIAL_OBSERVERS_V0 |
| World A vs World B | S1, observers (none execute S1) | **writer provenance** | neither: identical decisions | STATE_VS_CAUSALITY_V0 |
| X4 vs honest fix | S1, O | live state after capture | captured S1 only | SELF_REFERENTIAL_OBSERVERS_V0 |

**Not yet separated:**

- **observer validity without state validity changing.** X1 moved both
  at once (P false, O passed). No pair holds S1 fixed and varies only
  whether O is valid on it.
- **observer execution provenance.** In X1 it was recorded only by the
  test harness (`runner executed the judged greet.py: true`), never by
  v9r. No pair varies it with S1 fixed.
- **writer provenance with a self-referential observer.**
  STATE_VS_CAUSALITY_V0 used observers that execute nothing from the
  workspace.

### 6.3 The experiment

**One S1, the X1 state, held fixed. One rule, I3 on `tests(S1)`. Two
observers registered in turn, and two writers.** No new verifier
semantics; a test like `self_referential_observers.rs`.

Observers:

- **O_exec:** the existing runner. It executes S1's `greet.py`.
- **O_data:** reads S1's `src/greet.py` bytes from the
  content-addressed store and compares them with the honest fix's bytes,
  answering `passed`/`failed` for the same key. It executes nothing
  from S1.

  O_data is not a proposed design. It is a control observer whose
  independence holds by construction in the test, so the observer
  factor can vary with S1 held fixed.

| Cell | S1 | P(S1) | Observer | Writer | Predicted verdict | What it isolates |
|---|---|---|---|---|---|---|
| 1 | X1 | false | O_exec | actor | Allow (measured) | baseline: invalid observer |
| 2 | X1 | false | O_data | actor | Deny | **observer validity alone**: same S1, same rule, verdict flips |
| 3 | X1 | false | O_exec | outsider | Allow, decision identical to 1 | **writer provenance alone**, with a self-referential observer |
| 4 | X1 | false | O_data | outsider | Deny, decision identical to 2 | provenance × observer interaction: none expected |

Measured per cell:

- the verdict;
- P(S1), from the independent check;
- whether the decision record differs from its sibling cell, field by
  field as in STATE_VS_CAUSALITY_V0;
- the harness's out-of-band record of what each observer executed.

**What each outcome would show:**

- **1 vs 2** separates **observer validity** from **state validity**:
  S1 and P are fixed, and only O changes. The two decision records
  differ only in the I3 finding's value and observer name. Nothing in
  record 1 shows that O_exec executed S1. That missing field is the
  measured gap.
- **1 vs 3** and **2 vs 4** separate **execution provenance** from
  both, for each observer kind. If 3 equals 1 and 4 equals 2, G11 holds
  with self-referential observers too, and provenance is invisible
  whichever observer is used.
- **The harness's execution record vs the decision record**, across
  all cells, measures exactly what v9r would need to receive to
  represent independence as `Within` (§5.3): the set of judged files
  each observer executed. It is {`src/greet.py`, …} for O_exec and ∅
  for O_data.

**Why it is the smallest:** S1, the rule and the registry layout are
fixed; two binary factors; four cells. Every component already exists.
O_exec is the measured runner, the outsider writer is the measured
State vs Causality construction, and the content store is the measured
`ObjectStore`. Fewer cells cannot vary observer validity with S1
fixed (needs 1 vs 2) and also test provenance under both observer kinds
(needs 3 and 4).

**What it does not cover:** I-eff (G′), decision currency (X4, already
measured), and any observer that is neither pure data nor arbitrary
execution.

## 7. Summary

1. **Properties for valid evidence:** faithful, complete, current
   (observer freshness and decision currency) and independent (I-out,
   I-eff), plus the rule's adequacy.
2. **Enforced:**
   - faithfulness and completeness for content-addressed derivations
     only;
   - omitted directories and unreadable files;
   - request binding, which is not freshness;
   - decision currency at the start of execution only;
   - which observer may answer, and that claims never count;
   - Blocked on disagreement, given a second observer.
3. **Assumed:** faithfulness and completeness of raw observers;
   freshness; decision currency at completion (X4, measured failing);
   independence I-out (X1, measured failing) and I-eff (unmeasured);
   adequacy.
4. **Fact/Within:**
   - not as a condition on an existing `Fact`: the kernel ignores
     provenance;
   - as a separate `Fact`, only by regress to an assumption;
   - as `Within`, the shape fits, but needs an execution record of the
     observer that no observer supplies.

   Independence reduces to execution provenance.
5. **Smallest separating experiment:** the X1 state fixed, observers
   {executing, data-only} × writers {actor, outsider}, four cells.
