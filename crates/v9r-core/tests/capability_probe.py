"""Capability inventory probe: what authority is reachable from here?

Inventory only. Every fact comes from metadata: lstat, access(2), the
probe's own /proc/self, the public process table (/proc/<pid>/stat and
comm), sysctls, and the kernel's tables under /proc/net and
/proc/sysvipc. The probe never connects to a socket, never opens a
device, never signals or opens another process's private files, and
never reads a credential's contents: only whether the path exists and
is readable. Environment variables are reported by name only.

Each fact is tagged with where it came from:
  observed   the kernel answered it for this process
  rule       derived from observed facts by a documented kernel rule
  declared   a human-written meaning attached to an observed name
             (DECLARED below); inspection cannot produce it
Observed channels with no declared meaning are listed as unclassified.
Every enumeration reports its own gaps (unreadable directories, budget
exhaustion): a gap means "unknown", never "absent".

Usage:
  capability_probe.py [<canary-id>]   prints one JSON object
  capability_probe.py --hold-canary <canary-id>
      binds (never accepts on) a path socket in $XDG_RUNTIME_DIR, one in
      /tmp, and an abstract socket, all named v9r-canary-<id>, then
      sleeps. A probe given the same id reports which canaries it can
      see: the positive control for "this world cannot see that
      endpoint".
"""

import json, os, re, socket, stat, sys, time

SKIP = {"/proc", "/sys", "/nix/store"}
CAP = 60
WALK_BUDGET = 3_000_000
WALK_SECONDS = 240

