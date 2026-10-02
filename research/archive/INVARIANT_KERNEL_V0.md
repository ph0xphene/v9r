# Invariant Kernel v0

Research question: can verified filesystem facts feed a small, reusable
invariant/obligation kernel without turning v9r into a general theorem
prover?

Short answer: yes, for this scope. The kernel is 554 non-test lines of
typed Rust that import only `std` and `serde`, which a test enforces. It
knows nothing about files. All filesystem knowledge stays in the evidence
layer.

Getting there first required moving trusted state out of the agent's
workspace. That change was moderately invasive but self-contained.

Building it also surfaced four pre-existing gaps in how v9r decides that
a task succeeded (see *Security findings*).

## Baseline

- Branch `research/effect-runtime-v0`, starting commit `e199ee6` (the
  Effect Runtime V0 report). The four milestone-1 commits are untouched.
- `cargo test --workspace`: **125 tests passing**:
  - cap 11, core 64, effects 21, orchestrator 11, runtime 8, vfs 7;
  - doctests 3.
- Previous state:
  - filesystem effect receipts;
  - `Verified<T>` constructible only from two live observations;
  - receipts in the trace.
  - Known weakness: `.v9r/` and `trace.jsonl` lived in the agent-writable
    workdir, and runtime state was excluded from observation (a blind
    spot).
- Pre-existing tooling state:
  - `cargo fmt --check` fails on 3 hunks in `adapter.rs` and 5 in
    `execution.rs`;
  - clippy reports 4 warnings, in `v9r-cli/src/main.rs` and
    `v9r-runtime/src/wasm.rs`.
  - All of these are unchanged by this milestone.

## Trust boundary

### What is trusted, what is agent-controlled

| Trusted (runtime) | Agent-controlled |
|---|---|
| checkpoint backups and manifests | everything in the workspace, including `task.txt` and any `.v9r` or `trace.jsonl` it creates |
| checkpoint seals (in memory) | `ActionProposal`s: action, declared writes, expectations, the "requested" text |
| task trace, and the receipts and decisions in it | semantic evidence (oracle output) |
| trace seal (in memory) | persisted records it can get the runtime to read (bundles) |
| `Policy`: manifest and invariant set (in memory, digested) | |
| evidence base, authorizations (in memory) | |

### Where trusted state lives

```text
<state root>/tasks/<task id>/trace.jsonl
<state root>/tasks/<task id>/checkpoints/<checkpoint id>/{manifest.json, backups…}
```

- **State root.** It comes from `V9R_STATE_DIR`, else
  `$XDG_STATE_HOME/v9r`, else `$HOME/.local/state/v9r`. There is no
  hard-coded or `/tmp` fallback: if none is set, the run fails. Root and
  task directories are created with mode `0700`.
- **Separation.** `StateRoot::check_separation` refuses a workspace that
  contains the root or lies inside it. The check runs on canonical paths,
  so a symlinked spelling is caught too (tested).
- **Legacy layouts.** A workspace that still has a top-level `.v9r` or
  `trace.jsonl` from the old layout is refused with an explicit
  `LegacyLayout` error rather than silently treated as task data. Bundle
  import refuses such entries (`LegacyEntry`) and writes the carried
  trace into the new task's state directory.
- **Consequence for observation.** Observation no longer excludes
  anything. Writing `.v9r/…` in the workspace is now an ordinary,
  observed and rolled-back effect, which closes the previous blind spot
  (test `i_writes_to_workspace_dot_v9r_are_observed`).

### Why the agent cannot mutate it through its authority

The agent's authority is what v9r mediates:

- **Command execution.** Commands run with their cwd in the workspace.
- **Argument fence.** Every argument and declared path passes the
  structural fence: no absolute paths, no `~`, no `..`.
- **Write scope.** Declared writes must be inside `allow_write` (PRE
  obligation I1).
- **Write actions.** The legacy `<write>` action canonicalizes its target
  into the workspace.
- **No symlink following.** The runtime never follows workspace symlinks:
  - observation records symlinks and does not follow them;
  - rollback unlinks them;
  - checkpoints refuse them.

With trusted state outside the workspace, none of these paths can name
it.

This is **not an OS sandbox**. An allowlisted program runs with the same
UID and can open any path it likes. Against that, the milestone provides
detection, not prevention:

- **Trace.** `TraceLogger` keeps a running SHA-256 and length of what it
  wrote; `verify_integrity` catches edits, foreign appends and deletion.
