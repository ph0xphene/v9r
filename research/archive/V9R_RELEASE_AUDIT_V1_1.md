# v9r Release Audit v1.1

*The pre-tag audit of `v9r-review-v1.1`, run on 2026-10-02 against the
working tree on top of `v9r-review-v1`. No code was changed for it,
nothing was committed, and no tag was created. It records what was run,
what differs from v1, and what a v1.1 tag would contain.*

## 1. Environment

| | |
|---|---|
| host | Linux 6.12.80, one machine (as for every v9r measurement) |
| rustc | 1.98.1 (48a229cea 2026-09-01) |
| cargo | 1.98.0 (797e8a9bc 2026-08-05) |
| python3 | 3.12.13 |
| git | 2.51.2 |
| branch | `research/effect-runtime-v0`, `HEAD` = `560d8da` |

## 2. Release audit runs

All commands were run with `--offline`, on the working tree described
in §4.

### 2.1 `cargo test --workspace`

| Suite | Passed | Failed | Ignored |
|---|---|---|---|
| unit, `v9r-core` (`src/lib.rs`) | 19 | 0 | 0 |
| `atomic_capture` | 9 | 0 | 3 |
| `claim_guarantees` | 28 | 0 | 0 |
| `content_addressed` | 10 | 0 | 0 |
| `controlled_domain` | 0 | 0 | 1 |
| `counter` | 8 | 0 | 0 |
| `kernel_guard` | 1 | 0 | 0 |
| `scope_entries` | 1 | 0 | 0 |
| **`self_referential_observers`** (new in v1.1) | **2** | 0 | 0 |
| `state_vs_causality` | 2 | 0 | 0 |
| demo (`examples/v9r_demo.rs`) | 1 | 0 | 0 |
| doctests | 7 | 0 | 0 |
| **total** | **88** | **0** | **4** |

`v9r-review-v1` gave 86 / 0 / 4. The difference is exactly the two
tests of `self_referential_observers`: X1 and X4. The 4 ignored tests
are the same as in v1: three in `atomic_capture`, one in
`controlled_domain`.

### 2.2 Demo

```
cargo run --offline -p v9r-core --example v9r_demo
```

- Exit 0. Verdicts **ALLOW, DENY, BLOCKED, DENY, REFUSED**, as in v1.
  They are also asserted by the demo's test (§2.1).
- Run twice: the outputs are identical apart from lineage numbers.

The summary lines and closing text as printed:

```
  ALLOW    the agent edits src/greet.py
  DENY     the agent edits the protected test
  BLOCKED  the agent claims success and changes nothing
  DENY     same claim, test runner available
  REFUSED  someone else changes the code after approval

The agent's words never counted. Every decision came from what v9r's registered observers
reported to v9r's own requests. Observers are believed, not checked, and the test
observer runs the code it judges: I3 shows the exit status, not that greet() is correct.
```

### 2.3 State vs Causality

```
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
```

- `final workspace bytes identical: true`, `S0 identical: true`,
  `S1 identical: true`, `measurements identical: true`.
- `identical_s1_gets_identical_decisions_whoever_wrote_it ... ok`.
- 2 passed. Unchanged from v1.

### 2.4 Kernel hash guard

```
cargo test --offline -p v9r-core --test kernel_guard     → 1 passed
sha256sum crates/v9r-core/src/kernel.rs
git show v9r-review-v1:crates/v9r-core/src/kernel.rs | sha256sum
```

Both give `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`.
The kernel is byte-identical to `v9r-review-v1`, and therefore to
`v9r-review-v0` and to `bfebebe`.

## 3. `v9r-review-v1` is unchanged

| Check | Result |
|---|---|
| `git cat-file -t v9r-review-v1` | `tag` (annotated), object `a13e4784…4159` |
| `git rev-parse v9r-review-v1^{commit}` | `560d8daead6e32254bd64ba6dba7dfa007529698` = `HEAD` |
| commits after the tag | none |
| tags matching `v9r-review-v1*` | only `v9r-review-v1`. **No v1.1 tag exists** |

Every v1.1 change is uncommitted in the working tree. The tag and the
commit it names are untouched.

## 4. Exact diff from `v9r-review-v1`

### 4.1 Library, kernel, manifests: no change