# What a channel *means*. Nothing below is observable; it is knowledge
# about the program on the other side of a name.
DECLARED = [
    # (kind, pattern, authority class, meaning)
    ("socket", r"/docker\.sock$", "root-equivalent", "container API: run any image with host mounts"),
    ("socket", r"/containerd.*\.sock", "root-equivalent", "container runtime API"),
    ("socket", r"/libvirt/libvirt-sock$", "root-equivalent", "VM manager (polkit/group gated): define domains over host disks"),
    ("socket", r"/libvirt/", "mediated", "libvirt auxiliary daemons"),
    ("socket", r"/run/user/\d+/bus$", "spawn", "session D-Bus: systemd user manager StartTransientUnit"),
    ("socket", r"/run/user/\d+/systemd/", "spawn", "systemd user manager private socket"),
    ("socket", r"/run/dbus/system_bus_socket$", "mediated", "system D-Bus: polkit decides per call"),
    ("socket", r"/run/systemd/", "mediated", "system manager sockets (journal, userdb, machined, ...)"),
    ("socket", r"/niri\.|niri.*\.sock", "spawn", "compositor IPC: spawn action"),
    ("socket", r"Alacritty-.*\.sock", "spawn", "terminal IPC: create windows running commands"),
    ("socket", r"/wayland-|/xwls-", "input", "display server: clipboard, windows"),
    ("socket", r"\.X11-unix/", "input", "X11: other clients' input and windows"),
    ("socket", r"lan-mouse", "input", "input forwarding to other machines"),
    ("socket", r"/ssh-unix-local/", "remote", "sshd on a unix socket: log in as any user with a key or password"),
    ("socket", r"/ssh-|/ssh/|ssh-agent|/gcr/ssh|/openssh_agent", "remote", "ssh agent: authenticates as the user elsewhere"),
    ("socket", r"/gnupg/", "remote", "gpg agent: signs as the user"),
    ("socket", r"/keyring/", "credential", "secret service / keyring daemon"),
    ("socket", r"/cc-socks/|/claude", "agent-plane", "agent harness messaging"),
    ("socket", r"/pipewire|/pulse/", "device", "audio server: microphone, speakers"),
    ("socket", r"speechd", "device", "speech synthesis daemon"),
    ("socket", r"/nix/var/nix/daemon-socket", "build", "nix daemon: builds as build users, writes the store"),
    ("socket", r"/udev/", "mediated", "device manager control"),
    ("socket", r"/nscd/", "mediated", "name service cache"),
    ("socket", r"sddm", "mediated", "display manager authentication"),
    ("device", r"^/dev/kvm$", "compute", "hardware virtualisation"),
    ("device", r"^/dev/vhost-", "compute", "in-kernel virtio backends (net, vsock)"),
    ("device", r"^/dev/(kfd|dri/)", "compute", "GPU"),
    ("device", r"^/dev/i2c-", "hardware", "raw I2C bus writes (monitor, sensors)"),
    ("device", r"^/dev/input/", "input", "read every keystroke and pointer event"),
    ("device", r"^/dev/uinput$", "input", "inject keystrokes"),
    ("device", r"^/dev/(hidraw|uhid)", "input", "raw HID devices"),
    ("device", r"^/dev/(sd|nvme|dm-|mmcblk|loop)", "root-equivalent", "raw block device"),
    ("device", r"^/dev/(mem|kmem|port)$", "root-equivalent", "physical memory"),
    ("device", r"^/dev/net/tun$", "network", "create tun/tap (needs CAP_NET_ADMIN in a netns)"),
    ("device", r"^/dev/fuse$", "filesystem", "userspace filesystems (with fusermount)"),
    ("device", r"^/dev/snd/", "device", "sound hardware"),
    ("device", r"^/dev/(video|media)", "device", "camera"),
    ("device", r"^/dev/(tpm|tpmrm)", "credential", "TPM: sealed keys"),
    ("device", r"^/dev/rfkill$", "hardware", "radio kill switches"),
    ("device", r"^/dev/(tty\d|console|vcs)", "input", "virtual consoles"),
    ("setuid", r"/(sudo|sudoedit|su|doas|pkexec)$", "admin", "privilege elevation (password or policy gated)"),
    ("setuid", r"/(fusermount3?|mount|umount)$", "filesystem", "mount helpers"),
    ("setuid", r"/new[ug]idmap$", "namespace", "map extra ids into user namespaces"),
    ("setuid", r"/qemu-bridge-helper$", "network", "attach taps to host bridges"),
    ("group", r"^(wheel|sudo|admin)$", "admin", "sudo policy usually grants root"),
    ("group", r"^docker$", "root-equivalent", "docker.sock"),
    ("group", r"^(libvirtd|libvirt)$", "root-equivalent", "libvirt system connection without polkit prompt"),
    ("group", r"^(disk)$", "root-equivalent", "raw block devices"),
    ("group", r"^(kvm)$", "compute", "virtualisation devices"),
    ("group", r"^(input)$", "input", "input event devices"),
    ("group", r"^(i2c)$", "hardware", "I2C buses"),
    ("group", r"^(video|render)$", "compute", "GPU / display devices"),
    ("group", r"^(audio)$", "device", "sound devices"),
    ("group", r"^(systemd-journal|adm)$", "observe", "read system logs"),
    ("tcp", r":(22)$", "remote", "sshd"),
    ("tcp", r"127\.0\.0\.1:9050$", "network", "tor SOCKS proxy: anonymous egress"),
    ("tcp", r"127\.0\.0\.1:11434$", "compute", "ollama model server"),
    ("tcp", r"127\.0\.0\.1:5432$", "data", "postgres"),
    ("tcp", r":(2375|2376)$", "root-equivalent", "docker TCP API"),
]

CRED_ENV = re.compile(r"(TOKEN|SECRET|PASSW|API_?KEY|_KEY$|AUTH|CREDENTIAL|COOKIE|SESSION)", re.I)
REACH_ENV = re.compile(r"^(SSH_AUTH_SOCK|GPG_AGENT_INFO|DBUS_SESSION_BUS_ADDRESS|WAYLAND_DISPLAY|DISPLAY|XDG_RUNTIME_DIR|DOCKER_HOST|CONTAINER_HOST|KUBECONFIG|.*_PROXY|.*_proxy|NIRI_SOCKET|ALACRITTY_SOCKET|CLAUDE.*|MCP.*|ANTHROPIC.*)$")

