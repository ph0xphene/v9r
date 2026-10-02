# v9r: reviewer entrypoint

**Release under review: `v9r-review-v1.1`.** It is `v9r-review-v1` (the
frozen baseline) plus the X1/X4 measurement tests, the release
documentation and demo wording corrections they and
V9R_RELEASE_AUDIT_V0 required, and the post-freeze research documents.
The verifier implementation and the kernel are unchanged.

| | |
|---|---|
| baseline | `v9r-review-v1`, unchanged: code commit `6a9c717695e3cf00b72e97b758283af3ab5e707e` |
| v1.1 adds | the X1/X4 measurement tests (`crates/v9r-core/tests/self_referential_observers.rs`), release documentation corrections, the demo wording correction (`examples/v9r_demo.rs`, printed strings only), and the post-freeze research documents. The verifier implementation and the kernel are unchanged: `git diff v9r-review-v1 v9r-review-v1.1 -- crates/v9r-core/src Cargo.toml Cargo.lock` is empty |
| `cargo test --workspace` | **88 passed, 0 failed, 4 ignored** (v1: 86 / 0 / 4; the 2 added are X1 and X4); rustc 1.98.1, cargo 1.98.0 |
| kernel hash guard | passed; `kernel.rs` sha256 `85badb66…6177f`, unchanged since `v9r-review-v0` |
| modules | one crate, `v9r-core`: 12 modules + `lib.rs`, 4,838 lines of library code |
| archive | everything removed is at `v9r-archive-v0` (= `v9r-review-v0`) |
| release notes | [V9R_RELEASE_FREEZE_V1](research/V9R_RELEASE_FREEZE_V1.md) (§0: v1.1) |
| the claim and its boundary | [V9R_VERIFICATION_BOUNDARY_V0](research/V9R_VERIFICATION_BOUNDARY_V0.md) |
| measurement | [V9R_SELF_REFERENTIAL_OBSERVERS_V0](research/V9R_SELF_REFERENTIAL_OBSERVERS_V0.md): X1 and X4 of the adversarial review |

## What v9r is

A **state transition verifier for untrusted computation**:

1. An actor (e.g. an LLM agent) proposes a change.
2. v9r authorizes it once, against an approved state S0 identified by its
   content (a git-compatible SHA-256 tree id).
3. Immediately before the actor runs, v9r re-checks that authorization on
   a fresh observation.
4. When the actor's call returns, v9r captures the result S1 and asks
   its registered observers about it. It accepts S1 only if every
   declared rule is satisfied by their attestations. An attestation is
   as true as its observer, and an observer that executes S1 is
   controlled by S1 (measured, X1). Changes made after S1 is captured,
   even while v9r is still deciding, are not judged (measured, X4).

Each decision is **Allow**, **Deny** or **Blocked**. Missing or
contradictory evidence blocks; it never allows. An actor's statements
are claims and never satisfy a rule. After a rejection, that runtime
process authorizes nothing further; the hold is in memory and is not
persisted across a restart.

**v9r verifies states, not histories.** An unauthorized writer who
produces the same S1 gets the same decisions.

## What v9r is not

- **Not a proof of who did it.** It records no evidence about which
  process wrote a byte (no causal attribution).
- **Not a sandbox.** It does not contain the actor. Containment was
  measured, not built.
- **Not a judge of intent or reasoning.** Proposals and statements are
  never evidence.
- **Not a defence against its own observers.** A registered observer is
  believed. A second, independent one turns a lie into Blocked, not
  into the truth.
- **Not independent test evidence.** A rule "the test command exits 0
  on S1" is decided by S1's code when the test runs it. Measured: an
  agent whose `greet.py` is `sys.exit(0)` gets **ALLOW** with `greet`
  absent (X1).
- **Not rollback.** A rejected transition holds; nothing restores S0.

## Five-minute demo

```sh
cargo run --offline -p v9r-core --example v9r_demo     # needs python3; ~1 s after build
```

A tiny repository with a failing test, and five scripted runs:

| Run | Agent | Verdict |
|---|---|---|
| 1 | edits `src/greet.py` | **ALLOW** |
| 2 | edits the protected test | **DENY** |
| 3 | claims success, changes nothing | **BLOCKED** |
| 4 | the same claim, with the test runner available | **DENY** |
| 5 | someone else edits the code after approval | **REFUSED** |

