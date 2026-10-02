//! Capability manifest v0: build an agent's world from a manifest, observe
//! it, and check that what was observed is a subset of what was declared.
//!
//! ```text
//! CapabilityManifest ──plan()──▶ ConstructionPlan ──observe()──▶ Observation
//!                                        │                            │
//!                                        └──────── check() ◀──────────┘
//! ```
//!
//! The inventory experiment (research/CAPABILITY_INVENTORY_V0.md) showed
//! that ambient authority cannot be enumerated by observation. So the
//! world is *constructed*: an empty root, the declared paths bound in, a
//! fresh namespace for everything the manifest does not mention. The
//! observers then check the construction instead of discovering it.
//!
//! # Capability names
//!
//! Observed and declared capabilities share one `/`-separated namespace,
//! so containment is the kernel's `Within` requirement:
//!
//! | Name | Observed when | Declared by |
//! |---|---|---|
//! | `fs/read<p>`, `fs/write<p>` | `access(2)` grants it on file `p` | `read_paths` / `write_paths` (prefix) |
//! | `fs/read<d>/.`, `fs/write<d>/.` | ... on directory `d` | same; ancestors of a declared path as exact `fs/read<a>/.` |
//! | `special<p>` | a socket, fifo or device node at `p` | never |
//! | `env/<NAME>` | the variable is set | `env` |
//! | `proc/descendants` | `fork` succeeded | `descendants_allowed` |
//! | `net/if/<n>` | interface `n` is up | never |
//! | `net/connect/<t>` | a canary target accepted a connection | never |
//!
//! What an observer could not see becomes `Name::UnknownBelow`, never an
//! absence: an unreadable directory leaves its subtree undetermined.
//!
//! # Invariants
//!
//! | Id | Statement | Requirement |
//! |---|---|---|
//! | `C1.observed_within_declared` | every observed capability is declared | `Within` |
//! | `C2.declared_visible` | every declared capability is present, and is the declared object | `Fact` per capability |
//! | `C3.closed` | no inherited state outside the plan | `Fact` per closing property |
//!
//! # Observers
//!
//! * **Inside**: `world_probe.py`, run as the only program of the world.
//!   It walks the whole view, calls `access(2)` per entry, reads its own
//!   identity, tries one `fork` and connects to canary targets. Its source
//!   is compiled into this crate and passed on argv, so the world holds no
//!   observer files.
//! * **Outside**: the runtime reads `/proc/<pid>` of the held probe:
//!   namespace identities and the file descriptor table. These are kernel
//!   tables, complete by construction, and not reported by code in the world.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::kernel::{
    self, Decision, EvidenceBase, Fact, Name, Obligation, Phase, Provenance, Requirement, Strength,
    Verified,
};

const PROBE: &str = include_str!("world_probe.py");
const NAMESPACES: [&str; 5] = ["user", "mnt", "net", "ipc", "pid"];
pub const DESCENDANTS: &str = "proc/descendants";

/// What the agent may hold. Everything else is closed by construction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Absolute canonical paths, bound read-only.
    pub read_paths: Vec<PathBuf>,
    /// Absolute canonical paths, bound read-write.
    pub write_paths: Vec<PathBuf>,
    /// Whether the agent may create processes.
    pub descendants_allowed: bool,
    /// The complete environment.
    pub env: BTreeMap<String, String>,
    /// Objects bound at a path other than their own: the same object
    /// under another name, or another object under a shared name.
    #[serde(default)]
    pub aliases: Vec<Alias>,
}

/// Bind the host object at `source` (canonical, existing) at `target`
/// in the world (absolute, syntactically canonical; need not exist on
/// the host).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alias {
    pub source: PathBuf,
    pub target: PathBuf,
    pub writable: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("path is not absolute: {0:?}")]
    NotAbsolute(PathBuf),
    #[error("path is not valid UTF-8: {0:?}")]
    NotUtf8(PathBuf),
    #[error("path is not canonical (symlink, `..` or trailing slash): {0:?}")]
    NotCanonical(PathBuf),
    #[error("the root itself cannot be declared")]
    Root,
    #[error("cannot stat {0:?}: {1}")]
    Missing(PathBuf, String),
    #[error("invalid environment variable name: {0:?}")]
    BadEnvName(String),
    #[error("two declarations bind {0:?}")]
    DuplicateTarget(String),
    #[error("snapshot {0}: {1}")]
    Snapshot(String, String),
    #[error("cannot read runtime identity: {0}")]
    Identity(String),
}

/// The runtime's own identity: what the world inherits unless removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    pub overflow_gid: u32,
}

impl Identity {
    pub fn current() -> Result<Self, PlanError> {
        let overflow = fs::read_to_string("/proc/sys/kernel/overflowgid")
            .map_err(|e| PlanError::Identity(format!("overflowgid: {e}")))?;
        // SAFETY: getgroups with a buffer of the size it reported.
        let groups = unsafe {
            let n = libc::getgroups(0, std::ptr::null_mut());
            let mut buf = vec![0 as libc::gid_t; n.max(0) as usize];
            let m = libc::getgroups(n, buf.as_mut_ptr());
            if m < 0 {
                return Err(PlanError::Identity("getgroups failed".into()));
            }
            buf.truncate(m as usize);
            buf
        };
        Ok(Self {
            // SAFETY: always succeed.
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            groups,
            overflow_gid: overflow
                .trim()
                .parse()
                .map_err(|e| PlanError::Identity(format!("overflowgid: {e}")))?,
        })
    }

    /// Supplementary groups as a process in a user namespace that maps
    /// only `gid` will see them: every other gid becomes the overflow gid.
    fn groups_inside(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .groups
            .iter()
            .map(|&g| if g == self.gid { g } else { self.overflow_gid })
            .collect();
        out.sort_unstable();
        out
    }
}

/// A declared path, with the identity of the object the runtime saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Bind {
    /// The path in the world.
    pub path: String,
    /// The host path bound there (`path` itself unless aliased).
    pub source: String,
    pub writable: bool,
    pub dir: bool,
    pub dev: u64,
    pub ino: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// `unshare` user, mount, network, IPC and PID namespaces.
    Namespaces {
        kinds: Vec<String>,
    },
    DenySetgroups,
    /// Map only the runtime's own uid and gid; every other id is unmapped.
    MapUid {
        uid: u32,
    },
    MapGid {
        gid: u32,
    },
    /// Fork the world's first process, PID 1 of the new PID namespace.
    EnterPidNamespace,
    PrivatePropagation,
    /// An empty tmpfs that becomes the world's root.
    EmptyRoot,
    /// Bind one declared path (non-recursive: submounts are not carried).
    Bind(Bind),
    /// A sealed snapshot: a tmpfs mounted at `target` inside the world's
    /// mount namespace, filled with the verified entries of tree `root`,
    /// then made read-only (superblock and mount). No host path names it.
    Snapshot {
        target: String,
        root: String,
        entries: usize,
    },
    /// Remount the root read-only: the skeleton gains no writable names.
    SealRoot,
    /// Make the empty root `/` and detach the host's.
    PivotRoot,
    /// Leave the runtime's session: no controlling terminal.
    NewSession,
    NoNewPrivs,
    /// Clear inheritable and ambient capabilities.
    ClearCapabilities,
    /// `RLIMIT_NPROC = 1` (hard) unless descendants are allowed.
    LimitDescendants {
        allowed: bool,
    },
    /// Mark every fd above 2 close-on-exec.
    CloseInheritedFds,
    /// The environment execve receives: exactly the declared variables.
    Environment {
        names: Vec<String>,
    },
}

