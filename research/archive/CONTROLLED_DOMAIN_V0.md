# Controlled Domain v0: what is the minimum OS boundary for a runtime-owned execution domain?

Research question: **what is the least an unprivileged runtime must
build around an agent so that every writer of captured state is inside
a domain the runtime can freeze and destroy?**

Atomic Capture v0 ended with: the cgroup freezer gives true snapshots,
*if* every writer is inside the frozen cgroup and cannot leave. This
experiment measures which OS layers make that true.

Short answer (measured, unprivileged, this host):

1. **A delegated cgroup v2 subtree** with freeze and `cgroup.kill`
   contains every writer that stays in the agent's process tree: fork,
   `nohup`, `setsid`, double fork, a writer that starts after its parent
   exited.
2. **A cgroup namespace rooted at it** (it needs a user namespace when
   unprivileged) stops writers from moving themselves out. The kernel
   refuses to migrate a process to a cgroup outside the namespace root
   (`ENOENT`), whether the target is a sibling or the runtime's own
   cgroup.
3. **No IPC path to any process outside the domain that can spawn
   processes.** This is the largest escape class, and the cgroup layers
   do nothing against it. The agent *asks* someone outside to start the
   writer:
   - the systemd user manager (`systemd-run`, or raw D-Bus
     `StartTransientUnit`);
   - any host daemon that runs commands (sshd, CI runner, docker).

   Cutting these paths took:
   - **a mount namespace** that hides filesystem sockets (here
     `$XDG_RUNTIME_DIR` and `/tmp`);
   - **a network namespace** for abstract unix sockets and TCP
     localhost.
4. **A PID namespace is not part of the minimum.** The full stack
   without it contained all 11 behaviours. With the agent as the
   namespace's init, its exit kills everything the effect started,
   legitimate background work included. That is a semantic change, not
   a boundary: delegated spawning still escaped.

So the minimum boundary is **cgroup subtree + cgroupns + mountns +
netns** (userns as the unprivileged enabler). The mount namespace has to
be **an allowlist view**: the experiment closed two named socket
locations, and every other reachable socket of a spawning service
remains a hole by construction.

`kernel.rs` and `runtime.rs` are unchanged.

## Baseline

| | |
|---|---|
| commit | `8a49c4b` (atomic capture v0 report); untracked `tests/controlled_domain.rs` from the void attempt |
| tests | **281 passing, 6 ignored** (before and after; the experiment is one ignored test) |
| `kernel.rs` | sha256 `85badb66…6177f`, unchanged |
| toolchain | rustc/cargo 1.98.1 from the nix store. rustup's `cargo` is not on `PATH` and its binary cannot run on NixOS (unpatched loader) |

| Property | Value |
|---|---|
| kernel | Linux 6.12.80 |
| cgroups | v2, delegated systemd user scopes (systemd 258) |
| user namespaces | unprivileged, `user.max_user_namespaces = 126476` |
| tools | util-linux `unshare` 2.41.3, Python 3.12.13 |

## Why the previous attempt was void

The writer began with

```python
ws, target = sys.argv[1], sys.argv[2]
```

but `$W` passes only the workspace. **Every writer except "move to
another cgroup" died at startup with `IndexError`.** Its stderr went to
`/dev/null`, and nothing checked that a writer had ever run, so those
rows reported "quiescent: yes, contained: yes" about an empty
directory. The same run also printed "no cgroup" for 16 of 18 rows: the
scope had emptied and been removed, which the table did not treat as
suspicious. No number from that run is used here.

## Method

The harness is `crates/v9r-core/tests/controlled_domain.rs`, one ignored
test. Per run:

1. A fresh workspace; `dist/w` is replaced atomically every
   millisecond by the writer. A **host spawner** (outside every domain,
   a stand-in for sshd, a CI runner, or a container daemon) listens on
   three endpoints: an abstract unix socket, a unix socket in `/tmp`,
   and TCP localhost.
2. Start the agent in the domain. Its top process exits immediately,
   leaving a writer behind by one behaviour.
