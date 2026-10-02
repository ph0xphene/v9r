# Capability Object Identity v0 (X1): do grants bind to objects or to names?

Research question: **does v9r grant authority over objects, or only over
names that resolve to objects?**

This is experiment X1 of `CAPABILITY_COMPOSITION_DESIGN_V0.md`, run with
real worlds:

- world A, the grantor, resolves the object in its own view;
- the runtime issues a signed grant (Root of Trust v0) carrying that
  identity;
- world B, the grantee, resolves the object it was actually given.

Each B observation is judged under three identity schemes, against
ground truth fixed by the experiment's own construction.

Short answer: **before X1, v9r granted authority over names. With one
fact pin and an in-world resolver, it grants authority over objects.
"Object identity" has to mean the file handle, not the inode number.**

- **Names alone were wrong in 3 of 6 cases.** Each time it was an Allow
  for an object the grantor never held: same path with a different
  object; one world path across two filesystems; a deleted and recreated
  file.
- **`(st_dev, st_ino)` was wrong in 1 of 6.** ext4 reused the inode
  number in **500 of 500** delete-and-recreate pairs. The recreated file
  passed as the old one.
- **`st_dev` + the `name_to_handle_at` handle was right in 6 of 6.** On
  ext4 the handle is the inode number plus its generation. No recreated
  file kept its handle (0 of 1,000 across ext4 and tmpfs).
- **The identity must be observed in the grantor's view.** Taking it
  from the host at issue time allowed B an object A never held, under
  every scheme.
- **Directory grants are object grants only for the directory itself.**
  A hard link to the grantor's private file, placed inside a granted
  directory after the grant, passes every identity check. Only a
  per-object containment check sees it.
- **The design's submount mechanism does not exist for an unprivileged
  runtime.** The kernel refuses (`EINVAL`) a non-recursive bind that
  would reveal what lies under a locked mount.
- **No kernel change.** Identity is a `Fact` pin (`D9`); per-object
  containment is `Within`. `kernel.rs` sha256 `85badb66…6177f`,
  unchanged.

## Baseline

| | |
|---|---|
| tree | `8a49c4b` plus the uncommitted capability, delegation (X2) and root-of-trust work |
| tests before | 308 passed, 6 ignored |
| host | Linux 6.12.80, unprivileged uid 1000; temp dir on ext4 (`stat -f` reports `ext2/ext3`, the ext magic); `/dev/shm` tmpfs |
| `kernel.rs` | sha256 `85badb66…6177f` |

## What was built

| Piece | Change |
|---|---|
| `capability.rs` | **Aliases**: bind a host `source` at a different world `target`. `Bind` gained `source`, and C2 compares the inside object with the *source's* identity. **`observe_with`**: passes the probe its `ops` (already in the probe, never wired) and a `resolve` list. **Accessors**: `resolved(path)`, `op(key)`, `objects_within(path)`, the last over the walk's `(dev, ino)` ids, which the probe reported but nothing parsed |
| `world_probe.py` | `resolve(paths)`: `lstat` → `(st_dev, st_ino)`; `FS_IOC_GETVERSION` (inode generation) on an fd opened `O_NOFOLLOW`; `name_to_handle_at` (flags 0, no follow) → handle and mount id |
| `object_identity.rs` (172 lines) | three `IdScheme`s; `evidence(obs, [(grant path, world path)], scheme)` attests `object(grant path) = id` from what the **grantee's** probe resolved (`world-probe` provenance); `host_resolve` for comparison; `open_by_handle` |
| `delegation.rs` | unchanged in X1. `D9.object_identity` was added in Root of Trust v0 |
| `tests/object_identity.rs` | three live tests, 13 worlds, 1.4 s |

**The model being tested:**

```text
grant (signed, grantor's namespace)        grantee world (runtime-built)
objects: [P]   identities: {P: id_A}       P's object bound at T (alias, or T = P)
     │                                             │ probe resolves T → id_B
     └──── D9: Fact(object(P) = id_A) ◀── evidence: object(P) = id_B (world-probe)
```

