# v9r

**A state transition verifier.**

v9r separates proposing a change from accepting the state that results
from it. An actor (for example an LLM agent) proposes a change. v9r
authorizes it once, against an approved starting state S0 identified by
its content. When the actor's call returns, v9r captures the resulting
state S1 and accepts it only if every declared rule is satisfied by
attestations that its registered observers gave to v9r's own requests.

Every decision is **Allow**, **Deny** or **Blocked**, with a reason per
rule naming the observer behind each fact. Missing or contradictory
evidence blocks; it never allows. The actor's own statements are claims
and never satisfy a rule.

This is research code. Reviewers: start with
[README_REVIEWER.md](README_REVIEWER.md).

## What v9r verifies

- **States against declared rules.** S1, as captured when the actor's
  call returns, is accepted only if each declared rule is satisfied.
- **That each answer belongs to its request.** An attestation is bound
  to the observer and the request that produced it.
- **State identity.** S0 and S1 are identified by a git-compatible
  SHA-256 tree id; a claimed id is recomputed, and an omitted directory,
  unreadable file or altered object blocks.
- **Authorization against the approved state.** The authorization is
  re-checked on a fresh observation immediately before the actor runs; a
  change to S0 after approval is refused.

The formal statement of the claim and its conditions is
[research/verification-boundary.md](research/verification-boundary.md).

## What v9r does not verify

- **Who produced the state.** v9r verifies states, not histories. An
  unauthorized writer who produces the same S1 gets the same decisions
  ([research/causality.md](research/causality.md)).
- **Observer independence.** A registered observer is believed, not
  checked. An observer that runs the code being judged, such as a test
  runner, is controlled by that code. Measured: a module that calls
  `sys.exit(0)` on import is accepted with the tested function absent
  ([research/observer-boundary.md](research/observer-boundary.md), X1).
- **Program correctness.** A rule such as "the test command exits 0 on
  S1" reports an exit status, not that the code is correct.
- **Changes after capture.** Writes made after S1 is captured, even
  while v9r is still deciding, are not judged (X4, measured).
- **Containment.** v9r is not a sandbox; it does not contain the actor.
- **Intent or reasoning.** Proposals and statements are never evidence.

v9r does not restore S0 after a rejection; a rejected transition holds.

## Research status

Experimental. One crate, `v9r-core`, measured on one Linux host with
rustc 1.98.1 and cargo 1.98.0. At `v9r-review-v1.1`:
`cargo test --offline --workspace` gives **88 passed, 0 failed,
4 ignored**. The frozen kernel (`crates/v9r-core/src/kernel.rs`) is
guarded by a hash test.

Known false ALLOWs are kept as passing tests: a lying observer, a
caching observer, an omitted file, X1 and X4. X2, X3, X5 and G′ of the
adversarial review are predictions, not measurements. The full list of
limitations is in [README_REVIEWER.md](README_REVIEWER.md).

The project's earlier "transactional shell" (CLI, rollback, bundles,
LLM providers, VFS, Wasm agent host) was removed in Debloat Phase 1. It
is not part of the verifier
([research/archive-boundary.md](research/archive-boundary.md)).

## Reproduce the demo

```sh
cargo run --offline -p v9r-core --example v9r_demo     # needs python3
```

One small repository with a failing test, and five scripted runs:

| Run | Agent | Verdict |
|---|---|---|
| 1 | edits `src/greet.py` | **ALLOW** |
| 2 | edits the protected test | **DENY** |
| 3 | claims success, changes nothing | **BLOCKED** |
| 4 | the same claim, with the test runner available | **DENY** |
| 5 | the code is changed after approval | **REFUSED** |

The ALLOW means the captured S1 satisfied the declared rules. The test
rule is the exit status of a test that runs S1's own code, so it does
not show that the code is correct.

Further checks:

```sh
cargo test --offline --workspace
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
cargo test --offline -p v9r-core --test self_referential_observers -- --nocapture
cargo test --offline -p v9r-core --test kernel_guard
```

## Releases

- **`v9r-review-v1.1`**: observer boundary research release
  ([GitHub release](https://github.com/ph0xphene/v9r/releases/tag/v9r-review-v1.1)).
  Adds the X1/X4 observer-boundary measurements and corrects release
  and demo wording. The verifier implementation and `kernel.rs` are
  unchanged from the previous review release.

How to reproduce and review it: [research/release.md](research/release.md).

Documents for reviewers:

- [README_REVIEWER.md](README_REVIEWER.md): entrypoint, trusted
  computing base, limitations, reading order;
- [research/verification-boundary.md](research/verification-boundary.md):
  the claim and where it ends;
- [research/observer-boundary.md](research/observer-boundary.md):
  the X1/X4 measurements;
- [research/README.md](research/README.md): the research documents in
  reading order, and the archive of historical process records.
