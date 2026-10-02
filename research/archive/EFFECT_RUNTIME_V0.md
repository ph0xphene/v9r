# Effect Runtime v0: Filesystem Only

Research question: can v9r make a defensible distinction between *what a
command returned* and *what the filesystem demonstrably changed*?

Short answer: yes, within an explicitly stated scope. Building it also
turned up six defects in the existing snapshot, rollback and bundle code.
Two of them undermined exactly the property this milestone needs:
history evidence survived neither rollback nor bundling correctly.

## Baseline

- Branch `research/effect-runtime-v0`, starting commit `221994d`
  ("Delete agents/llm_gateway/data.txt"), clean tree.
- `cargo test --workspace`: **82 tests, all passing**
  (v9r-cap 11, v9r-core 45, v9r-orchestrator 11, v9r-runtime 8, v9r-vfs 7).
- `cargo fmt --check` already failed at baseline (`adapter.rs`,
  `execution.rs`). Those hunks were left untouched. `cargo-fmt` and
  `cargo-clippy` are not installed on this machine; they were run through
  `nix shell nixpkgs#rustfmt nixpkgs#clippy`.

Relevant existing architecture:

| Piece | Where | Role |
|---|---|---|
| Checkpoint / rollback | `v9r-core/src/vfs.rs` | Copies the workdir into `.v9r/backups/<task>/<cp>/` with a JSON manifest (`files` + FNV-64, `dirs`); rollback diffs the current tree against it. Refuses project roots. |
| Command execution | `v9r-core/src/execution.rs` | `run_task_step`: structural fence (no shells, no absolute or `..` args), manifest allowlists, direct `execve`, logs `CommandExecuted { command, exit_code }`. |
| Trace | `v9r-core/src/trace.rs` | `TaskEvent` JSONL, append-only file handle, by default `workdir/trace.jsonl`. |
| Bundles | `v9r-core/src/bundle.rs` | bincode `TaskBundle { files, artifact_hashes (FNV-64), trace_jsonl }`. |
| LLM loop | `v9r-core/src/adapter.rs` | `run_with_guards`: checkpoint, steps, validate, rollback on failure. |
| `v9r-vfs` / `v9r-cap` / `v9r-runtime` | separate crates | In-memory VFS, Plan-9-style namespace views, WASM host. **Not involved** in the host-filesystem transaction path; nothing to reuse for this milestone. |

## Existing machinery reused

- **Snapshot code.** The checkpoint crawler was generalized into
  `state::observe_with`, rather than adding a second crawler next to it.
  It is now the only filesystem walker used by checkpoint, rollback and
  effect observation. Checkpoint passes a visitor that writes each backup
  from the same bytes that were hashed, so a backup and its manifest
  cannot disagree. Bundle export (`bundle::collect_files`) and the LLM
  context builder (`context::collect_allowed_files`) still have their own
  walkers. They were left alone because they have different jobs (export
  content, bounded text for prompts) and are outside this milestone.
- **Rollback code.** `selective_rollback` now diffs two `FsState`s (the
  snapshot and a fresh observation) instead of carrying its own walker.
  Its original safety rules are kept: re-check `ensure_safe_directory`,
  validate every manifest path before any join, never `remove_dir_all`
  an unknown directory, and stay idempotent.
- **Trace code.** Receipts travel in the existing JSONL trace as one new
  `TaskEvent` variant, so bundles carry them with no format change.
- **Execution path.** `run_observed_step` wraps the unchanged
  `run_task_step`, so all fences, manifest checks and trace events apply
  exactly as before.
- **Capability code.** Not reused. `v9r-cap` governs the in-memory VFS
  namespace, not host commands. Command permissions are the manifest plus
  `validate_command_spec`, and receipts record their result (a denied
  command yields outcome `not_started`).

## New abstractions

### State snapshot: `state::FsState` (`v9r-core/src/state.rs`)

- `BTreeMap<String, Entry>`, with keys as `/`-separated paths relative to
  the root. Lexicographic order puts a parent before its children.
- `Entry { kind: file|dir|symlink|other, len, sha256, link_target }`.
  Equal entries mean equal *observable* state. mtime, permissions, owner,
  xattrs and inode identity are deliberately excluded.
- `unobserved: Vec<{path, reason}>` records paths whose state could not be
  established. A record covers the path and everything below it.
- `digest()` is SHA-256 over the canonical serialization, so equal states
  give equal digests on any tree.
