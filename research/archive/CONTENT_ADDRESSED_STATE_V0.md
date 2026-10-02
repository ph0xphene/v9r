# Content-Addressed State v0: turning filesystem reality into git objects

Research question: **can "a trusted observer says path X contains
content Y" be replaced by "the runtime creates a verifiable snapshot
object of observed reality", so that snapshot creation is the only
trusted step and everything after it is pure verification?**

Short answer: **yes. The snapshot *is* a git tree.**

- **One trusted step.** A primitive observer transcribes single system
  calls, and a deterministic verifier builds blobs and trees in git's
  object format with SHA-256 ids. That creation step is the only one
  that touches reality.
- **Identical to git.** A snapshot of `dist/` has exactly the root id
  git computes for the same content in a SHA-256 repository (tested).
- **Everything after creation is pure verification**, by the same code
  that verifies git objects. Snapshot-to-commit equality trusts no
  observer.
- **Observer trust shrank** from "understands the filesystem" (walks,
  hashes, normalizes, decides what counts) to "transcribes `readdir`,
  `lstat`, `read`, `readlink` faithfully, completely, now".
- **It did not shrink to zero.** One observer that drops a hidden *file*
  still gives a false ALLOW. A dropped hidden *directory* is caught by an
  independent check (the directory's link count). A dropped file is
  caught only by a second, independent observer.

`kernel.rs` and `runtime.rs` are unchanged.

## Baseline

| | |
|---|---|
| commit | `7224ee5` (verifiable observers report), clean tree |
| tests | **262 passing, 2 ignored** |
| `kernel.rs` | sha256 `85badb66…6177f`, unchanged since `bfebebe` |
| fs trust before | the fs observer walked the tree, hashed files and reported raw listings, bytes and targets; a verifier derived the digest, but listing completeness, freshness and correspondence were the observer's |

Environment: the temp directories are on ext4, where a directory's link
count is 2 plus its subdirectories. Tests run as a non-root user.

## Design

```text
 TRUSTED OBSERVATION (once)                           PURE VERIFICATION (any time, anyone)
 ────────────────────────────                         ─────────────────────────────────────
 RawFsObserver  fs_dir(p)   readdir + lstat mode  ┐
                fs_stat(p)  lstat nlink           │   FsSnapshot (verifier)
                fs_file(p)  read                  ├─▶  walks, checks nlink = 2 + subdirs,
                fs_link(p)  readlink              ┘    builds  blob <len>\0<bytes>
                                                               tree <len>\0(<mode> <name>\0<id32>)*
                                                       SHA-256 ids, git order, no empty trees
                                                       snapshot(dir) = root id ───────────────┐
                                                                 │ objects                    │
                                                       ObjectStore (untrusted provider)       │
                                                                 │ snap_object(id)            │
                                                       SnapshotObjects ◀─────────────────────┘
                                                         snapshot_valid(root)    every object checked by id
                                                         snapshot_digest(root, def)
                                                       MatchesSnapshot(def)
                                                         matches_snapshot(dir, approved)
                                                       SnapshotCommitEquality
                                                         snapshot_matches_commit(root, repo, X)
```

| Piece | Lines | Trusted for |
|---|---|---|
| `fs_raw.rs`: `RawFsObserver` | 95 | transcribing one call for one named path: no walking, hashing, normalizing, or following symlinks in path components |
| `snapshot.rs`: `FsSnapshot` | ~130 | building objects from transcriptions; the link-count check; git's ordering and modes |
| `snapshot.rs`: the other verifiers | ~180 | pure functions of stored objects |
| `verifiers.rs` `walk_tree` | shared with git | walking a content-addressed tree, checking every object by its id |
| `ObjectStore` | — | **nothing**: what it serves is checked by hash |

The invariant ("artifact matches approved snapshot") is
`matches_snapshot(dist, A) = true`. Here `A` is the root id taken at
approval, an identity, not a path.

Two definitions of "the same content":

- **`git-tree-sha256/1`**: identity of root ids, so names, content
  *and* file modes count, as in git.
- **The content manifest**: modes do not count.

## Results

**272 passing, 2 ignored**: 262 plus 10 in `tests/content_addressed.rs`.

| Case | Expected | Result | How |
|---|---|---|---|
| artifact matches approved snapshot | ALLOW | **ALLOW** | `snapshot(dist)` "trusts provider:fs"; `snapshot_valid(A)` "trusts no observer" |
| file modified after snapshot | DENY | **DENY** | the root id differs. The approved snapshot is still fully verifiable from its objects: it no longer depends on the directory it came from |
| a provider claims `snapshot(dist) = A` | — | **DENY** | claim demoted and recomputed; "derivation mismatch" |
| snapshot incomplete: unreadable file | BLOCKED | **BLOCKED** | no observation of `fs_file(dist/src/lib.txt)` |
| snapshot incomplete: approved snapshot lost or altered an object | BLOCKED | **BLOCKED**, once no live copy of that content exists | "object … not supplied" / "no supplied bytes hash to …" |
| …while the live directory still has that content | — | **ALLOW** | **content-addressed repair**: re-snapshotting regenerates the identical object |
| observer omits a hidden **directory** (one observer) | BLOCKED | **BLOCKED** | "link count 5 implies 3 subdirectories, listing shows 2" |
| observer omits a hidden **file**, one observer | BLOCKED | **false ALLOW** | nothing in a directory counts its files |
| observer omits a hidden file, plus an independent observer | BLOCKED | **BLOCKED** | "contradictory observations of fs_dir(dist)" |
| two definitions disagree (a file becomes executable) | BLOCKED | **BLOCKED**; identity alone gives DENY, manifest alone gives ALLOW | contradictory verified facts |
| snapshot vs. git, SHA-256 repo | — | **root id = `git rev-parse HEAD^{tree}`** | same objects |
| snapshot vs. git commit (SHA-1 and SHA-256) | — | `snapshot_matches_commit = true`, trusting only verifiers | pure |

Mutation checks. Each fails a specific test:

| Mutation | Fails |
|---|---|
| no link-count check | hidden directory |
| plain name order instead of git's (a `src.txt` beside a `src/` makes it matter) | git equivalence |
| keeping empty trees | git equivalence |
| no hash check | lost/altered object |

## Evaluation

### Did observer trust shrink from "understands the filesystem" to "can only read bytes correctly"?

**Mostly. What was removed, and what remains, is now exact.**

**Moved out of the observer**, into deterministic, testable code that
reproduces git's ids:

- recursion;
- deciding what counts: empty directories, modes, symlinks, special
  files;
- hashing;
- normal form;
- object construction;
- identity.

The observer went from a tree walker, hasher and normalizer of about
500 lines (provider, walk, capture) to a 95-line transcriber.

**Still the observer's word.** "Can only read bytes correctly"
understates it by three properties:

1. **Completeness of `readdir`.**
   - Hidden *subdirectories*: checked against `lstat`'s link count. This
     is independent of the listing, but holds only on filesystems that
     keep the `2 + subdirectories` rule (ext4, tmpfs; not btrfs, where it
     is 1 and the check is skipped).
   - Hidden *files*: no OS invariant counts them. One observer that
     drops a file is believed; only an independent second observer turns
     the omission into BLOCKED.
2. **Simultaneity.** A snapshot is built from many calls, one per
   directory and file, at different instants. A concurrent writer can
   produce a snapshot of a state that never existed. This was not
   tested here, and nothing in the walk detects it.
3. **Correspondence.** The calls really address `root/dist/…` on this
   machine. The observer refuses symlinked path components, which is a
   check-then-use, not an atomic guarantee.

So the residual trust is: **the OS primitives were transcribed
faithfully, completely and at one moment.** That is "can read
correctly", with completeness and atomicity added.

### Does this make the filesystem closer to git's trust model?

**It makes it *the same* model, and shows where git's own trust
begins.**

- **The same objects.** A snapshot is a git tree, with identical bytes
  and an identical id.
- **Identical properties after creation:**
  - untrusted stores are safe;
  - a wrong byte fails its hash;
  - a missing object is BLOCKED, never DENY;
  - an approved state stays verifiable after the source changes;
  - identical content repairs lost objects.
- **No observer between the domains.** Relating a filesystem snapshot to
  a git commit is pure verification, with no observer on either side.

The experiment also shows that **git has an observer too**:

- `git add` reads the working tree, exactly as `FsSnapshot` does.
- Git's trust model *starts after hashing*.
- Converting filesystem reality to content-addressed reality therefore
  does not remove the observer. It **confines it to ingestion**, the
  same place git puts it.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | "Observer omits hidden file" can be BLOCKED by verification | **False with one observer.** It is detectable only through an independent witness (a second observer), or an OS invariant that counts entries. Subdirectories have one (link count); files do not |
| 2 | Losing an approved snapshot's object always makes it unverifiable | **False.** Content addressing repairs it whenever the same content exists anywhere, including the live directory. That is correct: the object provably exists |
| 3 | After snapshot creation, nothing is trusted | **True for facts about the snapshot.** But the verifiers (and the definition choice: identity vs. manifest) are trusted code, and a weaker definition is believed |
| 4 | Creation is a single observation | **False.** It is many calls over time. Atomicity is an unverified assumption |
| 5 | Verifiers are free of side effects | **Bent.** `FsSnapshot` writes objects into the store while deriving. This is harmless (content-addressed, idempotent), but snapshot creation is not purely functional |

## Decision

**Filesystem reality can be converted into content-addressed reality,
and the trust boundary moves exactly where git's is: ingestion.**

- **The question as asked:** yes, observer trust shrank from
  "understands the filesystem" to "faithfully transcribes system calls".
  Everything after snapshot creation is pure verification, and the
  snapshot is literally a git tree.
- **The remainder is not zero, and it is precise:**
  - completeness of directory listings (files, not subdirectories);
  - atomicity of a multi-call walk;
  - correspondence of paths.

  None of these is verifiable from the transcriptions themselves.

## Next falsification experiment

**Atomicity of ingestion: can the verifier detect a snapshot of a state
that never existed?**

1. Run a concurrent writer during `FsSnapshot`'s walk. For example, it
   moves a file from `a/` to `b/` between the two directories' listings,
   so the snapshot contains it twice or not at all.
2. Try verifier-side detection that trusts the observer no further:
   - a second walk compared by root id (equal ⇒ no change visible
     across both walks);
   - re-checking each directory's `lstat` (link count, mtime/ctime)
     after its children were read.

- **Supports the claim that ingestion needs only faithful transcription:**
  the double walk or re-stat catches every injected interleaving that
  changes content, at a bounded cost.
- **Falsifies it:**
  - there are interleavings that both checks miss (ABA within the
    window, timestamp granularity); or
  - the checks need the observer to report something it cannot report
    faithfully.

  Either result would establish that atomic ingestion needs an OS
  mechanism (filesystem snapshots, freezing) and is irreducibly trusted.

CI history and external integrations remain deferred.

## Commits

| Commit | Content |
|---|---|
| `ab10b0d` | `walk_tree` over any content-addressed object source (refactor, no behaviour change) |
| `3ccbd7b` | `fs_raw.rs` primitive observer; `snapshot.rs` (creation, store, pure verifiers) |
| `c167b7b` | the experiment and attacks |
