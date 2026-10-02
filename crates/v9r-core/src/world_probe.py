# v9r world probe: the inside observer for crate::capability.
#
# Runs as the only program in a constructed world, from source passed
# on argv (`python3 -I -c`), so the world holds no observer files. It
# first performs the operations the runtime configured (`ops`: the
# world's work, chosen by the runtime, never by code in the world), then
# reads metadata and makes deliberate attempts (fork, and connects to the
# runtime's canary targets). It prints one JSON line, then holds until
# stdin closes so the runtime can observe it from outside.
#
# Every failed lookup is reported as an error, never as absence.

import ctypes
import errno
import fcntl
import hashlib
import json
import os
import resource
import socket
import stat
import struct
import sys

WALK_LIMIT = 1_000_000


def err(e):
    return errno.errorcode.get(e.errno, str(e.errno))


def name(p):
    return os.fsdecode(p).encode("utf-8", "backslashreplace").decode("utf-8")


def kind(mode):
    if stat.S_ISREG(mode):
        return "f"
    if stat.S_ISDIR(mode):
        return "d"
    if stat.S_ISLNK(mode):
        return "l"
    if stat.S_ISSOCK(mode):
        return "s"
    if stat.S_ISFIFO(mode):
        return "p"
    if stat.S_ISCHR(mode):
        return "c"
    if stat.S_ISBLK(mode):
        return "b"
    return "?"


def access(p, k):
    # access(2) uses the real ids, supplementary groups and mount flags:
    # what this process can do, not what the mode bits say.
    if k == "l":
        return True, False
    return os.access(p, os.R_OK), os.access(p, os.W_OK)


def walk():
    # `ids[i]` is the (st_dev, st_ino) of `entries[i]`: object identity,
    # independent of the path the object is reached by.
    entries, unknown, ids = [], [], []
    r, w = access("/", "d")
    st = os.lstat("/")
    entries.append(["/", "d", r, w])
    ids.append([st.st_dev, st.st_ino])
    stack = ["/"]
    while stack:
        d = stack.pop()
        try:
            with os.scandir(d) as it:
                children = list(it)
        except OSError as e:
            unknown.append([name(d), err(e)])
            continue
        for c in children:
            if len(entries) >= WALK_LIMIT:
                unknown.append([name(d), "WALK_LIMIT"])
                return entries, unknown, ids
            try:
                st = os.lstat(c.path)
            except OSError as e:
                unknown.append([name(c.path), err(e)])
                continue
            k = kind(st.st_mode)
            r, w = access(c.path, k)
            entries.append([name(c.path), k, r, w])
            ids.append([st.st_dev, st.st_ino])
            if k == "d":
                stack.append(c.path)
    return entries, unknown, ids


def declared(paths):
    out = {}
    for p in paths:
        try:
            st = os.lstat(p)
        except OSError as e:
            out[p] = {"error": err(e)}
            continue
        r, w = access(p, kind(st.st_mode))
        out[p] = {"dev": st.st_dev, "ino": st.st_ino, "r": r, "w": w}
    return out


class FileHandle(ctypes.Structure):
    _fields_ = [
        ("handle_bytes", ctypes.c_uint),
        ("handle_type", ctypes.c_int),
        ("f_handle", ctypes.c_ubyte * 128),
    ]