- Internal paths are excluded at the top level only: `.v9r`,
  `trace.jsonl`.
- **Symlinks** are recorded by target string (`readlink`) and never
  followed, neither to traverse nor to read.
- **Files** are opened with `O_NOFOLLOW | O_NONBLOCK`. The walker checks
  the open file against the walk's `lstat` (dev/inode) and checks length
  and mtime after reading. A path swapped for a symlink or FIFO, or
  modified mid-read, becomes `unobserved`; it is never read through.
  Directories get the same dev/inode check around their listing. FIFOs
  and devices are recorded as `other` and never opened for reading.
- Hashing moved from FNV-64 to SHA-256 (`sha2` was already in
  `Cargo.lock`). FNV-64 is not collision resistant: an adversarial
  modification with a matching FNV hash would have been skipped by
  rollback and reported as "unchanged" by effects.

### State delta: `effect::diff(&FsState, &FsState) -> StateDelta`

- Deterministic: effects sorted by path, one per path.
- `Created` (absent → present), `Modified` (present in both, observable
  state differs, which includes kind changes such as file → dir and
  symlink retargeting), `Deleted` (present → absent). Each effect carries
  the full before and/or after `Entry` as its evidence.
- A path absent from both states produces nothing. A path covered by an
  `unobserved` record in *either* state produces no effect, only an
  unknown.

### `EffectReceipt` (`v9r-core/src/effect.rs`)

The receipt keeps four facts apart:

| Field | Fact | Source |
|---|---|---|
| `requested` | REQUESTED effect | free text from the caller, never interpreted by Rust |
| `action.declared_writes` | DECLARED effect | `CommandSpec.writes` (already manifest-checked) |
| `outcome` | COMMAND RESULT | `exited{code}`, `terminated`, `completed`, `failed`, `not_started{reason}`, `unknown{reason}` |
| `verified` | OBSERVED effect | `diff(pre, post)` |

It also records `scope` (root, excluded names, `follows_symlinks: false`),
the `pre_state`/`post_state` digests as provenance, `unknown`, the
derived `undeclared` (verified effects outside the declared writes), and
`semantic`.

A real receipt from test D (`touch made.txt no-such-dir/x`):

```json
{
  "action": { "kind": "command", "argv": ["touch","made.txt","no-such-dir/x"], "declared_writes": ["made.txt"] },
  "requested": "create made.txt and no-such-dir/x",
  "outcome": { "outcome": "exited", "code": 1 },
  "scope": { "root": "/tmp/...", "excluded": [".v9r","trace.jsonl"], "follows_symlinks": false },
  "pre_state": "sha256:df52…", "post_state": "sha256:83ca…",
  "verified": [ { "effect": "created", "path": "made.txt",
                  "after": { "kind": "file", "len": 0, "sha256": "sha256:e3b0…" } } ],
  "undeclared": [], "unknown": [],
  "semantic": { "status": "unknown", "reason": "no semantic oracle consulted" }
}
```

In this receipt the process failed, one of the two requested files exists,
and the effect is verified. Whether the request was satisfied is
explicitly unknown.

### Epistemic status

- **VERIFIED** is `Verified<T>`, whose field is private. The only
  constructor is `EffectReceipt::from_observations`, which requires two
  `Observation`s. `Observation` can only be created by
  `Observation::capture(root)`, which runs a live scan. Neither type
  implements `Deserialize`. Three `compile_fail` doctests pin this down,
  checked against a control case that does compile. A forged
  `Verified` can therefore not come from a struct literal, from JSON, or
  from a deserialized `FsState`. That last point matters: `FsState` must
  stay deserializable because it is the checkpoint manifest.
- **SEMANTIC** is `SemanticAssessment::Semantic(SemanticEvidence)`. It is
  a different field of a different type, and no conversion into
  `Verified` exists.
- **UNKNOWN** is `UnknownEffect` (per path, or `"."` for the whole scope
  when post-observation fails) and `SemanticAssessment::Unknown`.
- **Persistence.** The trace stores a `ReceiptRecord`, a plain,
  deserializable report whose `verified` field means "classified as
  verified by the producing runtime". There is intentionally no
  `ReceiptRecord → EffectReceipt` conversion: reading a record back does
  not re-establish anything, and bundles are unsigned.

### Semantic oracle boundary

