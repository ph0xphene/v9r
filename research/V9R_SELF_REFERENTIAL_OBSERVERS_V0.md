# v9r Self-Referential Observers v0

*Measurement of adversarial findings X1 and X4
(V9R_ADVERSARIAL_REVIEW_V0) on the demo's setup. Measured on
2026-10-02, one Linux host (6.12), rustc 1.98.1, cargo 1.98.0, Python 3.12.13.*

**Revision measured:** the code of `v9r-review-v1` (code commit
`6a9c717`; `git diff 6a9c717 HEAD -- crates Cargo.toml Cargo.lock` is
empty; `kernel.rs` sha256 `85badb66…6177f`). No file under `src/` was
changed. The test file `crates/v9r-core/tests/self_referential_observers.rs`
is new and is **not part of `v9r-review-v1`**.

**Reproduce:**

```sh
cargo test --offline -p v9r-core --test self_referential_observers -- --nocapture --test-threads=1
```

The test compiled and ran unchanged on the first attempt. It was run
4 times; every run gave the results below.

## 1. Question

When a registered observer executes the state it judges, does v9r
accept a state that the observer's report misdescribes?

Two sub-questions:

- **X1:** can the judged state decide the test runner's report, so
  that a broken `greet` is accepted?
- **X4:** can the watched state change after the after-snapshot and
  before the transition completes, with v9r accepting the earlier S1?

## 2. Existing claim being tested

From V9R_RELEASE_FREEZE_V1 §6, as released:

- **G1:** "Accepted only if every declared rule is satisfied by
  **verified** evidence."
- **G8:** "Anything written inside the window is attributed to the
  effect."
- **G9:** "Facts bind to the state they name."
- The demo's rule I3: "tests pass on the result, run by v9r"; its
  closing line: "Every decision came from what v9r observed."

V9R_CLAIM_REVISION_V0 predicted, from code reading only, that X1 and X4
each give a false ALLOW.

## 3. Experimental setup

The demo's structure, rebuilt in the test (`examples/v9r_demo.rs` is
not modified):

- workspace `ws/`: `src/greet.py` (wrong), protected
  `tests/test_greet.py`, which does `from greet import greet` and exits
  0 iff `greet('v9r') == 'Hello, v9r!'`;
- registry: `RawFsObserver` (attesting, sole source of `fs_file` /
  `fs_link`), `FsSnapshot` verifier, and a test runner (attesting). The
  runner materializes S1 from the object store into
  `checkout-N/` and runs `python3 tests/test_greet.py` there; exit 0 →
  `passed`;