3. Wait 1 s (the effect window), freeze the domain's cgroup, capture
   twice 200 ms apart (**quiescent**), destroy with `cgroup.kill`,
   capture twice 300 ms apart (**contained**).

The repairs that make a row meaningful:

- **Liveness gate.** Before the freeze, two captures 50 ms apart must
  differ. A row with no live writer is reported only if it is
  *explained*:
  - **killed before freeze**: the writer reached its first line (a
    start marker exists) and then died;
  - **never started**: the identical agent script started a live
    writer with no domain, and the requester's stderr says why it
    failed here.

  Anything else is `INVALID` and fails the test.
- **Positive control.** The `none` level runs the agent with no domain.
  All 11 writers must survive destruction, or the detector is blind.
- **Ground truth independent of snapshots.** The host's `/proc` is
  scanned for the writer by its unique workspace path. This gives its
  cgroup at freeze (inside or outside the domain's subtree) and whether
  it survived destruction. The test asserts that the snapshot verdict
  ("contained") equals the `/proc` verdict ("0 survivors") in every
  row.
- **Runtime-held init.** With a PID namespace, the agent's shell would
  be PID 1, and its exit kills the namespace. The levels marked *held*
  keep PID 1 as a runtime process (`sh -c '…; sh agent.sh; exec sleep'`)
  for the whole window. This also performs the domain's mount setup.

Levels:

| | Level | How |
|---|---|---|
| L0 | none | plain child |
| L1 | scope | `systemd-run --user --scope -p Delegate=yes` |
| L2 | + userns + cgroupns | `unshare --user --map-root-user --cgroup` |
| L3 | + pidns, agent is init | `--pid --fork --mount-proc` |
| L4 | + pidns, runtime holds init | as L3, held |
| L5 | L4 + mountns hiding `$XDG_RUNTIME_DIR` | tmpfs over it |
| L6 | L5 + netns | `--net` |
| L7 | L6 + private `/tmp` (workspace bound back in) | tmpfs over `/tmp` |
| L8 | **L7 without pidns** | `--user --cgroup --mount --net`, held |

## Results

The verdicts were **identical in every row across three full runs** of
the final 9 × 11 matrix (each about 165 s). An earlier 7 × 10 matrix
was also run three times with identical verdicts, and agrees with every
shared cell.

Legend:
- **✗** a writer survived destruction (escape);
- **✓** frozen inside the domain, quiescent, killed;
- **⊘** never started: the request to an outside spawner failed;
- **†** killed when the namespace init exited, before the window
  ended.

| Behaviour | L0 | L1 | L2 | L3 | L4 | L5 | L6 | L7 | L8 |
|---|---|---|---|---|---|---|---|---|---|
| fork child | ✗ | ✓ | ✓ | † | ✓ | ✓ | ✓ | ✓ | ✓ |
| `nohup` daemon | ✗ | ✓ | ✓ | † | ✓ | ✓ | ✓ | ✓ | ✓ |
| `setsid` | ✗ | ✓ | ✓ | † | ✓ | ✓ | ✓ | ✓ | ✓ |
| double fork | ✗ | ✓ | ✓ | † | ✓ | ✓ | ✓ | ✓ | ✓ |
| delayed writer after parent exits | ✗ | ✓ | ✓ | † | ✓ | ✓ | ✓ | ✓ | ✓ |
| move to a sibling cgroup | ✗ | **✗** | ✓ ¹ | † | ✓ ¹ | ✓ ¹ | ✓ ¹ | ✓ ¹ | ✓ ¹ |
| service manager, `systemd-run` | ✗ | **✗** | **✗** | ⊘ ² | ⊘ ² | ⊘ ³ | ⊘ ³ | ⊘ ³ | ⊘ ³ |
| service manager, raw D-Bus | ✗ | **✗** | **✗** | **✗** | **✗** | ⊘ ³ | ⊘ ³ | ⊘ ³ | ⊘ ³ |
| host daemon, abstract unix socket | ✗ | **✗** | **✗** | **✗** | **✗** | **✗** | ⊘ ⁴ | ⊘ ⁴ | ⊘ ⁴ |
| host daemon, unix socket in `/tmp` | ✗ | **✗** | **✗** | **✗** | **✗** | **✗** | **✗** | ⊘ ⁵ | ⊘ ⁵ |
| host daemon, TCP localhost | ✗ | **✗** | **✗** | **✗** | **✗** | **✗** | ⊘ ⁶ | ⊘ ⁶ | ⊘ ⁶ |

1. The writer's own `cgroup.procs` write fails with `ENOENT`: the target
   is outside the cgroup namespace root. Its host path is visible and
   `mkdir` there succeeds; only membership is refused.
2. `Failed to connect to user scope bus: No data available`. This is
   `systemd-run`'s private-socket authentication failing across PID
   namespaces, an accident of the client. The raw D-Bus row, same
   level, escapes.
3. `Failed to connect to user scope bus: No such file or directory`
   (socket hidden).
4. `ConnectionRefusedError` (abstract sockets are per network
   namespace).
5. `FileNotFoundError` (the socket is not in the private `/tmp`).
6. `OSError: Network is unreachable` (new netns, loopback down).

Further observations from the same runs:

- **The `/proc` and snapshot verdicts agreed in all 99 rows** (asserted).
- **An escaped writer defeats the freezer exactly as predicted.**
  - At L4–L7, every escaped writer was outside the domain at freeze
    (`0 in / 1 out`), and the frozen captures were **not quiescent**.
  - At L1–L3, an escape emptied the scope and systemd removed it
    before the freeze. The runtime can see that its domain *vanished*,
    not where the writer went.
- **Migration into the runtime's own cgroup** (probe, outside the
  matrix): at L1 the agent's process moved into the cgroup of the
  shell that launched the runtime. Under a cgroupns it was refused with
  `ENOENT`, as in note 1.

