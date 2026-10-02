# Snapshot Capability Boundary v0: can a snapshot capability be realized outside the host's identity domain?

Research question: **can v9r realize a SnapshotCapability so that its
view is isolated from the host identity domain (the runtime's uid)?**

Content Identity v0 (X1b) left one gap. The materialized snapshot was a
host directory owned by the runtime's uid, so a same-uid host process
could change it after B's check.

This experiment builds the snapshot **inside the grantee world's own
mount namespace**:

1. mount a tmpfs at `/v/d`;
2. fill it with S's hash-checked entries;
3. remount it read-only (superblock, then mount);
4. all of this before `pivot_root`.

A same-uid host process then attacks the held world.

> **Only measured results.** Everything below comes from two runs of
> `tests/snapshot_boundary.rs` (2026-10-02), both run by the user. The
> second run added attacks A5 and A6. Every result common to the two
> runs was identical.

## Measured summary

**Short answer: no. The view can be isolated from every same-uid
process except the owner of the world's user namespace. Against the
owner, the immutable value detects the change but does not prevent it.**

- **An owner holding the capabilities `setns` grants changed the
  view.**
  - A5: remount read-write (superblock, then mount), then write: **ok**.
  - A6: mount a tmpfs over `/v/d`, then write: **ok**. Reading via
    `/proc/<pid>/root` then returned `OWNER`.
- **Both changes were detected.** The runtime's outside re-verification
  gave **root ≠ S** after A5 and after A6.
- **Everything short of the owner's capabilities failed:**
  - B itself;
  - the host directory the world's root was built on;
  - `/proc/<pid>/root` writes;
  - an entered process that had dropped its capabilities with `execve`.

- **B sees only S.**
  - In-world root = S.
  - The view holds exactly S's 4 entries plus its root.
  - The world has 25 mounts: `/`, 23 closure binds and `/v/d`. There
    are **no others**.
  - There is no `/proc` in the world: reading `/proc/self/mountinfo`
    gives `ENOENT`, and mounting `proc` gives `EPERM`.
  - C1–C3: **Allow**.
- **B cannot change its view:** write, create and mount all fail
  (`EROFS`, `EROFS`, `EPERM`).
- **No host path names the view.**
  - Writing the host directory the world's root was built on succeeded,
    and **did not appear in B's view**.
  - Through `/proc/<pid>/root`, a same-uid host process can **read** the
    view. Its write, create and chmod all failed (`EROFS`).
- **A same-uid process that entered B's user and mount namespaces, then
  exec'd** could not remount the view read-write or mount over it
  (`EPERM`). That process had **no capabilities** at the time of the
  calls.
- **The runtime re-verified the view from outside after every attack.**
  The root was S after A1–A4, and **not S after A5 and A6**.
- **Lifecycle:** after B was destroyed, its pid was gone, all 5 objects
  of S still verified, and S still expanded to 4 entries.
- **Reproducibility:** two further worlds built from the same tree hash
  had **identical capability names** (9,235 names in all three) and
  root = S. Their plan digests were identical. Both views had `st_dev`
  61.

`kernel.rs` was not modified.

## What was built

| Piece | Change |
|---|---|
| `capability.rs` | `plan_sealed(manifest, [(target, root)], store)`: loads S's entries through `snapshot::entries` (every object hash-checked) and inserts `Step::Snapshot` before `SealRoot`. Construction runs between fork and exec, inside the world's new mount namespace: `tmpfs` at the target (`nosuid,nodev`); directories, files (`O_EXCL`, then `fchmod` to `0444`/`0555`) and symlinks; `MS_REMOUNT\|MS_RDONLY` (superblock), then `MS_REMOUNT\|MS_BIND\|MS_RDONLY` (mount). The target is a declared read scope for C1. `observe_held(…, during)` runs `during(pid)` while the probe holds the world |
| `snapshot.rs` | `SnapEntry` and `entries(store, root)`: a verified flattening of a tree, built on X1b's load-everything-first checkout |
| `world_probe.py` | new op `mount` (fstype at path) |
| `tests/snapshot_boundary.rs` | world A → S; sealed world B; attacks A1–A6 while held; lifecycle; two reproductions. A5 and A6 report results but do not assert them |

## Results

```
cargo test --offline -p v9r-core --test snapshot_boundary -- --nocapture
run 1 (A1–A4):  test snapshot_capability_boundary ... ok   (0.56 s)
run 2 (A1–A6):  test snapshot_capability_boundary ... ok   (0.60 s)
```

S = `50efc8b4…f3dd`. The plan step reads:
`tmpfs /v/d = tree 50efc8b4… (4 entries), ro`.

### 2. B sees only S (B's own probe, before any attack)

| Measurement | Result |
|---|---|
| in-world root of `/v/d` | **= S** |
| names under `/v/d` | `/v/d`, `a.txt`, `l`, `sub`, `sub/b.txt` |
| read `/v/d/a.txt` | `ok:8ed3f6ad…` (sha256 of `alpha`) |
| write `/v/d/a.txt` | **EROFS** |
| create `/v/d/new` | **EROFS** |
| mount tmpfs at `/v/d` | **EPERM** |
| whole walk | 9,234 entries, 0 unknown |
| C1–C3 | **Allow** |

Mounts, read from the host via `/proc/<pid>/mountinfo`:

| Measurement | Result |
|---|---|
| total | **25** |
| `/v/d` | `tmpfs`, `ro,nosuid,nodev,relatime` |
| mounts other than `/`, `/nix/store/…` and `/v/d` | **none** |
| top level of `/proc/<pid>/root` | `{nix, v}` |

### 3. `/proc`

| From | Measurement | Result |
|---|---|---|
| inside B | `/proc` in the walk | **absent** |
| inside B | read `/proc/self/mountinfo` | **ENOENT** |
| inside B | mount `proc` at `/v` | **EPERM** |
| host, same uid | read `/proc/<pid>/root/v/d/a.txt` | **`alpha`**: the view is readable from the host |
| host, same uid | top level of `/proc/<pid>/root` | `{nix, v}`: the world's root, not the host's |

`/proc/<pid>/root` does not take B anywhere: B has no `/proc`. It takes
a same-uid host process **into** B's view, for reading.

### 1. Same-uid host process attacks the held view

| # | Attack | Result | Root of B's view, re-verified from outside afterwards |
|---|---|---|---|
| 0 | none | — | **= S** |
| A1 | write `<world root dir on host>/v/d/a.txt` | **ok** (the file exists on the host) | **= S**: it is not B's view |
| A2 | write `a.txt` via `/proc/<pid>/root/v/d` | **EROFS** | |
| A2 | create `new` via `/proc/<pid>/root/v/d` | **EROFS** | |
| A2 | chmod `a.txt` via `/proc/<pid>/root/v/d` | **EROFS** | **= S** |
| A3 | `setns` into B's user + mount namespaces, exec the world's python. In it: write (**EROFS**); remount superblock rw (**EPERM**); remount mount rw (**EPERM**); write again (**EROFS**) | blocked | **= S** |
| A4 | same entry; mount a tmpfs over `/v/d` (**EPERM**); write (**EROFS**) | blocked | **= S**; reading via `/proc` still gives `alpha` |

| A5 | `setns` into B's user + mount namespaces; **before `exec`**: `MS_REMOUNT` (superblock rw) **ok**, `MS_REMOUNT\|MS_BIND` (mount rw) **ok**; then exec the world's python: chmod and write `a.txt` **ok** | **changed** | **≠ S: detected** |
| A6 | same entry; **before `exec`**: mount a tmpfs over `/v/d` **ok**; then write `a.txt` **ok**. The chmod gave `ENOENT`: the new tmpfs was empty | **replaced** | **≠ S: detected**; reading via `/proc` gives `OWNER` |

In A3, the entered process saw `/` as `{v, nix}`. It was inside B's
mount namespace, at B's root.

**A3/A4 against A5/A6:** both pairs entered the same namespaces with
the same calls. The only difference is whether the calls were made
before or after `execve` as uid 1000. A3 and A4, after: `EPERM`. A5 and
A6, before: **ok**. The boundary was the capabilities, not the read-only
flags, not the mount namespace, and not the uid.

### 4. Lifecycle

| Measurement | Result |
|---|---|
| `/proc/<pid>` after B's release | **gone** |
| host directory A1 wrote into | still holds `a.txt` (A1's file, on the host; it was never in B's view) |
| S's objects in the runtime's store | **5 of 5 verify** |
| `entries(S)` | **4**, unchanged |

### 5. Reproducibility

| Measurement | B | B2 | B3 |
|---|---|---|---|
| capability names (C1's observed set) | 9,235 | identical | identical |
| C1–C3 | Allow | Allow | Allow |
| in-world root | S | S | S |
| `/v/d` `st_dev` | — | 61 | 61 |
| plan digest (two plans from the same S) | | identical | |

B2's and B3's sealed views are two separate tmpfs instances, created
and destroyed one after the other, and they reported the **same**
`st_dev` (61). Like X1's mount ids, a sealed view's device number names
an instance only while it exists. The capability world is reproduced by
content (root S, the same names), not by object identity.

## Answers

| Question | Measured answer |
|---|---|
| Does SnapshotCapability require a private mount namespace? | **Required, not sufficient.** The private mount namespace is what kept the host out of the view: A1's write landed in the host directory and never reached B, and B has no `/proc`. But a same-uid process can **enter** that namespace (A3–A6 all entered it, and saw B's root `{v, nix}`). Inside it, with the owner's capabilities, it changed the view (A5, A6) |
| Is read-only tmpfs sufficient? | **No.** It held against B, against host paths and `/proc/<pid>/root` writes, and against an entered process without capabilities (A2–A4). It did **not** hold against the owner with capabilities. The superblock and mount read-only flags were cleared (A5), and a tmpfs was mounted over the view (A6) |
| Does realization need a privileged helper? | **Not to build the view or to detect a change.** Every step of construction and outside verification ran unprivileged. **To make a change impossible, nothing unprivileged was enough here.** The attacks that succeeded used only the runtime's own uid. Not measured: a world whose user namespace is owned by a different uid (a privileged helper). Without one, this runtime cannot keep its own uid out |
| Is "immutable value" a stronger primitive than filesystem permissions? | **Stronger at detection, not at prevention.** Permissions (read-only superblock and mount, `0444`) were defeated by the owner in A5 and A6. The value was not: the runtime re-derived the root from outside after each attack, and it was **≠ S** after both, so a use pinned to `tree:S` (D9) would be **denied**. A1–A4 left the root = S. Two worlds from one hash reproduced the same capability world (5) |

## What the detection does and does not cover

- **Detected:** every change that was present when the runtime
  re-verified from outside (A5, A6: root ≠ S).
- **Not covered by these runs:** a change made after the last
  re-verification and before B reads, or a change undone before the
  next one. Detection holds only at the moments of verification. That
  is the time limit of Capability Manifest v0 and Atomic Capture v0,
  unchanged. "Detected before authority use" therefore requires a
  verification immediately before each use, with nothing able to
  change the view in between. That condition is the owner question
  again.

## Reproducing

```
cargo test --offline -p v9r-core --test snapshot_boundary -- --nocapture
```

This needs unprivileged user namespaces, `python3` and `nix-store`. It
takes about 0.6 s.