def resolve(paths):
    # Each path to the object behind it, by every identity the kernel
    # offers an unprivileged process: (st_dev, st_ino) from lstat, the
    # inode generation (FS_IOC_GETVERSION on an fd opened without
    # following a final symlink), and the export handle and mount id
    # from name_to_handle_at (not following either).
    libc = ctypes.CDLL(None, use_errno=True)
    out = {}
    for p in paths:
        try:
            st = os.lstat(p)
        except OSError as e:
            out[p] = {"error": err(e)}
            continue
        rec = {"kind": kind(st.st_mode), "dev": st.st_dev, "ino": st.st_ino}
        try:
            fd = os.open(p, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
            try:
                buf = fcntl.ioctl(fd, 0x80087601, b"\0" * 8)  # FS_IOC_GETVERSION
                rec["generation"] = {"Ok": struct.unpack("I", buf[:4])[0]}
            finally:
                os.close(fd)
        except OSError as e:
            rec["generation"] = {"Err": err(e)}
        fh, mnt = FileHandle(128, 0), ctypes.c_int(0)
        r = libc.name_to_handle_at(-100, os.fsencode(p), ctypes.byref(fh), ctypes.byref(mnt), 0)
        if r == 0:
            data = bytes(fh.f_handle[: fh.handle_bytes])
            rec["handle"] = {"Ok": "%d:%s" % (fh.handle_type, data.hex())}
            rec["mount"] = {"Ok": mnt.value}
        else:
            e = errno.errorcode.get(ctypes.get_errno(), str(ctypes.get_errno()))
            rec["handle"] = {"Err": e}
            rec["mount"] = {"Err": e}
        out[p] = rec
    return out


def git_mode(mode):
    if stat.S_ISDIR(mode):
        return "40000"
    if stat.S_ISLNK(mode):
        return "120000"
    if stat.S_ISREG(mode):
        return "100755" if mode & 0o100 else "100644"
    return "other"


def transcribe(dirs):
    # Raw observations in crate::fs_raw's formats, for the snapshot
    # verifier: fs_dir (name -> git mode), fs_stat (nlink), fs_file
    # (bytes, hex), fs_link (target). It interprets nothing; a failed
    # call leaves its observation out (the snapshot is then incomplete).
    out = []
    stack = list(dirs)
    while stack:
        d = stack.pop()
        try:
            st = os.lstat(d)
            names = sorted(os.listdir(d))
        except OSError:
            continue
        out.append(["fs_stat", d, {"nlink": str(st.st_nlink)}])
        listing = {}
        for n in names:
            p = d + "/" + n
            try:
                m = git_mode(os.lstat(p).st_mode)
            except OSError:
                listing = None
                break
            listing[n] = m
            try:
                if m == "40000":
                    stack.append(p)
                elif m == "120000":
                    out.append(["fs_link", p, os.readlink(p)])
                elif m != "other":
                    fd = os.open(p, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
                    with os.fdopen(fd, "rb") as f:
                        out.append(["fs_file", p, f.read().hex()])
            except OSError:
                pass
        if listing is not None:
            out.append(["fs_dir", d, listing])
    return out


def identity():
    libc = ctypes.CDLL(None, use_errno=True)
    nnp = libc.prctl(39, 0, 0, 0, 0)  # PR_GET_NO_NEW_PRIVS

    class Hdr(ctypes.Structure):
        _fields_ = [("version", ctypes.c_uint32), ("pid", ctypes.c_int)]

    class Data(ctypes.Structure):
        _fields_ = [(f, ctypes.c_uint32) for f in ("eff", "prm", "inh")]

    hdr, data = Hdr(0x20080522, 0), (Data * 2)()
    caps = None
    if libc.capget(ctypes.byref(hdr), data) == 0:
        caps = {
            f: (getattr(data[1], f) << 32) | getattr(data[0], f)
            for f in ("eff", "prm", "inh")
        }
        # PR_CAP_AMBIENT_IS_SET; -1 (EINVAL) past the last capability.
        amb = 0
        for cap in range(64):
            v = libc.prctl(47, 1, cap, 0, 0)
            if v < 0:
                break
            amb |= v << cap
        caps["amb"] = amb
    return {
        "pid": os.getpid(),
        "uid": os.getuid(),
        "gid": os.getgid(),
        "groups": os.getgroups(),
        "no_new_privs": nnp if nnp >= 0 else None,
        "caps": caps,
    }


def process():
    inf = resource.RLIM_INFINITY
    nproc = resource.getrlimit(resource.RLIMIT_NPROC)
    out = {"nproc": [-1 if v == inf else v for v in nproc]}
    try:
        pid = os.fork()
    except OSError as e:
        out["fork"] = err(e)
        return out
    if pid == 0:
        os._exit(0)
    os.waitpid(pid, 0)
    out["fork"] = "ok"
    return out


def interfaces():
    try:
        names = [n for _, n in socket.if_nameindex()]
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    except OSError as e:
        return {"error": err(e)}
    out = []
    with s:
        for n in names:
            try:
                req = struct.pack("16sh14x", n.encode(), 0)
                flags = struct.unpack("16sh14x", fcntl.ioctl(s, 0x8913, req))[1]
                out.append([n, bool(flags & 1)])  # SIOCGIFFLAGS, IFF_UP
            except OSError as e:
                out.append([n, err(e)])
    return {"list": out}


def target(t):
    try:
        if t["kind"] == "tcp":
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            addr = (t["host"], t["port"])
        else:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            addr = "\0" + t["addr"] if t["kind"] == "abstract" else t["addr"]
        with s:
            s.settimeout(2)
            s.connect(addr)
        return "connected"
    except OSError as e:
        return err(e) if e.errno else type(e).__name__


def op(o):
    # One operation: "ok" (with the sha256 of what was read), or the errno.
    try:
        if o["op"] == "read":
            with open(o["path"], "rb") as f:
                return "ok:" + hashlib.sha256(f.read()).hexdigest()
        if o["op"] == "write":
            with open(o["path"], "wb") as f:
                f.write(o["data"].encode())
            return "ok"
        if o["op"] == "link":
            os.link(o["path"], o["to"])
            return "ok"
        if o["op"] == "rename":
            os.rename(o["path"], o["to"])
            return "ok"
        if o["op"] == "mount":
            libc = ctypes.CDLL(None, use_errno=True)
            r = libc.mount(o["fstype"].encode(), o["path"].encode(), o["fstype"].encode(), 0, None)
            if r != 0:
                e = ctypes.get_errno()
                return errno.errorcode.get(e, str(e))
            return "ok"
        return "UNKNOWN_OP"
    except OSError as e:
        return err(e) if e.errno else type(e).__name__


def main():
    cfg = json.loads(sys.argv[1])
    ops = {k: op(o) for k, o in sorted(cfg.get("ops", {}).items())}
    entries, unknown, ids = walk()
    report = {
        "ops": ops,
        "resolved": resolve(cfg.get("resolve", [])),
        "transcript": transcribe(cfg.get("transcribe", [])),
        "identity": identity(),
        "env": dict(os.environ),
        "walk": {"entries": entries, "unknown": unknown, "ids": ids},
        "declared": declared(cfg["declared"]),
        "process": process(),
        "network": interfaces(),
        "targets": {k: target(t) for k, t in cfg["targets"].items()},
    }
    sys.stdout.write(json.dumps(report) + "\n")
    sys.stdout.flush()
    sys.stdin.read()


main()