## Evaluation

| Escape class | Mechanism that closes it | Measured first at |
|---|---|---|
| process tree (fork, daemonize, `setsid`, double fork, delayed start) | cgroup subtree + freeze + `cgroup.kill` | L1 |
| self-migration out of the subtree | cgroup namespace (kernel rule: migration target must be under the ns root) | L2 |
| delegated spawn via filesystem socket (service manager, `/tmp` daemon) | mount namespace that does not contain the socket | L5 (runtime dir), L7 (`/tmp`) |
| delegated spawn via abstract unix socket or localhost TCP | network namespace | L6 |

- **pidns adds no containment.** L8 (no pidns) and L7 are identical.
  - Agent-as-init (L3) contains process-tree writers only by killing
    them at once. That also kills every legitimate background process
    of the effect, and leaves no window to freeze or observe.
  - The pidns did block `systemd-run` (note 2), but only through a
    client quirk. A raw D-Bus client walks past it.
- **userns is an enabler, not a boundary.** It is what lets an
  unprivileged runtime create the other namespaces. With userns alone,
  self-migration succeeds (probe: the move into a sibling returned 0).
- **The first two layers are "where processes are"; the third is "who
  they can talk to".** Cgroups contain processes the agent *creates*.
  No cgroup mechanism touches processes the agent *causes someone else
  to create*. That is the difference between L2 and L8, and it is 5 of
  the 11 behaviours.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | A "contained: yes" row means a writer was contained | **False without a liveness gate.** The void run's writers never ran |
| 2 | Containing the process tree contains the writers | **False.** Every level up to L4 lost to an agent asking the user service manager or a host daemon to start the writer |
| 3 | A PID namespace is part of the minimum | **False.** L8 = L7 without it. Agent-as-init only trades escape for killing the effect's own work |
| 4 | The pidns blocked the service manager | **False.** Only `systemd-run`'s private-socket path failed (`ENODATA`); raw D-Bus via `busctl` started the unit |
| 5 | A cgroup namespace hides the host cgroup tree | **False.** `/sys/fs/cgroup` is the host mount, and the agent can `mkdir` cgroups in the host tree (litter outside the domain). The protection is the kernel's migration rule, not visibility |
| 6 | A network namespace cuts all socket IPC | **False.** Path-bound unix sockets cross it (L6, `/tmp` row). Only the mount view stops them |