- **Grant names stay in the grantor's namespace.** D3 and D7 attenuate
  them as in X2.
- **Where the grantee sees the object is the runtime's choice.** It is
  recorded as a mapping `P → T`.
- **The identity is the only thing that ties the two.**

## Results

```
cargo test --offline -p v9r-core --test object_identity -- --nocapture --test-threads=1
3 passed (13 worlds, 1.44 s)
cargo test --offline --workspace --no-fail-fast
311 passed, 0 failed, 6 ignored   (two runs)
```

### The cases

The verdicts are the full Root of Trust v0 decision, with D0–D9 and A2.
**✗** marks a verdict that disagrees with ground truth.

| # | Case | Same object? | Name | DevIno | Handle |
|---|---|---|---|---|---|
| 1 | **same path, different object**: `/w/o` is `o1` in A, `o2` in B | no | Allow ✗ | Deny | Deny |
| 2 | **same object, different path**: renamed on the host after the grant, bound at `/x/renamed` in B | yes | Allow | Allow | Allow |
| 3a | **hard link**: B is given the same inode under another name | yes | Allow | Allow | Allow |
| 3b | **directory grant**: the directory object itself, after a hard link to A's secret and a new file appeared inside | yes (the directory) | Allow | Allow | Allow |
| 4b | **mount boundary**: one world path `/m/o`, ext4 in A, tmpfs in B | no | Allow ✗ | Deny | Deny |
| 5 | **deleted and recreated**, same path (inode number reused, 1st try) | no | Allow ✗ | **Allow ✗** | Deny |
| 5′ | as 5, but the runtime took the grant's identity **from the host at issue time** | no | — | Allow ✗ | **Allow ✗** |
| 4a | **submount view**: A binds `/run/user` non-recursively, with `/run/user/1000` a tmpfs below it | — | world **not built**: step 7, `errno 22` (EINVAL) | | |

Supporting measurements from the same runs:

- **Case 2, locator.** Realizing the grant by its granted name after the
  rename gives `plan` → `Missing(…/c2/o3, "No such file or directory")`.
  The name lost the object; the identity did not.
- **Case 3b, objects inside the directory.** Within the granted
  directory, B's view holds two objects that were not there at grant
  time: `s` (the hard link) and `new.txt`.
  - Grant-time containment, as `Within(B's objects, grant-time
    objects)`: **Deny**, and it flags both.
  - Overlap with A's private objects: flags **only `s`**.
  - `s` has exactly the handle of A's `apriv/secret`.
- **Case 5, why DevIno failed.**
  - A saw `ino 24520240 gen 2318283410`.
  - B saw `ino 24520240 gen 2164621525`.
  - The handles were `1:3026760192322e8a` and `1:30267601d5800581`. Their
    first 4 bytes are the inode number (`0x01762630` = 24520240,
    little-endian) and their last 4 are the generation.

### Identity reuse by filesystem (`identity_reuse_by_filesystem`)

500 pairs of write, resolve, delete, write, resolve:

| Filesystem | same `(dev, ino)` | same handle | `FS_IOC_GETVERSION` |
|---|---|---|---|
| ext4 (temp dir) | **500 / 500** | 0 / 500 | ok |
| tmpfs (`/dev/shm`) | 0 / 500 | 0 / 500 | `ENOTTY`. The handle (12 bytes) still carries the generation |

### Stability across views (the same object, case 2)

| Field | host | world A | world B | Stable? |
|---|---|---|---|---|
| `st_dev` | 65025 | 65025 | 65025 | yes |
| `st_ino` | 24520220 | 24520220 | 24520220 | yes, but reused after deletion |
| generation | 2837200201 | 2837200201 | 2837200201 | yes |
| handle | `1:1c267601493d1ca9` | same | same | **yes** |
| mount id | 30 | 557 | 491 | **no.** It is also **reused across worlds**: in case 1, *different* objects in two different worlds both reported mount id 491 |

`open_by_handle_at` from this unprivileged runtime: **`EPERM`**. An
identity can be **checked**, but an object cannot be **reached** by its
identity alone.