/// Inherited state the runtime cannot remove unprivileged, accepted
/// explicitly. Its authority must show up elsewhere to matter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "residue", rename_all = "snake_case")]
pub enum Residue {
    /// Supplementary groups survive an unprivileged user namespace. They
    /// act only through objects in view, which the walk's `access(2)`
    /// calls already account for.
    SupplementaryGroups { host: Vec<u32>, inside: Vec<u32> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConstructionPlan {
    manifest: CapabilityManifest,
    identity: Identity,
    binds: Vec<Bind>,
    skeleton: Vec<String>,
    residue: Vec<Residue>,
    steps: Vec<Step>,
    /// Sealed snapshot targets (world paths).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    snapshots: Vec<String>,
    /// Their verified contents, by target. Identified in the digest by
    /// the tree root in `Step::Snapshot`, not serialized.
    #[serde(skip)]
    snapshot_entries: BTreeMap<String, Vec<crate::snapshot::SnapEntry>>,
}

impl ConstructionPlan {
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn binds(&self) -> &[Bind] {
        &self.binds
    }

    pub fn residue(&self) -> &[Residue] {
        &self.residue
    }

    pub fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    pub fn digest(&self) -> String {
        let json = serde_json::to_vec(self).expect("plan serializes");
        crate::verifiers::hex(&Sha256::digest(json))
    }

    /// Capability scopes the manifest declares.
    pub fn scopes(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        out.extend(self.skeleton.iter().map(|a| fs_name("read", a, true)));
        for bind in &self.binds {
            out.push(fs_name("read", &bind.path, false));
            if bind.writable {
                out.push(fs_name("write", &bind.path, false));
            }
        }
        out.extend(self.snapshots.iter().map(|t| fs_name("read", t, false)));
        out.extend(self.manifest.env.keys().map(|k| env_name(k)));
        if self.manifest.descendants_allowed {
            out.push(DESCENDANTS.to_string());
        }
        out
    }

    fn covers_read(&self, path: &str) -> bool {
        self.binds
            .iter()
            .any(|b| kernel::name_within(path, &b.path))
    }
}

/// `fs/<op><path>` for files, `fs/<op><path>/.` for directories, so a
/// directory scope can be exact (an ancestor) while a declared path
/// covers everything below it.
pub fn fs_name(op: &str, path: &str, dir: bool) -> String {
    let p = path.trim_end_matches('/');
    if dir {
        format!("fs/{op}{p}/.")
    } else {
        format!("fs/{op}{p}")
    }
}

pub fn env_name(var: &str) -> String {
    format!("env/{}", var.replace('%', "%25").replace('/', "%2F"))
}

/// Turn a manifest into the steps that build its world.
pub fn plan(manifest: &CapabilityManifest) -> Result<ConstructionPlan, PlanError> {
    plan_for(manifest, Identity::current()?)
}

pub fn plan_for(
    manifest: &CapabilityManifest,
    identity: Identity,
) -> Result<ConstructionPlan, PlanError> {
    for name in manifest.env.keys() {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(PlanError::BadEnvName(name.clone()));
        }
    }
    // target -> (source, writable)
    let mut declared: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for (paths, writable) in [(&manifest.read_paths, false), (&manifest.write_paths, true)] {
        for path in paths {
            let key = check_path(path)?;
            declared.entry(key.clone()).or_insert((key, false)).1 |= writable;
        }
    }
    for alias in &manifest.aliases {
        let source = check_path(&alias.source)?;
        let target = check_target(&alias.target)?;
        if declared.contains_key(&target) {
            return Err(PlanError::DuplicateTarget(target));
        }
        declared.insert(target, (source, alias.writable));
    }
    let mut binds = Vec::new();
    let mut skeleton: BTreeSet<String> = BTreeSet::from(["/".to_string()]);
    for (path, (source, writable)) in declared {
        let meta = fs::symlink_metadata(&source)
            .map_err(|e| PlanError::Missing(PathBuf::from(&source), e.to_string()))?;
        let mut ancestor = Path::new(&path).parent();
        while let Some(a) = ancestor {
            skeleton.insert(a.to_str().expect("utf-8 checked").to_string());
            ancestor = a.parent();
        }
        binds.push(Bind {
            path,
            source,
            writable,
            dir: meta.is_dir(),
            dev: meta.dev(),
            ino: meta.ino(),
        });
    }
    // Shallow first, so a nested declaration is mounted over its parent.
    binds.sort_by_key(|b| (b.path.matches('/').count(), b.path.clone()));
    // An ancestor that is itself declared is covered by its own scope.
    skeleton.retain(|a| !binds.iter().any(|b| kernel::name_within(a, &b.path)));

    let mut steps = vec![
        Step::Namespaces {
            kinds: NAMESPACES.iter().map(|s| s.to_string()).collect(),
        },
        Step::DenySetgroups,
        Step::MapUid { uid: identity.uid },
        Step::MapGid { gid: identity.gid },
        Step::EnterPidNamespace,
        Step::PrivatePropagation,
        Step::EmptyRoot,
    ];
    steps.extend(binds.iter().cloned().map(Step::Bind));
    steps.extend([
        Step::SealRoot,
        Step::PivotRoot,
        Step::NewSession,
        Step::NoNewPrivs,
        Step::ClearCapabilities,
        Step::LimitDescendants {
            allowed: manifest.descendants_allowed,
        },
        Step::CloseInheritedFds,
        Step::Environment {
            names: manifest.env.keys().cloned().collect(),
        },
    ]);
    let residue = vec![Residue::SupplementaryGroups {
        host: identity.groups.clone(),
        inside: identity.groups_inside(),
    }];
    Ok(ConstructionPlan {
        manifest: manifest.clone(),
        identity,
        binds,
        skeleton: skeleton.into_iter().collect(),
        residue,
        steps,
        snapshots: Vec::new(),
        snapshot_entries: BTreeMap::new(),
    })
}

/// Plan a world that also holds sealed snapshots: each `(target, root)`
/// is checked out of `store` (every object verified first) into a tmpfs
/// that exists only in the world's mount namespace.
pub fn plan_sealed(
    manifest: &CapabilityManifest,
    snapshots: &[(PathBuf, String)],
    store: &crate::snapshot::ObjectStore,
) -> Result<ConstructionPlan, PlanError> {
    let mut plan = plan(manifest)?;
    let seal = plan
        .steps
        .iter()
        .position(|s| matches!(s, Step::SealRoot))
        .expect("plan seals its root");
    for (i, (target, root)) in snapshots.iter().enumerate() {
        let target = check_target(target)?;
        if plan
            .binds
            .iter()
            .any(|b| kernel::name_within(&target, &b.path))
            || plan
                .snapshots
                .iter()
                .any(|t| kernel::name_within(&target, t))
        {
            return Err(PlanError::DuplicateTarget(target));
        }
        let entries = crate::snapshot::entries(store, root)
            .map_err(|e| PlanError::Snapshot(root.clone(), e))?;
        let mut ancestor = Path::new(&target).parent();
        while let Some(a) = ancestor {
            let s = a.to_str().expect("utf-8 checked").to_string();
            if !plan.skeleton.contains(&s) {
                plan.skeleton.push(s);
            }
            ancestor = a.parent();
        }
        plan.skeleton.sort();
        plan.steps.insert(
            seal + i,
            Step::Snapshot {
                target: target.clone(),
                root: root.clone(),
                entries: entries.len(),
            },
        );
        plan.snapshots.push(target.clone());
        plan.snapshot_entries.insert(target, entries);
    }
    Ok(plan)
}

/// A world path: absolute, UTF-8, not `/`, no empty, `.` or `..`
/// components. It is not resolved on the host.
fn check_target(path: &Path) -> Result<String, PlanError> {
    let Some(s) = path.to_str() else {
        return Err(PlanError::NotUtf8(path.to_path_buf()));
    };
    let Some(rest) = s.strip_prefix('/') else {
        return Err(PlanError::NotAbsolute(path.to_path_buf()));
    };
    if rest.is_empty() {
        return Err(PlanError::Root);
    }
    if rest
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(PlanError::NotCanonical(path.to_path_buf()));
    }
    Ok(s.to_string())
}

fn check_path(path: &Path) -> Result<String, PlanError> {
    if !path.is_absolute() {
        return Err(PlanError::NotAbsolute(path.to_path_buf()));
    }
    let Some(s) = path.to_str() else {
        return Err(PlanError::NotUtf8(path.to_path_buf()));
    };
    if s == "/" {
        return Err(PlanError::Root);
    }
    let canonical = fs::canonicalize(path)
        .map_err(|e| PlanError::Missing(path.to_path_buf(), e.to_string()))?;
    if canonical.as_os_str() != path.as_os_str() {
        return Err(PlanError::NotCanonical(path.to_path_buf()));
    }
    Ok(s.to_string())
}

// ---------------------------------------------------------------------------
// Construction

/// A canary endpoint outside the world, for the inside probe to try.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Abstract { addr: String },
    Path { addr: String },
    Tcp { host: String, port: u16 },
}