```
git diff --stat v9r-review-v1 -- crates/v9r-core/src Cargo.toml Cargo.lock crates/v9r-core/Cargo.toml
(empty)
```

### 4.2 Modified tracked files (7)

```
git diff --stat v9r-review-v1
 README.md                                  | 26 ++++++----
 README_REVIEWER.md                         | 80 ++++++++++++++++++++++--------
 crates/v9r-core/examples/v9r_demo.rs       | 25 ++++++----
 research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md | 43 ++++++++++++----
 research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md  | 51 +++++++++++++------
 research/V9R_RELEASE_FREEZE_V1.md          | 53 ++++++++++++++++----
 research/V9R_REVIEW_CHECKLIST_V1.md        | 21 ++++++--
 7 files changed, 221 insertions(+), 78 deletions(-)
```

| File | + / − | Nature |
|---|---|---|
| `README.md` | 17 / 9 | wording (audit H6, H7) |
| `README_REVIEWER.md` | 59 / 21 | v1.1 header, wording, reading order (X1/X4 corrections; audit M3, L4) |
| `crates/v9r-core/examples/v9r_demo.rs` | 16 / 9 | **printed strings and doc comment only** (§6); no logic, no verdict changed |
| `research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md` | 33 / 10 | status note; §1, §4, §5.3, §6 wording (X1/X4; audit M1, M2) |
| `research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md` | 36 / 15 | status note; §1, G1, G3, G4, G6, G8, G9, §4, §6, §8 Q1 (audit H8, M1–M3) |
| `research/V9R_RELEASE_FREEZE_V1.md` | 44 / 9 | §0 v1.1 release notes; G1, G3, G4, G6, G8, G9, §7–§9 wording |
| `research/V9R_REVIEW_CHECKLIST_V1.md` | 16 / 5 | central statement, claim table, falsification rows, checkout tag (audit H9, H10) |

The demo diff, every changed line:

```
- //! … four scripted agents, … from evidence it collects itself. …
+ //! … five scripted runs, … from what its registered observers attest to its own requests. …
+ //! The test observer runs the protected test on a checkout of S1, so S1's code runs inside it:
+ //! its report is not independent evidence about S1 (…V9R_SELF_REFERENTIAL_OBSERVERS_V0.md, X1).
- POLICY  "… required: tests pass on the result"
+ POLICY  "… required: the protected test command exits 0 on the result"
- "I3" => "tests pass on the result, run by v9r",
+ "I3" => "protected test exits 0 on S1 (runs S1's code)",
- "  evidence collected by v9r:"
+ "  attested by registered observers, at v9r's request:"
- "{t} (run by v9r on S1)"
+ "{t} (exit status of the protected test on a checkout of S1; S1's code runs inside it)"
- title: "honest fix",
+ title: "the agent edits src/greet.py",
- "The agent's words never counted. Every decision came from what v9r observed."
+ "The agent's words never counted. Every decision came from what v9r's registered observers
+  reported to v9r's own requests. Observers are believed, not checked, and the test
+  observer runs the code it judges: I3 shows the exit status, not that greet() is correct."
```

### 4.3 New files (10, untracked)

| File | Lines | Kind |
|---|---|---|
| `crates/v9r-core/tests/self_referential_observers.rs` | 485 | **test** (X1, X4) |
| `research/V9R_SELF_REFERENTIAL_OBSERVERS_V0.md` | 252 | measurement report |
| `research/V9R_ADVERSARIAL_REVIEW_V0.md` | 331 | predictions, with measured-status notes |
| `research/V9R_CLAIM_REVISION_V0.md` | 332 | proposed wording; X1/X4 labels measured |
| `research/V9R_OBSERVER_INDEPENDENCE_MODEL_V0.md` | 377 | model |
| `research/V9R_OBSERVER_CAPABILITY_BOUNDARY_V0.md` | 324 | model |
| `research/V9R_VERIFICATION_BOUNDARY_V0.md` | 354 | model: formal claim |
| `research/V9R_RESEARCH_THESIS_V0.md` | 386 | thesis |
| `research/V9R_RELEASE_AUDIT_V0.md` | 319 | the audit these corrections implement |
| `research/V9R_RELEASE_AUDIT_V1_1.md` | — | this document |

