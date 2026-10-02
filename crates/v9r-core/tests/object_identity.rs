//! X1: can capability grants bind to real object identity across worlds?
//!
//! Every case builds real worlds (Capability Manifest v0 construction):
//! world A, the grantor, resolves the granted path in its own view; the
//! runtime issues a signed grant carrying that identity; world B, the
//! grantee, resolves the object it was actually given. The same B
//! observation is then judged under three identity schemes:
//!
//! * `Name`: no identity, the name is trusted (X2's behaviour);
//! * `DevIno`: `(st_dev, st_ino)`;
//! * `Handle`: `st_dev` + `name_to_handle_at` handle.
//!
//! Ground truth is the experiment's own construction: which host object
//! it bound where.
//!
//! Needs unprivileged user namespaces, `python3` and `nix-store`.
//! `mount_boundary` (ignored) also reads metadata under
//! `/run/user/<uid>`.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use v9r_core::authority::{self, Authority, Request};
use v9r_core::capability::{self, Alias, CapabilityManifest, Observation, ProbeExtra, Resolved};
use v9r_core::delegation::{cap_name, DEvidence, Grant, Op, Parent, Use};
use v9r_core::kernel::{self, Obligation, Phase, Requirement, Verdict};
use v9r_core::object_identity::{self, host_resolve, object_id, IdScheme};

const SCHEMES: [IdScheme; 3] = [IdScheme::Name, IdScheme::DevIno, IdScheme::Handle];

// ------------------------------------------------------------ worlds