```rust
pub trait SemanticOracle {
    fn name(&self) -> &str;
    fn assess(&self, requested: &str, receipt: &EffectReceipt) -> Option<SemanticEvidence>;
}
```

`NoSemanticOracle` returns `None`, and the assessment stays `Unknown`
with the reason "oracle unavailable". `EffectReceipt::assess_request` can
only replace the `semantic` field. The oracle gets a shared reference and
returns plain data, so it cannot grant capabilities, mutate the receipt
or produce `Verified`. The type system *cannot* stop an implementation
from doing I/O; that remains a review rule. The trait was kept because it
costs about 30 lines and fixes where interpretation plugs in. Its exact
signature is a guess and should be expected to change once a real oracle
exists.

### Trace relationship (decision)

Receipts go in the trace as `TaskEvent::EffectObserved { receipt:
Box<ReceiptRecord> }`, after the step's own `CommandExecuted` and
`TaskFinished` events. Reasons:

- Bundles already carry the trace, so receipts travel with no bundle
  format change.
- Trace order puts each claim next to the execution history it is derived
  from, without merging the two. `CommandExecuted` remains the
  execution-history event.
- Old traces still parse. Old binaries cannot parse new traces that
  contain the variant.

Receipts are **opt-in**. `run_task_step` and the adapter's LLM loop are
unchanged. Wiring them in would put about 1 KB of JSON per step into the
trace window that `TaskSnapshot` feeds back to the model, and would hash
the whole workdir twice per step. Both deserve their own decision.

`vfs::rollback_observed` treats a rollback as an action with its own
receipt, so "what happened during execution" and "what rollback undid"
are two separate verified records.

## Guarantees

VERIFIED currently means exactly this. For a path *p* under the scope
root, excluding the top-level `.v9r` and `trace.jsonl`, and for the two
observations taken immediately before and after the action:

- **Created**: *p* was absent in the pre-observation and present in the
  post-observation, with the recorded kind, length and SHA-256 (or
  symlink target).
- **Deleted**: the converse.
- **Modified**: *p* was present in both, and kind, length, SHA-256 or
  symlink target differ, with both values recorded.
- **No effect on *p***: *p* was observed in both states, and its
  observable state is equal (or it was absent both times).
- Each claim was produced by comparing two live scans made by this
  process. It cannot be produced from stored or oracle data.
- The same pair of states always yields the same normalized delta and
  digests.

Rollback now also guarantees:

- It never writes through a symlink.
- It removes task-created symlinks and special files.
- It restores file ↔ directory replacements and deleted empty
  directories.
- It verifies backups against the manifest *before* mutating anything.
- It no longer rewinds the trace.

## Non-guarantees

These cannot be inferred from before/after state, and the receipt does
not claim them:

- **Causation.** The effects are those of the observation *window*.
  Anything else running then is included, such as a daemonized
  grandchild (`kill_on_drop` kills only the direct child) or another
  process. Changes made after the post-scan by lingering children land
  in a later receipt or in none.
- **Events.** There are no intermediate states, no counts, and no
  "touched". An identical rewrite produces no effect. Create-then-delete
  within the window produces nothing. A rename shows as `Deleted` plus
  `Created` with equal hashes; no rename is claimed.
- **Absence of change for excluded paths.** Writes into `.v9r/` or the
  root `trace.jsonl` are outside the scope, not "verified unchanged". A
  test pins this blind spot. Because both paths are inside the
  agent-writable workdir, the task could also rewrite trace records or
  checkpoint manifests without that showing up anywhere. Backup and
  manifest *inconsistency* is caught; a consistent forgery is not.
- **Anything outside the root**, including writes through hardlinks
  seen only from outside, `$HOME`, `/tmp` and the network.
- **Metadata**: mode bits, ownership, timestamps and xattrs. (Rollback
  also does not restore mode bits; that is pre-existing and not
  addressed here.)
- **Atomicity of observation.** The scan is not a snapshot. dev/inode
  and mtime checks catch many concurrent changes and turn them into
  unknowns, but a hostile concurrent process can still race directory
  swaps in the gap between checks. Fully closing that needs
  `openat`/`fdopendir` traversal.
- **Semantic satisfaction** of the request. It is always `Unknown` in
  this milestone.
- **Records.** A `ReceiptRecord` read from a trace or bundle is a report,
  not a verified fact.

## Tests

82 → **125** tests (`cargo test --workspace`), all passing.