**Only intended files differ.** Every modified file is a document, or
the demo's strings. Every new file is the X1/X4 test or a research
document. No file under `crates/v9r-core/src/` and no manifest or lock
file differs.

## 5. Do the README and release documents describe v1.1 correctly?

**Yes, with three discrepancies.** They are recorded here and not fixed,
because this task stops at the audit.

**Correct:**

- `README_REVIEWER.md` names v1.1 as the release under review, v1 as
  the frozen baseline, 88 / 0 / 4 against v1's 86 / 0 / 4, and the
  empty `src`/manifest diff.
- `V9R_RELEASE_FREEZE_V1.md` §0 says the same. §1–§9 are marked as
  describing v1, and their v1 counts (86) are correct for v1.
- V9R_PHASE1_EXTERNAL_REVIEW_V0 and V9R_REVIEW_CHECKLIST_V1 carry v1.1
  notes. The checklist's checkout line names `v9r-review-v1.1` "once
  tagged".
- No release-facing document still asserts "honest fix", "run by v9r",
  "Every decision came from what v9r observed", "file modes" or "exact
  identities" as a property of v9r. Remaining occurrences are quotations
  of the old wording, or rows naming it as a falsified assumption.

| # | Discrepancy | Where | Effect |
|---|---|---|---|
| D1 | §0 says v1.1 adds "the X1/X4 measurements **only**". The commit will also add eight research documents (§4.3), and lists three of them under "what v1.1 corrects", although relative to v1 they are new | `V9R_RELEASE_FREEZE_V1.md` §0, rows "what v1.1 adds" / "corrects" | understated delta. The accurate statement: v1.1 adds one test and research documents; it changes documents and demo strings; it changes no library code |
| D2 | the freeze header says "Rust 1.98.0"; §0 says "rustc 1.98.1". On this host rustc is 1.98.1 and cargo 1.98.0 | `V9R_RELEASE_FREEZE_V1.md` l. 5 and §0 | cosmetic; both runs succeed |
| D3 | the release notes cite `git diff v9r-review-v1 v9r-review-v1.1 -- …`, which needs the tag. Until it exists, the equivalent is `git diff v9r-review-v1 -- crates/v9r-core/src Cargo.toml Cargo.lock` on the release tree (empty, §4.1) | `README_REVIEWER.md` header; `V9R_RELEASE_FREEZE_V1.md` §0 | none after tagging |

**Not changed, by decision:**

- `V9R_DEMO_DESIGN_V0.md` still contains the original design wording
  ("run by v9r … and passed"; audit L5). It is a historical design
  document.
- The X1 test prints its control as `"honest fix (control)"`. There
  honesty is defined by the experiment's independent check, not judged
  by v9r.

## 6. Why the demo wording changed

The demo is the first thing a reviewer runs. In v1 four of its labels
were contradicted by tests that ship in the same release:

| v1 label | Contradicted by | Measured |
|---|---|---|
| I3 "tests pass on the result, run by v9r"; test line "(run by v9r on S1)"; policy "required: tests pass on the result" | X1. The test runner executes S1's code, so S1 decides the reported exit status. "Run by v9r" read as "independent of the agent"; it is not | `x1_judged_program_decides_the_test_runners_report`: Allow with `greet` absent; the same module without `sys.exit(0)` is denied |
| run 1 "honest fix" | G11. v9r cannot judge the agent: an unauthorized writer of the same bytes gets the same decision. And X1: the same ALLOW is given to a dishonest fix | `identical_s1_gets_identical_decisions_whoever_wrote_it`; X1 |
| "Every decision came from what v9r observed." | v9r observes nothing itself. It asks registered observers, believes their bound answers (liar, caching tests), and one of them is controlled by S1 (X1) | `a_trusted_liar_is_believed_alone_…`, `stale_…_attested_freshly_is_believed`, X1 |
| "evidence collected by v9r" | the same | the same |

**What changed:** printed strings and the doc comment only (§4.2). The
five verdicts, the scripted agents, the rules, the observers and the
assertions are identical. The new labels say what each line
establishes: an exit status, from an observer that runs S1's code, on
v9r's request.

**What it does not change:** the demo still uses a self-referential
test observer. v1.1 labels the limit; it does not remove it.

