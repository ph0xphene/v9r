# Capability Content Identity v0 (X1b): can a read-only directory capability be bound to a snapshot?

Research question: **can v9r grant authority over a directory snapshot
instead of a mutable namespace?**

Object Identity v0 (X1) showed:

- a file grant can attach to its object (`st_dev` + file handle);
- a directory grant cannot. The directory is one object, but what it
  contains is a namespace that changes, and a hard link can bring a
  foreign object into it without changing the directory's identity.

This experiment grants read access to a directory **by content**: the
git tree id of a snapshot taken in the grantor's world. It then gives
the grantee that content in two ways:

- **live**: the directory itself, verified against the snapshot;
- **materialized**: a read-only copy the runtime checks out of the
  snapshot's objects.

Short answer: **yes, but only when the view is built from the
snapshot.**

- **Content identity names the grant correctly.** It allows the same
  content anywhere (copy, rename, identical recreation). It denies any
  change to content, names or file modes.
- **Verifying a live directory against it is not enough.**
  - It allowed **2 of the 6 changes that must be refused**: an added
    empty directory, and a file replaced by a hard link to the grantor's
    private file holding identical bytes.
  - A git tree records neither empty directories nor which inode a name
    points to.
- **Materialized from the snapshot, the grantee's view was exactly S in
  9 of 9 cases.**
  - It had no extra name.
  - It shared no inode with anything of the grantor's.
  - It still read the original bytes after the source was modified or
    deleted.
  - Every write was refused (`EROFS`).
  - A tampered object in the store stops the checkout before anything is
    written.
- **Object and content capabilities are different things.** Each pin is
  wrong exactly where the other is right, and pinning both is wrong in
  4 of 9 live cases. v9r needs two capability kinds. They differ not in
  the grant record (both are `identities` pins) but in **how the runtime
  realizes them**:
  - an object grant: bind the object, check its handle;
  - a snapshot grant: materialize the tree, check its root.
- **No kernel change.** The content pin is the existing
  `D9.object_identity` `Fact`, with a `tree:` value. `kernel.rs` sha256
  `85badb66…6177f`, unchanged.

## Baseline

| | |
|---|---|
| tree | `8a49c4b` plus the uncommitted capability work through X1 |
| tests before | 311 passed, 6 ignored |
| host | Linux 6.12.80, unprivileged; temp dir on ext4 |
| `kernel.rs` | sha256 `85badb66…6177f` |

## What was built

| Piece | Change |
|---|---|
| `world_probe.py` | `transcribe(dirs)`: raw observations inside the world, in `fs_raw`'s formats (`fs_dir` name → git mode, `fs_stat` nlink, `fs_file` bytes, `fs_link` target). It interprets nothing |
| `capability.rs` | `ProbeExtra.transcribe`; `Observation::transcript()` → snapshot inputs |
| `snapshot.rs` | **`snapshot_from(store, transcript, dir)`**: drives the *unchanged* `FsSnapshot` verifier, including its link-count completeness check, over a transcript made elsewhere. **`ObjectStore::get_verified`**: an object only if its bytes hash to its id. **`materialize(store, root, dest)`**: loads the whole tree, checking every object, *then* writes files `0444`/`0555` and symlinks |
| `object_identity.rs` | `content_id(root)` = `tree:<root>`; `content_evidence(obs, [(key, dir)])` attests `object(key) = tree:<root>` from the grantee world's own transcription (`world-probe+fs-snapshot`) |
| `tests/content_identity.rs` | 9 cases × 2 arms, 27 worlds in 3.0 s; a tamper test |

**The flow:**

```text
world A (grantor)                runtime                               world B (grantee)
binds D, transcribes D ──▶ S = snapshot_from(A's transcript)
                           objects kept in its ObjectStore
                           signed grant: read D, identities pin
                               Object:  {D: fh:<D's handle>}
                               Content: {D#tree: tree:S}
                               Both:    both
                           ── live:         bind the (changed) host dir at /v/d ──▶ resolve + transcribe /v/d
                           ── materialized: materialize(S) → bind at /v/d ────────▶ resolve + transcribe /v/d
                           D9 pins judged on B's own observations (handle; tree root)
```

S is the grantor's view (X1: identities come from the grantor's world).
The tree id is git's: the same objects the Content-Addressed State v0
snapshot builds.

## Results

```
cargo test --offline -p v9r-core --test content_identity -- --nocapture --test-threads=1
2 passed (27 worlds, 3.0 s)
cargo test --offline --workspace --no-fail-fast
313 passed, 0 failed, 6 ignored   (two runs)
```

D is `{a.txt: "alpha", sub/b.txt: "beta", l → a.txt}`. The grantor
also has a private directory `{secret, alpha}`, where `alpha` holds the
same bytes as `a.txt`. Each case changes the host after the grant.

### Live arm: the changed directory, verified

Ground truth for this arm: may B see this view? It may only if the view
is exactly what was granted. **✗** marks a wrong verdict.