- **Checkpoints.**
  - Each manifest's SHA-256 is sealed in memory at checkpoint time.
  - Rollback refuses unsealed checkpoints, and manifests that no longer
    match their seal, even a consistent forgery of both backup and
    manifest.
  - Backups are checked against the manifest before anything is mutated.
- **I4 obligation.** `TrustedState = Intact` is required before *and*
  after every guarded action, so tampering blocks further authority.

Seals live in process memory. After a crash, checkpoints can no longer be
restored, and v9r fails explicitly rather than trusting disk.

### Invasiveness, and the previous decision

Part 1 touched:

- `vfs.rs`: registry, checkpoint location, seals;
- `trace.rs`: the seal;
- `bundle.rs` and `task.rs`: `import_bundle` / `Task::from_bundle` now
  take a `StateRoot`, an API break;
- the CLI (resolve and log the state root) and one REPL message;
- `state.rs` / `effect.rs`: dropped the exclusion list.

It needed one commit and no crate or data-model restructuring. A CLI
smoke test through a pty confirmed the new layout end to end: workspace
contains only task files, checkpoint and trace under the state root,
trace intact after rollback.

**The previous milestone's decision stays A.** The relocation was
contained, and it removed that milestone's main caveat.

## Kernel model (`crates/v9r-core/src/kernel.rs`)

- **Fact**: `Fact { subject: S, value: V }`. Subjects are keys, so two
  facts with the same subject and different values contradict each
  other. The kernel is generic over `S` and `V`.
- **Evidence**: a fact plus its class.
  - `Verified`: `Verified<T>` is attested by a deterministic observer
    inside v9r-core (crate-private constructor, no `Deserialize`) and
    stored with a `Provenance { observer, basis }`.
  - `Semantic`: `Semantic<T> { value, source, confidence_bp }`, which
    anyone may construct.
  - `Proposed`: a claim with a source, such as an agent's assertion or a
    record read back from disk.
  - Unknown: the absence of evidence for a subject.
  - `EvidenceBase` indexes evidence by subject. Insertion is idempotent,
    and the type has no `Deserialize`.
- **Obligation**: `{ invariant, phase: Pre|Post, requirement }`. The
  requirement language is closed and has three forms:
  - `Fact { subject, value, strength: Hard | Soft{min_confidence_bp} }`:
    exact value;
  - `Within { names, scopes }`: component-bounded containment in a `/`
    namespace. Names are `Known` or `UnknownBelow` (an unknown subtree);
  - `AtMost { quantity, value, limit }`: a numeric bound.
  - There are no quantifiers, no recursion and no user code.
- **Invariant**: `trait Invariant<C, S, V> { id; obligations(&C) }`,
  which derives obligations from a context and must not execute
  anything. v9r's invariants are a closed enum in `policy.rs`.
- **Decision**: `evaluate(phase, obligations, evidence)` is pure. It
  returns a `Verdict` and one `Finding` per obligation, each with a
  `Status` and a human-readable reason. `DecisionRecord` is its
  domain-free trace form.

## Decision semantics

Per obligation:

- **Satisfied**:
  - exactly one verified value exists and it equals the requirement;
  - or, for `Soft` only, all semantic evidence at or above the threshold
    agrees with the requirement;
  - or a local check (`Within`, `AtMost`) holds.
- **Violated**:
  - exactly one verified value exists and it differs from the
    requirement;
  - or a known name lies outside every scope;
  - or a bound is exceeded.
- **Undetermined**:
  - anything else: no evidence, only semantic or proposed evidence,
    several contradictory verified values, or unknown names that might
    lie outside the scope.

Verdict:

- **ALLOW**: every applicable obligation of this phase is satisfied. For
  `Pre`, this yields a single-use `Authorization`. For `Post`, it means
  the transition is accepted.
- **DENY**: at least one obligation is *shown violated by verified
  evidence or a deterministic local check*. The runtime has enough
  evidence to conclude the action (or its result) violates an invariant.
- **BLOCKED**: nothing is shown violated, but at least one obligation
  cannot be shown satisfied. The runtime lacks evidence to authorize or
  accept safely.

Deny dominates Blocked.

Examples from the tests:

