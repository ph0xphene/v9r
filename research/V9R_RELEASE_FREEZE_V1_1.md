# v9r Release Freeze v1.1

*The final freeze record for `v9r-review-v1.1`, prepared 2026-10-02.
Everything below was measured on the release tree before commit. **The
release is not committed and not tagged.** Those are the only remaining
steps (§9).*

## 1. Release identity

| | |
|---|---|
| **release** | `v9r-review-v1.1` (to be tagged, annotated) |
| **frozen baseline** | `v9r-review-v1`: annotated tag object `a13e4784…4159` → commit `560d8daead6e32254bd64ba6dba7dfa007529698`. Code commit `6a9c717` |
| **branch** | `research/effect-runtime-v0`. The release tree is the working tree on top of `560d8da` |
| **what v1.1 is** | v1 plus the X1/X4 measurement tests, release documentation corrections, the demo wording correction, and the post-freeze research documents |
| **what v1.1 is not** | a change to the verifier. Library, kernel and manifests are byte-identical to v1 |
| **`cargo test --workspace`** | **88 passed, 0 failed, 4 ignored** |
| **kernel** | `crates/v9r-core/src/kernel.rs` sha256 `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`, unchanged since `bfebebe` |
| **claim** | *v9r verifies states, not histories.* Formal statement: V9R_VERIFICATION_BOUNDARY_V0 §1 |
| **release notes** | V9R_RELEASE_FREEZE_V1 §0 |
| **pre-tag audit** | V9R_RELEASE_AUDIT_V1_1 (D1–D3 resolved here, §7) |

## 2. Toolchain and host

Measured with `--version` on the host that ran every check:

| | |
|---|---|
| rustc | **1.98.1** (48a229cea 2026-09-01) |
| cargo | **1.98.0** (797e8a9bc 2026-08-05) |
| python3 | 3.12.13 (demo; X1/X4) |
| git | 2.51.2 |
| host | Linux 6.12.80, one machine |

The release documents now name these two versions consistently:

- V9R_RELEASE_FREEZE_V1 (header, §0, §5);
- README_REVIEWER;
- V9R_PHASE1_EXTERNAL_REVIEW_V0 §7;
- V9R_REVIEW_CHECKLIST_V1;
- V9R_SELF_REFERENTIAL_OBSERVERS_V0.

The freeze v1 header previously said "Rust 1.98.0", which is cargo's
version.

## 3. Final checks

All run `--offline` on the release tree.

| Check | Command | Result |
|---|---|---|
| full suite | `cargo test --workspace` | **88 passed, 0 failed, 4 ignored**, exit 0 |
| demo | `cargo run -p v9r-core --example v9r_demo` | exit 0; **ALLOW, DENY, BLOCKED, DENY, REFUSED** (also asserted by its test) |
| State vs Causality | `cargo test -p v9r-core --test state_vs_causality -- --nocapture` | `final workspace bytes identical: true`, `S0 identical: true`, `S1 identical: true`, `measurements identical: true`; 2 passed |
| kernel hash guard | `cargo test -p v9r-core --test kernel_guard`; `sha256sum …/kernel.rs` | 1 passed; `85badb66…6177f` |

Per suite: unit 19; `atomic_capture` 9 (+3 ignored);
`claim_guarantees` 28; `content_addressed` 10; `controlled_domain` 0
(+1 ignored); `counter` 8; `kernel_guard` 1; `scope_entries` 1;
**`self_referential_observers` 2**; `state_vs_causality` 2; demo 1;
doctests 7.

**The baseline, re-measured.** `v9r-review-v1` was checked out in a
separate, clean worktree with its own target directory, and its suite
run with the same toolchain: **86 passed, 0 failed, 4 ignored**, exit 0.
The two releases differ by exactly the two X1/X4 tests. The worktree
was removed afterwards.

## 4. Verification of scope

| Requirement | Check | Result |
|---|---|---|
| `v9r-review-v1` unchanged | `git cat-file -t v9r-review-v1`; `git rev-parse v9r-review-v1 v9r-review-v1^{commit} HEAD` | annotated tag `a13e4784…`, commit `560d8da` = `HEAD`; no commit after it; no other `v9r-review-v1*` tag |
| no `src/` changes | `git diff --stat v9r-review-v1 -- crates/v9r-core/src` | empty |
| no manifest or lock changes | `git diff --stat v9r-review-v1 -- Cargo.toml Cargo.lock crates/v9r-core/Cargo.toml` | empty |
| no `kernel.rs` changes | `git diff --quiet v9r-review-v1 -- crates/v9r-core/src/kernel.rs`; sha256 of tree and tag | no diff; identical hashes |
| only intended files differ | §5 | yes |

## 5. Exact change set relative to `v9r-review-v1`

### 5.1 Modified (7 files, `git diff --numstat v9r-review-v1`)

| File | + | − | Change |
|---|---|---|---|
| `README.md` | 17 | 9 | release documentation correction |
| `README_REVIEWER.md` | 60 | 21 | release documentation correction; v1.1 header |
| `crates/v9r-core/examples/v9r_demo.rs` | 16 | 9 | **demo wording correction**: printed strings and doc comment only |
| `research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md` | 33 | 10 | release documentation correction |
| `research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md` | 37 | 16 | release documentation correction |
| `research/V9R_RELEASE_FREEZE_V1.md` | 50 | 12 | §0 release notes for v1.1; guarantee wording; toolchain |
| `research/V9R_REVIEW_CHECKLIST_V1.md` | 17 | 6 | release documentation correction |