`cargo test` asserts these verdicts too.

The ALLOW means the captured S1 satisfied I1–I3. I3 is the test
command's exit status on a checkout of S1, and S1's code runs inside
it, so it is not independent evidence that `greet` is correct: an S1
without `greet` that exits 0 on import is also accepted (X1, measured).
Up to `v9r-review-v1` the demo printed "tests pass on the result, run by
v9r", "honest fix" and "Every decision came from what v9r observed";
v1.1 replaces these labels.

## Main experiments

| Experiment | Question | Result | Report | Reproduce |
|---|---|---|---|---|
| **State vs Causality** | does v9r tell an authorized agent from an unauthorized writer of identical bytes? | **no**: every measured field, and the complete decisions, are identical. This is the boundary of the claim | [STATE_VS_CAUSALITY_V0](research/STATE_VS_CAUSALITY_V0.md) | `cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture` |
| **Content identity** (state) | can observed state become verifiable state? | yes: a snapshot is the git tree of the same content; a claimed id is recomputed; an omitted directory, unreadable file or altered object blocks | [CONTENT_ADDRESSED_STATE_V0](research/CONTENT_ADDRESSED_STATE_V0.md) | `cargo test --offline -p v9r-core --test content_addressed` |
| **Content identity** (grants), *archived* | can a read-only grant be bound to a snapshot? | exact only when materialized from the snapshot (9/9) | [CAPABILITY_CONTENT_IDENTITY_V0](research/CAPABILITY_CONTENT_IDENTITY_V0.md) | at the archive tag (below): `--test content_identity -- --nocapture --test-threads=1` |
| **Capability boundary**, *archived* | can an agent's world be built from a manifest and checked against it? | yes: observed ⊆ declared; a smuggled socket was denied | [CAPABILITY_MANIFEST_V0](research/CAPABILITY_MANIFEST_V0.md) | at the archive tag: `--test capability_manifest -- --nocapture` |
| **Snapshot boundary**, *archived* | can a snapshot view be isolated from the host's uid? | from the agent and host writes, yes; the namespace owner (same uid) changed it, and that was **detected, not prevented** | [SNAPSHOT_CAPABILITY_BOUNDARY_V0](research/SNAPSHOT_CAPABILITY_BOUNDARY_V0.md) | at the archive tag: `--test snapshot_boundary -- --nocapture` |

*Archived* means the code was removed from the verifier in Debloat Phase
1 because it is not part of the claim
([V9R_DEBLOAT_PHASE1_REPORT_V0](research/V9R_DEBLOAT_PHASE1_REPORT_V0.md)).
It is reproducible exactly as measured:

```sh
git worktree add ../v9r-archive v9r-archive-v0     # = v9r-review-v0, commit da6d694
cd ../v9r-archive && cargo test --offline -p v9r-core --test <name> -- --nocapture
```

Some archived experiments need a systemd user session or unprivileged
namespaces; their reports say which.

## Trusted computing base