| Situation | Verdict | Why |
|---|---|---|
| `mkdir out` declaring `out` | ALLOW / ALLOW | declared write in scope; observed `out` ⊆ declared; trusted state intact |
| `touch elsewhere.txt` with `allow_write = [out]` | DENY (pre), nothing runs | `I1.declared_writes_in_scope` violated |
| `touch out.txt stray.txt` declaring only `out.txt` | DENY (post), then every proposal DENY until rollback | `I1.observed_writes_declared` violated (`stray.txt`); then `workspace_accepted` violated |
| `true` expecting `report.txt` to exist | DENY (post) although exit 0 | verified `Exists(report.txt) = Absent` |
| `touch made.txt no-such-dir/x` expecting `made.txt` | ALLOW (post) although exit 1 | the verified effect satisfies the postcondition |
| export before any test run | BLOCKED | no evidence for `TestsAt(version)` |
| export after the test command exited 1 on this version | DENY | verified `Failed` |
| export after a pass, then any further change | BLOCKED | the pass is about an older version |
| export with an oracle at 10000 bp saying tests passed | BLOCKED | semantic evidence cannot satisfy a hard obligation |
| trace appended out of band | DENY | `I4.trusted_state_intact` violated |

## Evidence semantics

**SEMANTIC ≠ VERIFIED by construction, not by convention.**

- `Verified` can only be created inside v9r-core, and only by the
  evidence sources in `effect.rs`/`facts.rs`.
- Oracles have exactly one entry point, `SemanticOracle::confidence(fact)
  -> Option<u16>`. The runtime then wraps the answer as
  `Semantic { value: <that exact fact>, … }`. An oracle cannot choose the
  fact, the class, or the strength.
- Hard obligations ignore semantic evidence entirely, even at 10000 bp,
  and semantic evidence never produces `Violated`. A confident "the tests
  failed" blocks; it does not deny.
- Serialization cannot upgrade authority:
  - `Verified`, `Observation`, `EvidenceBase` and `Authorization` have no
    `Deserialize`. Compile-fail doctests pin this, together with a
    compiling control that confirms the names resolve.
  - A persisted `ReceiptRecord` ingests only as `Proposed`
    (`facts::ingest_record`), and live observation outranks it.

**UNKNOWN propagates, never collapses:**

- **Paths.** An unobservable path yields *no* fact, never `Absent`.
- **Effects.** Unknown effects become `UnknownBelow` names. They are
  harmless inside a declared scope and Blocked outside it.
- **Checks.** A seal check that cannot complete yields no
  `TrustedState` fact, which means Blocked.
- **Exit statuses.** A test command that was denied, signalled or has an
  unknown outcome yields no `TestsAt` fact.
- **Contradictions.** Contradictory verified values block instead of
  picking one.

## Pre/post invariants

| Invariant | Pre | Post | Evidence |
|---|---|---|---|
| `well_formed_proposal` | ✓ | | proposal (local check) |
| `workspace_accepted` | ✓ | | runtime's own verdict history |
| `I4.trusted_state_intact` | ✓ | ✓ | trace seal and checkpoint seals |
| `step_budget` | ✓ | | task counter vs `max_steps` |
| `I1.declared_writes_in_scope` | ✓ | | proposal vs `allow_write` |
| `export_gate` | ✓ | | `TestsAt(current version)`; `Exists(mandatory artifact)` from the live observation |
| `authorization_basis_current` | at execute | | a fresh observation's digest must equal the authorization's basis |
| `I1.observed_writes_declared` | | ✓ | receipt: verified effect paths, unknown subtrees |
| `expectations` | | ✓ | post-observation |
| `I3.rollback_restores_checkpoint` | | ✓ (rollback) | rollback receipt's post-state digest = checkpoint digest |

The phase is explicit in the types: `Obligation.phase`, and `evaluate`
only considers obligations of the requested phase. PRE yields an
`Authorization`; POST yields acceptance. A non-Allow POST verdict marks
the workspace *unaccepted*, and only a kernel-accepted rollback clears
it.

The invariants requested for this milestone map as follows:

- **I1**: two obligations, pre and post.
- **I3**: a post obligation on the rollback action. The "historical
  evidence preserved" half is covered by I4: the trace seal proves the
  earlier receipts are still there, byte for byte.
- **I4**: pre and post, as above.
- **I2 and I5** did **not** become policy entries, and forcing them in
  would be wrong:
  - **I2** (verified facts have valid deterministic provenance) is
    structural. Only attested evidence can be verified, every verified
    entry carries its provenance, records ingest as proposed, and
    state-dependent facts are keyed by version or rebuilt from the
    current observation for every decision.
  - **I5** (unknown or semantic facts never satisfy a hard obligation)
    is the kernel's definition of `Strength::Hard`.