## Evaluation

### Can object identity remain stable across worlds?

**Yes, as `(st_dev, handle)`. Not as `(st_dev, st_ino)` over time, and
never as a mount id.**

- Device, inode, generation and handle were identical in the grantor's
  world, the grantee's world and on the host, through aliases, renames
  and hard links (cases 2, 3a, 3b, and the stability table).
- **Mount ids** are per mount. They differ between views of one object,
  and a freed id is reused by the next world. The design proposed
  `Inode(dev, ino, mnt_id)`. That is falsified twice: the inode number
  is not unique over time, and the mount id is not stable across views.
- What was **not** measured:
  - Stability across reboots or remounts. tmpfs `st_dev` is an anonymous
    device number (22 here), valid only while mounted.
  - Filesystems without export support (no `name_to_handle_at`). There,
    the handle scheme yields no fact, so D9 is undetermined: Blocked,
    not Allow.

### Does capability provenance need object resolvers?

**Yes, two of them, and at least one must run in the grantor's view.**

- **At issue time**, the identity written into the grant must be what
  the grantor actually held: observed in A's world.
- **At use time**, the identity must be what the grantee actually holds:
  observed in B's world.

Case 5′ shows why the host is not a substitute. A runtime that resolves
the granted path on the host at issue time names the *current* object.
That object was created after A's observation, and every scheme then
allows B to use it. The host is a third view, and it is not the
grantor's.

Both resolvers are the existing world probe. Its facts are `Verified`
with the same provenance as Capability Manifest v0's C1–C3, so no new
trust was added. The probe is now trusted for one more thing: reporting
`name_to_handle_at` faithfully.

### Is path normalization enough?

**No.** Every name in cases 1, 4b and 5 was canonical. In case 5 it was
the identical host path, and the object was still different. Case 2
shows the converse: a canonical name cannot follow an object that was
renamed (the plan fails `Missing`), while the identity can.

Normalization is necessary: X2's D0 stops `..` from passing as a prefix.
It answers "which name?", not "which object?".

### Hard links: object identity or namespace identity?

**Measured:** authority follows the object.

- In 3a, B holds A's object under a different name, and the identity
  schemes allow it. So does Name, but by accident.
- In 3b, the reverse: a hard link brings A's private object into a
  granted directory, and the identity of the *directory* does not change.
  The directory is the same object. Its contents are whatever its
  namespace holds.

**Decision for v0:**

| Grant on | Authority attaches to | Check |
|---|---|---|
| a file | **the object** (`st_dev` + handle), under any name | D9 |
| a directory | the directory object (D9) **plus its namespace**: whatever is linked into it | per-object containment is possible but over-approximates: grant-time containment flags legitimate new files too. Overlap with the grantor's private set is exact, but needs the grantor's whole object set |

For **read-only** directory grants, the object-level answer already
exists: grant the **content identity** (Content-Addressed State v0's
tree root id), and any change, a hard link included, is a different
root. That was not run here. **Writable** directory grants are
namespace authority by nature. That is the Design's S2b and X6, and R9
(co-writers form one domain).

### Mount boundaries

The design's X1 mechanism (Design H3) was:

- the grantor's world binds a parent non-recursively;
- so it sees the directory *under* a host mount;
- so a grant resolved on the host would hand the grantee the mount, an
  object the grantor never saw.

**This cannot happen in an unprivileged world.** Mounts inherited into a
user-namespace mount namespace are locked. The kernel refuses a
non-recursive bind that would expose what lies beneath a locked mount
(case 4a: `EINVAL` at the bind step). The world fails closed and is
never built.

This also corrects Capability Manifest v0's note that "submounts below a
declared path are not carried". **For inherited submounts, the bind is
refused, not trimmed.** Manifest v0 never hit this because its declared
paths (a nix closure, temp dirs) have no submounts.

H3's conclusion survives: identity must come from the grantor's view.
Its mechanism is different. The divergence between the grantor's view
and the host arises **over time** (case 5′) and **through aliases**
(cases 1, 4b), not through hidden submounts.

