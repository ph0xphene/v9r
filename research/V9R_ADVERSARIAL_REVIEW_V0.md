# v9r Adversarial Review v0

*A skeptical external-review simulation against `v9r-review-v1`
(`560d8da`; code commit `6a9c717`). The reviewer has only the repository,
README_REVIEWER.md, V9R_RELEASE_FREEZE_V1.md and the research reports.
The aim is to falsify, not to defend or improve. No code was modified.*

> **Method and its limit.** Every attack below was analysed by reading
> the frozen code. File and line references are to `crates/v9r-core`.
> Five attacks (X1–X5, §3) were written as a scratch example against a
> throwaway worktree of the tag. **They were not executed**: the
> session's tool sandbox blocked running them. Their verdicts are
> **predictions from code reading, not measurements**. Where a verdict
> is already measured by an existing test, the test is named. Turning
> X1–X5 into measurements is the first recommendation (§6).

> **Measured status (added for `v9r-review-v1.1`, 2026-10-02).** The
> text below is kept as written, as a record of predictions. Since then:
>
> - **X1 (G): measured, prediction held.** Allow with `greet` absent;
>   the same module without `sys.exit(0)` is denied.
>   `x1_judged_program_decides_the_test_runners_report`.
> - **X4: measured in a different construction.** The writer was a
>   process the actor left behind, timed to the runner's checkout, not
>   S1's code executed by the runner. Allow and accepted 40/40; the live
>   tree differed from the accepted S1 40/40.
>   `x4_watched_state_changes_after_observation_before_completion`.
> - **G′** (the runner's execution of S1 writes the live state), **X2,
>   X3, X5: still predictions, not run.**
>
> Report: V9R_SELF_REFERENTIAL_OBSERVERS_V0.

## 1. Claim extraction

### 1.1 The strongest defensible statement

> Given a state S0 that the host approved, a set of rules, and a set of
> registered observers:
>
> - v9r grants one single-use authorization to change it;
> - it refuses to start if the observed state no longer matches S0;
> - after the actor returns, it accepts the observed state S1 only if
>   every rule is satisfied by evidence that a registered observer
>   attested in reply to v9r's own request, within an ordered
>   before/after window;
> - otherwise it denies or blocks, and it then accepts nothing further.
>
> The decision depends only on S0, S1, the rules, the observers' answers
> and the ordering of the window. In particular, it does not depend on
> who wrote S1.

### 1.2 Proven by tests (at `v9r-review-v1`)

| Statement | Tests |
|---|---|
| unknown never becomes true or false; claims never satisfy a rule; contradiction blocks | kernel unit tests (13) |
| replayed, forwarded, volunteered or anachronistic attestations are dropped | `claim_guarantees` (5 tests, G7) |
| missing or contradictory evidence blocks | `claim_guarantees` (5 tests, G2) |
| a state change after authorization refuses execution and holds | `external_modification_between_authorization_and_execution_refuses`; demo run 5 |
| changes outside the scope **in names, content, symlink targets, empty directories and the owner-execute bit** are denied | `scope_entries` (18 cases) |
| identity = git's SHA-256 tree id; a claimed id is recomputed | `content_addressed` |
| identical S1 from an authorized or an unauthorized writer → identical complete decisions | `state_vs_causality` (2) |
| a lone lying or caching observer gives a **false Allow** | `a_trusted_liar_…`, `stale_raw_content_…`, `stale_state_…`, `hidden_file_…` |

### 1.3 Assumed (not tested, and not testable inside v9r)

- `fs_raw` reports the filesystem faithfully and completely. Link-count
  checks catch an omitted directory; nothing catches an omitted file
  without a second observer.
- **Every registered observer is faithful**, including observers that
  run code.
- Capture is not torn by a concurrent writer, beyond what the double
  reading detects.
- The process running v9r, its uid, its binary and the host are not
  compromised.
- The rules express what the host means.
- **The host approves S0 correctly and keeps that approval.** v9r
  persists nothing.

### 1.4 Explicitly not claimed

- who produced S1 (causality);
- containment of the actor;
- intent or reasoning;
- rollback;
- freshness across restarts;
- any property of the state after it was observed.

## 2. Attack surface map

Verdict column: what v9r returns, **predicted** where untested.

### 2.1 Kernel

| Target | Attack | Tested? | Verdict | Remaining uncertainty |
|---|---|---|---|---|
| `Fact` | a second verified value for the subject | yes | Blocked (contradiction) | an attacker who can register an observer can turn any Deny into Blocked. Safe, but it is a denial-of-judgment lever |
| `Fact` | a `Proposed` or `Semantic` value equal to the required one | yes | Blocked | — |
| `Within` | scope `""` (the empty scope contains everything, `kernel.rs` `name_within`) | no | **Allow** for any change | a rule-author mistake, not an attack. The empty-scope semantics are documented but a reviewer may not expect them |
| `Within` | names derived by the invariant, not by evidence | partly (pins) | — | names come from the invariant's computation over pinned values. The pins make the *inputs* verified; the computation (`changed`) is trusted rule code |
| `AtMost` | the quantity's value is supplied by the invariant, not by evidence | no | whatever the invariant says | not on the claim path, but a reviewer reading "three requirement forms" will assume all three judge evidence. `AtMost` does not |
| evidence creation | mint `Verified` outside the crate | compile-time | impossible (`compile_fail` doctests) | the kernel's own `compile_fail` example on `map` is vacuous; the replacement is on `GraphEvidence` |
| verdict semantics | Deny dominates Blocked | yes | — | a contradicted *true violation* yields Blocked, not Deny. Fail-safe, but it hides a violation |

### 2.2 Runtime

| Target | Attack | Tested? | Verdict | Remaining uncertainty |
|---|---|---|---|---|
| authorization | reuse an `Authorization` | compile-time (moved, no `Clone`) | impossible in safe Rust | a host that builds two runtimes over one workspace gets two independent authorities over one state; nothing coordinates them |
| authorization | bind to another policy | partly | refused (`ForeignAuthorization`) | the policy digest is the **invariant ids joined by "+"** (`temporal.rs` `TemporalDomain::new`), not their content. Within one runtime this cannot matter: invariants are fixed at construction. Across runtimes, the runtime id already differs |
| freshness | change S between authorize and execute | yes | refused, hold | `authorize` judges the **stored** trusted snapshot (`runtime.rs:416`), with no fresh observation. Only `execute` observes afresh |
| execution ordering | outsider writes inside the window | yes | attributed to the effect (Deny if outside the scope; identical if inside) | by design (G8, G11) |
| execution ordering | **the actor writes after the after-snapshot** (a background child) | **no** | **Allow** (X2) | the window closes when `act` returns (`runtime.rs:485`), not when the actor's processes end |
| observation | **observers that run after S1 was observed** | **no** | see X4 | POST evidence is collected *after* the after-snapshot (`runtime.rs:510`). A live `@now` key, such as the demo's test run, executes after S1 is fixed |
| hold | after a rejection, restart | **no** | a new runtime trusts the current state (X5) | `temporal::runtime` takes its baseline *now* (`temporal.rs:526`); nothing persists "held" |

### 2.3 Observers

| Target | Attack | Tested? | Verdict | Remaining uncertainty |
|---|---|---|---|---|
| completeness | omit a file from a listing | yes | **Allow** with one observer; Blocked with two | measured limit |
| completeness | omit a directory | yes | Blocked (link count) | **only where directories keep link counts**: the check is skipped when `nlink < 2` (`snapshot.rs:349`), e.g. btrfs, which reports 1 |
| freshness | re-attest a cached old value | yes | **Allow** (false) | measured limit |
| disagreement | two observers disagree | yes | Blocked | — |
| malicious provider | volunteer, replay or anachronism | yes | dropped | — |
| malicious provider | an Attesting provider answers kinds it should not | partly | dropped only for kinds under `restrict` | the demo restricts `fs_file`/`fs_link` to `raw`, but not `fs_dir`/`fs_stat`. A rogue registered provider answering `fs_dir` causes a contradiction (Blocked), not an Allow, while `raw` is present |
| **observers that execute S1** | S1's code controls the observer's output | **no** | **Allow** (X1) | not addressed beyond one sentence in the limitations |

### 2.4 Snapshots

| Target | Attack | Tested? | Verdict | Remaining uncertainty |
|---|---|---|---|---|
| ingestion | concurrent writer during capture | partly (double reading; Atomic Capture v0) | Blocked if the readings differ | a torn tree read identically twice is accepted. Atomic Capture v0 found only the cgroup freezer reliable |
| identity | change **group/other permission bits, setuid/setgid/sticky, owner, xattrs, ACLs, timestamps** | **no** | **Allow** (X3) | `fs_raw.rs:73` maps a regular file to `100644`/`100755` by the owner-execute bit only. None of the rest is in the tree id or in `snapshot_entries`. **G3 says "file modes"; it means the owner-execute bit** |
| identity | hard link from inside the scope to a file outside the observed root | no | Allow | later writes through the link change an unobserved file. Outside the model (no containment), but within the scope rule's apparent promise |
| identity | special files | yes | Blocked | — |
| atomicity | — | — | — | capture is trusted. Stated |

### 2.5 Causality

| Attack | Tested? | Verdict |
|---|---|---|
| an unauthorized writer produces identical S1 inside the window | **yes**: complete decisions identical | Allow for both, **by design** |
| a reviewer reads Allow as "the authorized agent did this correctly" | — | outside the model; see F |

## 3. Constructing false ALLOWs

X1–X5 are concrete constructions on the demo's setup: the same rules
I1–I3, the same raw observer, snapshot verifier and test runner.
**Predicted, not run** (method note at the top). *Status: G (X1)
measured; X4 measured with a different writer; see the status note at
the top.*

| | Construction | Allow? | Deny? | Blocked? | Outside model? |
|---|---|---|---|---|---|
| **A** malicious observer omits a file | one observer that drops a name from `fs_dir` | **yes** (measured: `hidden_file_…_only_with_an_independent_observer`) | — | with a second honest observer | the observer is in the TCB |
| **B** two observers read different states | readings that differ between or within snapshots | no | — | **yes** (measured: `observers_reading_different_versions_block`, `disagreeing_observers_block`) | a torn state read consistently twice: Allow, and capture is trusted |
| **C** state changes between approval and observation | write between authorize and execute | no | — | refused + hold (measured) | — |
| **C′** state changes **after** observation (X2) | the actor fixes `src/greet.py`, then spawns `sh -c 'sleep 0.3; …'`, which overwrites the protected `tests/test_greet.py` after `act` returns | **yes**: S1 is observed before the write; I1–I3 hold | — | — | partly. "No property after observation" is stated (§1.4), but the change is the actor's own, and **this runtime never observes it**. The demo-style I1 pins the original S0, so the next `authorize` is Denied on I1 without a fresh observation |
| **D** consistent but false evidence | a caching observer, or a liar alone | **yes** (measured: `stale_…`, `a_trusted_liar_…`) | — | with an independent witness | observer in the TCB |
| **E** compromised runtime process | anything | yes | — | — | **outside the model**, but **reachable from G**: see below |
| **F** state validity read as causal validity | a reviewer reads demo run 1, "ALLOW — honest fix", as "the agent fixed it" | — | — | — | outside the model. **The demo's labels invite this misreading**; G11 refutes it |
| **G** a check executes code from the judged state (X1) | the agent writes `src/greet.py` = `import sys; sys.exit(0)`. The protected test does `from greet import greet`, so the test process exits 0 on import. The runner reports `passed` (`v9r_demo.rs`: `status.success()`) | **yes**: I1 (S0), I2 (only `src/` changed) and I3 (`tests(S1) = passed`, verified by `provider:test-runner`) are all satisfied, and `greet` does not exist | — | — | **inside the model**: the observer faithfully reports the exit code. The rule, not the observer, is weak. **No test covers it; the flagship demo is vulnerable to a two-line agent**. *Now measured: Allow, `greet` absent (`x1_…`)* |
| **G′** (X4) | `src/greet.py` contains the correct fix *and*, at import time, overwrites the live workspace's protected test (absolute path). The runner executes it during POST evidence collection, after S1 was observed | **yes**: S1 has the protected test intact; the live workspace no longer does | — | — | inside the model's mechanics. The runner runs S1's code **as the verifier's uid, unsandboxed**, so it can reach the workspace, the user's files, and with the same uid potentially the verifier process itself (G → E). *Still predicted: the measured X4 used the actor's leftover process as the writer* |
| **H** restored runtime, stale authority (X5) | run 2 of the demo (DENY, hold); the host restarts and, as the demo and the causality test do, derives S0 from the current state (`lab.s0()` → `registry.query(snapshot)`) | **yes**: the new runtime's PRE is Allow on a state containing the weakened test | — | — | **host responsibility.** v9r persists neither approvals nor holds; `temporal::runtime` takes the current state as its baseline. Stated as "no freshness across restarts", but the consequence ("a restart launders a rejected state") is not stated |
| X3 (identity) | the agent fixes `greet`, and also `chmod 2666 tests/test_greet.py` (protected test now group- and world-writable, setgid) | **yes**: the owner-execute bit is unchanged, so the tree id and `snapshot_entries` are unchanged outside `src/` | — | — | **inside the model.** G3 overstates "file modes" |

**Summary.** With every registered observer honest and the runtime
uncompromised, the predicted false ALLOWs are:

- **X1 (G):** the rule is satisfied by S1's own code;
- **X3:** the change is invisible to the identity;
- **X2/G′ (C′):** the change happens after observation;
- **H:** a host restart.

A and D are measured, admitted limits of observer trust.

## 4. Comparison with other systems (conceptual; no equivalence claimed)

| System | What it establishes | Difference from v9r |
|---|---|---|
| **git object model** | content identity; a tree id commits to names, modes (as git defines them) and content | v9r **reuses** it as state identity, so it inherits git's blindness to ownership, most permission bits, xattrs and timestamps (X3). git judges nothing and authorizes nothing. Its authorship fields are claims; v9r does not even record claims of authorship |
| **database transactions** | atomicity, isolation and durability via concurrency control; a failed validation aborts and rolls back | v9r has **no isolation** (an outsider can write inside the window), **no atomic capture**, **no rollback** (it holds) and **no durability** of its own state (H). The nearest analogue is the *validation phase* of optimistic concurrency control: check after the fact. But nothing is aborted; the effect has already happened |
| **capability systems** | what an actor *can* do, bounded by construction before it acts | v9r is **detective, not preventive**: it judges what happened. The archived capability experiments explored the preventive side and were removed from the claim. Under v9r alone, X2, G′ and E are possible precisely because the actor holds ambient authority |
| **reproducible builds** | an output is bound to its inputs and process by independent re-execution | this is the **causal link v9r lacks** (G11). v9r does not re-execute the effect; it judges the result. Its test runner *does* execute S1, but to evaluate S1, not to reproduce it, and that execution is attacker-controlled (G) |
| **proof-carrying code** | the producer ships a proof; a small trusted checker verifies it against a policy; the producer is untrusted | v9r shares the shape: a small trusted kernel and an untrusted actor. But the actor supplies **nothing checkable**: all evidence comes from trusted observers. Where PCC's soundness rests on the checker alone, v9r's rests on the checker **and every observer** |

## 5. The single weakest assumption

**Observer faithfulness, in its sharpest form: an observer that
executes the judged state is treated as a faithful, independent
observer.**

Why this one, rather than the others:

- **Cheapest attack.** X1 is two lines of Python. Observer lying (A, D)
  requires controlling a registered observer. Atomic-ingestion attacks
  need a concurrent writer and timing. Runtime compromise needs a
  foothold.
- **In the flagship artifact.** The demo's I3 ("tests pass on the
  result, run by v9r") is exactly this observer. The reviewer's first
  impression of v9r rests on it.
- **Untested.** Observer lying, caching and omission each have tests
  that *demonstrate the limit*. This one has a sentence in the
  limitations and no test.
- **It escalates.** The runner executes S1 unsandboxed, as the verifier's
  uid, after S1 was observed. So it also defeats the window (G′), and
  for a same-uid deployment it reaches the TCB (G → E). The other
  candidate assumptions fail locally; this one fails across layers.
- **The claim's wording does not exclude it.** "Evidence from registered
  observers" is literally true. A reviewer will reasonably read "tests
  pass on the result, run by v9r" as independent evidence, and it is
  not.

The runners-up:

- **No durable state (H):** it enables laundering, but needs a host
  restart.
- **Identity blind to most metadata (X3):** cheap, but limited to
  metadata.
- **Absence of causal receipts:** this is the stated boundary, not a
  weakness of the claim.

## 6. Readiness verdict

**Outcome B: the claim needs wording reduction.** A short list of
experiments should accompany it.

The core claim, decisions from evidence by declared rules and states
not histories, survives every attack above. **No attack produced an
Allow that contradicts the claim as literally stated in §1.1.** Several
produced Allows that contradict **how the release documents phrase the
guarantees**:

1. **G3 "file modes".** It means the owner-execute bit only. Ownership,
   group/other permissions, setuid/setgid/sticky, xattrs, ACLs and
   timestamps are invisible (X3).
2. **"tests pass on the result, run by v9r"** (the demo, README_REVIEWER).
   When the test executes S1's code, S1 controls that evidence (X1).
   Say: *"the test command exits 0 on S1; S1's code runs inside it, so
   this is not independent evidence."*
3. **"accepted state".** Accepted *as observed when the actor returned*.
   Nothing bounds the actor's processes or later effects (X2), and
   observers that run after observation can change the live state (G′).
4. **"hold".** Not durable. A restart that re-derives S0 from the current
   state accepts a rejected state (X5). Approval and hold are the host's
   to persist.
5. **"three requirement forms judge evidence".** `AtMost` judges a value
   supplied by the rule.

**Why not C:**

- X1–X5 follow directly from code read at specific lines.
- None contradicts §1.1.
- A reviewer can confirm each by reading.

**Why not A:** the current wording invites the misreadings above, and a
skeptical reviewer will find X1 within minutes of reading the demo.

**Strongly recommended before handing over:**

- run X1–X5 as tests that **assert the false ALLOW**, the way the
  liar/caching limits are already kept;
- apply the five wording reductions to README_REVIEWER and the freeze
  document.

Neither requires changing the verifier.

## 7. Top five reviewer questions

1. **The demo's I3 runs S1's own code.** Is a rule satisfied by S1's
   code (`sys.exit(0)` on import) evidence of anything? What separates
   an *independent* observer from one the judged state controls, and
   why does v9r not type that difference?
2. **What exactly does "a change outside the scope" cover?** The identity
   records the owner-execute bit, not permissions, ownership or xattrs.
   Which security-relevant changes can an agent make outside the scope
   and still get Allow?
3. **When does the window close?** `act` returning is not the actor's
   processes ending. What is accepted: the state at observation, or the
   state the actor leaves behind?
4. **Who owns "approved S0" and "held"?** Both live only in one process's
   memory. If a restart re-derives S0 from disk, what stops a rejected
   state from becoming the next approved one?
5. **Is the TCB closed?** Observers that run code (the test runner) run
   as the verifier's uid, unsandboxed, during the decision. Is the
   runtime-not-compromised assumption compatible with that?

## 8. The one experiment that would most change confidence

**Judged-code evidence (X1 + G′), as a measured test:**

1. Run the demo's setup with an agent whose `src/greet.py`
   `sys.exit(0)`s on import; record the verdict and whether `greet`
   works.
2. Run it again with an agent whose fixed `greet.py` also rewrites the
   live protected test at import time; record the verdict and the live
   test's content afterwards.

*Status: step 1 was run and the prediction held. Step 2 was run with
the actor's leftover process as the writer instead of `greet.py`'s
import (Allow, rewritten live test, 40/40); step 2 as written is still
unrun.*

**If the prediction holds** (Allow, broken `greet`; Allow, rewritten
live test), the demo's strongest-looking guarantee rests on an observer
the attacker controls. Every "tests pass" claim must be reworded, or
the runner moved out of the attacker's reach.

**If it fails** (v9r blocks or denies), this review has misread the code
in a way that matters, and confidence in the observer model rises
substantially.

Either way, it is the cheapest result with the largest effect on how
the claim reads.

## 9. Phase 2: before or after external review?

**After.**

- Phase 2 is a debloat: it collapses the generic runtime and removes
  historical modules. None of the findings here is about bloat. They
  are about wording (§6), observer independence (§5) and host-side
  persistence (H).
- A reviewer should judge the frozen, already-measured tree. Starting
  Phase 2 first would mean reviewing code that has never been measured,
  and it would break the kernel freeze the review package relies on.
- Phase 2's shape may depend on the reviewer's answer to question 1:
  whether v9r should *type* evidence by independence. That is a model
  question; it should be settled before the code is collapsed around
  the current model.