## Effect integration

`EffectReceipt` is an evidence *source*, not policy:

- **Verified effects to names.** `facts::receipt_names` turns verified
  effects into `Name::Known` and unknown effects into
  `Name::UnknownBelow`, which feeds `Within`.
- **Receipt to version fact.** `facts::from_receipt` turns a live
  receipt's post-state digest into a verified `Workspace = Digest` fact
  (I3).
- **Post-observation to path facts.** `facts::from_observation` answers
  `Path`, `Exists` and `Workspace` for exactly the subjects the
  obligations mention, using the post-observation.
- **Exit status to test facts.** `facts::test_outcome` turns the
  runtime's own wait status of a configured test command into
  `TestsAt(version started on)`.

The kernel never sees an `Entry`, a hash algorithm or a path type.
`Within` is about hierarchical names in general (it would fit git refs as
well), and the guard test fails if `kernel.rs` imports anything but `std`
and `serde`.

## Security findings

Pre-existing defects found this milestone:

1. **Policy self-weakening across runs (legacy REPL).**
   - The REPL re-reads the manifest file on every `run`, and the
     shorthand default puts `task.toml` inside the workspace, where
     `allow_write = ["."]` covers it.
   - A task can therefore rewrite its own `allow_exec`/`allow_write` for
     the next run.
   - The guarded path refuses a policy source inside the workspace
     (`Policy::from_trusted_source`); the legacy path is unchanged.
2. **Test validation passes on missing or stale evidence (legacy).**
   `Task::validate_outcome` only fails if a test command *ran and
   failed*. It therefore passes when the configured tests never ran, and
   when they passed before the agent modified files. This is exactly the
   "deploy on `Semantic`/`Unknown`" failure mode. The guarded
   `export_gate` requires a verified pass on the *current* version.
3. **Agent claims recorded as runtime observations (legacy).** On
   `<finish>`, the adapter appends a synthetic `CommandExecuted { command:
   "finish: <reason>", exit_code: 0|1 }` (`adapter.rs:443`). In the trace,
   the model's self-reported success is indistinguishable from an exit
   status the runtime observed: PROPOSED is written in the shape of
   VERIFIED.
4. **Prefix test matching.** `record_test_command` matches by string
   prefix, so `cargo test` also matches `cargo testify`. The guarded path
   uses token-bounded matching.

Also:

- `v9r run -f` (script mode) is now **confirmed at runtime** to be
  refused by the shell denylist (pty smoke test). The previous report
  had it by inspection only.
- The existing test helpers still leak temp directories into `/tmp`.
  They were cleaned after each run here, but not fixed.
- `README.md` still describes `.v9r/backups` and a workdir
  `trace.jsonl`. It was left untouched as unrelated documentation; it is
  now stale on those points.

Defects in this milestone's own code, caught during development:

- A rollback judged Deny or Blocked did not re-mark the workspace
  unaccepted. Fixed, and tested (`checkpoint_tampering_…`).
- The whole-state digest was recomputed 4–5 times per step, which the
  performance measurements exposed (below). Fixed.

### Adversarial classification

| Attack | Status | How |
|---|---|---|
| forged serialized receipts | **solved** | records ingest as `Proposed`; the authority types are not `Deserialize`; a live observation outranks the claim (test `serialized_receipts_cannot_upgrade_authority`) |
| tampered trusted-state files | **solved (detection)** for the trace, checkpoint manifests and backups; **unsupported** for other files under the state root, which nothing relies on | seals + I4; rollback refuses |
| symlinks targeting trusted state | **explicitly blocked** | present at start: checkpoint refuses; planted mid-task: authorization becomes stale; the runtime never follows them. **Future research**: a same-UID tool (e.g. `tar`) that *creates* an escaping symlink inside a declared scope passes I1. A symlink-target invariant is the obvious next obligation |
| stale evidence | **solved** | `TestsAt` is keyed by version; path facts are rebuilt from the current observation per decision |
| duplicated evidence | **solved** | idempotent evidence insertion; `Authorization` is consumed by `execute` and is not `Clone` |
| semantic evidence at 1.0 | **solved** | `Hard` ignores semantic evidence |
| contradictory verified facts | **explicitly blocked** | several verified values → Undetermined → Blocked |
| rollback after violation | **solved** | workspace held; only a kernel-accepted rollback (I3 + I4) re-opens it |
| authorization goes stale before execution | **solved** for changes visible to the freshness scan; the scan-to-spawn window remains **future research** (TOCTOU) | freshness obligation, and out-of-band changes mark the workspace unaccepted |
| agent-supplied invariant modifications | **solved** on the guarded path (no API for it; expectations only add obligations; digested immutable `Policy`); **unsupported** on the legacy REPL (finding 1) | |
| invariant removal via task actions | **solved** on the guarded path | policy is in memory; its source may not be in the workspace |

