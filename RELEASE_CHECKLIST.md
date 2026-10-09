# Transactional-runtime research release candidate

Proposed tag: `v0.1.0-transactional-rc.1` (not created).

This is a source release candidate, not a stable or production-safe release. It is separate from the existing public observer-boundary release `v9r-review-v1.1`.

Before tagging:

- [ ] Reconcile local `main` (origin: `merelinmrelin-web/v9r`) with the public `ph0xphene/v9r` default branch `research/effect-runtime-v0`. Do not overwrite either lineage.
- [ ] Review and push the intended branch with explicit approval.
- [ ] Obtain green Linux/macOS hosted CI for that exact commit.
- [ ] Run the model-free example from a fresh clone: `cargo run --locked --manifest-path examples/task-runtime/Cargo.toml`.
- [ ] Verify workspace format, tests, and strict clippy.
- [ ] Keep `SUPPORT.md`, `CLAIMS.md`, and the demo limitations attached to the release.
- [ ] Exclude tracked build outputs from the release source package in a separately reviewed cleanup; this repository currently tracks example `target/` files.
- [ ] Review changelog and tag the exact verified commit only after approval.

Release notes should state the source lineage, reproducibility commands, OS/toolchain, local versus hosted evidence, and lack of general subprocess containment. Do not claim benchmarks, user adoption, or independent security review.
