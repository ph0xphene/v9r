# Atomic Capture v0: can a snapshot be shown to represent a state that existed?

Research question: **can v9r capture filesystem state atomically, or
must snapshot ingestion remain trusted?**

A snapshot is built from many system calls at different instants. A
writer running at the same time can make the result a mix of states,
**a tree that never existed**. The experiment mutates the directory
adversarially during capture and judges each result against ground
truth.

Short answer: **without an OS mechanism, no.**

- **Detection works against careless writers, not adversaries.**
  - **A** (walk twice) and **B** (`lstat` before and after) catch every
    naive interleaving.
  - **W** (an inotify change journal, OS-assisted *detection*) catches
    those too.
  - Each has a structural blind spot an adversary can aim at:
    repetition (A), the 1 ms timestamp tick (B), writes through a hard
    link from outside the watched tree (W).
  - **All three together still accepted a false snapshot**, on the
    first or second attempt.
- **Quiescence works, but only across a privilege boundary.** The cgroup
  v2 **freezer**, available unprivileged here, gave **200/200 true
  snapshots** with a plain single walk while the writer's scope was
  frozen. But any same-user process can move itself out of the scope
  first, and then false snapshots return at the unfrozen rate.
- **An atomic OS snapshot is not available** on this host. The root
  filesystem is ext4; `fsfreeze` is refused, even in a user namespace;
  there is no btrfs or LVM access.

So **v9r needs an atomic capture primitive**: an OS-level snapshot, or
quiescence enforced across a privilege boundary. Without one, ingestion
stays trusted, and "trusted" now has a precise meaning.

Along the way, a **correspondence defect** in the primitive observer was
found and fixed. Under a racing swap of a directory for a symlink, it
read *outside the root* (3 in 200,000). fd-relative system calls closed
it (0 in 600,000).

`kernel.rs` and `runtime.rs` are unchanged.

## Baseline

| | |
|---|---|
| commit | `fe675e4` (content-addressed state report), clean tree |
| tests | **272 passing, 2 ignored** |
| `kernel.rs` | sha256 `85badb66…6177f`, unchanged since `bfebebe` |

The host:

| Property | Value | Consequence |
|---|---|---|
| kernel | Linux 6.12.80, `CONFIG_HZ=1000` | file timestamps are coarse (1 ms): **1863 of 2000** back-to-back writes left `ctime` unchanged |
| filesystem | ext4 (temp dirs on `/`) | directory link counts keep `2 + subdirectories`; no snapshots |
| privileges | uid 1000 | `fsfreeze` refused (also in a user namespace); no btrfs; no LVM |
| user namespaces | available | no snapshot capability in them |
| cgroups | v2, delegated systemd user scopes | **the freezer works unprivileged** |

## Method

### Ground truth

A capture is a **false snapshot** iff its root id is not the snapshot of
any state the directory actually passed through.

- **Deterministic cases.** The mutations are logged, replayed on a
  scratch copy, and every intermediate state is snapshotted.
- **Concurrent writer.** The writer cycles through ten **atomic**
  operations (write aside and rename into place; retarget a symlink by
  renaming a new one over it), so the states it visits are exactly the
  ten after each operation.

The first version of the writer used non-atomic writes (`fs::write`
truncates and then writes). Real transient states (an empty file, a
missing link) were then absent from the oracle, and the measurement
reported impossible results (W "accepting false snapshots" while seeing
no events). **Ground truth must include the writer's own atomicity.**

### Adversary

**Deterministic interleavings.** `Interleaver`, an observer, applies
mutations to the real directory immediately before answering chosen
requests:

- requests are matched by kind, path and reading tag;
- in a torn read, the file is rewritten between the two halves of the
  read.

This makes every interleaving exact and repeatable. The mutations are
real filesystem operations; only their timing is chosen.

**Concurrent processes.** A writer thread, and a writer *process* in its
own systemd scope (for the freezer).

### Strategies

All strategies are combinable `Capture` settings of the snapshot
verifier (`snapshot.rs`):