| Trusted | For |
|---|---|
| `kernel.rs` (604 lines, frozen; `cargo test --offline -p v9r-core --test kernel_guard`) | the verdict |
| `runtime.rs`, `temporal.rs` | authorization, freshness, before/after pairing, hold |
| `graph.rs` registry, `verify.rs` | binding each answer to the provider and request that produced it |
| `snapshot.rs` (`FsSnapshot`) | state identity and scope entries |
| `fs_raw.rs` + the Linux filesystem | true, complete observation (`openat`/`O_NOFOLLOW`; directory link counts) |
| every registered observer | faithful, complete, current answers, and independent of the state it observes. The demo's test runner is **not** independent: it runs S1's code, as the verifier's uid, unsandboxed (X1, measured) |
| the rules and whoever holds the registry | what counts as acceptable, and who counts as an observer |
| snapshot capture | no undetected concurrent writer while a snapshot is taken |
| `serde`, `serde_json`, `sha2`, `libc`, `rustc` | serialization, hashing, FFI (20 external crates in the library's build graph) |

Not trusted:

- the actor and anything it says;
- stored object bytes (checked by hash);
- answers not bound to their request.

Details:
[V9R_PHASE1_EXTERNAL_REVIEW_V0 §4](research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md).

## Known limitations

- **Observers are believed.** A lying or caching observer alone gives a
  false Allow. This is measured and kept as tests.
- **Scope and identity rest on one filesystem observer.**
- **Checks that run the judged code are controlled by it.** The demo's
  test imports the code under test. Measured: `sys.exit(0)` on import
  gives ALLOW with `greet` absent; the same module without the exit is
  denied (X1).
- **The decision covers S1 as captured when the actor's call returns.**
  Measured: a process left behind by the actor rewrote the protected
  test in the live workspace while v9r was still deciding. v9r accepted
  the earlier S1 in 40/40 transitions, and its trusted state no longer
  matched the workspace (X4).
- **Special files** (FIFO, socket, device) make a state unrepresentable:
  Blocked, not Deny.
- **No rollback; no freshness across restarts.**
- **Explanations are parsed from kernel text.** A contradiction names no
  provenance.
- **The frozen kernel carries idle features and stale text:**
  - `Semantic`, `Soft` and `AtMost` have no producer;
  - the `compile_fail` example on `map` is vacuous and has been replaced
    elsewhere;
  - one doc comment names a removed module.
- **Two historical modules remain** (`counter`, `fs_watch`), and
  `content` gives a second definition of "same content" beside the git
  tree id. They stay until the runtime is restructured
  ([V9R_ARCHIVE_BOUNDARY_V0](research/V9R_ARCHIVE_BOUNDARY_V0.md)).
- **Research code**, measured on one Linux host.

The full list, with the questions we would attack first, is in
[V9R_PHASE1_EXTERNAL_REVIEW_V0 §6, §8](research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md).

## Where to start reading

1. [V9R_PHASE1_EXTERNAL_REVIEW_V0](research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md):
   the claim, guarantees with conditions, TCB, limitations, questions.
   It was written one step before the freeze. Its counts (128 passed)
   still include 37 tests of the four product-phase crates and 5
   `lib.rs` tests, all since archived. Its guarantee wording was
   corrected for v1.1 (G1, G3, G4, G6, G8, G9); its test names are
   unchanged.
2. [V9R_VERIFICATION_BOUNDARY_V0](research/V9R_VERIFICATION_BOUNDARY_V0.md):
   the formal claim, what it does not cover, the trusted computing base
   in two tiers.
3. [V9R_SELF_REFERENTIAL_OBSERVERS_V0](research/V9R_SELF_REFERENTIAL_OBSERVERS_V0.md)
   and [V9R_ADVERSARIAL_REVIEW_V0](research/V9R_ADVERSARIAL_REVIEW_V0.md):
   X1 and X4 are measured; X2, X3, X5 and G′ remain predictions.
4. [V9R_REVIEW_CHECKLIST_V1](research/V9R_REVIEW_CHECKLIST_V1.md):
   what a review should be able to confirm.
5. `crates/v9r-core/examples/v9r_demo.rs`, then
   `crates/v9r-core/tests/state_vs_causality.rs` and
   `crates/v9r-core/tests/self_referential_observers.rs`.
6. Code, in dependency order:
   - `src/kernel.rs`
   - `src/runtime.rs`
   - `src/temporal.rs`
   - `src/graph.rs`
   - `src/snapshot.rs`
   - `src/fs_raw.rs`
7. The negative cases: `tests/claim_guarantees.rs`,
   `tests/scope_entries.rs`.
8. What was removed and why:
   - [V9R_DEBLOAT_PHASE1_REPORT_V0](research/V9R_DEBLOAT_PHASE1_REPORT_V0.md)
   - [V9R_PHASE1_CORE_AUDIT_V0](research/V9R_PHASE1_CORE_AUDIT_V0.md)
   - [V9R_PUBLIC_SURFACE_AUDIT_V0](research/V9R_PUBLIC_SURFACE_AUDIT_V0.md)

Older documents describe earlier architectures. Read them as history:
[V9R_ARCHITECTURE_OVERVIEW_V0](research/V9R_ARCHITECTURE_OVERVIEW_V0.md)
lists them by role.
