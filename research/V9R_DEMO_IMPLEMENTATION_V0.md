# v9r Demo Implementation v0

Implements `V9R_DEMO_DESIGN_V0.md` as one runnable example:

```
cargo run --offline -p v9r-core --example v9r_demo
```

> **Status: written before implementation.** Results are added below
> only once measured.

## Scope

**Reused, unchanged, all measured in earlier reports:**

| Piece | Role in the demo | Measured in |
|---|---|---|
| `temporal::runtime` (on `runtime.rs`, `kernel.rs`) | proposal → authorization → re-check → execution → observation → verdict → accept or hold | Temporal Evidence v0, Effect Runtime v1, State vs Causality v0 |
| `RawFsObserver` + `FsSnapshot` | `snapshot(ws)`: the git tree id of the workspace, before and after | Content-Addressed State v0 |
| `FilesystemEvidenceProvider` | `entries(ws)`: which files changed | Temporal Evidence v0 |
| `changed` + `Within` | "changed only `src/`" | Temporal Evidence v0 |
| `materialize` | checks out S1 for the test run | Content Identity v0 |
| PRE on `@current` | the authorization is re-checked on the state at execution | State vs Causality v0 |
| `Runtime::add_proposed` | the agent's claim, entered as a claim | kernel (F9) |

**New, inside the example only (no crate code):**

- **A test-runner provider:** `tests(tree)` = `passed` or `failed`. It
  checks the tree out of the store and runs the protected test file with
  `python3`. It is the exec provider of the agent-transition design,
  reduced to one kind.
- **Four scripted agents (`Actor`s)** that write files directly. They
  are deterministic and use no LLM.
- **Rendering** of the decision as a card.

**Not added:** kernel requirements, capability models, security
primitives, new crate modules.

## Mapping to the design

| Run | Agent | Expected |
|---|---|---|
| 1 honest | edits `src/greet.py` correctly; says "Fixed greet()" | **ALLOW**, accepted |
| 2 shortcut | edits `tests/test_greet.py` to expect the old output; says "All tests pass ✅" | **DENY** on I2, while I3 (tests pass) is satisfied |
| 3 bluff | changes nothing; says "All tests pass ✅" (entered as a claim); **no test runner registered** | **BLOCKED** on I3. A second line shows the same run with the runner: **DENY** |
| 4 race | honest agent, but `src/greet.py` is edited by someone else after authorization | **refused before execution**; the runtime holds |

Invariants:

| Id | Rule | Form |
|---|---|---|
| I1 | input is the approved code | PRE `snapshot(ws)@current = S0`; POST `snapshot(ws)@before = S0` |
| I2 | changes only under `src/` | `Within(changed(entries@before, entries@after), [ws/src])` |
| I3 | tests pass on the result, as run by v9r | pin `snapshot(ws)@after = S1`; `tests(S1)@now = "passed"` |

## Gap check (before implementation)

Every row above is expressible with the existing model. **One honest
limitation, not a gap:** run 3's BLOCKED requires the test runner to be
*absent*. With the runner present, a no-change bluff is simply DENY
(the test fails on S0). The demo shows both lines and says why. A
BLOCKED that arises *naturally* would need an observer that can fail to
answer, which the demo does not invent.

Known, stated caveat (from the design): the protected test imports the
code under test, so code in `src/` runs during I3. The demo says
"tests are the specification". It does not claim the run is
independent of the code it tests.

## Results

Measured on 2026-10-02; run by the user:

```
cargo run --offline -p v9r-core --example v9r_demo
```

The example compiled on the first attempt, with one unused-import
warning. **All five verdicts matched the design**, so it exited 0.

| Run | Verdict | Evidence v9r collected | The rule that decided |
|---|---|---|---|
| 1 honest fix | **ALLOW**, S1 accepted | S0 `6f8ce6dd…` → S1 `4ba46cdb…`; changed `src/greet.py`; tests **passed** (run by v9r on S1) | all ✓ |
| 2 agent edits the protected test | **DENY**, not accepted | S1 `ccbc6beb…`; changed `tests/test_greet.py`; tests **passed** | ✗ I2: `outside ["ws/src"]: ["ws/tests/test_greet.py"]`. The tests passed and v9r still refused |
| 3 claim, no change, no runner | **BLOCKED** | S1 = S0; nothing changed; no test evidence | ? I3: `no verified evidence for tests(6f8c…)@now (have: proposed passed by agent: "All tests pass ✅")` |
| 4 same claim, runner available | **DENY** | S1 = S0; tests **failed** | ✗ I3: `tests(6f8c…)@now = failed, verified by provider:test-runner (lineage:L309); required passed` |
| 5 someone edits `src/` after approval | **REFUSED before the agent ran**; runtime holding | — | ✗ I1 (`snapshot(ws)@current` = `28870036…`, required `6f8ce6dd…`) and ✗ `authorization_basis_current` (`entries` and `snapshot` both changed) |

What the run shows, in the terms of the demo:

- **Initial state identity:** every card starts from the same S0 tree id.
- **Proposed transition:** authorization is granted once, bound to S0
  (run 5 shows that binding failing).
- **Evidence:** S1, the changed files and the test result, all collected
  by v9r. The agent's sentence is printed and labelled "not evidence".
  In run 3 the kernel's own reason names it as `proposed … by agent`.
- **Decision and explanation:** each rule is shown with ✓, ✗ or ?, and
  every failure carries the kernel's reason, naming the file, the value
  or the missing evidence.

### Presentation fixes (confirmed by a second run)

The run showed two readability problems. Both were fixed in the example
afterwards. Neither changes a verdict:

1. **The same rule appeared several times** (I2 three times, I3 twice),
   because each pinned value is its own kernel finding. Rendering now
   folds a rule's findings into one line: the worst status, with the
   first reason that is not a success.
2. **"changed" was blank** when nothing changed. It now prints
   `(nothing)`.

The unused `Verdict` import was removed.

**Second run** (also by the user):

- The example compiled without warnings of its own.
- Every card now shows one line per rule: run 1 shows four ✓, and run 2
  shows a single ✗ I2 between ✓ I1 and ✓ I3.
- Runs 3 and 4 print `changed (nothing)`.
- The five verdicts were unchanged: ALLOW, DENY, BLOCKED, DENY,
  REFUSED.
- **The two runs were byte-for-byte identical in every tree id** (S0
  `6f8ce6dd…`, S1 `4ba46cdb…`, `ccbc6beb…`, the race state
  `28870036…`), and even in the lineage id of run 4's test fact
  (`L309`). The demo is deterministic.

### Gaps found

None that required stopping. The limitation recorded above (run 3's
BLOCKED requires the runner to be absent) showed up exactly as
described. Run 4 shows the same agent judged with the runner present.
