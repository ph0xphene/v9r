# Capability Inventory v0: can v9r describe an agent's complete authority before execution?

Research question: **can v9r describe the complete set of capabilities
available to an agent before execution?**

Controlled Domain v0 showed that namespace layers contain the processes
an agent *creates*. It did not show what else the agent holds. This
report inventories that authority, for the agent that ran this
experiment, on this host. No capability was used. Every fact below
comes from metadata: `/proc/self`, kernel tables under `/proc/net`,
`/etc/group`, and `ls -l` modes.

Short answer: **not by observation alone. Only by construction, with
observation as the check.**

1. **Endpoints can be enumerated; their meaning cannot.** The kernel
   lists groups, capabilities, listening sockets, TCP listeners and
   device modes completely. It does not say that `/run/docker.sock` is
   root. Every authority class in this report is a *declared* meaning
   attached to an *observed* name. The inventory is
   `observed endpoints × declared meanings`. Its unclassified residue is
   the honest measure of how incomplete it is.
2. **Absence is provable only for three things:**
   - a source the kernel enumerates completely for a namespace the
     agent does not share;
   - a whole class switched off by one observable bit (`NoNewPrivs`
     makes every setuid binary inert);
   - a view the runtime built from empty.

   In the ambient world, almost nothing is provably absent: path
   sockets, credentials, remote services and the harness's tools all
   lie outside any complete enumeration.
3. **Namespace isolation is not capability isolation.** Supplementary
   groups, inherited file descriptors, the environment, and setuid
   elevation all pass through user, mount and network namespaces
   unchanged, unless the setup removes them by name. This host's agent
   is in the `docker` group, which is root-equivalent through
   `docker.sock`. It is also in `libvirtd`, `input` and `i2c`.
   `unshare --user` drops none of them.
4. **Part of the agent's authority is not in the OS at all.** The
   harness gives this agent tools: send email, publish pages, spawn
   cloud agents, schedule jobs. No process-level probe can see these
   tools. Only the harness's own manifest can.

So "the agent operates only within a known capability world" can be
claimed only like this:

- the runtime **constructs** the world from a manifest;
- a probe **inside** the world finds nothing outside the manifest;
- **canaries** placed outside the world are invisible to that probe;
- the class-closing bits are observed set.