## 7. Final claim

As stated in V9R_VERIFICATION_BOUNDARY_V0 §1.2–§1.3, unchanged by
v1.1:

> **For one runtime process, a host-approved S0, rules R and the
> registry's observers:**
>
> 1. execution starts only if every PRE obligation is satisfied on a
>    fresh observation at the start of execution, and an authorization
>    is used at most once;
> 2. S1, captured when the actor's call returns, is accepted only if
>    every POST obligation is satisfied by attestations the registered
>    observers gave in reply to v9r's own requests;
> 3. claims never satisfy or violate an obligation;
> 4. missing or contradictory attestations, or unknown names possibly
>    outside a scope, give Blocked, never Allow;
> 5. the decision is a function of (S0, S1, R, attestations, window
>    order), the same up to counter values whoever wrote S1;
> 6. after a refusal for drift or a rejected transition, that runtime
>    process authorizes nothing further;
> 7. S0 and S1 are identified by their git tree ids (names, content,
>    symlink targets, owner-execute bit). A claimed id is recomputed;
>    an unreadable entry or a lost or altered object gives Blocked.
>
> An accepted S1 has the property a rule means only if every
> contributing observer is faithful, complete, current and independent
> of S1, and the rule's key is adequate for that property. It has it
> only as captured at the after-snapshot.

**v9r verifies states, not histories.**

## 8. Known limitations

Each is either a passing test that demonstrates it, or labelled.

| Limitation | Status |
|---|---|
| a registered observer is believed; a lone liar gives a false Allow | measured (`a_trusted_liar_…`) |
| a caching observer is believed | measured (`stale_…_attested_freshly_…`) |
| an omitted file is invisible without a second observer | measured (`hidden_file_…`) |
| an observer that executes S1 is controlled by S1 | measured (X1) |
| changes after the after-snapshot, during the decision, are not judged | measured (X4, 40/40) |
| the writer of S1 is not an input to the decision | measured (G11); by design |
| program correctness and test adequacy are not established | follows from X1 |
| capture is trusted against a same-uid writer | measured (Atomic Capture v0) |
| the hold and the approved S0 are in memory only | code (`temporal.rs:526`); restart laundering (X5) predicted |
| identity covers names, content, symlink targets, the owner-execute bit only | code (`fs_raw.rs:73`); metadata attack (X3) predicted |
| write after `execute` returns (X2); the observer's execution writing the live state (G′) | predicted, not measured |
| no containment, no rollback | by design |
| explanations parsed from kernel text; a contradiction names no provenance | code |
| frozen-kernel residue (`Semantic`, `Soft`, `AtMost` without producer; a vacuous `compile_fail` example; a stale doc comment) | code |
| research code, one Linux host | stated |

## 9. Status of V9R_RELEASE_AUDIT_V0 findings

| Finding | Status in this tree |
|---|---|
| H1–H5: demo labels | **fixed** (§6) |
| H6, H7: `README.md` | **fixed** |
| H8: Phase 1 external review | **fixed**; still first in the reading order, now corrected |
| H9, H10: review checklist | **fixed** |
| M1–M3: G3, G4, G6 | **fixed** in the freeze, the Phase 1 review and the v0 package |
| M4: claim revision "complete" | **fixed** |
| L1: thesis stale-evidence row | **fixed** |
| L2: adversarial review status | **fixed** (status notes) |
| L3: rustc version | **open** (D2) |
| L4: test count in README_REVIEWER | **fixed** |
| L5: demo design document | **not changed**, by decision (§5) |
| L6: Verification Boundary §7.1 | unchanged; classified as future ideas |
| §1 qualification: clause 5 with a self-referential observer; X4's decision equality by inspection | **open**, accepted as limits of the result |

## 10. Readiness

**Ready to commit and tag, subject to D1.**

- Every audit run passed: 88 / 0 / 4, demo, State vs Causality, kernel
  guard.
- The library and kernel are unchanged.
- The v1 tag is intact.
- Every High and Medium finding of V9R_RELEASE_AUDIT_V0 is fixed.

D1 should be corrected before tagging, because the release notes
understate what v1.1 adds. D2 and D3 do not block.

**Not done, as instructed:** no commit, no tag.
