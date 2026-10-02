# How to reproduce and review v9r-review-v1.1

*Based on the freeze record
[V9R_RELEASE_FREEZE_V1_1](archive/V9R_RELEASE_FREEZE_V1_1.md), which
was written before the tag was created. Every expected result below is
the one measured there, on one Linux host.*

## 1. Release identity

| | |
|---|---|
| **release** | `v9r-review-v1.1`: annotated tag `9a1859f2…` → commit `cf967d963fffd28070f88ab6d0ef73ef853b60ec` |
| **baseline** | `v9r-review-v1`: annotated tag `a13e4784…` → commit `560d8daead6e32254bd64ba6dba7dfa007529698`; code commit `6a9c717` |
| **what v1.1 adds** | the X1/X4 measurement tests (`crates/v9r-core/tests/self_referential_observers.rs`); release documentation corrections; the demo wording correction (`examples/v9r_demo.rs`, printed strings only); the post-freeze research documents |
| **what v1.1 does not change** | the verifier. Library, kernel and manifests are byte-identical to v1 |
| **kernel** | `crates/v9r-core/src/kernel.rs` sha256 `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`, unchanged since `v9r-review-v0` |
| **claim** | *v9r verifies states, not histories.* Formal statement: [verification-boundary](verification-boundary.md) §1 |
| **GitHub release** | <https://github.com/ph0xphene/v9r/releases/tag/v9r-review-v1.1> |

The research documents in this directory were reorganized after the
tag. To review exactly what was released, check out the tag:

```sh
git checkout v9r-review-v1.1
```

## 2. Toolchain

| | |
|---|---|
| rustc | 1.98.1 |
| cargo | 1.98.0 |
| python3 | 3.12.13 (demo; X1/X4) |
| host | Linux 6.12.80, one machine |

## 3. Reproduce

All commands run from the repository root, `--offline`.

| Check | Command | Expected |
|---|---|---|
| full suite | `cargo test --offline --workspace` | **88 passed, 0 failed, 4 ignored** |
| demo | `cargo run --offline -p v9r-core --example v9r_demo` | **ALLOW, DENY, BLOCKED, DENY, REFUSED** (also asserted by its test) |
| state vs causality | `cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture` | `S0 identical: true`, `S1 identical: true`, `measurements identical: true`; 2 passed |
| X1 / X4 | `cargo test --offline -p v9r-core --test self_referential_observers -- --nocapture --test-threads=1` | X1 Allow with `greet` absent; X4 40/40 runs accepted a stale S1 (4 runs × 10); 2 passed |
| kernel hash | `cargo test --offline -p v9r-core --test kernel_guard`; `sha256sum crates/v9r-core/src/kernel.rs` | 1 passed; `85badb66…6177f` |

Per suite: unit 19; `atomic_capture` 9 (+3 ignored);
`claim_guarantees` 28; `content_addressed` 10; `controlled_domain` 0
(+1 ignored); `counter` 8; `kernel_guard` 1; `scope_entries` 1;
`self_referential_observers` 2; `state_vs_causality` 2; demo 1;
doctests 7.

The baseline `v9r-review-v1`, run with the same toolchain, gives
86 passed, 0 failed, 4 ignored. The difference is exactly the X1 and X4
tests.

## 4. Verify that the verifier did not change

```sh
git diff v9r-review-v1 v9r-review-v1.1 -- crates/v9r-core/src Cargo.toml Cargo.lock crates/v9r-core/Cargo.toml
# expected: empty
git rev-parse v9r-review-v1^{commit}
# expected: 560d8daead6e32254bd64ba6dba7dfa007529698
```

These need the `v9r-review-v1` tag in your clone.

## 5. Archived experiments

Experiments on the pre-debloat architecture (capability manifest,
content identity of grants, snapshot boundary, controlled domain, ...)
are reproducible only at the archive tag, as measured:

```sh
git worktree add ../v9r-archive v9r-archive-v0     # = v9r-review-v0, commit da6d694
cd ../v9r-archive && cargo test --offline -p v9r-core --test <name> -- --nocapture
```

See [archive-boundary](archive-boundary.md) for what was moved and why.

## 6. What to review

- The claim and its conditions: [verification-boundary](verification-boundary.md).
- Where it breaks, measured: [observer-boundary](observer-boundary.md),
  [causality](causality.md).
- The kept false Allows, each a passing test: the lying observer, the
  caching observer, the omitted file, X1, X4. Still predictions, not
  measured: X2, X3, X5, G′.
- The trusted computing base, known limitations and a code reading
  order: [README_REVIEWER.md](../README_REVIEWER.md).
- The reviewer's checklist for this release:
  [V9R_REVIEW_CHECKLIST_V1](archive/V9R_REVIEW_CHECKLIST_V1.md).