New: 7 state unit tests, 7 rollback regression tests, 1 bundle test,
4 diff unit tests, 21 integration tests in `v9r-core/tests/effects.rs`
(real coreutils commands through `run_task_step`, no shell), and
3 `compile_fail` doctests.

| Class | Test | Outcome × effect |
|---|---|---|
| A | `touch created.txt` | success, VERIFIED Created (len 0, empty-file hash) |
| B | `cat existing.txt` | success, no effects, `pre_state == post_state` |
| C | `cp missing.txt out.txt` | failure, no effects |
| **D** | `touch made.txt no-such-dir/x` | **exit 1, VERIFIED Created**, persisted side by side in the trace |
| E | `cp source.txt target.txt` | Modified with before/after len and hash |
| F | `rm victim.txt` | Deleted with before hash |
| G | `mv a.txt c.txt dest` | one receipt: 2 Deleted, 1 Created, 1 Modified |
| H | checkpoint → step → rollback | step receipt still says Created; rollback receipt says Modified/Deleted; final state == pre-task state; both records survive in the trace |
| I | checkpoint + trace writes | no effects; scope lists exclusions. Also: a write into `.v9r/` is invisible (blind spot, pinned) |
| J | same transition in two trees | identical records and JSON after normalizing the root |
| adversarial | identical rewrite, `rm -f` of an absent file, rename, `mkdir -p`, `rm -r`, symlink create (not followed), `chmod 000` → UNKNOWN (not Modified or Deleted), denied `../` command → `not_started`, effects outside declared writes flagged | |
| semantic | no oracle / `NoSemanticOracle` → Unknown; mock oracle → Semantic, `verified` unchanged | |

Tests that depend on permission denial skip their assertions when run as
root.

## Surprises / bugs

Found in the existing code, reproduced with failing tests first, then
fixed. Each has a regression test.

1. **Rollback erased execution history.** `trace.jsonl` sits in the
   workdir and was part of the snapshot, so rollback restored it to its
   checkpoint-time content. Every `CommandExecuted` and violation event
   logged during the failed task was lost; only `RollbackPerformed`
   survived. This is exactly the failure mode the prompt warns about.
   Fix: runtime-internal paths are excluded from observation.
2. **Rollback wrote outside the workdir.** Replacing a snapshotted file
   with a dangling symlink made `path.exists()` false, and `fs::copy`
   then followed the link and created its target outside the workdir.
   Fix: stray non-directories are unlinked first, and restores use
   `create_new` (`O_EXCL`).
3. **Bundles shipped checkpoint backups.** `.v9r/backups/**` was exported,
   including pre-task copies of files the task had *deleted*, such as a
   removed secret. Fix: the top-level `.v9r` is skipped on export.
4. **File → directory replacement was never rolled back** (the directory
   "exists").
5. **Task-created symlinks survived rollback**, including one masking an
   original file (which then was not restored either).
6. **Deleted empty directories were not recreated** on rollback.

Other findings, documented but not fixed (out of scope):

- **`v9r run -f script` cannot work.** `run_script` spawns `sh -c` with
  an absolute `cwd`, and both are refused by the runtime fence added in
  `6826a00`. Found by inspection. An end-to-end run was not possible here
  because the CLI treats a non-TTY stdin as a bundle pipe.
- **Checkpoint integrity depends on agent-writable storage.** `.v9r/`
  and `trace.jsonl` live inside the agent-writable workdir, so the task
  can tamper with checkpoints and history. This is the biggest remaining
  structural weakness.
- **Bundle exclusion mismatch.** Bundles drop *any* file named
  `trace.jsonl` at any depth, while observation excludes only the root
  one. A task file `sub/trace.jsonl` is therefore silently missing from
  bundles.
- **Test suite leaks temp directories.** The existing `vfs` and `adapter`
  tests leave about 16 directories in `/tmp` per run, each with backup
  copies. This is the likely reason `/tmp` was full at the start of this
  session. The new tests clean up after themselves; the rollback
  regression tests reuse the existing helper and inherit the leak.
- **Manifest format change.** Checkpoint manifest v1 (FNV-64) is no
  longer readable, so a checkpoint taken by an older binary cannot be
  rolled back by this one. Checkpoints are per-task and transient.

## Architecture consequences

The pattern that worked is: **domain observation → normalized state →
deterministic diff → `Verified` only via a live-observation pair**.
Extending it means one `Observation`/`diff` pair per domain. A generic
`trait StateDomain` is not introduced until a second domain exists.