### Does this require kernel changes?

**No.** Every check is in an existing form:

- identity: `Fact(object(P) = id)` (D9);
- per-object containment: `Within` over `obj/<dev>/<ino>` names (3b).

The kernel stays domain-free. A guard test (Delegation v0) checks its
hash.

## Status of the design's claims

| Claim | After X1 |
|---|---|
| H3: name-level attenuation is unsound; object identity resolved in the grantor's view is sound | **Supported.** Name: 3 of 6 wrong. Handle from the grantor's view: 6 of 6 right. Identity from the host: wrong (5′). **Mechanism corrected**: not submounts (refused, 4a), but time and aliases |
| R4: grants attach to `Inode(dev, ino, mnt_id)` for mutable objects | **Falsified as stated.** `ino` is reused (500/500 on ext4); `mnt_id` is per-view and reused. **Replace with `(st_dev, handle)`**. Keep the path only as a locator |
| R4: `Content(sha256)` for immutable objects | not tested; proposed for read-only directory grants (3b) |
| Q4: abstract IDs name grants, not objects | consistent: the grant id stays the content hash of the record |
| F6: submounts are not carried | **Refined:** inherited (locked) submounts make the bind fail |
| Manifest v0 C2: identity = `(dev, ino)` at plan time vs inside | **Weakened by X1.** A delete-and-recreate between `plan` and construction reuses the ino on ext4, and C2 would pass. C2 should compare handles. Not changed here |

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | `(st_dev, st_ino)` is object identity | **False over time.** ext4 reused the number in 500 of 500 recreations. It identifies a *slot*, not an object |
| 2 | A mount id distinguishes views | **False for identity.** It differs between views of one object *and* repeats across views of different objects |
| 3 | An unprivileged world can see under a host mount (Design X1 arm (a)) | **False.** Locked-mount rule: `EINVAL`. Fail-closed |
| 4 | Object identity lets the runtime find an object | **False unprivileged.** `open_by_handle_at` gives `EPERM`. Identity verifies; a path still has to locate |
| 5 | A directory's identity covers what it contains | **False.** A hard-linked foreign object leaves the directory's identity unchanged (3b) |
| 6 | The runtime can resolve a grant's object itself, on the host | **False.** The host is a third view. Its resolution at issue time named an object A never held (5′) |
| 7 | The probe's walk ids were in use | **False.** The probe reported `ids` since the Manifest work, but `Inside` never parsed them. They are now parsed; `objects_within` relies on them |

## What this does not show

- **Writable grants and concurrent change.** All binds here are
  read-only, and every world is observed once. A grantee writing into a
  shared object, or the object changing between B's observation and B's
  use, is the time question again (Manifest v0 "Time", Temporal v0
  re-check).
- **Persistence.** Handles and devices across reboot, remount or
  `fsck`.
- **Filesystems without handles.** Network and FUSE filesystems: D9
  would be undetermined (Blocked), which fails closed but is unusable.
- **Directory grants as content identity.** Proposed, not run.

## Next falsification experiment

**Directory grants as content: X1b.** Grant a read-only directory by its
snapshot root id (Content-Addressed State v0). Then repeat 3b: hard
link in, new file in, file modified, mode changed, a symlink retargeted.

- **Supports the model:** each change is a different root, so B's use is
  Deny, and an unchanged directory is Allow under any alias.
- **Falsifies it:** a change that keeps the root id (e.g. hard-link
  count, which a git tree does not record). That would mean content
  identity misses a channel, and the hard link has to be caught another
  way (link count, or per-object overlap).

After that, the design's remaining live experiments: X3 (revocation of
a held fd), X4 (confused deputy), X5 (write-sharing quiescence).

## Reproducing

```
cargo test --offline -p v9r-core --test object_identity -- --nocapture --test-threads=1
```

This needs unprivileged user namespaces, `python3` and `nix-store`. It
takes about 1.5 s. `locked_submount_cannot_be_revealed` attempts one
bind of `/run/user`; the world fails before its probe runs, so nothing
under `/run/user/<uid>` is read.
