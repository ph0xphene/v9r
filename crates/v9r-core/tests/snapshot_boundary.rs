//! Snapshot Capability Boundary v0: can a SnapshotCapability be realized
//! outside the host's identity domain?
//!
//! World A transcribes D; S = its tree root. World B is built with a
//! *sealed* snapshot: a tmpfs mounted at /v/d inside B's own mount
//! namespace, filled with S's verified entries, then remounted read-only
//! (superblock and mount), before `pivot_root`. No host path names it.
//!
//! While B is held, a same-uid host process attacks it, and the runtime
//! re-verifies B's view from outside (`/proc/<pid>/root`) with the
//! crate's own raw observer and snapshot verifier.
//!
//! Needs unprivileged user namespaces, `python3` and `nix-store`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use v9r_core::capability::{
    self, CapabilityManifest, ConstructionPlan, Observation, ProbeExtra, ProbeOp,
};
use v9r_core::fs_raw::RawFsObserver;
use v9r_core::graph::{Key, Registry, Term, Trust};
use v9r_core::kernel::Verdict;
use v9r_core::snapshot::{self, snapshot_from, FsSnapshot, ObjectStore};

const VIEW: &str = "/v/d";

fn toolchain() -> Result<(PathBuf, Vec<PathBuf>), String> {
    let out = Command::new("python3")
        .args(["-I", "-c", "import os; print(os.readlink('/proc/self/exe'))"])
        .output()
        .map_err(|e| format!("python3: {e}"))?;
    let real = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let store: PathBuf = real.components().take(4).collect();
    if !store.starts_with("/nix/store") {
        return Err(format!("{real:?} is not in /nix/store"));
    }
    let out = Command::new("nix-store")
        .arg("-qR")
        .arg(&store)
        .output()
        .map_err(|e| format!("nix-store: {e}"))?;
    let closure: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(PathBuf::from)
        .collect();
    if !out.status.success() || closure.is_empty() {
        return Err("nix-store -qR failed".into());
    }
    Ok((real, closure))
}

/// The runtime's outside view of a held world's directory: the crate's
/// raw observer rooted at `/proc/<pid>/root`, through the snapshot
/// verifier.
fn outside_root(pid: u32, dir: &str) -> Option<String> {
    let store = ObjectStore::new();
    let registry = Registry::new();
    registry.register(
        RawFsObserver::new("outside", format!("/proc/{pid}/root")),
        Trust::Attesting,
    );
    registry.add_verifier(FsSnapshot::new(store));
    match registry.query(&Key::new("snapshot", [dir.trim_start_matches('/')])) {
        Some(Term::Id(root)) => Some(root),
        _ => None,
    }
}

fn errno_of(r: std::io::Result<()>) -> String {
    match r {
        Ok(()) => "ok".into(),
        Err(e) => format!("{:?}", e.kind()),
    }
}

/// A same-uid host process that enters the world's user and mount
/// namespaces (as their owner) and runs `script` with the world's own
/// interpreter. Returns its stdout, or why it could not start.
fn inbound(pid: u32, python: &Path, script: &str) -> String {
    let user = fs::File::open(format!("/proc/{pid}/ns/user")).unwrap();
    let mnt = fs::File::open(format!("/proc/{pid}/ns/mnt")).unwrap();
    let (u, m) = (user.as_raw_fd(), mnt.as_raw_fd());
    let mut cmd = Command::new(python);
    cmd.args(["-I", "-c", script]).env_clear().env("LC_ALL", "C");
    // SAFETY: only setns between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setns(u, libc::CLONE_NEWUSER) != 0 || libc::setns(m, libc::CLONE_NEWNS) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.output() {
        Ok(o) => format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout).trim(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => format!("spawn: {e}"),
    }
}

/// What the owner may do *with* the capabilities `setns` grants: the
/// mount calls run between `setns` and `exec` (an `execve` by a non-root
/// uid drops them, which is what `inbound` measures). Then `WRITE` runs.
#[derive(Clone, Copy)]
enum Privileged {
    /// Superblock rw, then mount rw.
    RemountRw,
    /// A fresh tmpfs over the view.
    Overmount,
}