CRED_FILES = [
    ".ssh", ".ssh/id_rsa", ".ssh/id_ed25519", ".ssh/id_ecdsa", ".ssh/config",
    ".gnupg", ".netrc", ".git-credentials", ".config/git/credentials",
    ".config/gh/hosts.yml", ".aws/credentials", ".config/gcloud", ".azure",
    ".kube/config", ".docker/config.json", ".npmrc", ".pypirc",
    ".cargo/credentials", ".cargo/credentials.toml", ".local/share/keyrings",
    ".password-store", ".mozilla", ".config/chromium", ".config/google-chrome",
    ".claude.json", ".claude/.credentials.json", ".claude/settings.json",
    ".config/claude", ".config/anthropic", ".config/openai",
]

SYSCTLS = [
    "kernel/yama/ptrace_scope", "dev/tty/legacy_tiocsti",
    "kernel/unprivileged_userns_clone", "user/max_user_namespaces",
    "kernel/unprivileged_bpf_disabled", "kernel/perf_event_paranoid",
    "kernel/kptr_restrict", "kernel/dmesg_restrict", "kernel/io_uring_disabled",
    "fs/protected_symlinks", "fs/protected_hardlinks", "fs/protected_regular",
    "net/ipv4/ping_group_range", "kernel/modules_disabled",
]


def read(path, default=None):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return default


def declared(kind, name):
    for k, pat, cls, meaning in DECLARED:
        if k == kind and re.search(pat, name):
            return {"class": cls, "meaning": meaning, "provenance": "declared"}
    return {"class": "unclassified", "provenance": "observed"}


def access(path):
    r = ""
    for flag, ch in ((os.R_OK, "r"), (os.W_OK, "w"), (os.X_OK, "x")):
        try:
            if os.access(path, flag, follow_symlinks=False):
                r += ch
        except OSError:
            pass
    return r


def capped(items):
    items = sorted(items, key=lambda x: json.dumps(x, sort_keys=True))
    return {"count": len(items), "items": items[:CAP], "truncated": len(items) > CAP}


# ---- identity ---------------------------------------------------------------

def identity(inherited_fds):
    status = dict(
        l.split(":", 1) for l in (read("/proc/self/status") or "").splitlines() if ":" in l
    )
    status = {k: v.strip() for k, v in status.items()}
    groups = {}
    for line in (read("/etc/group") or "").splitlines():
        parts = line.split(":")
        if len(parts) >= 3:
            groups[parts[2]] = parts[0]
    gids = status.get("Groups", "").split()
    ns = {}
    for n in sorted(os.listdir("/proc/self/ns")):
        try:
            ns[n] = os.readlink(f"/proc/self/ns/{n}")
        except OSError:
            ns[n] = None
    uid_map = (read("/proc/self/uid_map") or "").split()
    return {
        "uid": os.getuid(), "euid": os.geteuid(), "gid": os.getgid(),
        "groups": [
            {"gid": g, "name": groups.get(g, "?"), **declared("group", groups.get(g, "?"))}
            for g in gids
        ],
        "caps": {k: status.get(k) for k in ("CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb")},
        "no_new_privs": status.get("NoNewPrivs"),
        "seccomp": status.get("Seccomp"),
        "init_userns": uid_map == ["0", "0", "4294967295"],
        "uid_map": " ".join(uid_map),
        "namespaces": ns,
        "cgroup": (read("/proc/self/cgroup") or "").strip(),
        "inherited_fds": inherited_fds,
        "sysctls": {s: (read(f"/proc/sys/{s}") or "").strip() or None for s in SYSCTLS},
        "provenance": "observed",
    }


# ---- filesystem -------------------------------------------------------------

def mounts():
    out = []
    for line in (read("/proc/self/mountinfo") or "").splitlines():
        a, b = line.split(" - ", 1)
        a, b = a.split(), b.split()
        out.append({"mount": a[4], "opts": a[5], "fstype": b[0], "source": b[1]})
    return out