| # | Case | B may see it? | B's root | Object pin | Content pin | Both | What B's view exposes |
|---|---|---|---|---|---|---|---|
| 1 | same content, different path (`cp -a`) | yes | = S | Deny ✗ | **Allow** | Deny ✗ | |
| 2 | same path, modified content | no | ≠ S | Allow ✗ | **Deny** | Deny | the new bytes |
| 3a | file added after grant | no | ≠ S | Allow ✗ | **Deny** | Deny | `/new.txt` |
| 3b | **empty directory** added after grant | no | **= S** | Allow ✗ | **Allow ✗** | Allow ✗ | the name `/secret-plans` |
| 4a | hard link to a private file added | no | ≠ S | Allow ✗ | **Deny** | Deny | `/s`, an inode of A's |
| 4b | **file replaced by a hard link to identical private bytes** | no | **= S** | Allow ✗ | **Allow ✗** | Allow ✗ | `a.txt` is now A's private inode |
| 5 | rename | yes | = S | Allow | **Allow** | Allow | |
| 6a | delete and recreate, different content | no | ≠ S | Deny | **Deny** | Deny | |
| 6b | delete and recreate, identical content | yes | = S | Deny ✗ | **Allow** | Deny ✗ | |

**Wrong verdicts on the live arm:**

| Pin | Wrong |
|---|---|
| Object | 7 of 9 |
| Content | **2 of 9**: 3b and 4b |
| Both | 4 of 9 |

### Materialized arm: S checked out, read-only

In every case, whatever happened to D:

| Measurement | Result |
|---|---|
| B's root | **= S** in 9 of 9 |
| extra names in B's view | **none** in 9 of 9 |
| inodes shared with the grantor's private files | **0** in 9 of 9 (4b's live view shared 1) |
| read `a.txt` | `ok:8ed3f6ad…` (sha256 of `alpha`) in 9 of 9, including after D was modified (case 2) or deleted (6a) |
| write `a.txt` | `EROFS` in 9 of 9 |
| Content pin | **Allow, correct** in 9 of 9 |
| Object pin | Deny in 9 of 9: the checkout is a new object, so a handle pin cannot describe it |

**Tamper test:** one blob in the store was overwritten.
`materialize` returned "stored bytes do not hash to it", and **nothing
was written**. The first version checked and wrote one level at a time,
which left a partial checkout. It was changed to load and check the
whole tree before writing.

## Evaluation

### Does content identity solve directory capability ambiguity?

**It solves naming. Only construction solves enforcement.**

- **As a name for what was granted**, `tree:S` is exact for everything a
  git tree records: names, bytes, symlink targets, and the executable
  bit. It is indifferent to everything else: which object, which path,
  which filesystem. That is why cases 1, 5 and 6b allow, and 2, 3a, 4a
  and 6a deny.
- **As a check on a live view**, it is blind to what the tree does not
  record, and a live view exposes exactly that:
  - **empty directories (3b)**: a name, an information channel, invisible
    to the root id;
  - **inode identity (4b)**: B's `a.txt` has the right bytes *now*, but
    it is the grantor's private file. Any later write by the grantor
    reaches B through a grant that was supposed to be immutable. The
    snapshot capability has silently become an object capability on a
    private object;
  - not exercised, by the same argument: ownership, non-executable mode
    bits, timestamps, xattrs and hard-link counts;
  - **time**: a live view is checked at one instant. After the check,
    the directory can change under B. That is Capability Manifest v0's
    "Time" and Atomic Capture v0, unchanged.
- **Materialization removes all of these by construction.** B's view
  *is* the tree:
  - empty directories cannot be in it;
  - every file is a fresh inode;
  - the source can change or vanish without effect.

  This is Capability Inventory v0's rule again: **construction, with
  observation as the check.** B's in-world root equal to S is the check;
  the checkout is the construction.

### Is a capability over mutable state fundamentally different from one over immutable state?

**Yes.** The measurements separate them along every axis. Each pin is
right exactly where the other is wrong (live arm: 1, 6b vs 2, 3a, 4a).

| | ObjectCapability (mutable) | SnapshotCapability (immutable) |
|---|---|---|
| authority over | one object, **over time** | one value |
| identity | `st_dev` + file handle (X1) | `tree:<git tree id>` |
| equal when | same object (rename, hard link: yes; copy, identical recreation: **no**) | same content (copy, rename, identical recreation: **yes**) |
| sees later changes | yes, by design | **never** |
| realized by | binding the object (live) | **materializing** the tree (constructed) |
| check | handle in the grantee's world (D9) | tree root in the grantee's world (D9) |
| source deleted | the authority is gone (6a/6b deny) | unaffected: the store has the objects |
| verifiable without an observer | no: "this object now" is the observer's word | **yes**, after creation (Content-Addressed State v0) |
| amplification by delegation | possible through what the object later contains | impossible: the value is fixed |
| revocation | needs enforcement on a live binding (Design X3) | stops future materializations; cannot undo a disclosure |