/// The real interpreter (`/proc/self/exe` of `python3`) and its closure.
fn toolchain() -> Result<(PathBuf, Vec<PathBuf>), String> {
    let out = Command::new("python3")
        .args([
            "-I",
            "-c",
            "import os; print(os.readlink('/proc/self/exe'))",
        ])
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

struct Lab {
    base: PathBuf,
    python: PathBuf,
    closure: Vec<PathBuf>,
    worlds: usize,
}

impl Lab {
    fn new() -> Option<Self> {
        let (python, closure) = match toolchain() {
            Ok(t) => t,
            Err(e) => {
                println!("SKIPPED (environment, not evidence): {e}");
                return None;
            }
        };
        let base = std::env::temp_dir().join(format!("v9r-x1-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        Some(Lab {
            base: fs::canonicalize(base).unwrap(),
            python,
            closure,
            worlds: 0,
        })
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn s(&self, rel: &str) -> String {
        self.p(rel).to_str().unwrap().to_string()
    }

    fn file(&self, rel: &str, data: &str) -> String {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, data).unwrap();
        self.s(rel)
    }

    /// Build a world with these read paths and aliases, resolve `resolve`
    /// inside it, and return the observation.
    fn world(&mut self, read: &[&str], aliases: &[(&str, &str)], resolve: &[&str]) -> Observation {
        self.worlds += 1;
        let mut read_paths = self.closure.clone();
        read_paths.extend(read.iter().map(PathBuf::from));
        let manifest = CapabilityManifest {
            read_paths,
            write_paths: vec![],
            descendants_allowed: false,
            env: BTreeMap::from([("LC_ALL".into(), "C".into())]),
            aliases: aliases
                .iter()
                .map(|(s, t)| Alias {
                    source: PathBuf::from(s),
                    target: PathBuf::from(t),
                    writable: false,
                })
                .collect(),
        };
        let plan = capability::plan(&manifest).expect("plan");
        let root = self.p(&format!("root-{}", self.worlds));
        fs::create_dir_all(&root).unwrap();
        let extra = ProbeExtra {
            ops: BTreeMap::new(),
            resolve: resolve.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        capability::observe_with(&plan, &root, &self.python, &BTreeMap::new(), &extra)
            .expect("world")
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

// ------------------------------------------------------------ judging

fn reads(world: &str, paths: &[&str]) -> Use {
    Use {
        world: world.to_string(),
        names: paths.iter().map(|p| cap_name(Op::Read, p)).collect(),
    }
}

/// Issue root → A → B for `grant_path` with the identity A saw (under
/// `scheme`), then decide B's use of it against what B resolved at
/// `b_path`.
fn decide(
    scheme: IdScheme,
    granted: &Resolved,
    grant_path: &str,
    b: &Observation,
    b_path: &str,
) -> Verdict {
    let mut auth = Authority::new();
    let a = auth.enroll("A");
    let bk = auth.enroll("B");
    let identities: BTreeMap<String, String> = object_id(granted, scheme)
        .map(|id| BTreeMap::from([(grant_path.to_string(), id)]))
        .unwrap_or_default();
    let g = |parent, grantor: &str, grantee: &str, depth| Grant {
        parent,
        grantor: grantor.into(),
        grantee: grantee.into(),
        objects: vec![grant_path.to_string()],
        operations: vec![Op::Read],
        constraints: BTreeMap::new(),
        not_after: 100,
        depth,
        identities: identities.clone(),
    };
    let root = auth.root_grant(g(Parent::Root, "", &a, 1));
    let child = auth
        .delegate("A", g(Parent::Grant(root), &a, &bk, 0))
        .unwrap();
    let copy = auth.publish();
    let claimed = authority::chain(&copy, &child);
    let context =
        object_identity::evidence(b, &[(grant_path.to_string(), b_path.to_string())], scheme);
    let use_ = reads(&bk, &[grant_path]);
    let request = Request {
        copies: std::slice::from_ref(&copy),
        claimed: &claimed,
        leaf: &child,
        use_: &use_,
        context: &context,
    };
    authority::authorize(&auth, &auth.anchor(), &auth.witness(), &request).verdict
}

fn short(r: &Resolved) -> String {
    match r {
        Resolved::Seen {
            dev,
            ino,
            generation,
            handle,
            mount,
            ..
        } => format!(
            "dev {dev} ino {ino} gen {} mnt {} fh {}",
            generation
                .as_ref()
                .map(|g| g.to_string())
                .unwrap_or_else(|e| e.clone()),
            mount
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|e| e.clone()),
            handle
                .as_ref()
                .map(String::as_str)
                .unwrap_or_else(|e| e.as_str()),
        ),
        Resolved::Error { error } => error.clone(),
    }
}

/// One row: the case, ground truth, and each scheme's verdict.
fn row(
    name: &str,
    same_object: bool,
    a: &Resolved,
    grant_path: &str,
    b: &Observation,
    b_path: &str,
) -> BTreeMap<IdScheme, Verdict> {
    let b_res = b.resolved(b_path).expect("B resolved").clone();
    println!("\n{name}  (same object: {same_object})");
    println!("  A sees {grant_path}: {}", short(a));
    println!("  B sees {b_path}: {}", short(&b_res));
    let mut out = BTreeMap::new();
    for scheme in SCHEMES {
        let v = decide(scheme, a, grant_path, b, b_path);
        let correct = (v == Verdict::Allow) == same_object;
        println!(
            "  {:<7} {:?}{}",
            format!("{scheme:?}"),
            v,
            if correct { "" } else { "   <-- WRONG" }
        );
        out.insert(scheme, v);
    }
    out
}

fn resolved<'a>(obs: &'a Observation, path: &str) -> &'a Resolved {
    obs.resolved(path).expect("resolved")
}

// ------------------------------------------------------------ cases

#[test]
fn object_identity_across_worlds() {
    let Some(mut lab) = Lab::new() else { return };
    let start = Instant::now();
    use IdScheme::*;
    use Verdict::*;

    // 1. Same path, different object: /w/o is o1 in A, o2 in B.
    let o1 = lab.file("c1/o1", "one");
    let o2 = lab.file("c1/o2", "two");
    let a = lab.world(&[], &[(&o1, "/w/o")], &["/w/o"]);
    let b = lab.world(&[], &[(&o2, "/w/o")], &["/w/o"]);
    let v = row(
        "1 same path, different object",
        false,
        resolved(&a, "/w/o"),
        "/w/o",
        &b,
        "/w/o",
    );
    assert_eq!((v[&Name], v[&DevIno], v[&Handle]), (Allow, Deny, Deny));

    // 2. Same object, different path: renamed on the host after the
    //    grant, bound at another path in B.
    let o3 = lab.file("c2/o3", "three");
    let a = lab.world(&[&o3], &[], &[&o3]);
    let renamed = lab.s("c2/renamed");
    fs::rename(&o3, &renamed).unwrap();
    let b = lab.world(&[], &[(&renamed, "/x/renamed")], &["/x/renamed"]);
    let v = row(
        "2 same object, renamed, other path",
        true,
        resolved(&a, &o3),
        &o3,
        &b,
        "/x/renamed",
    );
    assert_eq!((v[&Name], v[&DevIno], v[&Handle]), (Allow, Allow, Allow));
    // Realizing the grant by its name is no longer possible.
    let by_name = capability::plan(&CapabilityManifest {
        read_paths: vec![PathBuf::from(&o3)],
        ..Default::default()
    });
    println!("  realize by granted name: {by_name:?}");
    assert!(matches!(by_name, Err(capability::PlanError::Missing(..))));

    // 3a. Hard link: same inode under another name.
    let o4 = lab.file("c3/o4", "four");
    let link = lab.s("c3/links/o4");
    fs::create_dir_all(lab.p("c3/links")).unwrap();
    fs::hard_link(&o4, &link).unwrap();
    let a = lab.world(&[&o4], &[], &[&o4]);
    let b = lab.world(&[], &[(&link, "/y/o4")], &["/y/o4"]);
    let v = row(
        "3a hard link: same inode, other name",
        true,
        resolved(&a, &o4),
        &o4,
        &b,
        "/y/o4",
    );
    assert_eq!((v[&Name], v[&DevIno], v[&Handle]), (Allow, Allow, Allow));

    // 3b. Directory grant; afterwards a hard link to A's private file,
    //     and a new plain file, appear inside it.
    let shared = lab.s("c3/shared");
    lab.file("c3/shared/a.txt", "shared");
    let secret = lab.file("c3/apriv/secret", "secret");
    let apriv = lab.s("c3/apriv");
    let a = lab.world(&[&shared, &apriv], &[], &[&shared, &secret]);
    let at_grant: Vec<String> = a
        .objects_within(&shared)
        .iter()
        .map(|(_, d, i)| format!("obj/{d}/{i}"))
        .collect();
    let private: Vec<String> = a
        .objects_within(&apriv)
        .iter()
        .map(|(_, d, i)| format!("obj/{d}/{i}"))
        .collect();
    fs::hard_link(&secret, lab.p("c3/shared/s")).unwrap();
    lab.file("c3/shared/new.txt", "new");
    let b = lab.world(&[&shared], &[], &[&shared, &lab.s("c3/shared/s")]);
    let v = row(
        "3b directory grant (the directory itself)",
        true,
        resolved(&a, &shared),
        &shared,
        &b,
        &shared,
    );
    assert_eq!((v[&Name], v[&DevIno], v[&Handle]), (Allow, Allow, Allow));
    let in_b: Vec<(String, String)> = b
        .objects_within(&shared)
        .iter()
        .map(|(p, d, i)| (p.to_string(), format!("obj/{d}/{i}")))
        .collect();
    let contained = |scopes: &[String]| {
        kernel::evaluate(
            Phase::Pre,
            &[Obligation {
                invariant: "objects_within_grant".into(),
                phase: Phase::Pre,
                requirement: Requirement::Within {
                    names: in_b
                        .iter()
                        .map(|(_, o)| kernel::Name::Known(o.clone()))
                        .collect(),
                    scopes: scopes.to_vec(),
                },
            }],
            &DEvidence::new(),
        )
    };
    let since_grant = contained(&at_grant);
    let leaked: Vec<&str> = in_b
        .iter()
        .filter(|(_, o)| private.contains(o))
        .map(|(p, _)| p.as_str())
        .collect();
    println!("  objects in B's view not in the grant-time set: {since_grant}");
    println!("  objects in B's view that are A's private objects: {leaked:?}");
    assert_eq!(since_grant.verdict, Deny);
    assert_eq!(leaked, vec![lab.s("c3/shared/s")]);
    // The secret's identity, seen through the shared directory, is A's.
    assert_eq!(
        object_id(resolved(&b, &lab.s("c3/shared/s")), Handle),
        object_id(resolved(&a, &secret), Handle)
    );

    // 4b. Mount boundary: one world path, objects on two filesystems
    //     (ext4 temp dir in A, tmpfs /dev/shm in B).
    let shm = PathBuf::from("/dev/shm").join(format!("v9r-x1-{}", uuid::Uuid::new_v4()));
    if fs::create_dir(&shm).is_ok() {
        let other = shm.join("o");
        fs::write(&other, "tmpfs").unwrap();
        let other = other.to_str().unwrap().to_string();
        let o6 = lab.file("c4/o", "ext4");
        let a = lab.world(&[], &[(&o6, "/m/o")], &["/m/o"]);
        let b = lab.world(&[], &[(&other, "/m/o")], &["/m/o"]);
        let v = row(
            "4b one world path, two filesystems",
            false,
            resolved(&a, "/m/o"),
            "/m/o",
            &b,
            "/m/o",
        );
        assert_eq!((v[&Name], v[&DevIno], v[&Handle]), (Allow, Deny, Deny));
        let _ = fs::remove_dir_all(&shm);
    }

    // 5. Deleted and recreated at the same path. Recreate until the
    //    filesystem reuses the inode number, if it does.
    let o5 = lab.file("c5/o5", "old");
    let a = lab.world(&[&o5], &[], &[&o5]);
    let granted_ino = fs::metadata(&o5).unwrap().ino();
    let mut tries = 0;
    loop {
        tries += 1;
        fs::remove_file(&o5).unwrap();
        fs::write(&o5, "new").unwrap();
        if fs::metadata(&o5).unwrap().ino() == granted_ino || tries == 200 {
            break;
        }
    }
    let reused = fs::metadata(&o5).unwrap().ino() == granted_ino;
    println!("\n5: inode number reused: {reused} (after {tries} recreations)");
    let b = lab.world(&[&o5], &[], &[&o5]);
    let v = row(
        "5 deleted and recreated, same path",
        false,
        resolved(&a, &o5),
        &o5,
        &b,
        &o5,
    );
    assert_eq!(v[&Name], Allow);
    assert_eq!(v[&Handle], Deny);
    assert_eq!(v[&DevIno], if reused { Allow } else { Deny });
    // The same grant, had the runtime taken the identity from the host
    // at issue time instead of from A's observation: it names the new
    // object, which A never held, and every scheme allows it.
    let host_now = host_resolve(&o5);
    for scheme in [DevIno, Handle] {
        let v = decide(scheme, &host_now, &o5, &b, &o5);
        println!("  identity from the host at issue time, {scheme:?}: {v:?}   <-- B gets an object A never held");
        assert_eq!(v, Allow);
    }

    // Stability: the same object seen from A, B and the host.
    let h = host_resolve(&renamed);
    let z = lab_b2(&mut lab, &renamed);
    println!("\nstability (case 2's object): host {}", short(&h));
    let (
        Resolved::Seen {
            dev: hd,
            ino: hi,
            handle: hh,
            mount: hm,
            ..
        },
        Resolved::Seen {
            dev: bd,
            ino: bi,
            handle: bh,
            mount: bm,
            ..
        },
    ) = (&h, resolved(&z, "/z"))
    else {
        panic!("unresolved")
    };
    assert_eq!((hd, hi, hh), (bd, bi, bh));
    println!("  mount id: host {hm:?}, world {bm:?}");
    assert_ne!(hm, bm, "mount ids are per namespace");

    println!(
        "\n{} worlds in {:?}; open_by_handle_at unprivileged: {:?}",
        lab.worlds,
        start.elapsed(),
        object_identity::open_by_handle(&renamed)
    );
}

/// One more world binding `source` at `/z`, for the stability check.
fn lab_b2(lab: &mut Lab, source: &str) -> Observation {
    lab.world(&[], &[(source, "/z")], &["/z"])
}

/// Inode reuse and generations on this host's filesystems, without
/// worlds: how often does delete-and-recreate give the same identity?
#[test]
fn identity_reuse_by_filesystem() {
    for dir in [std::env::temp_dir(), PathBuf::from("/dev/shm")] {
        let d = dir.join(format!("v9r-x1-reuse-{}", uuid::Uuid::new_v4()));
        if fs::create_dir(&d).is_err() {
            println!("{dir:?}: not writable, skipped");
            continue;
        }
        let f = d.join("f");
        let f_s = f.to_str().unwrap().to_string();
        let (mut same_ino, mut same_handle, mut n) = (0, 0, 0);
        let mut gen_err = None;
        for _ in 0..500 {
            fs::write(&f, "x").unwrap();
            let before = host_resolve(&f_s);
            fs::remove_file(&f).unwrap();
            fs::write(&f, "y").unwrap();
            let after = host_resolve(&f_s);
            if let (
                Resolved::Seen {
                    ino: i1,
                    handle: h1,
                    generation: g1,
                    ..
                },
                Resolved::Seen {
                    ino: i2,
                    handle: h2,
                    ..
                },
            ) = (&before, &after)
            {
                n += 1;
                same_ino += (i1 == i2) as u32;
                same_handle += (h1 == h2 && h1.is_ok()) as u32;
                if let Err(e) = g1 {
                    gen_err = Some(e.clone());
                }
            }
            fs::remove_file(&f).unwrap();
        }
        let fstype = Command::new("stat")
            .args(["-f", "-c", "%T"])
            .arg(&d)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        println!(
            "{dir:?} ({fstype}): of {n} delete+recreate pairs, same (dev, ino) {same_ino}, same handle {same_handle}; generation ioctl: {}",
            gen_err.unwrap_or_else(|| "ok".into())
        );
        let _ = fs::remove_dir_all(&d);
        assert_eq!(
            same_handle, 0,
            "a recreated object must never keep its handle"
        );
    }
}

/// 4a. The submount view the design predicted (a grantor that binds a
/// parent non-recursively sees the directory *under* a mount): can an
/// unprivileged runtime build it? Only construction is attempted; the
/// world fails before its probe runs, so nothing under the mount is read.
#[test]
fn locked_submount_cannot_be_revealed() {
    let Some(mut lab) = Lab::new() else { return };
    // SAFETY: getuid always succeeds.
    let uid = unsafe { libc::getuid() };
    let p = format!("/run/user/{uid}");
    let parent = "/run/user";
    let (Ok(pm), Ok(qm)) = (fs::metadata(&p), fs::metadata(parent)) else {
        println!("SKIPPED: no {p}");
        return;
    };
    if pm.dev() == qm.dev() {
        println!("SKIPPED: {p} is not a mount point");
        return;
    }
    let mut read_paths = lab.closure.clone();
    read_paths.push(PathBuf::from(parent));
    let plan = capability::plan(&CapabilityManifest {
        read_paths,
        env: BTreeMap::from([("LC_ALL".into(), "C".into())]),
        ..Default::default()
    })
    .expect("plan");
    let step = plan
        .steps()
        .iter()
        .position(|s| matches!(s, capability::Step::Bind(b) if b.path == parent))
        .unwrap();
    let root = lab.p("root-locked");
    fs::create_dir_all(&root).unwrap();
    lab.worlds += 1;
    let r = capability::observe(&plan, &root, &lab.python, &BTreeMap::new());
    println!("bind {parent} (non-recursive; {p} is a mount below it) = step {step}: {r:?}");
    let Err(capability::WorldError::Construction { stderr, .. }) = r else {
        panic!("the world was built: {r:?}")
    };
    assert_eq!(
        stderr.trim(),
        format!("v9r world: step {step} errno {}", libc::EINVAL)
    );
}