Harness and probe bugs found along the way:

- **The writer `IndexError`** (above). It is the reason the previous
  experiment is void.
- **`sh sh agent.sh`** in my own first repair of the `none` level. The
  new gate failed the run with the shell's `cannot execute binary file`
  in the row diagnostics, rather than reporting a contained control.
- **A probe that lied.** `open(p, "w").write(pid)` without `with` reports
  success under a cgroupns, because CPython discards the error raised by
  the implicit close (the kernel refuses at `close`, when the buffered
  write is flushed). The probe was redone with a checked close and
  verified from the host's `/proc`. The harness writer uses `with` and
  was not affected.

## Not tested (the next holes to try)

The boundary above is minimal *for the behaviours tested*. Known
untested routes:

- **Other filesystem sockets of spawning services**: the system D-Bus,
  `docker.sock`, an ssh-agent-forwarded login, `cron`/`at` via their
  setuid clients. Hiding two directories is a denylist. Only a mount
  view that starts empty and adds the workspace and read-only system
  paths closes this class by construction.
- **ptrace or `/proc/<pid>/mem` of same-user processes outside the
  domain.** At L1 nothing stops it except YAMA. Under a userns, the
  kernel requires capabilities in the target's namespace.
- **Inherited file descriptors** to outside processes or sockets,
  passed in by the runtime itself.
- **Network egress.** L6–L8 have no network at all. Agents usually
  need some (package fetches, model APIs). A real domain needs egress
  that reaches the internet but no host-local service. That changes
  the netns row from "nothing reachable" to a policy that has to be
  tested in its own right.
- **Pre-existing outside writers of the same user.** These are outside
  any domain by definition. The workspace itself must not be writable
  by them, which is a filesystem-permission question (a separate uid),
  not a namespace one.

## Decision

**A runtime-owned execution domain needs four unprivileged layers, and
they are available on this host:**

1. cgroup v2 subtree (freeze, kill);
2. cgroup namespace (no migration out);
3. mount namespace built as an allowlist (no filesystem-socket path to
   outside spawners);
4. network namespace (no abstract or localhost path), with any egress
   provided by a mechanism that cannot reach host-local services.

User namespaces are required as the unprivileged enabler. A PID
namespace is optional: useful for hygiene (no visibility of or signals
to outside processes), but not part of the containment measured here.

With these, the Atomic Capture v0 precondition ("every writer is inside
the frozen cgroup and cannot leave") held in every live row of the
matrix. A freeze-then-walk capture of a domain built this way can be
treated as atomic **with respect to writers originating in the
domain**.

## Next falsification experiment

**Allowlist domain: does an empty-by-default mount view with a
host-local-blind egress path hold against the untested routes?**

- Build L8 with a mount view from an empty tmpfs: the workspace
  read-write, `/nix/store` (or `/usr`) read-only, nothing from `/run`,
  `/var/run`, `/tmp` or `$HOME`.
- Give it egress through a userspace network stack (slirp/pasta-style)
  that is configured to refuse host loopback.
- Add the untested routes as behaviours:
  - system-bus `StartTransientUnit`;
  - a spawner on a socket under `/run`;
  - ptrace and `/proc/<pid>/mem` of an outside same-user process;
  - a spawner reached through the egress path at the host's external
    address;
  - an inherited fd.
- **Supports the decision:** every route is ⊘ or ✓, and the positive
  control escapes.
- **Falsifies it:** any ✗. In particular, a spawner reachable at the
  host's own external address through the egress path would show that
  "host-local-blind" egress is harder than one flag.

## Reproducing

```
cargo test -p v9r-core --test controlled_domain -- --ignored --nocapture
```

This needs a systemd user session with delegation, unprivileged user
namespaces, `python3`, `busctl`, `unshare` and `systemd-run`. It takes
about 165 s.