### 5.2 Added (11 files)

**Tests (1):**

- `crates/v9r-core/tests/self_referential_observers.rs`, 485 lines:
  the X1/X4 measurement tests.

**Research documents (10):**

| File | Lines |
|---|---|
| `V9R_SELF_REFERENTIAL_OBSERVERS_V0.md` | 252 |
| `V9R_ADVERSARIAL_REVIEW_V0.md` | 331 |
| `V9R_CLAIM_REVISION_V0.md` | 332 |
| `V9R_OBSERVER_INDEPENDENCE_MODEL_V0.md` | 377 |
| `V9R_OBSERVER_CAPABILITY_BOUNDARY_V0.md` | 324 |
| `V9R_VERIFICATION_BOUNDARY_V0.md` | 354 |
| `V9R_RESEARCH_THESIS_V0.md` | 386 |
| `V9R_RELEASE_AUDIT_V0.md` | 319 |
| `V9R_RELEASE_AUDIT_V1_1.md` | 323 |
| `V9R_RELEASE_FREEZE_V1_1.md` | this document |

### 5.3 Unchanged

- every file under `crates/v9r-core/src/`, including `kernel.rs`;
- `Cargo.toml`, `Cargo.lock`, `crates/v9r-core/Cargo.toml`;
- every other test file;
- every other research document;
- the `v9r-review-v0`, `v9r-archive-v0` and `v9r-review-v1` tags.

## 6. Demo wording correction

The v1 demo printed four labels that tests in the same release
contradict:

| v1 label | v1.1 label | Why |
|---|---|---|
| I3 "tests pass on the result, run by v9r"; "(run by v9r on S1)"; policy "required: tests pass on the result" | "protected test exits 0 on S1 (runs S1's code)"; "(exit status of the protected test on a checkout of S1; S1's code runs inside it)"; "required: the protected test command exits 0 on the result" | **independent test execution.** X1 measured that S1's code decides the reported exit status |
| run 1 "honest fix" | "the agent edits src/greet.py" | **honest agent behaviour.** G11: v9r cannot judge the agent; X1: a dishonest fix gets the same ALLOW |
| "evidence collected by v9r"; "Every decision came from what v9r observed." | "attested by registered observers, at v9r's request"; "…what v9r's registered observers reported to v9r's own requests. Observers are believed, not checked, and the test observer runs the code it judges: I3 shows the exit status, not that greet() is correct." | **observer truth.** The liar and caching tests: observers are believed. X1: one is controlled by S1 |

The agents, rules, observers, verdicts and assertions are unchanged.
The demo still uses a self-referential test observer. v1.1 labels that
limit; it does not remove it.

## 7. Pre-tag audit findings, resolved

| V9R_RELEASE_AUDIT_V1_1 | Resolution |
|---|---|
| **D1**: release notes said v1.1 adds "the X1/X4 measurements only" | V9R_RELEASE_FREEZE_V1 §0 and README_REVIEWER now list the four components separately: X1/X4 measurement tests; release documentation corrections; demo wording correction; unchanged verifier implementation and kernel. The added research documents are listed by name |
| **D2**: "Rust 1.98.0" vs "rustc 1.98.1" | every release document names rustc 1.98.1 and cargo 1.98.0 (§2) |
| **D3**: `git diff v9r-review-v1 v9r-review-v1.1 …` needs the tag | resolved by tagging. Before then, the same check on the release tree is §4 (empty) |

All High and Medium findings of V9R_RELEASE_AUDIT_V0 were already fixed
(V9R_RELEASE_AUDIT_V1_1 §9). Open by decision: L5 (the historical demo
design document).

## 8. Claim and limitations at this release

**Claim.** V9R_VERIFICATION_BOUNDARY_V0 §1.2, unchanged by v1.1:

- v9r accepts the S1 captured when the actor's call returns, only if
  every declared rule is satisfied by attestations its registered
  observers gave to its own requests;
- claims never count, and missing or contradictory attestations block;
- the decision does not depend on who wrote S1;
- an accepted S1 has the meant property only if every contributing
  observer is faithful, complete, current and independent of S1, and
  each rule's key is adequate.

**Limitations.** V9R_RELEASE_AUDIT_V1_1 §8. The five kept false ALLOWs
are passing tests:

- the lying observer;
- the caching observer;
- the omitted file;
- X1;
- X4.

Still predictions, not measured: X2, X3, X5, G′.

## 9. Remaining steps (not done)

1. Commit the change set of §5, and nothing else. Check with
   `git status` that no other path is staged.
2. Create the annotated tag `v9r-review-v1.1` on that commit.
3. After tagging, re-run:
   - `git diff v9r-review-v1 v9r-review-v1.1 -- crates/v9r-core/src Cargo.toml Cargo.lock crates/v9r-core/Cargo.toml`
     (expect empty);
   - `git rev-parse v9r-review-v1^{commit}` (expect `560d8da`).

**Stopped here: no commit, no tag.**