def walk(canary):
    """Walk the visible tree without following symlinks. Collects writable
    directory roots, sockets, fifos, devices, setuid/setgid files, and the
    subtrees this process could not enumerate."""
    found = {"writable_roots": [], "sockets": [], "fifos": [], "devices": [],
             "setid": [], "unreadable_dirs": 0, "unreadable_examples": [],
             "entries": 0, "budget_exhausted": False}
    deadline = time.monotonic() + WALK_SECONDS
    stack = [("/", False)]
    while stack:
        path, parent_writable = stack.pop()
        if found["entries"] >= WALK_BUDGET or time.monotonic() > deadline:
            found["budget_exhausted"] = True
            found["unwalked_example"] = path
            break
        try:
            it = os.scandir(path)
        except OSError as e:
            found["unreadable_dirs"] += 1
            if len(found["unreadable_examples"]) < CAP:
                found["unreadable_examples"].append(f"{path} ({e.strerror})")
            continue
        with it:
            for entry in it:
                found["entries"] += 1
                p = entry.path
                try:
                    st = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                mode = st.st_mode
                if stat.S_ISDIR(mode):
                    if p in SKIP:
                        continue
                    w = os.access(p, os.W_OK, follow_symlinks=False) and os.access(p, os.X_OK)
                    if w and not parent_writable:
                        found["writable_roots"].append({"path": p, "owner": st.st_uid,
                                                        "sticky": bool(mode & stat.S_ISVTX)})
                    stack.append((p, w))
                elif stat.S_ISSOCK(mode):
                    found["sockets"].append({"path": p, "owner": st.st_uid, "gid": st.st_gid,
                                             "mode": oct(mode & 0o7777), "access": access(p),
                                             "canary": bool(canary) and p.endswith(f"v9r-canary-{canary}"),
                                             **declared("socket", p)})
                elif stat.S_ISFIFO(mode):
                    found["fifos"].append({"path": p, "access": access(p)})
                elif stat.S_ISCHR(mode) or stat.S_ISBLK(mode):
                    a = access(p)
                    if "r" in a or "w" in a:
                        found["devices"].append({"path": p, "type": "chr" if stat.S_ISCHR(mode) else "blk",
                                                 "access": a, **declared("device", p)})
                elif stat.S_ISREG(mode) and mode & (stat.S_ISUID | stat.S_ISGID):
                    if os.access(p, os.X_OK):
                        found["setid"].append({"path": p, "owner": st.st_uid,
                                               "setuid": bool(mode & stat.S_ISUID),
                                               "setgid": bool(mode & stat.S_ISGID),
                                               **declared("setuid", p)})
    return found


# ---- process ----------------------------------------------------------------

def proc_stat(pid):
    s = read(f"/proc/{pid}/stat")
    if not s:
        return None
    comm = s[s.index("(") + 1:s.rindex(")")]
    rest = s[s.rindex(")") + 2:].split()
    return {"comm": comm, "state": rest[0], "ppid": int(rest[1]), "pgrp": int(rest[2]),
            "session": int(rest[3]), "tty_nr": int(rest[4])}