- **Git.** A good fit. Refs, `HEAD`, the index and object IDs are already
  content hashes, so pre/post observation is cheap and exact. Effects
  would be ref moves and index changes. The object store is append-only
  and needs no diffing. Git semantics ("a commit that fixes X") stay
  semantic.
- **SQLite.** Observation would be the schema plus per-table ordered row
  digests, taken inside a read transaction to get a true snapshot, which
  is better than the filesystem can offer. File-level observation already
  sees "db file modified", which is verified but nearly useless;
  row-level deltas are the useful claim. Cost grows with table size, so
  it needs incremental digests.
- **External APIs.** Usually there is no authoritative pre/post read, so
  most effects would be UNKNOWN. HTTP status is COMMAND RESULT, never
  OBSERVED. VERIFIED is possible only where a read API is authoritative
  and read-after-write consistent. Rollback is generally impossible, so
  receipts become the only record, which makes it more important that
  they be unforgeable and stored outside agent reach.
- **MCP.** A tool call is an `Action`, the tool's response is an
  `outcome`, and its self-description is at best DECLARED. Verified
  effects are available only for resources v9r can observe itself (for
  example the workdir, if the server writes there). A tool that claims
  success yields no verified effect.

Across all domains the receipt shape holds: requested, declared,
outcome, verified, unknown and semantic stay separate. The expensive
part is honest observation per domain, not the receipt.

## Future JEV attachment point

JEV attaches only at `SemanticOracle::assess(requested, &receipt)`, and
its answer lands only in `EffectReceipt::semantic`. It would be useful
for:

1. **Request satisfaction.** "Does this delta satisfy *'fix the failing
   test in main.rs'*?" Rust can say `src/main.rs` changed from hash A to
   hash B; only interpretation can say whether that is a fix.
2. **Undeclared-effect triage.** `undeclared` is a deterministic list;
   JEV could rank entries as expected build artifacts versus suspicious
   writes. Rust computes the list, and JEV only annotates it.
3. **Content-level description** of `Modified` evidence, given the
   before/after bytes (which would need to be fetched from the
   checkpoint, since receipts store only hashes).

JEV cannot do the following:

- Resolve an `UnknownEffect`. Only another observation can.
- Promote anything to VERIFIED. There is no API for it, `Verified`
  cannot be constructed or deserialized outside `effect.rs`, and
  `SemanticEvidence` has no conversion into it.
- Feed a semantic assessment into a policy (commit, rollback, grant)
  without that policy's input type saying SEMANTIC.

*Rust establishes facts. JEV may later interpret meaning.*

## Decision

**A: filesystem effect receipts are a useful foundation.**

Grounds, from the implementation rather than preference:

- **It fit the existing architecture without restructuring.** One
  crawler was generalized, rollback was rebuilt on the same primitive,
  and the receipts reuse the existing trace and bundle transport
  unchanged. Integration surface: `run_observed_step`,
  `rollback_observed`, one `TaskEvent` variant.
- **All ten required behaviours (A–J) hold deterministically**, including
  the key case D, and the non-overclaiming cases (absent/absent, identical
  rewrite, rename, unreadable → UNKNOWN) behave as specified.
- **The VERIFIED boundary is enforced by the compiler**, not by
  convention.
- **Precise before/after comparison paid for itself immediately.** Doing
  it exposed six real defects in the existing transactional guarantees,
  two of them security-relevant (writing outside the workdir; leaking
  deleted files in bundles).

What would move this to **B**: making receipts trustworthy against a
hostile agent requires moving runtime state (`.v9r/` and the trace) out
of the agent-writable workdir, and scanning via `openat`. If either
proves invasive, for example because CLI, bundle import and adapter all
assume `workdir/trace.jsonl`, the verdict should be revisited. So should
it if double full-tree hashing per step is too slow on realistic
workdirs (a Cargo `target/` directory, for instance).

## Most informative next experiment

Wire `run_observed_step` into the adapter's LLM loop, and add a receipt
for `<write>` actions. Then run a batch of real agent tasks and measure:

- how often the outcome and the observed effects disagree (classes B
  and D);
- how often effects fall outside the declared writes;
- the per-step observation cost.

The results show whether the returned-vs-changed distinction matters in
practice or is only theoretically clean, and where the observation cost
ceiling is. That decides whether runtime state has to move out of the
workdir before anything semantic is built on top.
