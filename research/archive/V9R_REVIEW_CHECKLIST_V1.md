# v9r Review Checklist v1

*For the reviewer of the Phase 1 release candidate. V9R_REVIEW_FREEZE_CHECKLIST_V0
was the maintainers' freeze list for `v9r-review-v0`; this one is the
reviewer's. Each item says how to check it and what you should see.
Expected values were measured on 2026-10-02.*

## The invariant this checklist relies on

> **The demo, the State vs Causality test and the Self-Referential
> Observers test are enough to understand the central claim and its
> boundary.**
>
> *(Corrected for `v9r-review-v1.1`: the demo and State vs Causality
> alone do not show that the demo's test rule is controlled by the code
> it judges, or that the decision covers S1 only as captured. X1 and X4
> show both. V9R_RELEASE_AUDIT_V0, H9–H10.)*

The two artifacts split the claim between them:

| Part of the claim | Demo shows it | State vs Causality shows it |
|---|---|---|
| a transition is accepted only if registered observers' attestations, given to v9r's own requests, satisfy the rules | run 1 (ALLOW): every rule satisfied, each fact named with its observer | World A: 6 findings satisfied |
| an attestation is as true as its observer; an observer that runs S1 is controlled by S1 | — (run 1's I3 is such an observer) | — ; `self_referential_observers` X1: Allow with `greet` absent |
| the decision covers S1 as captured when the actor's call returns | — | — ; `self_referential_observers` X4: stale S1 accepted 40/40 |
| rules bind the result, not the agent's account | run 2 (tests pass, but the test was edited → DENY); run 3 (claim only → BLOCKED); run 4 (claim, tests fail → DENY) | — |
| authorization is fresh and single-use | run 5 (state changed after approval → REFUSED, runtime holds) | PRE re-checked on a fresh snapshot (`@current`) |
| **states, not histories** | — | World B: an unauthorized writer of identical bytes gets **identical** complete decisions; World C (different bytes) does not |

Everything else is supporting evidence:

- `claim_guarantees` (28 negative cases);
- `scope_entries` (18 scope cases);
- `content_addressed` (identity);
- `kernel_guard`.

If reading the demo, `state_vs_causality.rs` and
`self_referential_observers.rs` leaves the claim
unclear, the claim is badly stated. Report that as a finding.

## Before you start

```sh
git checkout v9r-review-v1.1      # once tagged; until then the release-candidate tree
cargo fetch                       # once, with network; afterwards everything runs --offline
```

You need:

- Rust 1.98 (measured with rustc 1.98.1, cargo 1.98.0);
- `python3` (the demo's test runner);
- `git` (one identity test).

Linux. The project was measured on one Linux host.

## Checklist

### □ 1. Can reproduce the demo from a clean checkout

```sh
cargo run --offline -p v9r-core --example v9r_demo
```

**Expect:** the summary `ALLOW`, `DENY`, `BLOCKED`, `DENY`, `REFUSED`,
and exit code 0.

- In a fresh copy with no `target/`, building and running took 8.5 s.
- `cargo test --offline -p v9r-core --example v9r_demo` asserts the same
  five verdicts.
- Run it twice: the output is identical apart from lineage numbers.

**Look for:** whether each verdict follows from the rules printed under
it, and whether "agent says" ever influences a verdict. It should not.

### □ 2. Can run the State vs Causality experiment

```sh
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
```

**Expect:** 2 passed.

- `state_vs_causality` prints World A and World B. Then it prints
  `final workspace bytes identical: true`, `S0 identical: true`,
  `S1 identical: true` and `measurements identical: true`, with every
  field `same`. S0 is `b69da1dd…1497`; S1 is `8348b34c…fd5e`.
- `identical_s1_gets_identical_decisions_whoever_wrote_it` compares the
  complete serialized PRE and POST decisions and the journal. Only
  counters are replaced by their roles: snapshot ids, the effect's clock
  reading, lineage ids.

**Look for:**

- Is the normalization (`normalize` in the test) hiding anything other
  than counters?
- The test asserts that the raw decisions differ, and that World C
  (different bytes) gets a different POST decision. Do those assertions
  convince you?

### □ 3. Can identify the trusted components

**Read:**

- `README_REVIEWER.md` "Trusted computing base";
- V9R_PHASE1_EXTERNAL_REVIEW_V0 §4.

**You should be able to name, without looking:**

- the kernel;
- the runtime and temporal layers;
- the registry's binding;
- `FsSnapshot` and `fs_raw` together with the OS;
- every registered observer;
- the rules and whoever holds the registry;
- capture.

**And what is not trusted:**

- the actor;
- stored object bytes;
- unbound answers.

**Check one claim of the table yourself.** For example, delete
`registry.register(...)` for the test runner in the demo and confirm
run 1 becomes BLOCKED, not ALLOW.

### □ 4. Can identify the false assumptions

Each assumption below was tested and turned out false. The tests that
demonstrate them still pass:

| Assumption | Why it is false | Test |
|---|---|---|
| "a verified fact is true" | a registered liar is believed | `claim_guarantees::a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness` |
| "binding attestations to requests makes them current" | a caching observer re-attests stale state freshly: false ALLOW | `stale_raw_content_attested_freshly_is_believed`, `stale_state_attested_freshly_is_believed_unless_independently_witnessed` |
| "one observer sees everything it lists" | an omitted *file* goes unnoticed without a second observer | `content_addressed::hidden_file_omitted_by_the_observer_blocks_only_with_an_independent_observer` |
| "an explanation that checks out is correct" | provenance explains, it does not verify (Evidence Provenance v0) | `claimed_state_is_shown_as_a_claim_beside_the_established_round` |
| "a PRE fact may name the snapshot it was derived on" | PRE is re-checked on a fresh snapshot; it must name `@current` | STATE_VS_CAUSALITY_V0, failed assumption 1 |
| "lineage or snapshot ids distinguish who wrote a state" | they are counters | STATE_VS_CAUSALITY_V0, failed assumption 2 |
| "scope entries without modes are enough" | `chmod +x` outside the scope was allowed by the old source | `scope_entries` header (Debloat Phase 1) |
| "a test run by v9r on S1 is independent evidence about S1" | S1's code decides the test's exit status: false ALLOW, `greet` absent | `self_referential_observers::x1_judged_program_decides_the_test_runners_report` |
| "the accepted S1 is the state when the transition completes" | a write after the after-snapshot, during the decision, is not judged: 40/40 | `self_referential_observers::x4_watched_state_changes_after_observation_before_completion` |

**Look for:** assumptions we still make without knowing they are false.
External Review §8 lists ten questions.

### □ 5. Can reproduce the kernel hash guard

```sh
cargo test --offline -p v9r-core --test kernel_guard
sha256sum crates/v9r-core/src/kernel.rs
git diff v9r-review-v0 -- crates/v9r-core/src/kernel.rs
```

**Expect:**

- 1 passed;
- `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`;
- an empty diff.

The guard lives outside `kernel.rs`, so it cannot change the hash it
checks. It also asserts the kernel never mentions delegation, ledgers or
grants.

**Look for:** whether "frozen" is still worth it. The kernel carries
features nothing produces any more, plus one vacuous `compile_fail`
example (README_REVIEWER, limitations).

### □ 6. Can understand why v9r does not prove causality

**Read** STATE_VS_CAUSALITY_V0, "Answers" and "Decision".

**You should be able to explain in two sentences:**

1. A decision is a function of S0, S1, the rules, the observers' answers
   and the ordering of the window. None of these records which process
   wrote a byte, so an unauthorized writer of identical S1 inside the
   window gets the same decision (item 2).
2. Telling the two apart would need evidence that binds the change to its
   writer. That evidence would come from an isolating executor, which v9r
   could check as an ordinary `Fact` but does not have.

**Try:** write a rule over the existing observers that distinguishes
World A from World B (External Review §8, question 6). If you succeed,
v9r observes something about histories, and the claim is wrong.

## Recording the review

For each item, record:

- passed, or failed with the output;
- anything that surprised you.

Findings against External Review §8 are the most useful.