**Pinning both is not a stronger capability.** It is the intersection of
two different capabilities. It refused legitimate copies and identical
recreations (1, 6b), and it still allowed 3b and 4b, because the live
view's object (the directory) and its content were both unchanged.

### Does v9r need separate ObjectCapability and SnapshotCapability?

**Yes, as two kinds of realization. Not as two kinds of record or
kernel concept.**

- **The grant record and its checks need nothing new.** Both kinds are
  one `identities` entry pinned by `D9.object_identity`. They differ
  only in the value's scheme: `fh:` or `tree:`. Ledger, signatures,
  attenuation and the kernel are the same.
- **The difference is in what the runtime does with the grant:**

  | Kind | Pin | Runtime must | Runtime must not |
  |---|---|---|---|
  | ObjectCapability | `fh:<dev>:<handle>` | bind the object; check the handle in the grantee's world | — |
  | SnapshotCapability | `tree:<root>` | materialize from verified objects; check the root in the grantee's world | **satisfy it with a live bind**: that is the 3b/4b failure |

- Proposed rule for the realization step: **the identity scheme selects
  the realization.** A `tree:` pin is never realized by binding a host
  path, and a grant must not carry both kinds on one object. Both are
  runtime rules, not kernel ones.

### Does this require kernel changes?

**No.** Content pins are the existing `D9` `Fact(object(key) = value)`.
The tree root is derived by the unchanged `FsSnapshot` verifier from
in-world observations and attested like any other verified fact. Every
case used the full Root of Trust v0 decision with an unmodified kernel.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | Checking a live directory against a snapshot id enforces a snapshot grant | **False.** 3b (empty directory) and 4b (identical bytes through a private inode) pass. The root id does not record what the live view exposes |
| 2 | Pinning object *and* content identity is stricter, so safer | **False.** It is wrong in 4 of 9 live cases. It refuses legitimate copies and still allows 3b and 4b |
| 3 | A snapshot grant is "the same object, frozen" | **False.** Its identity is content, not an object. It survives the source's deletion, and it allows any copy with the same bytes. The checkout is a new object (the object pin denies it 9 of 9) |
| 4 | Materialization can check and write level by level | **False.** A tampered object deep in the tree left a partial checkout. It now loads and checks everything before writing |
| 5 | The materialized directory is immutable | **Not established.** It is an ordinary host directory owned by the runtime's uid. Its files are `0444` and its bind is read-only inside B, but **a same-uid host process can still change it after the check**. Nothing in a grantor's *world* can reach it (it is in no world's view). See *What this does not show* |

## What this does not show

- **A sealed materialization.** The checkout sits in the runtime's own
  directory. B cannot write it (`EROFS`), and no world can see it. A
  same-uid process on the host (Capability Inventory v0: the ambient
  world) could replace a file in it between B's check and B's read. The
  next step closes that by construction: materialize **inside B's mount
  namespace**, into a tmpfs the world creates before `pivot_root`, then
  make it read-only. No host path then reaches it.
- **Atomicity of S.** A's transcription is many calls over an unfrozen
  directory. S can be a tree that never existed (Atomic Capture v0).
  Whatever S is, B gets exactly S. Whether S was a real state of D is
  the capture question, and it is not answered here.
- **Fidelity.** S drops what git drops: empty directories, owners,
  timestamps, permissions beyond the executable bit, xattrs, special
  files. A grant of "D as A saw it" is therefore *less* than A saw.
  That loses fidelity, not authority. Special files make the
  transcription fail (mode `other`), which Blocks the grant.
- **Writable directories.** A snapshot cannot express "B may add files".
  A writable directory grant remains a namespace (object) grant, with
  X1's per-object containment as its only object-level check.
- **Size.** The probe returns file bytes in its JSON report, and
  materialization copies everything. Both are fine for test trees; real
  trees want streaming and a shared, deduplicated store.

## Next falsification experiment

**A sealed SnapshotCapability.** The construction plan gains a step:
"mount a tmpfs at T, write S's verified objects into it, remount it
read-only". It runs inside the world's mount namespace, before
`pivot_root`.

- **Supports the model:**
  - B's root = S;
  - no host path names any inode in B's view (search the host for B's
    handles);
  - a same-uid host process that tries to change the content after
    construction cannot reach it.
- **Falsifies it:**
  - some host path still reaches the tmpfs (for example
    `/proc/<pid>/root` of the world's init: same uid, same user
    namespace owner, Manifest v0's "inbound authority"). If that path
    works, a SnapshotCapability is only as immutable as the world's
    inbound authority is closed, and the next problem is inbound access
    to worlds, not identity.

The design's remaining live experiments: X3 (revocation of a held fd),
X4 (confused deputy), X5 (write-sharing quiescence).

## Reproducing

```
cargo test --offline -p v9r-core --test content_identity -- --nocapture --test-threads=1
```

This needs unprivileged user namespaces, `python3` and `nix-store`. It
takes about 3 s.