fn inbound_privileged(pid: u32, python: &Path, how: Privileged) -> String {
    let user = fs::File::open(format!("/proc/{pid}/ns/user")).unwrap();
    let mnt = fs::File::open(format!("/proc/{pid}/ns/mnt")).unwrap();
    let (u, m) = (user.as_raw_fd(), mnt.as_raw_fd());
    let mut cmd = Command::new(python);
    cmd.args(["-I", "-c", WRITE]).env_clear().env("LC_ALL", "C");
    // SAFETY: only syscalls on static C strings between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            let fail = || Err(std::io::Error::last_os_error());
            if libc::setns(u, libc::CLONE_NEWUSER) != 0 || libc::setns(m, libc::CLONE_NEWNS) != 0 {
                return fail();
            }
            let ok = match how {
                Privileged::RemountRw => {
                    libc::mount(
                        std::ptr::null(),
                        c"/v/d".as_ptr(),
                        std::ptr::null(),
                        libc::MS_REMOUNT | libc::MS_NOSUID | libc::MS_NODEV,
                        std::ptr::null(),
                    ) == 0
                        && libc::mount(
                            std::ptr::null(),
                            c"/v/d".as_ptr(),
                            std::ptr::null(),
                            libc::MS_REMOUNT | libc::MS_BIND | libc::MS_NOSUID | libc::MS_NODEV,
                            std::ptr::null(),
                        ) == 0
                }
                Privileged::Overmount => {
                    libc::mount(
                        c"tmpfs".as_ptr(),
                        c"/v/d".as_ptr(),
                        c"tmpfs".as_ptr(),
                        0,
                        std::ptr::null(),
                    ) == 0
                }
            };
            if ok {
                Ok(())
            } else {
                fail()
            }
        });
    }
    match cmd.output() {
        Ok(o) => format!(
            "mount calls ok; then {}{}",
            String::from_utf8_lossy(&o.stdout).trim(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => format!("mount calls failed: {e}"),
    }
}

const WRITE: &str = r#"
import errno, json, os
out = {}
try:
    os.chmod("/v/d/a.txt", 0o644)
except OSError as e:
    out["chmod"] = errno.errorcode[e.errno]
try:
    with open("/v/d/a.txt", "w") as f:
        f.write("OWNER")
    out["write"] = "ok"
except OSError as e:
    out["write"] = errno.errorcode[e.errno]
print(json.dumps(out))
"#;

const REMOUNT_AND_WRITE: &str = r#"
import ctypes, errno, json, os
libc = ctypes.CDLL(None, use_errno=True)
def m(src, fstype, flags):
    r = libc.mount(src, b"/v/d", fstype, flags, None)
    return "ok" if r == 0 else errno.errorcode[ctypes.get_errno()]
def w():
    try:
        os.chmod("/v/d/a.txt", 0o644)
        with open("/v/d/a.txt", "w") as f:
            f.write("PWNED")
        return "ok"
    except OSError as e:
        return errno.errorcode[e.errno]
out = {"root": os.listdir("/"), "write_before": w()}
out["remount_superblock_rw"] = m(None, None, 32)          # MS_REMOUNT
out["remount_mount_rw"] = m(None, None, 32 | 4096)        # MS_REMOUNT|MS_BIND
out["write_after_remount"] = w()
print(json.dumps(out))
"#;

const OVERMOUNT: &str = r#"
import ctypes, errno, json, os
libc = ctypes.CDLL(None, use_errno=True)
r = libc.mount(b"tmpfs", b"/v/d", b"tmpfs", 0, None)
out = {"overmount": "ok" if r == 0 else errno.errorcode[ctypes.get_errno()]}
try:
    with open("/v/d/a.txt", "w") as f:
        f.write("REPLACED")
    out["write"] = "ok"
except OSError as e:
    out["write"] = errno.errorcode[e.errno]
print(json.dumps(out))
"#;

struct Lab {
    base: PathBuf,
    python: PathBuf,
    closure: Vec<PathBuf>,
    worlds: usize,
}

impl Lab {
    fn manifest(&self, read: &[&str]) -> CapabilityManifest {
        let mut read_paths = self.closure.clone();
        read_paths.extend(read.iter().map(PathBuf::from));
        CapabilityManifest {
            read_paths,
            env: BTreeMap::from([("LC_ALL".into(), "C".into())]),
            ..Default::default()
        }
    }

    fn root(&mut self) -> PathBuf {
        self.worlds += 1;
        let r = self.base.join(format!("root-{}", self.worlds));
        fs::create_dir_all(&r).unwrap();
        r
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn b_probe() -> ProbeExtra {
    ProbeExtra {
        ops: BTreeMap::from([
            ("1-read".into(), ProbeOp::Read { path: format!("{VIEW}/a.txt") }),
            ("2-write".into(), ProbeOp::Write { path: format!("{VIEW}/a.txt"), data: "x".into() }),
            ("3-create".into(), ProbeOp::Write { path: format!("{VIEW}/new"), data: "x".into() }),
            ("4-mountinfo".into(), ProbeOp::Read { path: "/proc/self/mountinfo".into() }),
            ("5-mount-proc".into(), ProbeOp::Mount { path: "/v".into(), fstype: "proc".into() }),
            ("6-mount-tmpfs".into(), ProbeOp::Mount { path: VIEW.into(), fstype: "tmpfs".into() }),
        ]),
        resolve: vec![VIEW.into()],
        transcribe: vec![VIEW.into()],
    }
}

/// Mount points of a held world, read from the host.
fn mounts(pid: u32) -> Vec<(String, String, String)> {
    fs::read_to_string(format!("/proc/{pid}/mountinfo"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            let sep = f.iter().position(|x| *x == "-")?;
            Some((f[4].to_string(), f[sep + 1].to_string(), f[5].to_string()))
        })
        .collect()
}

#[test]
fn snapshot_capability_boundary() {
    let (python, closure) = match toolchain() {
        Ok(t) => t,
        Err(e) => {
            println!("SKIPPED (environment, not evidence): {e}");
            return;
        }
    };
    let base = std::env::temp_dir().join(format!("v9r-sb-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let mut lab = Lab {
        base: fs::canonicalize(&base).unwrap(),
        python,
        closure,
        worlds: 0,
    };

    // ---- World A: S from A's view of D.
    let d = lab.base.join("d");
    fs::create_dir_all(d.join("sub")).unwrap();
    fs::write(d.join("a.txt"), "alpha").unwrap();
    fs::write(d.join("sub/b.txt"), "beta").unwrap();
    std::os::unix::fs::symlink("a.txt", d.join("l")).unwrap();
    let ds = d.to_str().unwrap().to_string();
    let plan_a = capability::plan(&lab.manifest(&[&ds])).unwrap();
    let root_a = lab.root();
    let a = capability::observe_with(
        &plan_a,
        &root_a,
        &lab.python,
        &BTreeMap::new(),
        &ProbeExtra { transcribe: vec![ds.clone()], ..Default::default() },
    )
    .unwrap();
    let store = ObjectStore::new();
    let s = snapshot_from(&store, &a.transcript(), &ds, "provider:world-probe").unwrap();
    println!("S = {s}");

    // ---- World B, sealed; attacked while held.
    let plan_b = capability::plan_sealed(&lab.manifest(&[]), &[(PathBuf::from(VIEW), s.clone())], &store)
        .unwrap();
    for step in plan_b.steps().iter().filter(|s| matches!(s, capability::Step::Snapshot { .. })) {
        println!("plan: {step}");
    }
    let root_b = lab.root();
    let python = lab.python.clone();
    let host_shadow = root_b.join("v/d");
    let (b, held) = capability::observe_held(
        &plan_b,
        &root_b,
        &lab.python,
        &BTreeMap::new(),
        &b_probe(),
        |pid| {
            let mut r = BTreeMap::new();
            r.insert("0 outside root before", format!("{:?}", outside_root(pid, VIEW).map(|x| x == s)));
            let ms = mounts(pid);
            r.insert("0 mounts", format!("{} total", ms.len()));
            r.insert("0 mount at /v/d", format!("{:?}", ms.iter().find(|m| m.0 == VIEW)));
            let foreign: Vec<_> = ms
                .iter()
                .filter(|m| m.0 != "/" && m.0 != VIEW && !m.0.starts_with("/nix/store/"))
                .collect();
            r.insert("0 mounts other than /, closure, /v/d", format!("{foreign:?}"));
            let top: BTreeSet<String> = fs::read_dir(format!("/proc/{pid}/root"))
                .map(|it| it.filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into())).collect())
                .unwrap_or_default();
            r.insert("0 /proc/<pid>/root top level", format!("{top:?}"));
            // A1: the host directory the world's root was built on.
            fs::create_dir_all(&host_shadow).unwrap();
            r.insert("A1 write host <root>/v/d/a.txt", errno_of(fs::write(host_shadow.join("a.txt"), "HOST")));
            // A2: through /proc/<pid>/root.
            let via = format!("/proc/{pid}/root{VIEW}");
            r.insert("A2 read via /proc/<pid>/root", format!("{:?}", fs::read_to_string(format!("{via}/a.txt"))));
            r.insert("A2 write via /proc/<pid>/root", errno_of(fs::write(format!("{via}/a.txt"), "PROC")));
            r.insert("A2 create via /proc/<pid>/root", errno_of(fs::write(format!("{via}/new"), "PROC")));
            r.insert(
                "A2 chmod via /proc/<pid>/root",
                errno_of(fs::set_permissions(
                    format!("{via}/a.txt"),
                    std::os::unix::fs::PermissionsExt::from_mode(0o666),
                )),
            );
            r.insert("A2 outside root after", format!("{:?}", outside_root(pid, VIEW).map(|x| x == s)));
            // A3: enter the world's namespaces as their owner.
            r.insert("A3 remount rw + write", inbound(pid, &python, REMOUNT_AND_WRITE));
            let after3 = outside_root(pid, VIEW);
            r.insert("A3 outside root after", format!("{:?}", after3.as_ref().map(|x| x == &s)));
            r.insert("A4 overmount + write", inbound(pid, &python, OVERMOUNT));
            let after4 = outside_root(pid, VIEW);
            r.insert("A4 outside root after", format!("{:?}", after4.as_ref().map(|x| x == &s)));
            r.insert("A4 read via /proc/<pid>/root", format!("{:?}", fs::read_to_string(format!("{via}/a.txt"))));
            // A5/A6: the owner, using the capabilities setns grants.
            r.insert(
                "A5 owner with caps: remount rw, then write",
                inbound_privileged(pid, &python, Privileged::RemountRw),
            );
            let after5 = outside_root(pid, VIEW);
            r.insert("A5 outside root after", format!("{:?}", after5.as_ref().map(|x| x == &s)));
            r.insert(
                "A6 owner with caps: overmount, then write",
                inbound_privileged(pid, &python, Privileged::Overmount),
            );
            let after6 = outside_root(pid, VIEW);
            r.insert("A6 outside root after", format!("{:?}", after6.as_ref().map(|x| x == &s)));
            r.insert("A6 read via /proc/<pid>/root", format!("{:?}", fs::read_to_string(format!("{via}/a.txt"))));
            (r, pid)
        },
    )
    .unwrap();
    let (results, pid) = held;

    println!("\nB's own probe (before any attack):");
    let in_root = snapshot_from(&ObjectStore::new(), &b.transcript(), VIEW, "provider:world-probe");
    println!("  in-world root = S: {:?}", in_root.as_ref().map(|r| r == &s));
    for k in ["1-read", "2-write", "3-create", "4-mountinfo", "5-mount-proc", "6-mount-tmpfs"] {
        println!("  {k}: {}", b.op(k).unwrap_or("?"));
    }
    let names_in_view: BTreeSet<&str> =
        b.objects_within(VIEW).iter().map(|(p, _, _)| *p).collect();
    println!("  names in /v/d: {names_in_view:?}");
    let walked_proc = b.walked("/proc").is_some();
    println!("  /proc in the walk: {walked_proc}; walk (entries, unknown) = {:?}", b.walk_len());
    let decision = capability::check(&plan_b, &b);
    println!("  C1–C3 verdict: {:?}", decision.verdict);
    println!("\nwhile held (same-uid host process):");
    for (k, v) in &results {
        println!("  {k}: {v}");
    }

    // ---- Lifecycle.
    let gone = !Path::new(&format!("/proc/{pid}")).exists();
    let shadow: Vec<String> = fs::read_dir(&host_shadow)
        .map(|it| it.filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into())).collect())
        .unwrap_or_default();
    let objects_ok = store.ids().iter().all(|id| store.get_verified(id).is_ok());
    let again = snapshot::entries(&store, &s).map(|e| e.len());
    println!("\nafter B is destroyed: /proc/{pid} gone: {gone}; host <root>/v/d holds {shadow:?}; {} objects all verify: {objects_ok}; entries(S) = {again:?}", store.ids().len());

    // ---- Reproducibility: two more B worlds from the same S, unattacked.
    let mut worlds: Vec<(Observation, u64)> = Vec::new();
    for _ in 0..2 {
        let plan = capability::plan_sealed(&lab.manifest(&[]), &[(PathBuf::from(VIEW), s.clone())], &store)
            .unwrap();
        let root = lab.root();
        let obs = capability::observe_with(&plan, &root, &lab.python, &BTreeMap::new(), &b_probe()).unwrap();
        let dev = match obs.resolved(VIEW) {
            Some(capability::Resolved::Seen { dev, .. }) => *dev,
            _ => 0,
        };
        assert_eq!(capability::check(&plan, &obs).verdict, Verdict::Allow);
        worlds.push((obs, dev));
    }
    let names = |o: &Observation| -> Vec<String> {
        let mut v: Vec<String> = o.names().iter().map(|n| format!("{n:?}")).collect();
        v.sort();
        v
    };
    let roots: Vec<_> = worlds
        .iter()
        .map(|(o, _)| snapshot_from(&ObjectStore::new(), &o.transcript(), VIEW, "p").ok())
        .collect();
    let same_names = names(&worlds[0].0) == names(&worlds[1].0) && names(&worlds[0].0) == names(&b);
    println!(
        "\nreproducibility: capability names identical across B, B2, B3: {same_names} ({} names); roots = S: {:?}; /v/d st_dev: B2 {}, B3 {}",
        names(&b).len(),
        roots.iter().map(|r| r.as_deref() == Some(s.as_str())).collect::<Vec<_>>(),
        worlds[0].1,
        worlds[1].1
    );
    let plan_digest_same = {
        let p1: ConstructionPlan =
            capability::plan_sealed(&lab.manifest(&[]), &[(PathBuf::from(VIEW), s.clone())], &store).unwrap();
        let p2 = capability::plan_sealed(&lab.manifest(&[]), &[(PathBuf::from(VIEW), s.clone())], &store).unwrap();
        p1.digest() == p2.digest()
    };
    println!("plan digest identical for the same S: {plan_digest_same}");
    let _ = d.metadata().map(|m| m.ino());

    // Assertions: only what was measured above.
    assert_eq!(in_root.as_deref(), Ok(s.as_str()));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(b.op("2-write"), Some("EROFS"));
    assert!(!walked_proc);
    assert!(gone && objects_ok && same_names && plan_digest_same);
    assert!(roots.iter().all(|r| r.as_deref() == Some(s.as_str())));
}