## Limitations

- **Stale observations and TOCTOU.**
  - Freshness is checked by a scan immediately before the spawn, and
    nothing stops a concurrent process from changing the workspace
    between that scan and the command.
  - Export reads files after its freshness scan.
  - Closing either gap requires OS support: filesystem snapshots, or
    locking out other writers.
- **External side effects.** Anything outside the workspace (network,
  `$HOME`, other processes) is invisible, so no POST obligation can
  catch it. The kernel is only as good as its observers.
- **Incomplete specifications.**
  - A task is safe only relative to its manifest and proposal.
  - Declared writes are coarse: declaring `.` makes I1-post vacuous.
  - The policy cannot say "this write must be a valid Rust file"; that
    is SEMANTIC.
- **Observer trust.** The TCB is v9r-core, the OS and the programs it
  runs.
  - `TestsAt = Passed` means "the configured test command exited 0 when
    started on version *v*", not "the code is correct".
  - A test command that writes into the workspace changes the version.
    `cargo test` writing `target/` is the typical case, and then the
    export gate can never pass. Real Cargo projects would need a notion
    of version that excludes build output. That is a real specification
    gap, not a bug.
- **Detection, not prevention.** A same-UID process can still write to
  the state root; v9r only notices. Seals do not survive a crash.
- **Physical and open-world uncertainty.** None of this says anything
  about the world beyond the observed tree. The kernel proves no global
  safety property; it checks a handful of local obligations against the
  evidence it was given.

## Performance

Release build, 12 cores, ext4 on LUKS, medians of 7 steps, command `true`
(`tests/perf.rs`, run with `--ignored`). Times in ms.

| workspace | observe | legacy step | observed step | guarded start | authorize | execute | guarded total | trace verify | no-op rollback |
|---|---|---|---|---|---|---|---|---|---|
| 10 × 1 KiB | 0.12 | 1.20 | 1.64 | 1.06 | 0.07 | 2.18 | 2.25 | 0.02 | 0.53 |
| 1 000 × 4 KiB | 10.7 | 1.76 | 24.4 | 32.1 | 0.19 | 24.2 | 24.4 | 0.06 | 32.5 |
| 10 000 × 1 KiB | 94.3 | 2.00 | 193.6 | 301.0 | 0.94 | 195.3 | 196.0 | 0.38 | 285.4 |
| 100 × 1 MiB | 55.6 | 1.97 | 113.9 | 102.0 | 0.06 | 113.6 | 113.7 | 0.02 | 167.9 |

- **Kernel evaluation.** About 1.1 µs per decision with 10 obligations
  (100k iterations). This is negligible.
- **Guarded overhead.** A guarded step costs the same as a plain observed
  step (two scans). Authorization adds under 1 ms at 10k files, mostly
  the checkpoint-manifest seal check, which is O(entries).
- **Trusted-state overhead.**
  - Trace verification re-reads the whole trace per decision: O(trace),
    so O(n²) over a long task. At 13 KiB it is negligible, and it will
    matter only for very long tasks.
  - Seal checks are O(manifest size).
- **Scanning cost.** Observation is roughly 8–9 µs per file plus about
  0.55 ms per MB. Everything is linear, and nothing is pathological
  after the fix below.
- **Duplicate work.**
  - **Found and fixed:** the whole-state digest was recomputed 4–5 times
    per step (23 ms of "authorize" at 10k files). `Observation` now
    carries its digest.
  - **Remaining by design:** the freshness scan before execution repeats
    the previous step's post-scan whenever nothing changed out of band.
    That is one of the two scans per step, and it is the price of
    TOCTOU detection.
  - **Avoided:** `GuardedTask::start` reuses the checkpoint walk instead
    of scanning again.
