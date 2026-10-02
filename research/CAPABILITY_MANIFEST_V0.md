# Capability Manifest v0: can a world built from a manifest be checked against it?

Research question: **can v9r build an agent's world from a small manifest,
and then show from evidence that what the agent holds is a subset of what
the manifest declared?**

Capability Inventory v0 ended with three results:

- ambient authority cannot be fully inferred from observation;
- namespaces do not isolate authority;
- sound accounting needs a world **constructed** from a manifest, with
  observation as the check.

This experiment builds the smallest such system.

> **Status: implemented, compiled, run.**
> The code was written in a session whose shell was blocked, so it was
> written blind. It was first compiled and run on 2026-10-02. The first
> run found two defects that the blind write could not have seen: a
> deadlock in `observe` and a launcher that adds env. Both are listed in
> [Failed assumptions](#failed-assumptions). After the fixes, all
> nine predictions held. See [Results](#results).

`kernel.rs` is unchanged.

## The system

```text
CapabilityManifest ──plan()──▶ ConstructionPlan ──observe()──▶ Observation
                                       │                            │
                                       └──────── check() ◀──────────┘
                                                    │
                                     kernel::evaluate(Post, …) → Decision
```

Files:

- `crates/v9r-core/src/capability.rs`
- `crates/v9r-core/src/world_probe.py`: the inside observer, compiled into
  the crate with `include_str!`
- `crates/v9r-core/tests/capability_manifest.rs`: the live test

### Manifest

```rust
pub struct CapabilityManifest {
    pub read_paths: Vec<PathBuf>,        // bound read-only
    pub write_paths: Vec<PathBuf>,       // bound read-write
    pub descendants_allowed: bool,       // may the agent create processes
    pub env: BTreeMap<String, String>,   // the complete environment
}
```

Nothing else can be declared. There is no network, no devices, no `/proc`,
no `/tmp`, no IPC and no tools. Everything not declared is closed by
construction.

`plan()` rejects:

- relative paths;
- non-UTF-8 paths;
- `/` itself;
- missing paths;
- **non-canonical** paths (symlinks, `..`, trailing slash).

A declared symlink would make "the declared object" ambiguous, so it is
refused rather than resolved.

### Construction plan

`plan()` is a function of the manifest and the runtime's own identity
(uid, gid, supplementary groups, overflow gid). The plan is a list of
steps, and **the executor runs exactly that list**: `prepare()` turns each
step into one pre-built operation, and a failing step exits with its
index.

| # | Step | Closes |
|---|---|---|
| 0 | `unshare` user, mount, net, IPC, PID | host namespaces |
| 1–3 | `setgroups deny`; map **only** the runtime's uid and gid to themselves | root inside the userns (the agent is uid 1000, so execve drops all capabilities) |
| 4 | fork: the world's first process is PID 1 of the new pidns | visibility of, and signals to, outside processes |
| 5 | `mount --make-rprivate /` | propagation back to the host |
| 6 | empty tmpfs as the new root | everything not bound in |
| 7… | bind each declared path, shallowest first. Read paths are remounted ro, repeating the locked flags (`nosuid`, `nodev`, …) | — |
| | remount the root read-only | writes to the skeleton |
| | `pivot_root`, detach the host root | the host filesystem |
| | `setsid` | the controlling terminal |
| | `PR_SET_NO_NEW_PRIVS` | setuid and file capabilities |
| | clear inheritable and ambient capabilities | inherited capability sets (the inventory found `CAP_WAKE_ALARM` inheritable) |
| | `RLIMIT_NPROC = 1/1` unless descendants are allowed | process creation |
| | `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` | inherited file descriptors |
| | `env_clear` + declared variables | the inherited environment |

What the plan does **not** build, on purpose:

- no `/proc`, `/dev`, `/sys` or `/tmp`;
- no cgroup namespace (no cgroupfs is mounted, so migration has no
  interface);
- no network beyond the fresh netns's `lo`, which is down.

The observers adapt to this (see [Observers](#observers)) instead of
the world growing to suit them.

### Residue: what construction cannot remove

An unprivileged runtime cannot drop **supplementary groups** (Inventory
v0, step 1). The plan names them as accepted residue:
`Residue::SupplementaryGroups { host, inside }`. Here `inside` is what a
userns that maps only the runtime's gid shows: every other gid becomes
the overflow gid, 65534.

They are accepted on one argument, and the report states it as a
dependency of the claim:

- a group acts only through an object;
- the inside walk calls `access(2)` on **every** object in the view,
  which applies supplementary groups;
- so any authority a group grants appears as an `fs/read` or `fs/write`
  name, which C1 checks.

The argument holds only if the walk is complete. An incomplete walk
leaves `UnknownBelow` names, and C1 cannot be satisfied.

### Capability names

Observed and declared capabilities share one hierarchical namespace, so
"observed ⊆ declared" is the kernel's existing `Within` requirement. No
kernel change was needed.

| Name | Observed when | Declared by |
|---|---|---|
| `fs/read<p>`, `fs/write<p>` | `access(2)` grants it on non-directory `p` | `read_paths` / `write_paths`, as prefixes |
| `fs/read<d>/.`, `fs/write<d>/.` | same, on directory `d` | same; each skeleton ancestor `a` of a declared path as the **exact** `fs/read<a>/.` |
| `special<p>` | any socket, fifo or device node in the view | **never** |
| `env/<NAME>` | the variable is set | `env` |
| `proc/descendants` | `fork` succeeded | `descendants_allowed` |
| `net/if/<n>` | interface `n` is up | never |
| `net/connect/<t>` | a canary target accepted a connection | never |

The `/.` suffix solves one problem. Skeleton directories (`/`, `/nix`,
`/nix/store`, …) must be listable for the agent to reach a declared
path, but declaring them as prefixes would cover everything below them.
`fs/read/nix/.` is exact, and `fs/read/nix/store/x/.` is not within it.

**Special files are never declarable.** A socket is a channel to a
process. Its authority is whatever that process does for the caller,
which is declared knowledge, not observable (Inventory v0, finding 1).
So the manifest has no way to grant one.

### Unknown is never absence

Every observer gap becomes `Name::UnknownBelow`, which makes the kernel
return Undetermined, never Satisfied:

| Gap | Name |
|---|---|
| directory not listable; `lstat` failed; walk limit | `UnknownBelow` of `fs/read`, `fs/write` and `special` at that path |
| `fork` failed with anything but `EAGAIN` under a hard limit ≤ 1 | `UnknownBelow(proc/descendants)` |
| interface list unreadable, or flags unreadable | `UnknownBelow(net)` / `UnknownBelow(net/if/<n>)` |
| canary answered something other than `ECONNREFUSED`, `ENOENT` or `ENETUNREACH` (e.g. timeout) | `UnknownBelow(net/connect/<t>)` |
| declared path not `lstat`-able (other than `ENOENT`) | no C2 fact |
| outside `/proc` unreadable | no C3 fact |

One consequence, found while writing the unit tests: **an unreadable
directory inside a declared write path blocks the invariant.** The
subtree is in the declared `fs/*` scope, but the `special` names under
it are never declared, and the unreadable directory might hold a socket.
This is deliberate. The alternative would mean "we could not look, so
there is no channel".

### Invariants

All three are `Phase::Post` obligations given to `kernel::evaluate`.

| Id | Statement | Requirement |
|---|---|---|
| `C1.observed_within_declared` | **observed capabilities ⊆ declared capabilities** | one `Within { names: observed, scopes: declared }` |
| `C2.declared_visible` | each declared capability is present **and is the declared object** | `Fact(Declared(name)) = Granted`, Hard, per declared name |
| `C3.closed` | no inherited state outside the plan | `Fact(Closing(x)) = Holds`, Hard, for NoNewPrivs, NoCapabilities, Groups (= residue), FdTable, and Namespace × {user, mnt, net, ipc, pid} |

C2 compares **identity, not names.** The plan records `(st_dev, st_ino)`
of every declared path on the host. The probe reports `lstat` of the
same path inside. A bind mount preserves both, so a path that resolves
to another object inside (a failed or substituted mount) is
`NotGranted`, and the obligation is Violated.

### Observers

Two observers, because each one sees something the other cannot.

**Inside** (`world_probe.py`). It runs as the only program in the world.
Its source is passed on argv (`python3 -I -c`), so the world holds no
observer files.

- Walks the whole view from `/`. For every entry it records the
  `lstat` kind and `access(R_OK)` / `access(W_OK)`, which reflect the
  real ids, the groups and the mount flags.
- Reads its identity through syscalls: `getgroups`, `capget`,
  `prctl(PR_GET_NO_NEW_PRIVS)`, `PR_CAP_AMBIENT_IS_SET`. There is no
  `/proc` to read.
- Makes one `fork` attempt.
- Reads interface flags (`SIOCGIFFLAGS`).
- Connects to each canary target.
- Prints one JSON line, then **holds** until the runtime closes its
  stdin.

**Outside** (the runtime, through host `/proc/<pid>` of the held probe):

- `ns/{user,mnt,net,ipc,pid}` compared with the runtime's own;
- the complete fd table, each link compared with the runtime's own three
  pipes.

These are kernel tables. They are complete, and no code in the world
reports them. The inside probe cannot list its fds or name its
namespaces without `/proc`, and the world is better without one.

**Trust.** The inside report is attested `Verified` with the provenance
`world-probe`, basis `probe sha256 <digest> in plan <digest>`. That rests
on three facts:

- the probe is the world's only process;
- its source is part of the trusted crate;
- its report crosses a runtime-owned pipe.

If the world ran an agent before the probe, none of this would hold.
`Observation` has no public constructor, so code outside the crate
cannot get `Verified` capability facts from arbitrary JSON.

## Experiment

`capability_manifest.rs` runs two worlds over one set of canaries.

Each canary is **proven live from the ambient world first**: connect and
accept.

| Canary | Where |
|---|---|
| abstract unix socket `@v9r-canary-<tag>` | host netns |
| path socket `/tmp/v9r-canary-<tag>.sock` | outside the view |
| path socket `<base>/outside/spawner.sock` | sibling of a declared path |
| TCP `127.0.0.1:<port>` | host netns |
| file `<base>/outside/canary-secret.txt` | sibling of a declared path |
| that file opened **without** `O_CLOEXEC` in the runtime | inherited-fd canary |
| the runtime's own environment (`HOME`, `PATH`, `XDG_RUNTIME_DIR`, …) | env canary |
| `<base>/work/inside-canary.txt` | **inside** a declared write path |

**World A (the claim):**

- `read_paths` = the interpreter's nix closure (`nix-store -qR`) +
  `<base>/shared`;
- `write_paths` = `<base>/work`;
- no descendants;
- `env` = `{LC_ALL=C, V9R_WORLD=a}`.

`LC_ALL` is set so that Python's C-locale coercion does not add
`LC_CTYPE` to its own environment. Without it, the observer would
perturb the observation.

**World B (negative and positive control):**

- same, except `write_paths` = `<base>/smuggle`, which holds a **live
  socket bound by an outside listener**;
- descendants allowed.

### Predictions

| # | Prediction | Falsified by |
|---|---|---|
| P1 | World A: verdict **Allow**; C1, every C2 and every C3 Satisfied | any other verdict |
| P2 | A: `work/inside-canary.txt` walked `(f, r, w)`; `shared/data.txt` walked `(f, r, ¬w)`; interpreter readable | either one missing or with the wrong access |
| P3 | A: `outside/`, the secret file, the `/tmp` socket, `smuggle/`, `/proc` and `/dev` are not in the walk | any of them walked |
| P4 | A: every canary target answers something other than `connected` (abstract: `ECONNREFUSED`; TCP: `ENETUNREACH`; paths: `ENOENT`) | any `connected` |
| P5 | A: no `env/HOME`; no `proc/descendants` (fork `EAGAIN`, `RLIMIT_NPROC` 1/1) | either present |
| P6 | A: outside fd table = {0, 1, 2} → the runtime's pipes; the canary fd is absent | any fd ≥ 3 |
| P7 | A: inside groups = residue: one gid 100 and the rest 65534, the same count as the host's 10 | other gids, or another count |
| P8 | B: the walk finds the smuggled socket as kind `s`; C1 **Violated** with `special/…`; verdict **Deny** | Allow or Blocked |
| P9 | B: `proc/descendants` C2 Satisfied (fork ok inside the pidns); pid namespace C3 Satisfied | fork refused |

Unit tests (synthetic observations, in `capability.rs`) cover the
checker's logic independently of the host:

- a clean report gives Allow;
- each of 8 undeclared capabilities gives Deny through C1;
- each of 6 observer gaps gives Blocked;
- an unreadable directory inside a declared path gives Blocked;
- a substituted declared object gives Deny; an unseen one gives Blocked;
- each of 5 closing breaks gives Deny;
- 2 unreadable closing facts give Blocked;
- plan validation and mount order.

## Results

Measured on 2026-10-02, on this host (Linux 6.12.80, unprivileged,
uid 1000, gid 100, 10 supplementary groups). `kernel.rs` sha256
`85badb66…6177f`, unchanged. Probe sha256 `b6f8b0f7…3550d97`.

```
cargo test -p v9r-core --test capability_manifest -- --nocapture
test result: ok. 1 passed; 0 failed; finished in 1.25s

cargo test --workspace
291 passed, 0 failed, 6 ignored   (baseline 281 + 9 unit + 1 live)
```

### Predictions

| # | Outcome | Measured |
|---|---|---|
| P1 | **Held** | World A: verdict `Allow`. C1 is `9238 name(s) within` the scopes. Every C2 and C3 obligation is Satisfied |
| P2 | **Held** | `inside-canary.txt` `(f, r, w)`, `data.txt` `(f, r, ¬w)`, interpreter readable |
| P3 | **Held** | `outside/`, the secret, the `/tmp` socket, `smuggle/`, `/proc` and `/dev` are absent from a walk of 9,234 entries with 0 unknown |
| P4 | **Held** | abstract `ECONNREFUSED`, TCP `ENETUNREACH`, both paths `ENOENT`. All four canaries were reachable from the runtime |
| P5 | **Held** | env is exactly `{LC_ALL, V9R_WORLD}`. fork `EAGAIN` with `RLIMIT_NPROC` `[1, 1]` |
| P6 | **Held** | outside fd table `{0, 1, 2}` = the runtime's three pipes. The canary fd (open without CLOEXEC in the runtime) is absent |
| P7 | **Held** | groups `[65534 ×9, 100]`, the same count as the host's 10 |
| P8 | **Held** | World B: the smuggled socket is walked as `s`. C1 Violated with exactly one name, `special/<base>/smuggle/agent.sock`. Verdict `Deny` |
| P9 | **Held** | B: fork `ok`, `RLIMIT_NPROC` back to the host's 126476. `proc/descendants` C2 Satisfied. pid namespace C3 Satisfied |

The untested assumptions listed before the run (locked mount flags,
`RLIMIT_NPROC` charged per userns, Python with no `/dev` and no `/proc`,
`CONFIG_PROC_CHILDREN`) all held. None of them failed.

All five namespaces differ from the runtime's (`user`, `mnt`, `net`,
`ipc`, `pid`), read from `/proc/<pid>/ns` outside. World A's plan has 40
steps, including 23 read-only binds for the interpreter's closure, plus the
`shared` and `work` binds.

### World W: what the first run actually measured

The first run used the `python3` on PATH, as the code was written to.
On this host that is a nix `makeCWrapper` binary (`python3-3.12.13-env`).
It sets `PYTHONNOUSERSITE=true` and then execs the real interpreter.
That run gave `Deny`. C1 named exactly one capability outside the
manifest:

```
world W outside the manifest: ["env/PYTHONNOUSERSITE"]
world W verdict: Deny
```

The check was right and the prediction was wrong. The runtime passed
exactly the declared env (step 39, `env exactly [LC_ALL, V9R_WORLD]`).
The world's first program changed its own state before the probe could
read it.

This shows the boundary of what C1 is about. **C1 observes the state
the agent holds at the moment of observation, not the state the runtime
passed.** It cannot attribute that state to its source. Here that is
the right failure direction: it denied rather than allowed. It also
means that every program between `exec` and the observer is part of the
trusted base, whether it is a launcher, a loader or an interpreter
start-up. The "Observers" section already lists the interpreter. The
wrapper was a second, unlisted program in front of it.

The test keeps this as world W, a regression. When the PATH `python3`
is a wrapper, world W must be `Deny`, and every out-of-manifest name
must be an `env/` name. Worlds A and B now use the real interpreter,
resolved through `/proc/self/exe`. The result is a closure of 23 store
paths instead of 186, and a walk of 9,234 entries instead of 62,402.

## Evaluation

**The claim (P1–P9 held).** For a world W built from
manifest M on this host:

- every capability the inside walk, identity, process and network
  observers could see is declared in M;
- every declared one is present as the declared object;
- the world shares no namespace with the runtime, holds only the
  runtime's pipes, cannot gain privileges, holds no capabilities, and
  its only inherited identity state is the named group residue.

**What the claim trusts:**

- the kernel;
- `prepare` / `construct`;
- the probe source and the interpreter in the closure (the probe runs
  on it, so a compromised interpreter is a compromised observer);
- the argument that `access(2)` over a complete walk accounts for
  supplementary groups;
- the declared meaning of "read path" and "write path".

**What it does not cover:**

- **Inbound authority.** Any host process with the runtime's uid has
  every capability in the world's userns (it owns it). It can `setns`
  into the world or `ptrace` PID 1. That is authority *over* the agent,
  not *of* it, and nothing here observes it.
- **Effective access inside a declared path depends on the inherited
  identity.** A declared read path grants "whatever this uid and these
  groups may read there". The manifest names paths, not permissions.
- **Submounts below a declared path** are not carried (non-recursive
  bind). They appear as their underlying directories. That is safe, but
  can be surprising.
- **Threads.** `RLIMIT_NPROC = 1` also forbids threads. A multithreaded
  agent needs `descendants_allowed`, or a finer mechanism (seccomp on
  `clone` flags), which v0 does not have.
- **Time.** The observation is one instant. A world that later gains a
  socket through a declared write path (an outside process binding
  into it) is not re-checked. World B shows the checker would catch it
  *if* it observed again.
- **The harness tool plane** (Inventory v0, finding 4). Still outside
  any OS construction.

## Failed assumptions

Rows 1–3 come from writing the system. Rows 4–6 come from running it.

| # | Assumption | Outcome |
|---|---|---|
| 1 | An unreadable directory inside a declared path is within scope | **False.** Its `fs` names are within scope, but its possible `special` names are not. My first unit-test expectation (Allow) was wrong; corrected to Blocked |
| 2 | The world needs `/proc` for the inside observer | **False.** Identity comes from syscalls; the fd table and namespaces come from outside, where they are complete and not reported by code in the world |
| 3 | `prctl` arguments can be passed as plain integers | **Unsafe.** `PR_SET_NO_NEW_PRIVS` rejects nonzero unused arguments, read as `unsigned long`. Every variadic argument is now passed at `c_ulong` width |
| 4 | `Command::spawn` returns once the world's program execs | **False: deadlock.** std's spawn waits for EOF on a CLOEXEC status socket. The intermediate process (the parent of PID 1, outside the new pid namespace) never execs, so it held that socket open and `spawn` never returned. Then nobody read the probe's stdout, and PID 1 blocked in `pipe_write`. Fix: the intermediate `close_range(0, ~0)` before `waitpid`. That also drops its copies of the runtime's pipes and inherited fds |
| 5 | The PATH `python3` is the interpreter | **False on nix.** It is a `makeCWrapper` launcher that adds `PYTHONNOUSERSITE`. C1 caught it (world W). The observer is now the `/proc/self/exe` binary |
| 6 | The experiment could not be run (session blocked) | Resolved. Compiled and run on 2026-10-02 |

## Next falsification experiment

After the run fills in [Results](#results):

**An agent, not a probe, first.** The trust argument depends on the
probe being the only process. The next step:

1. run an adversarial agent in the world (try every closed class:
   `fork`, connecting to canaries, writing to read paths, binding a
   socket in the write path, `setns` attempts);
2. **then** run the probe in a *second* process entering the same
   namespaces from the runtime side;
3. check C1 again.

Falsified if the agent leaves anything (a socket, a fifo, a held
descendant when none are allowed) that the second observation misses.

## Reproducing

```
cargo test -p v9r-core --test capability_manifest -- --nocapture
```

It takes about 1.3 s. It needs:

- unprivileged user namespaces;
- `python3` from the nix store;
- `nix-store`.

World W runs only when the PATH `python3` is not the binary it execs.

Without nix it prints `SKIPPED (environment, not evidence)` and passes
vacuously. That is a gap in the test, not evidence.
