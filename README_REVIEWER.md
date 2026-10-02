# v9r: reviewer entrypoint

**Release under review: `v9r-review-v1`.**

| | |
|---|---|
| code commit | `6a9c717695e3cf00b72e97b758283af3ab5e707e` (the tag sits on the documentation commit after it, which changes no code) |
| `cargo test --workspace` | **86 passed, 0 failed, 4 ignored** |
| kernel hash guard | passed; `kernel.rs` sha256 `85badb66…6177f`, unchanged since `v9r-review-v0` |
| modules | one crate, `v9r-core`: 12 modules + `lib.rs`, 4,838 lines of library code |
| archive | everything removed is at `v9r-archive-v0` (= `v9r-review-v0`) |
| release notes | [V9R_RELEASE_FREEZE_V1](research/V9R_RELEASE_FREEZE_V1.md) |

## What v9r is

A **state transition verifier for untrusted computation**:

1. An actor (e.g. an LLM agent) proposes a change.
2. v9r authorizes it once, against an approved state S0 identified by its
   content (a git-compatible SHA-256 tree id).
3. Immediately before the actor runs, v9r re-checks that authorization on
   a fresh observation.
4. Afterwards, v9r observes the result S1 itself, through registered
   observers. It accepts S1 only if that evidence satisfies declared
   rules.

Each decision is **Allow**, **Deny** or **Blocked**. Missing or
contradictory evidence blocks; it never allows. An actor's statements
are claims and never satisfy a rule. After a rejection, v9r holds.

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
- **Not rollback.** A rejected transition holds; nothing restores S0.

## Five-minute demo

```sh
cargo run --offline -p v9r-core --example v9r_demo     # needs python3; ~1 s after build
```

A tiny repository with a failing test, and five scripted agents:

| Run | Agent | Verdict |
|---|---|---|
| 1 | honest fix | **ALLOW** |
| 2 | edits the protected test | **DENY** |
| 3 | claims success, changes nothing | **BLOCKED** |
| 4 | the same claim, with the tests run by v9r | **DENY** |
| 5 | someone else edits the code after approval | **REFUSED** |

`cargo test` asserts these verdicts too.

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
| every registered observer (e.g. the demo's test runner) | faithful, complete, current answers |
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
- **Checks that run the judged code are not independent of it.** The
  demo's test imports the code under test.
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
   `lib.rs` tests, all since archived. Its guarantees and test names are
   unchanged.
2. [V9R_REVIEW_CHECKLIST_V1](research/V9R_REVIEW_CHECKLIST_V1.md):
   what a review should be able to confirm.
3. `crates/v9r-core/examples/v9r_demo.rs`, then
   `crates/v9r-core/tests/state_vs_causality.rs`.
4. Code, in dependency order:
   - `src/kernel.rs`
   - `src/runtime.rs`
   - `src/temporal.rs`
   - `src/graph.rs`
   - `src/snapshot.rs`
   - `src/fs_raw.rs`
5. The negative cases: `tests/claim_guarantees.rs`,
   `tests/scope_entries.rs`.
6. What was removed and why:
   - [V9R_DEBLOAT_PHASE1_REPORT_V0](research/V9R_DEBLOAT_PHASE1_REPORT_V0.md)
   - [V9R_PHASE1_CORE_AUDIT_V0](research/V9R_PHASE1_CORE_AUDIT_V0.md)
   - [V9R_PUBLIC_SURFACE_AUDIT_V0](research/V9R_PUBLIC_SURFACE_AUDIT_V0.md)

Older documents describe earlier architectures. Read them as history:
[V9R_ARCHITECTURE_OVERVIEW_V0](research/V9R_ARCHITECTURE_OVERVIEW_V0.md)
lists them by role.