- **Scaling concern.** Two full-tree hashes per step. At 10k files each
  guarded step costs about 0.2 s. An incremental index (mtime/inode
  cache) would be the first optimization if real workspaces demand it;
  nothing here required one yet.

## Declarative syntax (not implemented)

Nine invariants fit comfortably in a Rust `match`. The awkward part is
not the requirement forms, which map directly onto s-expressions:
`(require hard (tests-at version) passed)`,
`(within declared-writes allow-write)`, `(at-most steps max-steps)`.

The awkward part is **context accessors** such as `version`,
`declared-writes`, `touched` and `allow-write`, which are Rust match arms
today. A future syntax would need:

- a fixed, typed accessor set per stage;
- a per-domain subject vocabulary;
- phase tags;
- **no** user-defined functions or recursion.

## Future JEV attachment point

JEV attaches at `SemanticOracle::confidence(&RuntimeFact) -> Option<u16>`
(and the receipt-level `assess`). Its answers become
`Semantic { value: fact, source: "oracle:<name>", confidence_bp }` in the
evidence base. They can satisfy only `Soft` obligations, and the
standard policy currently has none, so today semantic evidence only
appears in explanations.

Where it would help:

- **Soft obligations.** For example, "the observed delta satisfies the
  requested effect", or triage of undeclared effects.
- **Proposing postconditions.** It could propose expectations: these
  only *add* obligations, and Rust checks them.
- **Proposing invariants.** These would go through a separate privileged
  path (human review, then `Policy::new`), never through a proposal.

What JEV must never do: produce `Verified`, satisfy a `Hard` obligation,
turn Unknown into anything, or touch the `Policy`.

*Rust establishes facts. JEV may later interpret meaning.*

## Foundational-runtime hypothesis

> Did filesystem effects + a small invariant kernel compose cleanly
> enough to justify treating v9r as a general execution substrate?

For:

- **The kernel did not bend.** None of the nine invariants, nor the
  freshness check, needed a new requirement form or any domain knowledge
  in `kernel.rs`.
- **Clean separation between layers.** Receipts became evidence through
  about 240 lines in `facts.rs`, and all domain knowledge sits in
  `facts.rs` and `policy.rs`.
- **Decisions are deterministic and explainable.** Each finding carries
  its reason and provenance, and identical runs produce identical
  decision records (tested).
- **The authority boundary is compiler-enforced.** Proposals are data,
  authorizations are single-use and unforgeable, and semantic evidence
  cannot become verified.
- **The fixes were contained.** The trust-boundary fix was contained,
  and the performance cost is dominated by observation, not by the
  kernel.

Against, or not yet shown:

- **One domain.** Only one domain exists. `Subject`/`Value` is a closed
  enum that mixes filesystem and runtime facts, and "version" is the
  workspace digest, a filesystem notion that `guarded.rs` knows about.
- **Opt-in only.** The guarded path is a narrow prototype. The legacy
  paths (the adapter loop in particular) do not go through it, and they
  hold three of the four findings above.
- **Not OS-enforced.** The trust boundary relies on v9r-mediated
  authority plus same-UID detection.
- **Untested requirement forms.** Invariants that are *relational* (git
  ancestry, "no force-push") have not been tried. They are bounded but
  not local, and may not fit `Fact`/`Within`/`AtMost`.

## Decision

**A. The invariant kernel composes cleanly and is a credible
foundation.**

The claim is limited to what was built and tested. The kernel stayed
small, deterministic, explainable and domain-free; the domain parts stayed
isolated; and the uncertainty classes survived every attack attempted
here, either by construction or by an explicit Blocked.

It is not yet shown to be a *general* substrate. That is the next
experiment's job, and the result of that experiment would be the right
reason to move to B.

## Strongest falsification experiment for the next milestone

**Add a second, structurally different evidence domain, git, with at
least three real invariants, without changing `kernel.rs`.**

Candidate invariants:

- "export only commits that descend from the task's base commit";
- "no ref outside `refs/heads/task/*` moves";
- "the exported tree equals the verified workspace version".

Outcome:

- **Supports A:** the kernel diff is 0 lines, and the git facts fit
  `Fact`/`Within`/`AtMost` with a per-domain subject type.
- **Falsifies A:** ancestry-style relations need a new requirement form,
  domain knowledge in the kernel, or unbounded search. The honest
  verdict would then be B.