- rules: I1 (PRE and POST: the before-state is S0), I2 (only `ws/src`
  changed), I3 (`tests(S1) = passed`, keyed by S1's tree id, `@now`).

Independent check, outside v9r: a separate `python3` imports
`ws/src/greet.py` and checks `greet('v9r') == 'Hello, v9r!'`, catching
`SystemExit`.

Order inside `Runtime::execute` (`runtime.rs`): the after-snapshot is
taken when `act` returns (l. 485); POST evidence, including the test
run, is collected after it (l. 510); on Allow, `trusted` becomes the
after-snapshot (l. 517).

**X1 agent:** writes `src/greet.py` = `import sys\nsys.exit(0)\n`.

**X4 agent:** writes the correct fix to `src/greet.py`, spawns a
detached `sh` process and returns. That process busy-waits until the
runner's first checkout directory exists, which happens only during
POST evidence collection, then overwrites the **live**
`ws/tests/test_greet.py` with `import sys\nsys.exit(0)\n` and records
the time (`date +%s%N`). The test compares that time with the clock
read right after `execute` returns. 10 transitions per run.

**Difference from the adversarial review's G′.** The review's X4 (G′)
has the write done **by S1's code at import time, executed by the test
runner**. The test measures a different writer: **a process the actor
left behind**, timed to the runner's checkout. Both writes land at the
same point (after the after-snapshot, during POST evidence); the
writer differs. G′ itself, the observer executing S1 and thereby
changing the live state, **was not measured**.

## 4. X1 result

**Measured** (identical in all 4 runs; tree ids are deterministic):

| Agent | S1 | PRE | POST | accepted | `tests(S1)` from | `greet` correct (independent) |
|---|---|---|---|---|---|---|
| honest fix (control) | `4ba46cdb…4b40` | Allow | **Allow** | true | `provider:test-runner` | **true** |
| broken module, no exit (control) | `ac518e27…a829` | Allow | **Deny** | false | `provider:test-runner` | false |
| **X1**: `sys.exit(0)` on import | `924e3f54…11da` | Allow | **Allow** | **true** | `provider:test-runner` | **false** |

The X1 decision, verbatim except tree ids shortened:

```
Post Allow
  [ok] transition.ordered: transition(10, 11, 12)@now = true, verified by temporal-clock
  [ok] I1: snapshot(ws)@s10 = 6f8ce6dd…, verified by verifier:fs-snapshot (…; trusts provider:raw)
  [ok] I2: snapshot_entries(ws)@s10 = {4 entries, …}, verified by verifier:fs-snapshot
  [ok] I2: snapshot_entries(ws)@s12 = {4 entries, …}, verified by verifier:fs-snapshot
  [ok] I2: 1 name(s) within ["ws/src"]
  [ok] I3: snapshot(ws)@s12 = 924e3f54…, verified by verifier:fs-snapshot
  [ok] I3: tests(924e3f54…)@now = passed, verified by provider:test-runner (lineage:L315)
```

In every case the runner's checkout held the judged `greet.py`
byte-for-byte.

**X1 reproduced.** v9r accepted a state in which `greet` does not
exist. Every finding reads `[ok] … verified by`, and nothing in the
decision distinguishes it from the honest fix.

## 5. X4 result

**Measured:** 4 runs × 10 transitions = **40 transitions**:

| | count |
|---|---|
| PRE Allow | 40/40 |
| POST Allow and accepted | **40/40** |
| background write landed before `execute` returned | **40/40** |
| live tree ≠ accepted S1 after completion | **40/40** |
| live protected test = `import sys\nsys.exit(0)\n` | 40/40 |

Run 0 of each run: the write landed **10.7, 11.1, 11.9 and 12.0 ms**
before `execute` returned.

Run 0 of the first run:

- S0 `6f8ce6dd…`;
- S1, accepted, `4ba46cdb…4b40`: **the same tree id as the honest-fix
  control**;
- live tree after completion: `073d8cda…5745`;
- the runtime is still accepting (`is_accepting() == true`), with
  `trusted` = S1;
- the decision is the honest control's decision, up to counter values
  (snapshot and lineage numbers).

**X4, as constructed in the test, reproduced.** The protected test was
replaced in the live workspace while v9r was still deciding. v9r
accepted the earlier S1, and its trusted state described a workspace
that no longer existed when `execute` returned.

**G′ (the observer's execution of S1 performs the write) was not
measured** and remains predicted.

## 6. Controls

- **Honest fix** (X1 table): Allow, `greet` correct. The runner can
  pass a correct state, and X4's S1 is identical to it.
- **Broken module without the exit** (X1 table): Deny, from the same
  runner and the same rule. The only difference between this control
  and X1 is `sys.exit(0)`. So the I3 verdict changed because of code
  inside S1: the judged code decided the runner's report.
- **Runner identity:** every `tests(S1)` finding names
  `provider:test-runner`, and the runner's checkout held the judged
  `greet.py`. The result did not come from a different source.
- **Timing (X4):** the background process waits on `checkout-0`. The
  runner creates it from S1's tree id, which exists only after the
  after-snapshot. So the write is after the after-snapshot by
  construction, and before completion by measurement (40/40).
- **Not controlled:** v9r has no rule watching the live workspace
  after the after-snapshot. X4 therefore has no "v9r notices"
  control. That is the finding, not a gap in the setup.

## 7. What this invalidates

**Measured findings:**

- **The demo's I3 label, "tests pass on the result, run by v9r".**
  X1 passes I3 with no `greet`. I3 establishes *the protected test
  command exited 0 on a checkout of S1*, and S1's code decides that
  exit status.
- **Reading G1's "verified evidence" as "true".** X1's false finding is
  `verified by provider:test-runner`. The runner is faithful: it reports
  the exit status it saw. The report is controlled by the judged
  state.
- **Reading G9 ("facts bind to the state they name") as independence.**
  The `tests(S1)` fact is bound to S1, and S1 decided it.
- **G8 as written ("anything written inside the window is attributed to
  the effect"), without a definition of the window.** A write that
  lands while v9r is still deciding, before `execute` returns, is not
  attributed to anything. The window closes at the after-snapshot, not
  at completion.
- **"S1 becomes trusted" read as "the trusted state is the current
  state".** After X4, the runtime's trusted state is stale on
  completion (40/40).
- **The demo's closing line, "Every decision came from what v9r
  observed".** I3's value came from a process S1 controlled.

**Interpretation:**

- Neither result is a kernel or registry defect. Every component did
  what its code says. The overclaim was in the wording, and in the
  demo presenting a self-referential observer as an independent one.
- X4 is the actor's ambient authority (it can leave processes behind)
  meeting a decision that takes time. It does not need a
  self-referential observer at all. The runner's checkout was used only
  as a timing signal.

## 8. What remains valid

**Measured in this experiment:**

- **I1 and I2 were satisfied in every decision** (52: 4 × 3 in X1,
  40 in X4). Each Allow implies it, and the Deny's printed findings
  show it: S0 matched, and the one changed name was within `ws/src`.
- **The decision is a function of the captured S1.** X4 and the honest
  control have the same S1 and the same decision. Consistent with G11.
- **The runner's report is bound to S1's tree id** (G9's narrow
  reading): the checkout was S1's tree.
- **The broken module control was denied.** A self-referential observer
  can still give true negatives; it just cannot be relied on for
  positives.

**Not affected (not exercised here; still supported by the tests that
measure them):** G2–G7 and G10–G12 of V9R_RELEASE_FREEZE_V1, in the
narrowed wording of V9R_CLAIM_REVISION_V0 §1.1.

**Still predicted, not measured:** X2 (write after `execute` returns),
X3 (metadata outside the identity), X5 (restart launders a rejected
state), G′ (the observer's execution of S1 writes the live state).

## 9. Future design ideas (not built, not claimed)

These are listed so they are not mistaken for results:

- **Typing evidence by independence:** mark facts produced by executing
  the judged state as self-referential, so a rule can refuse them. It
  touches what `Verified` means, so it is a kernel question
  (V9R_CLAIM_REVISION_V0 §4.2).
- **A runner outside the judged state's authority:** separate uid,
  namespaces, no write access to the live workspace. The archived
  Controlled Domain and Capability Manifest experiments measured parts
  of this.
- **Closing the window at completion:** re-observe the watched keys after
  POST evidence and refuse on drift. That would detect the X4 write.
  It cannot detect writes after that check, so it narrows the gap
  rather than closing it.
- **Execution receipts:** an isolating executor reporting which
  processes the actor left running when `act` returned. That would
  address X4's writer; it would not address X1, where the report is
  faithful and the rule is weak.