def processes():
    me = os.getpid()
    uid = os.getuid()
    pids = [int(p) for p in os.listdir("/proc") if p.isdigit()]
    owners = {}
    same = []
    for pid in pids:
        try:
            owners[pid] = os.stat(f"/proc/{pid}").st_uid
        except OSError:
            continue
        if owners[pid] == uid and pid != me:
            st = proc_stat(pid)
            if st:
                same.append({"pid": pid, "comm": st["comm"]})
    chain = []
    pid = me
    while pid > 0 and len(chain) < 64:
        st = proc_stat(pid)
        if not st:
            break
        chain.append({"pid": pid, "comm": st["comm"], "owner": owners.get(pid)})
        pid = st["ppid"]
    mine = proc_stat(me)
    ptrace = (read("/proc/sys/kernel/yama/ptrace_scope") or "").strip()
    cg = (read("/proc/self/cgroup") or "").strip().split("::")[-1]
    cg_writable = []
    parts = cg.strip("/").split("/") if cg else []
    for i in range(len(parts) + 1):
        d = "/sys/fs/cgroup/" + "/".join(parts[:i])
        cg_writable.append({"cgroup": "/" + "/".join(parts[:i]),
                            "procs": access(d.rstrip("/") + "/cgroup.procs"),
                            "dir": access(d)})
    by_comm = {}
    for p in same:
        by_comm[p["comm"]] = by_comm.get(p["comm"], 0) + 1
    return {
        "visible_pids": len(pids),
        "owners": {str(u): sum(1 for o in owners.values() if o == u) for u in sorted(set(owners.values()))},
        "same_uid": {"count": len(same), "by_comm": dict(sorted(by_comm.items()))},
        "ancestry": chain,
        "session": mine and {"sid": mine["session"], "pgrp": mine["pgrp"], "tty_nr": mine["tty_nr"]},
        "rules": {
            "signal": "same real/effective uid as target -> kill(2) permitted (no capability needed)",
            "ptrace": {"0": "any same-uid process", "1": "descendants only (YAMA)",
                       "2": "CAP_SYS_PTRACE only", "3": "none"}.get(ptrace, "unknown"),
            "provenance": "rule",
        },
        "cgroup_access": cg_writable,
        "keys": [l.split()[7] if len(l.split()) > 7 else "?" for l in (read("/proc/keys") or "").splitlines()],
    }


# ---- IPC --------------------------------------------------------------------

def unix_table(canary):
    rows = []
    for line in (read("/proc/net/unix") or "").splitlines()[1:]:
        f = line.split()
        if len(f) < 8:
            continue
        flags, typ, st, path = int(f[3], 16), int(f[4], 16), int(f[5], 16), f[7]
        listening = bool(flags & 0x10000)
        if not listening:
            continue
        rows.append({"name": path, "abstract": path.startswith("@"),
                     "type": {1: "stream", 2: "dgram", 5: "seqpacket"}.get(typ, typ),
                     "canary": bool(canary) and path.endswith(f"v9r-canary-{canary}"),
                     **declared("socket", path)})
    uniq = {r["name"]: r for r in rows}
    return sorted(uniq.values(), key=lambda r: r["name"])


def sysv():
    out = {}
    for k in ("msg", "sem", "shm"):
        lines = (read(f"/proc/sysvipc/{k}") or "").splitlines()[1:]
        out[k] = len(lines)
    for d in ("/dev/shm", "/dev/mqueue"):
        try:
            out[d] = [{"name": e.name, "access": access(e.path)} for e in os.scandir(d)][:CAP]
        except OSError as e:
            out[d] = f"unreadable ({e.strerror})"
    return out


def fd_targets(fds):
    out = []
    for fd in fds:
        try:
            t = os.readlink(f"/proc/self/fd/{fd}")
        except OSError:
            continue
        flags = (read(f"/proc/self/fdinfo/{fd}") or "")
        m = re.search(r"flags:\s+(\d+)", flags)
        out.append({"fd": fd, "target": t, "flags_octal": m.group(1) if m else None})
    return out


# ---- network ----------------------------------------------------------------

def hexaddr(h):
    ip, port = h.split(":")
    port = int(port, 16)
    if len(ip) == 8:
        a = ".".join(str(b) for b in reversed(bytes.fromhex(ip)))
    else:
        words = [bytes.fromhex(ip[i:i + 8])[::-1].hex() for i in range(0, 32, 8)]
        a = "[" + ":".join(w[i:i + 4] for w in words for i in (0, 4)) + "]"
    return f"{a}:{port}"


def network():
    listen = []
    for proto in ("tcp", "tcp6", "udp", "udp6"):
        for line in (read(f"/proc/net/{proto}") or "").splitlines()[1:]:
            f = line.split()
            if len(f) < 8:
                continue
            state = f[3]
            if proto.startswith("tcp") and state != "0A":
                continue
            if proto.startswith("udp") and f[2] != ("00000000:0000" if proto == "udp" else "0" * 32 + ":0000"):
                continue
            addr = hexaddr(f[1])
            listen.append({"proto": proto, "addr": addr, "uid": int(f[7]), **declared("tcp", addr)})
    ifaces = [l.split(":")[0].strip() for l in (read("/proc/net/dev") or "").splitlines()[2:]]
    routes = (read("/proc/net/route") or "").splitlines()[1:]
    default = any(r.split()[1] == "00000000" for r in routes if r.split())
    resolv = [l.split()[1] for l in (read("/etc/resolv.conf") or "").splitlines()
              if l.startswith("nameserver") and len(l.split()) > 1]
    nsfs = [m["mount"] for m in mounts() if m["fstype"] == "nsfs"]
    return {"interfaces": ifaces, "default_route_v4": default, "nameservers": resolv,
            "listening": sorted({json.dumps(x, sort_keys=True) for x in listen}),
            "nsfs_mounts": nsfs}