| | Strategy | Mechanism |
|---|---|---|
| plain | one walk | — |
| **A** | second verification walk | walk again with fresh observations (reading tag `w2`); require the same root id |
| **B** | metadata before/after | `lstat` every directory before listing it and every entry before reading it; `lstat` all again after the walk; require identical inode, mode, size, link count, mtime, ctime. **Every value used must come from inside the bracket**: modes are taken from the bracketing `lstat`, not the earlier listing (see *Failed assumptions*) |
| **W** | change journal (OS-assisted detection) | `inotify` on every directory, via its opened descriptor, *before* listing it; require zero change events (and no overflow) until capture ends. Implemented as `fs_watch.rs`, the first deliberately **stateful** provider: a journal needs a session |
| **C** | OS-assisted quiescence | cgroup v2 freezer on the writer's scope during a plain capture |

## Results

### Deterministic interleavings

Tests: `tests/atomic_capture.rs`, 9 tests, stable over repeated runs.

| Case | plain | A | B | W |
|---|---|---|---|---|
| **file content changes while reading** (torn read of a 64 KiB file) | **FALSE** | blocked | blocked | blocked |
| **file deleted during snapshot** (listed, then removed) | blocked | blocked | blocked | blocked |
| **directory renamed during traversal** (`sub` moves into an already-listed directory: it ends up nowhere) | **FALSE** | blocked | blocked | blocked |
| …by an **adaptive** adversary (moves it back before walk 2, repeats) | — | **FALSE** | blocked (A+B) | blocked (A+W) |
| **permission change during traversal** (chmod a listed file, then change a file not yet read) | **FALSE** | blocked | **TRUE**: the post-change state, which existed | blocked |
| **symlink target change** (retarget a read link, then change a file not yet read) | **FALSE** | blocked | blocked (the parent directory changed) | blocked |
| same-tick change (x changed just before its `lstat` and again just after its read, within 1 ms; then an unbracketed change elsewhere) | — | — | **FALSE** (attempt 1–2) | blocked |
| writes through **hard links** from an unwatched directory | — | blocked | blocked (`ctime` is per inode) | **FALSE** |
| **combined**: same tick + hard links + adaptive repetition | — | — | — | **A+B+W: FALSE** (attempt 1–2) |

Deletion is blocked by construction: a listed entry without content
leaves the snapshot incomplete. Every other case produces a false
snapshot under plain capture.

Mutation checks. Each change below fails the named tests:

| Mutation | Fails |
|---|---|
| B takes modes from the listing | permission change |
| no recheck | 5 tests |
| no second-walk comparison | 5 tests |
| no event check | 6 tests |

### A continuous concurrent writer

`continuous_writer`, ignored (timing-dependent). A thread cycles through
atomic operations as fast as it can; 300 captures per strategy, three
runs:

| Strategy | false snapshots (of 300) | true | blocked |
|---|---|---|---|
| plain | **55 – 160** | 0 – 3 | rest |
| A | **0 – 2** | 0 | rest |
| B | **0 – 1** | 0 – 1 | rest |
| W | 0 | 0 | 300 |
| A+B+W | 0 | 0 | 300 |

Even a **non-adaptive** writer defeats:

- **A** occasionally: a cyclic writer reproduces the same mix in both
  walks;
- **B** occasionally: an entry changed and changed back (ABA) within one
  timestamp tick.

Detection strategies trade completeness for availability: against a
busy writer almost every capture is blocked.

### OS-assisted quiescence (the freezer)

`freezer_quiescence`, ignored (needs a systemd user session with
cgroup delegation). A fast writer process runs in its own
`systemd-run --user --scope -p Delegate=yes`. Two runs, 200 plain
single-walk captures per row:

| Writer | true | false | blocked |
|---|---|---|---|
| not frozen (baseline) | 0 – 1 | **28 – 39** | rest |
| scope frozen during each capture | **200** | **0** | 0 |
| scope frozen, but the writer first moved itself to a sibling cgroup | 0 – 1 | **20 – 44** | rest |

Freezing gives exactly what atomic capture needs: no writer runs, so
the walk observes one state. Yet a same-user process may migrate itself
to any cgroup in its user's delegated tree. Two migrations were
verified: into a sibling it creates itself, and into the observer's own
cgroup.

The emptied scope is then **garbage-collected by systemd**: the runtime
freezes nothing and believes it froze the writer.

### The observer's correspondence

`observer_correspondence_race`, ignored. `dist/z` flips between a
directory and a symlink to `outside/` while `fs_dir(dist/z)` is
observed:

| Observer | listings | listings showing `outside/` |
|---|---|---|
| path-based (`lstat(path)`, then `read_dir(path)`) | 29,769 of 200,000 attempts | **3** |
| fd-relative (`openat(O_NOFOLLOW)` per component; enumerate the opened descriptor; `fstatat`, `readlinkat`) | ~214,000 of 600,000 attempts | **0** |

The previous milestone's claim ("never follows a symlink in path
components") held only for the check, not the use. It is now true of
every call. The inotify watcher was changed the same way: it watches
the opened descriptor, not a path.

## Evaluation

### Can false snapshots be detected?

**Against a careless writer, yes. Against a same-user adversary, no
detection strategy suffices.**

| Strategy | Detects | Blind to |
|---|---|---|
| A: second walk | any change that makes the second walk differ | an adversary that **repeats** the interleaving (or a cyclic writer that does by chance) |
| B: metadata before/after | any change that alters an entry's `lstat` while it is bracketed | changes inside **one timestamp tick** (1 ms here; 93% of quick double writes are invisible), ABA, and in general writes through shared mappings, whose mtime update is deferred |
| W: change journal | any change through a watched directory, independent of timestamps | writes through a **hard link** whose parent is not watched; writes through shared mappings (inotify does not report them); changes before a directory is watched (harmless by construction) |
| A+B+W | the union | the intersection, which is non-empty: shown by construction |

Each strategy *infers* "nothing changed" from a signal that is coarser
than the change:

- a second sample (A);
- a timestamp (B);
- per-directory events (W).

The adversary chooses changes that do not move the signal.
**Detection can raise the cost of a false snapshot. It cannot certify
that one did not happen.**

### What remains trusted?

| Mechanism | Trusted for |
|---|---|
| plain capture | that nothing changed during the walk |
| A | that no writer repeats its interleaving |
| B | that every change moves an entry's `lstat` (coarser than reality) |
| W | that every change passes through a watched directory, by a means inotify reports |
| freezer | **that every writer is in the frozen cgroup and cannot leave it** |
| (all) | the observer's transcription: now fd-relative, so correspondence is no longer a trust item |

The freezer is the only mechanism whose residual trust is a single,
checkable **containment property**: the writer's cgroup membership.
That property holds only if the writer cannot migrate, which takes:

- **a different user** for the agent's processes;
- or a cgroup namespace whose ancestors it cannot write;
- or a runtime-owned, non-delegated cgroup.

Today, v9r runs commands as the same user, waits only for the direct
child (`kill_on_drop` + `output()`), and contains nothing else. So a
descendant that detaches from the pipes outlives the step and is
exactly the concurrent writer modelled here.

### Does v9r need an atomic capture primitive?

**Yes**, if snapshot claims are to mean "a state that existed" against
anything but a cooperative environment. The options, from strongest to
weakest:

1. **Filesystem snapshot** (btrfs/ZFS subvolume snapshot, LVM thin
   snapshot, overlay with a frozen lower layer). Atomic by the kernel's
   guarantee; trusted only as far as the filesystem. **Unavailable on
   this host** without privileges or a different filesystem.
2. **Quiescence across a privilege boundary.** Run agent effects in a
   runtime-owned cgroup the agent cannot leave (separate uid or a
   non-delegated cgroup), and freeze it for capture. **Shown to work**
   here (200/200), and **shown to fail** without the boundary.
3. **Avoid live capture.** Have effects write directly into the
   content-addressed store (outputs born content-addressed), so nothing
   live is ever captured. This needs effects to cooperate with the
   runtime.
4. **Detection (A, B, W).** A fallback that turns most races into
   BLOCKED rather than false, never a guarantee. W+B is the strongest
   pair measured; even A+B+W was defeated by a constructed interleaving.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | A second identical walk shows the state was stable | **False.** An adaptive adversary repeats the interleaving, and a cyclic writer does it by chance (0–2 per 300) |