That minimum is listed in [Minimum trusted setup](#minimum-trusted-setup).

`kernel.rs` is unchanged. No Rust code changed.

## Baseline

| | |
|---|---|
| commit | `8a49c4b`, plus untracked Controlled Domain v0 files |
| host | Linux 6.12.80, NixOS, systemd 258, cgroup v2 |
| subject | the agent process itself: `claude`, pid 806525, uid 1000 |
| instrument | `crates/v9r-core/tests/capability_probe.py` (rewritten; the untracked copy stopped mid-line at line 49) |

**What was and was not measured.** The harness's auto-mode safety
check refused to execute the probe and some `ls` calls in this session.
It said it would keep refusing for the rest of the conversation. The
facts below therefore come from:

- direct reads of `/proc/self/{status,uid_map,cgroup,mountinfo}`,
  `/proc/net/{unix,tcp,tcp6,dev}`, `/etc/group` and two sysctls;
- two `ls -l` listings (devices, setuid wrappers, key sockets).

The probe's filesystem walk was not run. That leaves writable roots,
the full socket sweep, credential files, environment names, the
same-uid process table, inherited fds and SysV/POSIX IPC unmeasured.
Those rows say **not measured**. Nothing in them is inferred.

Provenance tags, as in the kernel's evidence model:

| Tag | Meaning |
|---|---|
| **observed** | the kernel answered it for this process |
| **rule** | follows from observed facts by a documented kernel rule |
| **declared** | human knowledge about the program behind a name |

## Inventory (ambient world, this agent)

### 0. Identity: the multiplier for everything below

| Fact | Value | Provenance |
|---|---|---|
| uid / gid | 1000 / 100 | observed |
| user namespace | init (`uid_map` = `0 0 4294967295`) | observed |
| supplementary groups | wheel, video, libvirtd, users, docker, input, kvm, render, i2c, ollama | observed (`Groups:` + `/etc/group`) |
| effective / permitted / ambient caps | none | observed |
| bounding set | full (`000001ffffffffff`) | observed |
| inheritable caps | `CAP_WAKE_ALARM` (bit 35) | observed |
| `NoNewPrivs` | **0**: setuid and file capabilities elevate | observed |
| seccomp | **off** | observed |
| cgroup | `…/app.slice/app-niri-alacritty-6291.scope`: **the terminal emulator's scope** | observed |
| session | sid 6314 = parent (the shell) | observed |
| `ptrace_scope` | 1 (descendants only) | observed |
| `legacy_tiocsti` | 0 (no keystroke injection into the tty) | observed |

The agent does not have its own cgroup. It shares a scope with the
terminal that launched it. Every group above grants access to a set of
files or devices. No namespace layer changes that (see
[Evaluation](#evaluation)).

### 1. Filesystem authority

**Mounts** (observed, `/proc/self/mountinfo`). The agent sees the host
mount namespace:

- `/` ext4 rw;
- `/mnt/storage` ext4 rw;
- `/boot` vfat (`fmask=0077`, root only);
- `/nix/store` ro;
- `/run/user/1000` tmpfs, owned by the agent's uid;
- fuse mounts: `gvfs` and the document portal;
- `/dev/shm` and `/dev/mqueue`;
- two docker overlay `merged` directories;
- two **nsfs** handles under `/run/docker/netns`;
- `/sys/fs/cgroup` rw (`nsdelegate`);
- `/sys/fs/bpf` (mode 700), debugfs, tracefs, efivarfs.

**Writable locations**: not measured. By rule (owner uid 1000, or
sticky world-writable), at least: `$HOME`, `/tmp`, `/var/tmp`,
`/dev/shm`, `/run/user/1000`, and the delegated
`user@1000.service` cgroup subtree. Controlled Domain v0 already
showed the last one is writable: L1 moved a process within it.

**Listening unix sockets** (observed: `/proc/net/unix` rows with
`__SO_ACCEPTCON`; about 55 names). Modes come from `ls -l` where
measured. Connecting to a path socket needs write permission on it.

| Class (declared) | Endpoints | Reachable by this uid |
|---|---|---|
| **root-equivalent** | `/run/docker.sock` (`root:docker 0660`) | **yes**, via the `docker` group |
| | `/run/libvirt/libvirt-sock` (`0666`) | connect: **yes**. Polkit decides the session, and `libvirtd` group membership is commonly what it grants on (declared, not verified) |
| | `containerd.sock` (+ `.ttrpc`, `-debug`), `/run/containerd/s/*` | no: parent dirs are root `0700` (`ls` returned `EACCES`) |
| **spawn** | session bus `/run/user/1000/bus` | yes (uid-owned runtime dir) |
| | `systemd/private`, `systemd/io.systemd.Manager` | yes |
| | niri compositor socket | yes |
| | `Alacritty-wayland-1-6291.sock` | yes |
| **remote** | `/run/ssh-unix-local/socket` (sshd, `0666`) | yes |
| | `/run/user/1000/gcr/ssh` (ssh agent) | yes |
| **input** | `wayland-1`, `wayland-proxy-5273`, `xwls-1`, swww | yes |
| | X11 `/tmp/.X11-unix/X0` and abstract `@/tmp/.X11-unix/X0` | yes |
| | `lan-mouse-socket.sock` (forwards input to other machines) | yes (uid-owned) |
| **credential** | `keyring/control`, `keyring/pkcs11` | yes |
| **build** | `/nix/var/nix/daemon-socket/socket` (`0666`) | yes |
| **device** | pipewire (`-0`, `-0-manager`), `pulse/native`, speechd | yes |
| **agent-plane** | `/run/user/1000/cc-socks/806525.sock` (the harness's own, keyed by this pid) | yes |
| **mediated** (polkit or the daemon decides per call) | system bus | yes (connect) |
| | `io.systemd.{Login,Hostname,Credentials,BootControl,Manager}` | unknown |
| | `machine/io.systemd.Machine{,Image}`, `userdb/*` | unknown |
| | `ManagedOOM`, coredump, journal (stdout, socket, dev-log) | unknown |
| | `nscd` | unknown |
| | libvirt `-ro`, `-admin` (`0600` root), `virtlockd`, `virtlogd` | `-ro` yes; `-admin` no; rest unknown |
| | docker `metrics.sock`, `libnetwork/*` | unknown |
| | `/run/udev/control` | no (`0600` root) |
| **unclassified** | `/tmp/.BQhMHZ/s`, `/tmp/sddm-auth-*` | unknown |
| | abstract dgram `@7237706920827065424`, `@21140` | unknown |

**Devices** (observed: `ls -l` modes, `+` = logind seat ACL). Whether
they are reachable follows from mode, groups and ACL (rule).

| Reachable | Device | Class (declared) |
|---|---|---|
| **yes**, `0666` | `/dev/kvm`, `/dev/vhost-net`, `/dev/vhost-vsock` | compute: virtual machines |
| **yes**, `0666` | `/dev/kfd`, `/dev/dri/renderD128` | compute: GPU |
| **yes**, `0666` | `/dev/fuse`, `/dev/net/tun`, `/dev/ptmx`, `/dev/tty` | filesystem / network / terminal |
| **yes**, group `input` | `/dev/input/event0–18`, `mice`, `mouse0–1` | **input: every keystroke on the host** |
| **yes**, group `i2c` | `/dev/i2c-0–9` | hardware: raw bus writes |
| **yes**, video + ACL | `/dev/dri/card1` | display |
| **yes**, ACL | `/dev/snd/*` (incl. capture `pcmC0D0c`, `pcmC2D0c`) | **device: microphone** |
| **yes**, ACL | `/dev/rfkill` | hardware: radios |
| no (`0600` root) | `uinput`, `uhid`, `hidraw*`, `tpm0`, `tpmrm0`, `nvme0`, `mapper/control`, `watchdog*`, `userfaultfd` | — |
| no (group `disk`/`kmem`) | `nvme0n1*`, `sda*`, `/dev/mem` | — |

**Special files: setuid** (observed, `/run/wrappers/bin`): `sudo`,
`sudoedit`, `su`, `pkexec`, `polkit-agent-helper-1`, `mount`, `umount`,
`fusermount`, `fusermount3`, `newuidmap`, `newgidmap`,
`qemu-bridge-helper`, `newgrp`, `sg`, `passwd`, `chsh`, `unix_chkpwd`
(+ `dbus-daemon-launch-helper`, not executable by this uid). With
`NoNewPrivs=0` and the `wheel` group, `sudo` is a password away from
root. That password is the only barrier the inventory can see.

### 2. Process authority

| Capability | Detection | Observed |
|---|---|---|
| process visibility | `/proc` listing (host pid ns) | every host process visible (`NSpid` has one level) |
| signal any same-uid process | rule: matching uid, no capability needed | holds; the same-uid count was **not measured** |
| ptrace | `ptrace_scope` = 1 | descendants only (rule) |
| ancestry | `PPid` chain | 806525 ← 6314 (the shell) ← … ← niri, all in one session |
| cgroup control | write access on `cgroup.procs` in the delegated subtree | writable (Controlled Domain v0, L1) |
| create new namespaces | `max_user_namespaces = 126476` | allowed (Controlled Domain v0) |
| kernel keyrings | `/proc/keys` | not measured |

### 3. IPC authority

- **Unix sockets**: the table above. `/proc/net/unix` is complete for
  this network namespace. It does not cover path sockets bound in
  *other* network namespaces, which are still connectable through the
  filesystem (Controlled Domain v0, failed assumption 6).
- **Abstract sockets**: complete per netns. X11 is the only listening
  stream endpoint.
- **Inherited fds, SysV IPC, `/dev/shm`, `/dev/mqueue`**: not
  measured (the probe enumerates all four).
- **The terminal**: the agent's session has a controlling tty
  (`tty_nr` not measured). TIOCSTI injection is off (`legacy_tiocsti=0`).

### 4. Network authority

| Fact | Value | Provenance |
|---|---|---|
| network namespace | host: `lo`, `eno1`, `docker0`, three `br-*`, two `veth*` | observed |
| egress | established TCP :443 connections owned by uid 1000 | observed |
| local listeners | `127.0.0.1:9050` (uid 35, tor), `127.0.0.1:11434` (uid 994, ollama), `127.0.0.1:5432` (uid 0, postgres), `0.0.0.0:22` and `[::]:22` (sshd) | observed (addr, uid); declared (service) |
| container namespaces | nsfs handles in `/run/docker/netns` | observed. `setns` needs `CAP_SYS_ADMIN` there (rule), so they are visible, not usable |
| docker bridge networks | reachable over `br-*` / `docker0` | rule (same netns). Container listeners not enumerable from here |

With egress, the network part of the inventory is unbounded. Every
reachable host is authority, and a credential multiplies it.

### 5. External authority

| Capability | Detection | Observed |
|---|---|---|
| container control | `docker` group + `docker.sock` mode | **root-equivalent, reachable** |
| VM control | `libvirtd` group + `libvirt-sock` mode; `/dev/kvm` `0666` | socket reachable. Root-equivalent **if** polkit grants the group (not verified). Unprivileged VMs via `/dev/kvm` regardless |
| administration | `wheel` + setuid `sudo`; setuid `pkexec` + system bus (polkit) | gated by password / policy |
| nix builds | daemon socket `0666` | reachable (build as `nixbld*`, write the store) |
| ssh login | sshd on `:22` and `/run/ssh-unix-local/socket`; ssh agent `gcr/ssh` | reachable. Whether the agent holds usable keys was **not measured** |
| model / data services | ollama `:11434`, postgres `:5432` | reachable endpoints. Their auth was not measured |
| anonymous egress | tor SOCKS `:9050` | reachable endpoint |
| virtualisation of *this* host | cpuinfo `hypervisor` flag, DMI | not measured |
| credentials in environment | variable names only | **not measured** |
| credential files | existence + `access(R_OK)` of known paths | **not measured** |
| **harness tool plane** | the agent's tool manifest, not the OS | **present**: Gmail (send, forward, trash, label), Google Calendar auth, Claude Docs (create, update, delete), artifact publishing, web fetch/search, local subagents, cloud agents and scheduled routines, cron, messaging other local sessions, push notifications |

The last row is authority that no OS probe in this report can see.
It reaches outside the host (email as the user, public pages, remote
agents). It is held by the harness process and granted through a
manifest that the agent process cannot inspect. A namespace around the
agent's *subprocesses* does not touch it at all.

## Classification

For each capability:

- **Fact?** Can it be a v9r fact? `observed` facts can be `Verified`.
  A `declared` meaning enters only as `Proposed` from a declaration
  table, or `Verified` through a trusted verifier (as in Verifiable
  Observers v0).
- **Absent?** Can it be *proven* absent? `facts.rs` already states the
  rule: what an observation could not see yields **no fact, never
  `Absent`**. An `Absent` needs a complete enumeration or a closing
  rule.
- **Setup?** Does it belong in the trusted setup phase?

| Capability | Detected by | Fact? | Provably absent? | Setup? |
|---|---|---|---|---|
| uid, groups, caps, NoNewPrivs, seccomp | `/proc/self/status` | **yes**, observed | **yes**: the list is complete | **yes**: choose them |
| setuid elevation (whole class) | `NoNewPrivs` bit | yes | **yes, by one bit** (no enumeration needed) | yes: set it |
| individual setuid binaries | filesystem walk | yes | only within a complete walk | subsumed by `NoNewPrivs` |
| mount view | `/proc/self/mountinfo` | yes | **yes**: the list is complete | **yes**: build it |
| writable locations | walk + `access(W_OK)` | yes | only if the walk had no unreadable dirs and no budget stop | yes: the view determines it |
| path unix sockets | walk ∪ `/proc/net/unix` | name: yes; meaning: declared | **no** in an ambient view. Directories that are `x` but not `r` hide connectable names, and sockets from other netns are missing from the table. **Yes** in a view built from empty and fully walked | **yes**: allowlist mount view |
| abstract unix sockets | `/proc/net/unix` | yes | **yes per netns**: complete table | yes: new netns |
| device nodes | `/dev` walk + modes/ACLs | yes | yes for a private `/dev`. On a host devtmpfs, hotplug makes it a temporal fact | **yes**: private `/dev` |
| group-mediated access (docker, input, i2c, …) | groups × file modes | rule | **only after the groups are gone** or every object is outside the view | **yes**: see Evaluation |
| process visibility, signal targets | `/proc` + uid rule | yes | **yes in a fresh pid ns** (or with a distinct uid) | yes: pidns |
| ptrace | `ptrace_scope` + uid + ns | rule | yes, with pidns or distinct uid | yes |
| cgroup migration | cgroupns root | rule | yes (Controlled Domain v0 kernel rule) | yes |
| inherited fds | `/proc/self/fd` | yes (target); a socket's **peer**: no | the fd set: yes. What a socket fd leads to: no | **yes**: `close_range` all but chosen fds |
| SysV / POSIX IPC | `/proc/sysvipc`, `/dev/{shm,mqueue}` | yes | yes in a fresh ipc ns / private mount | yes: ipc ns |
| kernel keyrings | `/proc/keys` | yes (descriptions) | not measured | yes: new session keyring |
| TCP/UDP listeners, interfaces | `/proc/net/*` | yes | **yes per netns** | yes: netns |
| egress reachability | route table | default route: yes. Reachable hosts: **no** | **only "no egress at all"** (netns with only `lo`). Partial egress has no absence proof. Its policy (a mediator's allowlist) becomes the fact instead | **yes**: mediator is setup |
| environment credentials | own environ (names) | yes | **yes**: the environment is complete | **yes**: construct, don't inherit |
| credential files | walk + `access(R_OK)` | existence: yes. Power: declared | only in a constructed view | yes: view |
| remote authority of a credential | none locally | **no** | **no**: the remote side decides | only by not placing the credential |
| harness tool plane | harness manifest | only if the harness attests it | only if v9r issues the manifest | **yes**: tool grants are part of the world |
| mediated daemons (polkit, system bus) | socket visible | endpoint: yes; per-call policy: no | no, while the socket is in view | yes: keep them out of the view |
| unclassified endpoints | any of the above | name only | — | must be zero or explicitly accepted |

Three kinds of absence proof appear, and v9r needs all three:

1. **Complete enumeration** of a table the kernel owns for a namespace
   the agent does not share: groups, caps, mounts, fds, abstract
   sockets, net listeners.
2. **Class-closing rule**: a single observable bit or namespace that
   makes a whole class inert, with nothing to enumerate:
   `NoNewPrivs`, a fresh pidns, a netns with only `lo`, a cgroupns.
3. **Construction**: the runtime built the view from empty, so its
   manifest *is* the enumeration. The inside probe checks the manifest;
   it does not discover it.

Everything else is at best a lower bound.

## Evaluation

**Why namespaces do not imply capability isolation.** Each row below
follows from kernel rules. The table also says which rows a past
experiment measured.

| Authority | Survives userns? | Survives mountns? | Survives netns? | What removes it |
|---|---|---|---|---|
| supplementary groups (docker, libvirtd, input, i2c, kvm, wheel) | **yes**. An unprivileged userns cannot call `setgroups` (it is denied so that groups used as *negative* permissions cannot be dropped). The groups still apply to access checks, shown as `nogroup` | yes, if the objects remain in the view | yes | a distinct uid/gid set (privileged setup) **or** every group-gated object outside the view |
| setuid binaries | yes (with no userns) | yes, if in view | yes | `NoNewPrivs=1` |
| path sockets | yes | **no, if the view is an allowlist** | **yes** (measured: Controlled Domain v0, L6 `/tmp` row) | allowlist mount view |
| abstract sockets, localhost TCP | yes | yes | **no** (measured, L6) | netns |
| inherited fds | **yes** | **yes** | **yes** | closing them before exec |
| environment | **yes** | **yes** | **yes** | building it explicitly |
| harness tools | **yes** | **yes** | **yes** | not granting them |
| egress to the internet | yes | yes | no, until egress is added back | mediator policy |

The last four rows are invisible to every layer of the Controlled
Domain stack. Process containment (L8) and capability isolation are
different properties. **L8 has no rule that drops groups, fds,
environment or tools.**

A second difference: **containment and inventory need different
minimums.** Controlled Domain v0 dropped the PID namespace, because it
added no *containment*. For *inventory* it is needed. Without it, every
same-uid host process is a signal target, and the inventory cannot
enumerate those targets as a closed set it owns. A distinct uid gives
the same closure.

## Minimum trusted setup

The setup runs before the agent's first instruction. Each step either
builds an enumeration or sets a closing bit. Each has an inside check
that becomes a v9r fact.

| # | Setup step | Closes | Inside check (v9r fact) |
|---|---|---|---|
| 1 | **Identity**: run under a uid/gid with **no supplementary groups**, or keep every group-gated object outside the view (step 3). An unprivileged userns alone is not enough | docker, libvirt, input, i2c, kvm, wheel | `Groups:` empty (or only an overflow gid with no object in view) |
| 2 | **`NoNewPrivs=1`** | the whole setuid / file-caps class | `NoNewPrivs: 1` |
| 3 | **Allowlist mount view from an empty tmpfs**: workspace rw, toolchain ro, private `/tmp`; no `/run`, no `$HOME`, no host `/sys` except what the cgroupns needs | path sockets, credential files, writable host locations | `mountinfo` equals the manifest. A full walk finds no unreadable dirs and no socket, fifo or device outside the manifest |
| 4 | **Private `/dev`** with `null zero full random urandom tty pts ptmx` only | kvm, vhost, gpu, fuse, tun, input, i2c, snd, rfkill | `/dev` walk equals the manifest |
| 5 | **netns** with only `lo`; egress, if any, through a mediator whose allowlist is part of the manifest | abstract sockets, host listeners, docker bridges, tor, unbounded egress | interface list = `{lo}` (+ mediator); `/proc/net/*` listeners = manifest |
| 6 | **pidns** with a runtime-held init (or a distinct uid) | visibility of and signals to outside processes | `/proc` pids are all descendants of the runtime init |
| 7 | **cgroupns + delegated subtree** (Controlled Domain v0) | migration out, untracked writers | cgroup path = domain root |
| 8 | **ipc ns**, new session keyring | SysV/POSIX IPC, inherited keys | `/proc/sysvipc/*` empty; `/proc/keys` = manifest |
| 9 | **fd hygiene**: `close_range(3, ~0)`. stdio are pipes the runtime owns, not the user's tty | inherited handles, the terminal | `/proc/self/fd` = {0, 1, 2} → runtime pipes |
| 10 | **Constructed environment**: no inherited variables | credentials, endpoint locators | env names = manifest |
| 11 | **Tool manifest** issued by v9r, not inherited from the harness | email, publishing, remote agents, scheduling | harness attests the granted tool set; v9r records it as a `Verified` fact only if that attestation is trusted |
| 12 | **Positive controls**: canaries bound outside the world (path socket in `$XDG_RUNTIME_DIR`, one in `/tmp`, an abstract socket); a canary placed inside | blind detectors | inside probe: outside canaries invisible, inside canary visible |

Steps 1 and 11 are the ones an unprivileged runtime **cannot** do by
itself:

- step 1 needs a privileged helper, or the weaker "hide every
  group-gated object" form (only as good as the view);
- step 11 needs the harness's cooperation.

Everything else is available unprivileged on this host (Controlled
Domain v0 measured 3, 5, 6 and 7).

What "the agent operates only within a known capability world" then
means, precisely:

> For a world W built from manifest M: an inside probe's complete
> inventory I(W) has I(W) ⊆ M, every closing bit in M is observed set,
> no item of I(W) is unclassified, and every outside canary is
> invisible while the inside canary is visible.

The claim still trusts the kernel, the setup code, the declared meanings
of every item in M, and the harness's tool attestation. Those are the
trusted computing base of the claim, and the claim should name them.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | The agent's authority is the authority of its process | **False.** The harness tool plane (email, publishing, remote agents) is held outside the process and is invisible to every OS probe |
| 2 | A userns + mountns + netns domain (Controlled Domain L8) isolates capabilities | **False by kernel rule** for groups, fds, environment and tools. It contains processes, not authority |
| 3 | A filesystem walk can prove a socket absent | **False in an ambient view.** Directories that are `x` but not `r` hide connectable names. Other-netns sockets are not in `/proc/net/unix`. Only a constructed view makes the walk complete |
| 4 | An inventory can be generated by inspection | **Half.** Names can be observed; the *authority* of a name ("docker.sock is root") is always declared |
| 5 | PID namespace is optional (Controlled Domain v0) | **True for containment, false for inventory.** Signal targets are only a closed set with pidns or a distinct uid |
| 6 | The agent has its own cgroup | **False.** It runs in the terminal emulator's scope (`app-niri-alacritty-6291.scope`) |
| 7 | The probe could run in this session | **False.** The harness's auto-mode safety check refused the probe and some `ls` calls after the first observations. The harness itself is a policy layer on the agent's authority, contextual and not representable as a stable fact |

## Not measured

To be filled by running the probe (see Reproducing):

- writable roots and unreadable subtrees (the walk);
- path sockets outside `/proc/net/unix` (other netns);
- the abstract and dgram endpoints left unclassified above;
- credential files and environment variable names;
- the same-uid process table (signal targets);
- inherited fds, SysV/POSIX IPC, kernel keyrings;
- virtualisation of this host itself (cpuinfo, DMI);
- the probe inside an L8 domain, with canaries.

Expected, and the falsification test: inside L8,
- the groups row is **unchanged**;
- `docker.sock` is invisible only because `/run` is hidden.

If the docker socket is visible inside L8 with `rw` access, step 3's
form of step 1 is not optional.

## Next falsification experiment

**Constructed world: does an inside inventory equal the manifest?**

- Build the world from the 12 setup steps, with the step-1 fallback
  (hide group-gated objects) since the runtime is unprivileged.
- Run `capability_probe.py <id>` inside, with `--hold-canary <id>`
  running outside.
- **Supports the model:** I(W) ⊆ M; no unclassified item; outside
  canaries invisible; inside canary visible; the probe's walk reports
  zero gaps.
- **Falsifies it:** any item outside M. In particular, a
  group-gated object reachable through a path the view was assumed to
  hide, or an inherited fd the runtime did not know it passed.

## Reproducing

The probe needs Python 3 and no privileges. It only reads metadata.

```
# ambient inventory
python3 crates/v9r-core/tests/capability_probe.py > ambient.json

# with positive controls
python3 crates/v9r-core/tests/capability_probe.py --hold-canary c0 &
python3 crates/v9r-core/tests/capability_probe.py c0 > ambient.json
```

The walk stops at 3,000,000 entries or 240 s, whichever comes first. It
reports `budget_exhausted` when it hits either limit, and that turns
every walk-derived row into "unknown".