# ---- external ---------------------------------------------------------------

def external(canary):
    home = os.path.expanduser("~")
    creds = []
    for rel in CRED_FILES:
        p = os.path.join(home, rel)
        try:
            os.lstat(p)
        except OSError:
            continue
        creds.append({"path": "~/" + rel, "access": access(p)})
    env = []
    for name in sorted(os.environ):
        if CRED_ENV.search(name):
            env.append({"name": name, "kind": "credential-shaped name"})
        elif REACH_ENV.match(name):
            env.append({"name": name, "kind": "endpoint locator"})
    cpuinfo = read("/proc/cpuinfo") or ""
    virt = {
        "cpu_hypervisor_flag": bool(re.search(r"^flags\s*:.*\bhypervisor\b", cpuinfo, re.M)),
        "dmi_sys_vendor": (read("/sys/class/dmi/id/sys_vendor") or "").strip() or None,
        "dmi_product": (read("/sys/class/dmi/id/product_name") or "").strip() or None,
        "dockerenv": os.path.exists("/.dockerenv"),
        "containerenv": os.path.exists("/run/.containerenv"),
        "container_env": "container" in os.environ,
    }
    canaries = None
    if canary:
        rt = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
        names = {r["name"] for r in unix_table(None)}
        canaries = {
            "runtime_dir_path": os.path.exists(f"{rt}/v9r-canary-{canary}"),
            "tmp_path": os.path.exists(f"/tmp/v9r-canary-{canary}"),
            "abstract": f"@v9r-canary-{canary}" in names,
        }
    return {"credential_files": creds, "environment": env,
            "environment_names_total": len(os.environ), "virtualisation": virt,
            "canaries_visible": canaries}


def hold_canary(cid):
    rt = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
    socks = []
    for addr in (f"{rt}/v9r-canary-{cid}", f"/tmp/v9r-canary-{cid}", f"\0v9r-canary-{cid}"):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.bind(addr)
        s.listen(0)
        socks.append((s, addr))
    print("ready", flush=True)
    try:
        time.sleep(3600)
    finally:
        for s, addr in socks:
            s.close()
            if not addr.startswith("\0"):
                os.unlink(addr)


def main():
    if len(sys.argv) > 2 and sys.argv[1] == "--hold-canary":
        return hold_canary(sys.argv[2])
    canary = sys.argv[1] if len(sys.argv) > 1 else ""
    inherited = sorted(int(fd) for fd in os.listdir("/proc/self/fd"))
    inherited_fds = fd_targets(inherited)
    w = walk(canary)
    report = {
        "identity": identity(inherited_fds),
        "filesystem": {
            "mounts": mounts(),
            "writable_roots": capped(w["writable_roots"]),
            "sockets": capped(w["sockets"]),
            "fifos": capped(w["fifos"]),
            "devices": capped(w["devices"]),
            "setid": capped(w["setid"]),
            "gaps": {"unreadable_dirs": w["unreadable_dirs"],
                     "unreadable_examples": w["unreadable_examples"],
                     "entries": w["entries"], "budget_exhausted": w["budget_exhausted"]},
        },
        "process": processes(),
        "ipc": {"listening_unix": unix_table(canary), "sysv_posix": sysv()},
        "network": network(),
        "external": external(canary),
    }
    json.dump(report, sys.stdout, indent=1, sort_keys=True)
    print()


if __name__ == "__main__":
    main()