| 2 | `ctime` reveals every change | **False at 1 ms granularity.** 93% of quick double writes leave it unchanged; ABA within a tick is invisible |
| 3 | Bracketing every entry with `lstat` is enough | **Only if every value comes from inside the bracket.** Taking modes from the earlier directory listing left a chmod between listing and `lstat` invisible. The first version had this defect; it was fixed before measurement |
| 4 | inotify on every directory sees every change in the tree | **False.** Events follow the path used, not the inode: a write through a hard link elsewhere is silent |
| 5 | Freezing the writer's cgroup freezes the writer | **Only if it cannot leave.** A same-user process can migrate itself, and systemd then removes the emptied scope |
| 6 | The observer never follows symlinks | **False before this milestone.** Check and use were separate path resolutions, and 3 in 200,000 listings escaped the root. Fixed with fd-relative calls |
| 7 | A concurrent-writer measurement needs only the writer's visible states | **False.** The oracle must know the writer's atomicity: non-atomic writes create real transient states, and an oracle without them reports nonsense |
| 8 | Providers can stay stateless | **Not for capture.** A change journal is a session (W); quiescence is a held state (frozen). Atomic capture is inherently a stateful, bracketed operation |

Two harness bugs were found and fixed during the experiment; they are
recorded because they shaped the method.

- **`systemd-run` expands `$$` on its command line.** Early escape probes
  wrote the literal `$` into `cgroup.procs` and "showed" escape to be
  impossible. They were redone with script files.
- **The first freezer writer (a shell loop) was too slow** to race a
  capture. Without a no-freeze baseline its "200/200" would have meant
  nothing. It was replaced by a Python loop, and the baseline row
  added.

## Complexity

| Piece | Code lines | Notes |
|---|---|---|
| `fs_raw.rs` | 172 | fd-relative rewrite (was 95 path-based) |
| `fs_watch.rs` | 113 | inotify sessions (stateful provider) |
| `snapshot.rs` | 516 (was 314) | strategies A, B, W, combinable; bracketed modes |

- **Cost of detection.** A doubles the walk. B adds one `lstat` per
  entry twice. W adds one watch per directory and one read of the event
  queue.
- **Availability.** Against a busy writer, detection blocks almost
  every capture: it buys soundness against carelessness, never
  liveness.

## Decision

**Ingestion cannot be made atomic by verification. v9r needs an atomic
capture primitive from the OS.**

- **What verification-side strategies can do** (second walk, metadata
  bracket, change journal): turn ordinary races into BLOCKED.
- **What they cannot do:** certify that a captured tree existed. Each
  infers stability from a signal coarser than the change, and a
  same-user adversary aims below that resolution. All three together
  were defeated by construction.
- **The two OS mechanisms:**
  - **Filesystem snapshots** are unavailable here.
  - **The cgroup freezer** is available and *works* (200/200 true with a
    single plain walk). It reduces the trust in ingestion to one
    property: every writer is inside the frozen cgroup and cannot leave.
    In v9r today that property is false: agent processes run as the
    same user, nothing contains their descendants, and a same-user
    process can migrate itself.

## Next falsification experiment

**Contained effects: can v9r run agent commands so that freezing them
is sound?**

1. Run every guarded command in a runtime-owned cgroup the agent cannot
   leave:
   - a separate uid;
   - or a user and cgroup namespace whose ancestors are not writable to
     it.
2. Freeze that cgroup around snapshot capture.
3. Kill everything in it at the end of a step.
4. Rerun this suite's continuous writer and escape attempts as agent
   processes:
   - migration to a sibling cgroup;
   - into the runtime's own cgroup;
   - daemonizing past the step;
   - `setsid`, double fork.

- **Supports "quiescence across a privilege boundary is an atomic
  capture primitive":**
  - every escape fails;
  - every capture under freeze is true;
  - an orphaned writer cannot outlive the step.
- **Falsifies it:**
  - some escape works without privileges (a cgroup/namespace gap); or
  - a writer the runtime cannot freeze exists, such as an out-of-band
    process of the same user, a shared mapping, or another machine via
    a network filesystem.

  Either result would leave filesystem snapshots as the only sound
  primitive.

CI history and external integrations remain deferred.

## Commits

| Commit | Content |
|---|---|
| `ff008a3` | fd-relative primitive observer (correspondence fix); `fs_meta`; reading tags |
| `96fb768` | capture strategies A, B, W (`fs_watch.rs`); bracketed modes |
| `b9126f7` | deterministic interleavings, continuous writer, correspondence race |
| `d09ee1e` | cgroup freezer quiescence and escape |