#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error("interpreter {0:?} is not inside a declared read path")]
    InterpreterOutside(PathBuf),
    #[error("construction failed (status {status:?}): {stderr}")]
    Construction { status: Option<i32>, stderr: String },
    #[error("probe report unreadable: {0}")]
    Report(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

enum Op {
    Unshare(libc::c_int),
    Write {
        path: CString,
        data: Vec<u8>,
    },
    ForkInit,
    Private,
    Tmpfs {
        target: CString,
    },
    Bind {
        mkdirs: Vec<CString>,
        target: CString,
        source: CString,
        dir: bool,
        readonly: Option<libc::c_ulong>,
    },
    Seal {
        target: CString,
    },
    Snapshot {
        mkdirs: Vec<CString>,
        target: CString,
        items: Vec<SnapOp>,
    },
    Pivot {
        root: CString,
    },
    SetSid,
    NoNewPrivs,
    ClearCaps,
    LimitProcs,
    CloseFds,
    Nothing,
}

enum SnapOp {
    Dir(CString),
    File {
        path: CString,
        bytes: Vec<u8>,
        mode: libc::mode_t,
    },
    Link {
        target: CString,
        path: CString,
    },
}

fn cstr(s: impl AsRef<[u8]>) -> CString {
    CString::new(s.as_ref()).expect("no interior NUL")
}

/// Mount flags the kernel locks on a mount copied into a less privileged
/// namespace. A read-only remount must repeat them or fail with `EPERM`.
fn locked_flags(path: &str) -> std::io::Result<libc::c_ulong> {
    let c = cstr(path);
    // SAFETY: valid C string and out-pointer.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut flags = 0;
    for (st_flag, ms_flag) in [
        (libc::ST_NOSUID, libc::MS_NOSUID),
        (libc::ST_NODEV, libc::MS_NODEV),
        (libc::ST_NOEXEC, libc::MS_NOEXEC),
        (libc::ST_NOATIME, libc::MS_NOATIME),
        (libc::ST_NODIRATIME, libc::MS_NODIRATIME),
        (libc::ST_RELATIME, libc::MS_RELATIME),
    ] {
        if st.f_flag & st_flag != 0 {
            flags |= ms_flag;
        }
    }
    Ok(flags)
}

fn prepare(plan: &ConstructionPlan, root: &Path) -> std::io::Result<Vec<(usize, Op)>> {
    let root_s = root.to_str().expect("root is UTF-8").trim_end_matches('/');
    let mut ops = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        let op = match step {
            Step::Namespaces { .. } => Op::Unshare(
                libc::CLONE_NEWUSER
                    | libc::CLONE_NEWNS
                    | libc::CLONE_NEWNET
                    | libc::CLONE_NEWIPC
                    | libc::CLONE_NEWPID,
            ),
            Step::DenySetgroups => Op::Write {
                path: cstr("/proc/self/setgroups"),
                data: b"deny".to_vec(),
            },
            Step::MapUid { uid } => Op::Write {
                path: cstr("/proc/self/uid_map"),
                data: format!("{uid} {uid} 1\n").into_bytes(),
            },
            Step::MapGid { gid } => Op::Write {
                path: cstr("/proc/self/gid_map"),
                data: format!("{gid} {gid} 1\n").into_bytes(),
            },
            Step::EnterPidNamespace => Op::ForkInit,
            Step::PrivatePropagation => Op::Private,
            Step::EmptyRoot => Op::Tmpfs {
                target: cstr(root_s),
            },
            Step::Bind(bind) => {
                let mut mkdirs = Vec::new();
                let mut acc = root_s.to_string();
                let parent = Path::new(&bind.path).parent().expect("not root");
                for seg in parent
                    .to_str()
                    .expect("utf-8")
                    .split('/')
                    .filter(|s| !s.is_empty())
                {
                    acc = format!("{acc}/{seg}");
                    mkdirs.push(cstr(&acc));
                }
                Op::Bind {
                    mkdirs,
                    target: cstr(format!("{root_s}{}", bind.path)),
                    source: cstr(&bind.source),
                    dir: bind.dir,
                    readonly: if bind.writable {
                        None
                    } else {
                        Some(locked_flags(&bind.source)?)
                    },
                }
            }
            Step::Snapshot { target, .. } => {
                use crate::snapshot::SnapEntry;
                let mut mkdirs = Vec::new();
                let mut acc = root_s.to_string();
                for seg in target.split('/').filter(|s| !s.is_empty()) {
                    acc = format!("{acc}/{seg}");
                    mkdirs.push(cstr(&acc));
                }
                let base = format!("{root_s}{target}");
                let items = plan.snapshot_entries[target]
                    .iter()
                    .map(|e| match e {
                        SnapEntry::Dir(p) => SnapOp::Dir(cstr(format!("{base}/{p}"))),
                        SnapEntry::File {
                            path,
                            executable,
                            bytes,
                        } => SnapOp::File {
                            path: cstr(format!("{base}/{path}")),
                            bytes: bytes.clone(),
                            mode: if *executable { 0o555 } else { 0o444 },
                        },
                        SnapEntry::Link { path, target } => SnapOp::Link {
                            target: cstr(target),
                            path: cstr(format!("{base}/{path}")),
                        },
                    })
                    .collect();
                Op::Snapshot {
                    mkdirs,
                    target: cstr(base),
                    items,
                }
            }
            Step::SealRoot => Op::Seal {
                target: cstr(root_s),
            },
            Step::PivotRoot => Op::Pivot { root: cstr(root_s) },
            Step::NewSession => Op::SetSid,
            Step::NoNewPrivs => Op::NoNewPrivs,
            Step::ClearCapabilities => Op::ClearCaps,
            Step::LimitDescendants { allowed: false } => Op::LimitProcs,
            Step::LimitDescendants { allowed: true } => Op::Nothing,
            Step::CloseInheritedFds => Op::CloseFds,
            // Applied by execve from the Command's cleared environment.
            Step::Environment { .. } => Op::Nothing,
        };
        ops.push((i, op));
    }
    Ok(ops)
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Report a failed step on fd 2 and exit. Runs between fork and exec, so
/// it formats into a stack buffer.
fn fail(step: usize, err: i32) -> ! {
    let mut buf = [0u8; 64];
    let prefix = b"v9r world: step ";
    buf[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    let put = |buf: &mut [u8; 64], n: &mut usize, mut v: u32| {
        let mut digits = [0u8; 10];
        let mut k = 0;
        loop {
            digits[k] = b'0' + (v % 10) as u8;
            v /= 10;
            k += 1;
            if v == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            buf[*n] = digits[k];
            *n += 1;
        }
    };
    put(&mut buf, &mut n, step as u32);
    for &b in b" errno " {
        buf[n] = b;
        n += 1;
    }
    put(&mut buf, &mut n, err as u32);
    buf[n] = b'\n';
    n += 1;
    // SAFETY: writing an initialized stack buffer, then exiting.
    unsafe {
        libc::write(2, buf.as_ptr().cast(), n);
        libc::_exit(125)
    }
}

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: libc::c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// Execute the prepared steps between fork and exec. No allocation: every
/// string was built by `prepare`. Returns only in the world's first
/// process; the intermediate process (outside the new PID namespace)
/// waits for it and exits with its status.
///
/// # Safety
/// Must run in a freshly forked, single-threaded child.
unsafe fn construct(ops: &[(usize, Op)]) {
    let check = |step: usize, r: libc::c_long| {
        if r < 0 {
            fail(step, errno());
        }
    };
    for (step, op) in ops {
        let step = *step;
        match op {
            Op::Unshare(flags) => check(step, libc::unshare(*flags) as _),
            Op::Write { path, data } => {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                check(step, fd as _);
                let n = libc::write(fd, data.as_ptr().cast(), data.len());
                if n != data.len() as isize {
                    fail(step, errno());
                }
                libc::close(fd);
            }
            Op::ForkInit => {
                let pid = libc::fork();
                check(step, pid as _);
                if pid > 0 {
                    // The intermediate never execs, so it must drop every
                    // fd itself: std's CLOEXEC exec-status socket (else
                    // `spawn` waits for this process forever) and its copies
                    // of the runtime's pipes and other inherited fds.
                    libc::syscall(libc::SYS_close_range, 0, libc::c_uint::MAX, 0);
                    let mut status = 0;
                    loop {
                        let r = libc::waitpid(pid, &mut status, 0);
                        if r == -1 && errno() == libc::EINTR {
                            continue;
                        }
                        break;
                    }
                    libc::_exit(if libc::WIFEXITED(status) {
                        libc::WEXITSTATUS(status)
                    } else {
                        128 + libc::WTERMSIG(status)
                    });
                }
                check(
                    step,
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) as _,
                );
            }
            Op::Private => check(
                step,
                libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                ) as _,
            ),
            Op::Tmpfs { target } => check(
                step,
                libc::mount(
                    c"tmpfs".as_ptr(),
                    target.as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    c"mode=0755".as_ptr().cast(),
                ) as _,
            ),
            Op::Bind {
                mkdirs,
                target,
                source,
                dir,
                readonly,
            } => {
                for d in mkdirs {
                    if libc::mkdir(d.as_ptr(), 0o755) != 0 && errno() != libc::EEXIST {
                        fail(step, errno());
                    }
                }
                if libc::access(target.as_ptr(), libc::F_OK) != 0 {
                    if *dir {
                        check(step, libc::mkdir(target.as_ptr(), 0o755) as _);
                    } else {
                        let fd = libc::open(
                            target.as_ptr(),
                            libc::O_CREAT | libc::O_WRONLY | libc::O_CLOEXEC,
                            0o644,
                        );
                        check(step, fd as _);
                        libc::close(fd);
                    }
                }
                check(
                    step,
                    libc::mount(
                        source.as_ptr(),
                        target.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    ) as _,
                );
                if let Some(locked) = readonly {
                    check(
                        step,
                        libc::mount(
                            std::ptr::null(),
                            target.as_ptr(),
                            std::ptr::null(),
                            libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | locked,
                            std::ptr::null(),
                        ) as _,
                    );
                }
            }
            Op::Snapshot {
                mkdirs,
                target,
                items,
            } => {
                for d in mkdirs {
                    if libc::mkdir(d.as_ptr(), 0o755) != 0 && errno() != libc::EEXIST {
                        fail(step, errno());
                    }
                }
                check(
                    step,
                    libc::mount(
                        c"tmpfs".as_ptr(),
                        target.as_ptr(),
                        c"tmpfs".as_ptr(),
                        libc::MS_NOSUID | libc::MS_NODEV,
                        c"mode=0755".as_ptr().cast(),
                    ) as _,
                );
                for item in items {
                    match item {
                        SnapOp::Dir(p) => check(step, libc::mkdir(p.as_ptr(), 0o755) as _),
                        SnapOp::Link { target, path } => {
                            check(step, libc::symlink(target.as_ptr(), path.as_ptr()) as _)
                        }
                        SnapOp::File { path, bytes, mode } => {
                            let fd = libc::open(
                                path.as_ptr(),
                                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_CLOEXEC,
                                0o600,
                            );
                            check(step, fd as _);
                            let mut done = 0;
                            while done < bytes.len() {
                                let n = libc::write(
                                    fd,
                                    bytes[done..].as_ptr().cast(),
                                    bytes.len() - done,
                                );
                                check(step, n as _);
                                done += n as usize;
                            }
                            check(step, libc::fchmod(fd, *mode) as _);
                            libc::close(fd);
                        }
                    }
                }
                // Read-only twice: the superblock, then this mount.
                for flags in [
                    libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
                    libc::MS_REMOUNT
                        | libc::MS_BIND
                        | libc::MS_RDONLY
                        | libc::MS_NOSUID
                        | libc::MS_NODEV,
                ] {
                    check(
                        step,
                        libc::mount(
                            std::ptr::null(),
                            target.as_ptr(),
                            std::ptr::null(),
                            flags,
                            std::ptr::null(),
                        ) as _,
                    );
                }
            }
            Op::Seal { target } => check(
                step,
                libc::mount(
                    std::ptr::null(),
                    target.as_ptr(),
                    std::ptr::null(),
                    libc::MS_REMOUNT
                        | libc::MS_BIND
                        | libc::MS_RDONLY
                        | libc::MS_NOSUID
                        | libc::MS_NODEV,
                    std::ptr::null(),
                ) as _,
            ),
            Op::Pivot { root } => {
                check(step, libc::chdir(root.as_ptr()) as _);
                check(
                    step,
                    libc::syscall(libc::SYS_pivot_root, c".".as_ptr(), c".".as_ptr()),
                );
                check(step, libc::umount2(c".".as_ptr(), libc::MNT_DETACH) as _);
                check(step, libc::chdir(c"/".as_ptr()) as _);
            }
            Op::SetSid => check(step, libc::setsid() as _),
            // prctl checks unused arguments are zero as `unsigned long`;
            // pass every variadic argument at that width.
            Op::NoNewPrivs => check(
                step,
                libc::prctl(
                    libc::PR_SET_NO_NEW_PRIVS,
                    1 as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                ) as _,
            ),
            Op::ClearCaps => {
                check(
                    step,
                    libc::prctl(
                        libc::PR_CAP_AMBIENT,
                        libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
                        0 as libc::c_ulong,
                        0 as libc::c_ulong,
                        0 as libc::c_ulong,
                    ) as _,
                );
                let mut header = CapHeader {
                    version: 0x2008_0522,
                    pid: 0,
                };
                let mut data = [CapData {
                    effective: 0,
                    permitted: 0,
                    inheritable: 0,
                }; 2];
                check(
                    step,
                    libc::syscall(
                        libc::SYS_capget,
                        &mut header as *mut CapHeader,
                        data.as_mut_ptr(),
                    ),
                );
                data[0].inheritable = 0;
                data[1].inheritable = 0;
                check(
                    step,
                    libc::syscall(
                        libc::SYS_capset,
                        &mut header as *mut CapHeader,
                        data.as_ptr(),
                    ),
                );
            }
            Op::LimitProcs => {
                let limit = libc::rlimit {
                    rlim_cur: 1,
                    rlim_max: 1,
                };
                check(step, libc::setrlimit(libc::RLIMIT_NPROC, &limit) as _);
            }
            Op::CloseFds => check(
                step,
                libc::syscall(
                    libc::SYS_close_range,
                    3u32,
                    u32::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                ),
            ),
            Op::Nothing => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Observation

#[derive(Clone, Debug, Deserialize)]
struct Inside {
    #[serde(default)]
    ops: BTreeMap<String, String>,
    #[serde(default)]
    resolved: BTreeMap<String, Resolved>,
    /// `(kind, path, value)` raw observations for the snapshot verifier.
    #[serde(default)]
    transcript: Vec<(String, String, serde_json::Value)>,
    identity: InsideIdentity,
    env: BTreeMap<String, String>,
    walk: InsideWalk,
    declared: BTreeMap<String, InsideDeclared>,
    process: InsideProcess,
    network: InsideNetwork,
    targets: BTreeMap<String, String>,
}

/// `pid`, `uid` and `gid` are reported verbatim (`inside_json`); the
/// checks use the rest.
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
struct InsideIdentity {
    pid: i64,
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
    no_new_privs: Option<i64>,
    caps: Option<Caps>,
}

#[derive(Clone, Debug, Deserialize)]
struct Caps {
    eff: u64,
    prm: u64,
    inh: u64,
    amb: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct InsideWalk {
    entries: Vec<(String, String, bool, bool)>,
    unknown: Vec<(String, String)>,
    /// `(st_dev, st_ino)` of `entries[i]`.
    #[serde(default)]
    ids: Vec<(u64, u64)>,
}

/// One path resolved to an object inside a world, by the probe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resolved {
    Seen {
        kind: String,
        dev: u64,
        ino: u64,
        /// `FS_IOC_GETVERSION` (the inode generation), or the errno.
        generation: Result<u32, String>,
        /// `name_to_handle_at`: `<type>:<hex>`, or the errno.
        handle: Result<String, String>,
        /// The mount id `name_to_handle_at` reported, or the errno.
        mount: Result<i64, String>,
    },
    Error {
        error: String,
    },
}

/// An operation the probe performs in the world before observing, chosen
/// by the runtime (never by code in the world).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProbeOp {
    Read { path: String },
    Write { path: String, data: String },
    Link { path: String, to: String },
    Rename { path: String, to: String },
    Mount { path: String, fstype: String },
}

/// What the probe does besides observing: operations first, then
/// resolving paths to objects.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ProbeExtra {
    pub ops: BTreeMap<String, ProbeOp>,
    pub resolve: Vec<String>,
    /// Directories to transcribe for a snapshot (`fs_dir`, `fs_stat`,
    /// `fs_file`, `fs_link`, as `crate::fs_raw` reports them).
    pub transcribe: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum InsideDeclared {
    Seen {
        dev: u64,
        ino: u64,
        r: bool,
        w: bool,
    },
    Error {
        error: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
struct InsideProcess {
    nproc: (i64, i64),
    fork: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum InsideNetwork {
    List {
        list: Vec<(String, serde_json::Value)>,
    },
    Error {
        error: String,
    },
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outside {
    /// Host pid of the world's first process.
    pub pid: u32,
    /// Namespace identity: (world, runtime). `None` if unreadable.
    pub namespaces: BTreeMap<String, Option<(String, String)>>,
    /// `/proc/<pid>/fd`: (fd, link). `None` if unreadable.
    pub fds: Option<Vec<(u32, String)>>,
    /// The links fds 0, 1, 2 must have: the runtime's own pipes.
    pub expected_fds: Vec<String>,
}

/// What the observers saw in one constructed world. Only this module
/// builds one, from live observation.
#[derive(Clone, Debug)]
pub struct Observation {
    raw: String,
    inside: Inside,
    outside: Outside,
    probe_digest: String,
    exit: Option<i32>,
}

impl Observation {
    /// The inside probe's report, verbatim.
    pub fn inside_json(&self) -> &str {
        &self.raw
    }

    pub fn outside(&self) -> &Outside {
        &self.outside
    }

    pub fn exit(&self) -> Option<i32> {
        self.exit
    }

    pub fn probe_digest(&self) -> &str {
        &self.probe_digest
    }

    /// What a canary target answered from inside.
    pub fn target(&self, key: &str) -> Option<&str> {
        self.inside.targets.get(key).map(String::as_str)
    }

    /// The walk's entry for `path`: (kind, readable, writable).
    pub fn walked(&self, path: &str) -> Option<(&str, bool, bool)> {
        self.inside
            .walk
            .entries
            .iter()
            .find(|e| e.0 == path)
            .map(|e| (e.1.as_str(), e.2, e.3))
    }

    /// What a configured operation returned: `ok`, `ok:<sha256>` or an errno.
    pub fn op(&self, key: &str) -> Option<&str> {
        self.inside.ops.get(key).map(String::as_str)
    }

    /// The object a path resolved to inside the world.
    pub fn resolved(&self, path: &str) -> Option<&Resolved> {
        self.inside.resolved.get(path)
    }

    /// The probe's raw observations as snapshot inputs. A malformed item
    /// is dropped (the snapshot that needs it is then incomplete).
    pub fn transcript(&self) -> BTreeMap<crate::graph::Key, crate::graph::Term> {
        use crate::graph::{Key, Term};
        let mut out = BTreeMap::new();
        for (kind, path, value) in &self.inside.transcript {
            let term = match (kind.as_str(), value) {
                ("fs_dir" | "fs_stat", serde_json::Value::Object(m)) => Term::Map(
                    m.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                        .collect(),
                ),
                ("fs_file", serde_json::Value::String(h)) => {
                    match (0..h.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok())
                        .collect::<Option<Vec<u8>>>()
                    {
                        Some(bytes) => Term::Bytes(bytes),
                        None => continue,
                    }
                }
                ("fs_link", serde_json::Value::String(t)) => Term::Text(t.clone()),
                _ => continue,
            };
            out.insert(Key::new(kind, [path.as_str()]), term);
        }
        out
    }

    /// Every walked entry at or below `path`, with its `(st_dev, st_ino)`.
    /// Empty if the probe reported no ids.
    pub fn objects_within(&self, path: &str) -> Vec<(&str, u64, u64)> {
        let w = &self.inside.walk;
        if w.ids.len() != w.entries.len() {
            return Vec::new();
        }
        w.entries
            .iter()
            .zip(&w.ids)
            .filter(|(e, _)| kernel::name_within(&e.0, path))
            .map(|(e, id)| (e.0.as_str(), id.0, id.1))
            .collect()
    }

    pub fn walk_len(&self) -> (usize, usize) {
        (
            self.inside.walk.entries.len(),
            self.inside.walk.unknown.len(),
        )
    }

    /// Every capability the observers saw, known or unknown.
    pub fn names(&self) -> Vec<Name> {
        let mut out = Vec::new();
        let i = &self.inside;
        for (path, kind, r, w) in &i.walk.entries {
            match kind.as_str() {
                "f" | "d" | "l" => {
                    let dir = kind == "d";
                    if *r {
                        out.push(Name::Known(fs_name("read", path, dir)));
                    }
                    if *w {
                        out.push(Name::Known(fs_name("write", path, dir)));
                    }
                }
                _ => out.push(Name::Known(format!("special{path}"))),
            }
        }
        for (path, _) in &i.walk.unknown {
            let p = path.trim_end_matches('/');
            out.push(Name::UnknownBelow(format!("fs/read{p}")));
            out.push(Name::UnknownBelow(format!("fs/write{p}")));
            out.push(Name::UnknownBelow(format!("special{p}")));
        }
        out.extend(i.env.keys().map(|k| Name::Known(env_name(k))));
        match (i.process.fork.as_str(), i.process.nproc.1) {
            ("ok", _) => out.push(Name::Known(DESCENDANTS.into())),
            // Observed refusal, held by a hard limit the world cannot
            // raise (raising needs CAP_SYS_RESOURCE in the initial userns).
            ("EAGAIN", 0..=1) => {}
            _ => out.push(Name::UnknownBelow(DESCENDANTS.into())),
        }
        match &i.network {
            InsideNetwork::Error { .. } => out.push(Name::UnknownBelow("net".into())),
            InsideNetwork::List { list } => {
                for (n, up) in list {
                    match up {
                        serde_json::Value::Bool(false) => {}
                        serde_json::Value::Bool(true) => {
                            out.push(Name::Known(format!("net/if/{n}")))
                        }
                        _ => out.push(Name::UnknownBelow(format!("net/if/{n}"))),
                    }
                }
            }
        }
        for (key, result) in &i.targets {
            match result.as_str() {
                "connected" => out.push(Name::Known(format!("net/connect/{key}"))),
                "ECONNREFUSED" | "ENOENT" | "ENETUNREACH" => {}
                _ => out.push(Name::UnknownBelow(format!("net/connect/{key}"))),
            }
        }
        out
    }
}

/// Construct the world `plan` describes under `root` (an empty directory
/// the runtime owns), run the inside probe in it with `interpreter`, and
/// observe the held probe from outside.
pub fn observe(
    plan: &ConstructionPlan,
    root: &Path,
    interpreter: &Path,
    targets: &BTreeMap<String, Target>,
) -> Result<Observation, WorldError> {
    observe_with(plan, root, interpreter, targets, &ProbeExtra::default())
}

/// As [`observe`], with operations performed and paths resolved by the
/// probe.
pub fn observe_with(
    plan: &ConstructionPlan,
    root: &Path,
    interpreter: &Path,
    targets: &BTreeMap<String, Target>,
    extra: &ProbeExtra,
) -> Result<Observation, WorldError> {
    observe_held(plan, root, interpreter, targets, extra, |_| ()).map(|(o, ())| o)
}

/// As [`observe_with`], and while the world is still held (after its
/// report, before it is released) run `during` with the host pid of the
/// world's first process (0 if construction failed).
pub fn observe_held<R>(
    plan: &ConstructionPlan,
    root: &Path,
    interpreter: &Path,
    targets: &BTreeMap<String, Target>,
    extra: &ProbeExtra,
    during: impl FnOnce(u32) -> R,
) -> Result<(Observation, R), WorldError> {
    let interp = interpreter
        .to_str()
        .filter(|s| plan.covers_read(s))
        .ok_or_else(|| WorldError::InterpreterOutside(interpreter.to_path_buf()))?;
    let ops = prepare(plan, root)?;
    let config = serde_json::json!({
        "declared": plan.binds.iter().map(|b| &b.path).collect::<Vec<_>>(),
        "targets": targets,
        "ops": extra.ops,
        "resolve": extra.resolve,
        "transcribe": extra.transcribe,
    });
    let config = config.to_string();
    let mut cmd = Command::new(interp);
    cmd.args(["-I", "-c", PROBE, config.as_str()])
        .env_clear()
        .envs(&plan.manifest.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: `construct` allocates nothing and only makes syscalls.
    unsafe {
        cmd.pre_exec(move || {
            construct(&ops);
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let pipes = [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()];
    let expected_fds = pipes
        .iter()
        .map(|&fd| {
            let link = fs::read_link(format!("/proc/self/fd/{fd}"))?;
            Ok(link.to_string_lossy().into_owned())
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let mut line = String::new();
    let mut reader = BufReader::new(stdout);
    reader.read_line(&mut line)?;
    let outside = if line.is_empty() {
        Outside::default()
    } else {
        observe_outside(child.id(), expected_fds)
    };
    let held = during(outside.pid);
    drop(stdin);
    let status = child.wait()?;
    let mut err = String::new();
    stderr.read_to_string(&mut err)?;
    if line.is_empty() {
        return Err(WorldError::Construction {
            status: status.code(),
            stderr: err,
        });
    }
    let inside: Inside =
        serde_json::from_str(&line).map_err(|e| WorldError::Report(format!("{e}: {err}")))?;
    Ok((
        Observation {
            raw: line.trim_end().to_string(),
            inside,
            outside,
            probe_digest: crate::verifiers::hex(&Sha256::digest(PROBE.as_bytes())),
            exit: status.code(),
        },
        held,
    ))
}

fn observe_outside(intermediate: u32, expected_fds: Vec<String>) -> Outside {
    let children = fs::read_to_string(format!("/proc/{intermediate}/task/{intermediate}/children"))
        .unwrap_or_default();
    let Some(pid) = children
        .split_whitespace()
        .next()
        .and_then(|p| p.parse().ok())
    else {
        return Outside {
            expected_fds,
            ..Outside::default()
        };
    };
    let link = |p: String| {
        fs::read_link(p)
            .ok()
            .map(|l| l.to_string_lossy().into_owned())
    };
    let namespaces = NAMESPACES
        .iter()
        .map(|k| {
            let world = link(format!("/proc/{pid}/ns/{k}"));
            let runtime = link(format!("/proc/self/ns/{k}"));
            (k.to_string(), world.zip(runtime))
        })
        .collect();
    let fds = fs::read_dir(format!("/proc/{pid}/fd"))
        .ok()
        .and_then(|dir| {
            let mut out = Vec::new();
            for entry in dir {
                let entry = entry.ok()?;
                let fd = entry.file_name().to_str()?.parse().ok()?;
                out.push((fd, link(entry.path().to_string_lossy().into_owned())?));
            }
            out.sort();
            Some(out)
        });
    Outside {
        pid,
        namespaces,
        fds,
        expected_fds,
    }
}

// ---------------------------------------------------------------------------
// Facts and the invariant

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "subject", content = "of", rename_all = "snake_case")]
pub enum CapSubject {
    /// A declared capability: present in the world, and the declared object.
    Declared(String),
    /// A closing property of the construction.
    Closing(Closing),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Closing {
    NoNewPrivs,
    NoCapabilities,
    /// Supplementary groups are exactly the plan's accepted residue.
    Groups,
    /// The world's namespace of this kind is not the runtime's.
    Namespace(String),
    /// fds are exactly 0, 1, 2, each the runtime's own pipe.
    FdTable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "value", rename_all = "snake_case")]
pub enum CapValue {
    Granted,
    NotGranted { observed: String },
    Holds,
    Fails { observed: String },
}

pub type CapFact = Fact<CapSubject, CapValue>;
pub type CapEvidence = EvidenceBase<CapSubject, CapValue>;

fn fact(evidence: &mut CapEvidence, subject: CapSubject, value: CapValue, provenance: &Provenance) {
    evidence.add_verified(
        Verified::attest(Fact { subject, value }),
        provenance.clone(),
    );
}

fn holds(ok: bool, observed: impl FnOnce() -> String) -> CapValue {
    if ok {
        CapValue::Holds
    } else {
        CapValue::Fails {
            observed: observed(),
        }
    }
}

/// Verified facts from one observation. What an observer could not
/// establish yields no fact.
pub fn evidence(plan: &ConstructionPlan, observation: &Observation) -> CapEvidence {
    let mut ev = CapEvidence::new();
    let i = &observation.inside;
    let inside = Provenance {
        observer: "world-probe".into(),
        basis: format!(
            "probe sha256 {} in plan {}",
            observation.probe_digest,
            plan.digest()
        ),
    };
    let outside = Provenance {
        observer: "runtime-procfs".into(),
        basis: format!("/proc/{}", observation.outside.pid),
    };

    for bind in &plan.binds {
        let ops: &[&str] = if bind.writable {
            &["read", "write"]
        } else {
            &["read"]
        };
        let value = |op: &str| match i.declared.get(&bind.path)? {
            InsideDeclared::Error { error } if error == "ENOENT" => Some(CapValue::NotGranted {
                observed: "ENOENT".into(),
            }),
            InsideDeclared::Error { .. } => None,
            InsideDeclared::Seen { dev, ino, r, w } => {
                let same = (*dev, *ino) == (bind.dev, bind.ino);
                let access = if op == "read" { *r } else { *w };
                Some(if same && access {
                    CapValue::Granted
                } else {
                    CapValue::NotGranted {
                        observed: format!("dev {dev} ino {ino} r {r} w {w}"),
                    }
                })
            }
        };
        for op in ops {
            if let Some(v) = value(op) {
                fact(
                    &mut ev,
                    CapSubject::Declared(fs_name(op, &bind.path, false)),
                    v,
                    &inside,
                );
            }
        }
    }
    for (name, declared) in &plan.manifest.env {
        let value = match i.env.get(name) {
            Some(v) if v == declared => CapValue::Granted,
            Some(_) => CapValue::NotGranted {
                observed: "different value".into(),
            },
            None => CapValue::NotGranted {
                observed: "unset".into(),
            },
        };
        fact(
            &mut ev,
            CapSubject::Declared(env_name(name)),
            value,
            &inside,
        );
    }
    match i.process.fork.as_str() {
        "ok" => fact(
            &mut ev,
            CapSubject::Declared(DESCENDANTS.into()),
            CapValue::Granted,
            &inside,
        ),
        "EAGAIN" => fact(
            &mut ev,
            CapSubject::Declared(DESCENDANTS.into()),
            CapValue::NotGranted {
                observed: "fork: EAGAIN".into(),
            },
            &inside,
        ),
        _ => {}
    }

    if let Some(nnp) = i.identity.no_new_privs {
        let v = holds(nnp == 1, || format!("NoNewPrivs = {nnp}"));
        fact(
            &mut ev,
            CapSubject::Closing(Closing::NoNewPrivs),
            v,
            &inside,
        );
    }
    if let Some(c) = &i.identity.caps {
        let v = holds((c.eff | c.prm | c.inh | c.amb) == 0, || format!("{c:?}"));
        fact(
            &mut ev,
            CapSubject::Closing(Closing::NoCapabilities),
            v,
            &inside,
        );
    }
    let accepted = plan.residue.iter().find_map(|r| match r {
        Residue::SupplementaryGroups { inside, .. } => Some(inside),
    });
    let mut groups = i.identity.groups.clone();
    groups.sort_unstable();
    let v = holds(accepted == Some(&groups), || format!("groups {groups:?}"));
    fact(&mut ev, CapSubject::Closing(Closing::Groups), v, &inside);

    for (kind, ids) in &observation.outside.namespaces {
        if let Some((world, runtime)) = ids {
            let v = holds(world != runtime, || format!("shares {runtime}"));
            fact(
                &mut ev,
                CapSubject::Closing(Closing::Namespace(kind.clone())),
                v,
                &outside,
            );
        }
    }
    if let Some(fds) = &observation.outside.fds {
        let expected: Vec<(u32, String)> = (0u32..)
            .zip(observation.outside.expected_fds.iter().cloned())
            .collect();
        let v = holds(*fds == expected, || format!("{fds:?}"));
        fact(&mut ev, CapSubject::Closing(Closing::FdTable), v, &outside);
    }
    ev
}

/// The three invariants, instantiated for this plan and observation.
pub fn obligations(
    plan: &ConstructionPlan,
    observation: &Observation,
) -> Vec<Obligation<CapSubject, CapValue>> {
    let post = |invariant: &str, requirement: Requirement<CapSubject, CapValue>| Obligation {
        invariant: invariant.to_string(),
        phase: Phase::Post,
        requirement,
    };
    let hard = |subject: CapSubject, value: CapValue| Requirement::Fact {
        subject,
        value,
        strength: Strength::Hard,
    };
    let mut out = vec![post(
        "C1.observed_within_declared",
        Requirement::Within {
            names: observation.names(),
            scopes: plan.scopes(),
        },
    )];
    let mut declared: Vec<String> = Vec::new();
    for bind in &plan.binds {
        declared.push(fs_name("read", &bind.path, false));
        if bind.writable {
            declared.push(fs_name("write", &bind.path, false));
        }
    }
    declared.extend(plan.manifest.env.keys().map(|k| env_name(k)));
    if plan.manifest.descendants_allowed {
        declared.push(DESCENDANTS.into());
    }
    out.extend(declared.into_iter().map(|name| {
        post(
            "C2.declared_visible",
            hard(CapSubject::Declared(name), CapValue::Granted),
        )
    }));
    let mut closing = vec![
        Closing::NoNewPrivs,
        Closing::NoCapabilities,
        Closing::Groups,
        Closing::FdTable,
    ];
    closing.extend(NAMESPACES.iter().map(|k| Closing::Namespace(k.to_string())));
    out.extend(
        closing
            .into_iter()
            .map(|c| post("C3.closed", hard(CapSubject::Closing(c), CapValue::Holds))),
    );
    out
}

/// "Observed capabilities are a subset of declared capabilities", plus
/// the declared ones being present and the world being closed.
pub fn check(plan: &ConstructionPlan, observation: &Observation) -> Decision<CapSubject, CapValue> {
    kernel::evaluate(
        Phase::Post,
        &obligations(plan, observation),
        &evidence(plan, observation),
    )
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Namespaces { kinds } => write!(f, "unshare {}", kinds.join(",")),
            Step::DenySetgroups => write!(f, "setgroups deny"),
            Step::MapUid { uid } => write!(f, "uid_map {uid} {uid} 1"),
            Step::MapGid { gid } => write!(f, "gid_map {gid} {gid} 1"),
            Step::EnterPidNamespace => write!(f, "fork pid 1"),
            Step::PrivatePropagation => write!(f, "mount --make-rprivate /"),
            Step::EmptyRoot => write!(f, "tmpfs <root>"),
            Step::Bind(b) => write!(
                f,
                "bind {} {}",
                if b.writable { "rw" } else { "ro" },
                b.path
            ),
            Step::Snapshot {
                target,
                root,
                entries,
            } => write!(f, "tmpfs {target} = tree {root} ({entries} entries), ro"),
            Step::SealRoot => write!(f, "remount <root> ro"),
            Step::PivotRoot => write!(f, "pivot_root <root>; detach host root"),
            Step::NewSession => write!(f, "setsid"),
            Step::NoNewPrivs => write!(f, "PR_SET_NO_NEW_PRIVS"),
            Step::ClearCapabilities => write!(f, "clear inheritable + ambient caps"),
            Step::LimitDescendants { allowed: true } => write!(f, "descendants allowed"),
            Step::LimitDescendants { allowed: false } => write!(f, "RLIMIT_NPROC 1/1"),
            Step::CloseInheritedFds => write!(f, "close_range(3, ~0, CLOEXEC)"),
            Step::Environment { names } => write!(f, "env exactly {names:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{Status, Verdict};

    fn identity() -> Identity {
        Identity {
            uid: 1000,
            gid: 100,
            groups: vec![1, 100, 131],
            overflow_gid: 65534,
        }
    }

    fn manifest(dir: &Path) -> CapabilityManifest {
        let dir = fs::canonicalize(dir).unwrap();
        fs::create_dir_all(dir.join("ro")).unwrap();
        fs::create_dir_all(dir.join("rw")).unwrap();
        CapabilityManifest {
            read_paths: vec![dir.join("ro")],
            write_paths: vec![dir.join("rw")],
            descendants_allowed: false,
            env: BTreeMap::from([("LC_ALL".into(), "C".into())]),
            aliases: vec![],
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v9r-cap-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A probe report for `plan` in which everything is as declared.
    fn clean(plan: &ConstructionPlan) -> serde_json::Value {
        let mut entries = vec![serde_json::json!(["/", "d", true, false])];
        for a in &plan.skeleton {
            if a != "/" {
                entries.push(serde_json::json!([a, "d", true, false]));
            }
        }
        let mut declared = serde_json::Map::new();
        for b in &plan.binds {
            entries.push(serde_json::json!([b.path, "d", true, b.writable]));
            entries.push(serde_json::json!([
                format!("{}/f", b.path),
                "f",
                true,
                b.writable
            ]));
            declared.insert(
                b.path.clone(),
                serde_json::json!({"dev": b.dev, "ino": b.ino, "r": true, "w": b.writable}),
            );
        }
        serde_json::json!({
            "identity": {"pid": 1, "uid": 1000, "gid": 100, "groups": [100, 65534, 65534],
                         "no_new_privs": 1, "caps": {"eff": 0, "prm": 0, "inh": 0, "amb": 0}},
            "env": plan.manifest.env,
            "walk": {"entries": entries, "unknown": []},
            "declared": declared,
            "process": {"nproc": [1, 1], "fork": "EAGAIN"},
            "network": {"list": [["lo", false]]},
            "targets": {"abstract": "ECONNREFUSED"},
        })
    }

    fn observation(report: serde_json::Value) -> Observation {
        let raw = report.to_string();
        let pipes: Vec<String> = (0..3).map(|i| format!("pipe:[{i}]")).collect();
        Observation {
            inside: serde_json::from_str(&raw).unwrap(),
            raw,
            outside: Outside {
                pid: 42,
                namespaces: NAMESPACES
                    .iter()
                    .map(|k| {
                        (
                            k.to_string(),
                            Some((format!("{k}:[2]"), format!("{k}:[1]"))),
                        )
                    })
                    .collect(),
                fds: Some((0..).zip(pipes.iter().cloned()).collect()),
                expected_fds: pipes,
            },
            probe_digest: "test".into(),
            exit: Some(0),
        }
    }

    fn statuses(d: &Decision<CapSubject, CapValue>, id: &str) -> Vec<Status> {
        d.findings
            .iter()
            .filter(|f| f.obligation.invariant == id)
            .map(|f| f.status.clone())
            .collect()
    }

    #[test]
    fn plan_rejects_ambiguous_paths() {
        let d = tmp("reject");
        let m = |p: PathBuf| CapabilityManifest {
            read_paths: vec![p],
            ..CapabilityManifest::default()
        };
        assert!(matches!(
            plan_for(&m("rel".into()), identity()),
            Err(PlanError::NotAbsolute(_))
        ));
        assert_eq!(plan_for(&m("/".into()), identity()), Err(PlanError::Root));
        assert!(matches!(
            plan_for(&m(d.join("nope")), identity()),
            Err(PlanError::Missing(..))
        ));
        let real = fs::canonicalize(&d).unwrap();
        fs::create_dir(real.join("a")).unwrap();
        std::os::unix::fs::symlink(real.join("a"), real.join("link")).unwrap();
        assert!(matches!(
            plan_for(&m(real.join("link")), identity()),
            Err(PlanError::NotCanonical(_))
        ));
        assert!(matches!(
            plan_for(&m(real.join("a/../a")), identity()),
            Err(PlanError::NotCanonical(_))
        ));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn plan_mounts_parents_first_and_write_wins() {
        let d = fs::canonicalize(tmp("order")).unwrap();
        fs::create_dir_all(d.join("a/b")).unwrap();
        let m = CapabilityManifest {
            read_paths: vec![d.join("a/b"), d.join("a"), d.join("a")],
            write_paths: vec![d.join("a/b")],
            ..CapabilityManifest::default()
        };
        let p = plan_for(&m, identity()).unwrap();
        let binds: Vec<(&str, bool)> = p
            .binds()
            .iter()
            .map(|b| (b.path.as_str(), b.writable))
            .collect();
        let a = d.join("a");
        let ab = d.join("a/b");
        assert_eq!(
            binds,
            [(a.to_str().unwrap(), false), (ab.to_str().unwrap(), true)]
        );
        // The declared parent covers the nested one; only its ancestors
        // are skeleton.
        assert!(!p.skeleton.iter().any(|s| s == a.to_str().unwrap()));
        assert!(p.skeleton.iter().any(|s| s == d.to_str().unwrap()));
        assert!(p.skeleton.iter().any(|s| s == "/"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn residue_is_what_an_unmapped_userns_shows() {
        assert_eq!(identity().groups_inside(), vec![100, 65534, 65534]);
    }

    #[test]
    fn skeleton_scopes_are_exact_declared_scopes_are_prefixes() {
        assert!(!kernel::name_within("fs/read/a/b/.", "fs/read/a/."));
        assert!(kernel::name_within("fs/read/a/.", "fs/read/a/."));
        assert!(kernel::name_within("fs/read/a/b/.", "fs/read/a"));
        assert!(kernel::name_within("fs/read/a/b", "fs/read/a"));
        assert!(!kernel::name_within("fs/read/ab", "fs/read/a"));
        assert_eq!(fs_name("read", "/", true), "fs/read/.");
        assert_eq!(env_name("A/B%"), "env/A%2FB%25");
    }

    #[test]
    fn clean_world_is_allowed() {
        let d = tmp("clean");
        let p = plan_for(&manifest(&d), identity()).unwrap();
        let decision = check(&p, &observation(clean(&p)));
        assert_eq!(decision.verdict, Verdict::Allow, "{decision}");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn undeclared_capabilities_are_violations() {
        let d = tmp("undeclared");
        let p = plan_for(&manifest(&d), identity()).unwrap();
        let ro = p.binds()[0].path.clone();
        let cases: Vec<(&str, Box<dyn Fn(&mut serde_json::Value)>)> = vec![
            (
                "write in ro path",
                Box::new(move |r: &mut serde_json::Value| {
                    r["walk"]["entries"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!([format!("{ro}/x"), "f", true, true]))
                }),
            ),
            (
                "writable root",
                Box::new(|r: &mut serde_json::Value| r["walk"]["entries"][0][3] = true.into()),
            ),
            (
                "file outside",
                Box::new(|r: &mut serde_json::Value| {
                    r["walk"]["entries"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!(["/etc/passwd", "f", true, false]))
                }),
            ),
            (
                "socket in rw path",
                Box::new(|r: &mut serde_json::Value| {
                    let rw = r["walk"]["entries"].as_array().unwrap().last().unwrap()[0].clone();
                    r["walk"]["entries"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!([
                            format!("{}.sock", rw.as_str().unwrap()),
                            "s",
                            true,
                            true
                        ]))
                }),
            ),
            (
                "extra env",
                Box::new(|r: &mut serde_json::Value| r["env"]["HOME"] = "/home/x".into()),
            ),
            (
                "fork",
                Box::new(|r: &mut serde_json::Value| r["process"]["fork"] = "ok".into()),
            ),
            (
                "interface up",
                Box::new(|r: &mut serde_json::Value| r["network"]["list"][0][1] = true.into()),
            ),
            (
                "canary",
                Box::new(|r: &mut serde_json::Value| r["targets"]["abstract"] = "connected".into()),
            ),
        ];
        for (label, mutate) in cases {
            let mut report = clean(&p);
            mutate(&mut report);
            let decision = check(&p, &observation(report));
            assert_eq!(decision.verdict, Verdict::Deny, "{label}: {decision}");
            assert!(
                matches!(
                    statuses(&decision, "C1.observed_within_declared")[0],
                    Status::Violated(_)
                ),
                "{label}"
            );
        }
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn unknown_is_never_absence() {
        let d = tmp("unknown");
        let p = plan_for(&manifest(&d), identity()).unwrap();
        let cases: Vec<(&str, Box<dyn Fn(&mut serde_json::Value)>)> = vec![
            (
                "unreadable dir outside scope",
                Box::new(|r: &mut serde_json::Value| {
                    r["walk"]["unknown"] = serde_json::json!([["/hidden", "EACCES"]])
                }),
            ),
            (
                "walk limit at root",
                Box::new(|r: &mut serde_json::Value| {
                    r["walk"]["unknown"] = serde_json::json!([["/", "WALK_LIMIT"]])
                }),
            ),
            (
                "fork failed otherwise",
                Box::new(|r: &mut serde_json::Value| r["process"]["fork"] = "ENOMEM".into()),
            ),
            (
                "EAGAIN under a raisable limit",
                Box::new(|r: &mut serde_json::Value| {
                    r["process"]["nproc"] = serde_json::json!([1, -1])
                }),
            ),
            (
                "interfaces unreadable",
                Box::new(|r: &mut serde_json::Value| {
                    r["network"] = serde_json::json!({"error": "EPERM"})
                }),
            ),
            (
                "canary timed out",
                Box::new(|r: &mut serde_json::Value| {
                    r["targets"]["abstract"] = "TimeoutError".into()
                }),
            ),
        ];
        for (label, mutate) in cases {
            let mut report = clean(&p);
            mutate(&mut report);
            let decision = check(&p, &observation(report));
            assert_eq!(decision.verdict, Verdict::Blocked, "{label}: {decision}");
        }
        // Even inside a declared path: the unread subtree may hold a
        // socket or device, and those are never declared.
        let mut report = clean(&p);
        let rw = p.binds()[1].path.clone();
        report["walk"]["unknown"] = serde_json::json!([[format!("{rw}/private"), "EACCES"]]);
        let decision = check(&p, &observation(report));
        assert_eq!(decision.verdict, Verdict::Blocked);
        match &statuses(&decision, "C1.observed_within_declared")[0] {
            Status::Undetermined(why) => assert!(why.contains("special"), "{why}"),
            other => panic!("{other:?}"),
        }
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn declared_capabilities_must_be_the_declared_objects() {
        let d = tmp("declared");
        let p = plan_for(&manifest(&d), identity()).unwrap();
        let rw = p.binds()[1].path.clone();

        let mut report = clean(&p);
        report["declared"][&rw]["ino"] = 1.into();
        let decision = check(&p, &observation(report));
        assert_eq!(
            decision.verdict,
            Verdict::Deny,
            "substituted object: {decision}"
        );

        let mut report = clean(&p);
        report["declared"][&rw] = serde_json::json!({"error": "EACCES"});
        let decision = check(&p, &observation(report));
        assert_eq!(decision.verdict, Verdict::Blocked, "unseen: {decision}");

        let mut report = clean(&p);
        report["env"]["LC_ALL"] = "POSIX".into();
        assert_eq!(check(&p, &observation(report)).verdict, Verdict::Deny);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn inherited_state_breaks_closure() {
        let d = tmp("closing");
        let p = plan_for(&manifest(&d), identity()).unwrap();
        fn verdict(
            p: &ConstructionPlan,
            report: serde_json::Value,
            outside: impl FnOnce(&mut Outside),
        ) -> Verdict {
            let mut o = observation(report);
            outside(&mut o.outside);
            check(p, &o).verdict
        }
        let same = |_: &mut Outside| {};
        let mut r = clean(&p);
        r["identity"]["no_new_privs"] = 0.into();
        assert_eq!(verdict(&p, r, same), Verdict::Deny);
        let mut r = clean(&p);
        r["identity"]["caps"]["inh"] = (1u64 << 35).into();
        assert_eq!(verdict(&p, r, same), Verdict::Deny);
        let mut r = clean(&p);
        r["identity"]["groups"] = serde_json::json!([100, 65534, 65534, 65534]);
        assert_eq!(verdict(&p, r, same), Verdict::Deny);
        let tty = |o: &mut Outside| o.fds.as_mut().unwrap().push((7, "/dev/tty".into()));
        assert_eq!(verdict(&p, clean(&p), tty), Verdict::Deny);
        let shared_net = |o: &mut Outside| {
            let net = o.namespaces.get_mut("net").unwrap().as_mut().unwrap();
            net.0 = net.1.clone();
        };
        assert_eq!(verdict(&p, clean(&p), shared_net), Verdict::Deny);
        // Unreadable from outside: undetermined, not closed.
        let unread = |o: &mut Outside| o.fds = None;
        assert_eq!(verdict(&p, clean(&p), unread), Verdict::Blocked);
        let mut r = clean(&p);
        r["identity"]["caps"] = serde_json::Value::Null;
        assert_eq!(verdict(&p, r, same), Verdict::Blocked);
        fs::remove_dir_all(d).unwrap();
    }
}
