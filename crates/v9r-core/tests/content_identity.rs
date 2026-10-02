//! X1b: can read-only directory capabilities bind to content-addressed
//! state?
//!
//! World A (grantor) binds directory D and transcribes it; the snapshot
//! verifier turns the transcription into S, a git tree root, and the
//! runtime keeps S's objects. A signed grant gives B read access to D,
//! pinned by one or both identities:
//!
//! * `Object`: D's file handle (X1's ObjectCapability);
//! * `Content`: `tree:S` (a SnapshotCapability);
//! * `Both`.
//!
//! B receives it one of two ways:
//!
//! * **live**: the (possibly changed) host directory is bound read-only,
//!   and B's own probe transcribes it so its root can be compared;
//! * **materialized**: the runtime checks S out of its object store
//!   (every object hash-checked) into a directory it owns, and binds that.
//!
//! Needs unprivileged user namespaces, `python3` and `nix-store`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use v9r_core::authority::{self, Authority, Request};
use v9r_core::capability::{self, Alias, CapabilityManifest, Observation, ProbeExtra, ProbeOp};
use v9r_core::delegation::{cap_name, DEvidence, Grant, Op, Parent, Use};
use v9r_core::kernel::Verdict;
use v9r_core::object_identity::{self, content_evidence, content_id, object_id, IdScheme};
use v9r_core::snapshot::{materialize, snapshot_from, ObjectStore};

const VIEW: &str = "/v/d";

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
        let base = std::env::temp_dir().join(format!("v9r-x1b-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        Some(Lab {
            base: fs::canonicalize(base).unwrap(),
            python,
            closure,
            worlds: 0,
        })
    }

    fn s(&self, rel: &str) -> String {
        self.base.join(rel).to_str().unwrap().to_string()
    }

    fn world(
        &mut self,
        read: &[&str],
        aliases: &[(&str, &str)],
        extra: &ProbeExtra,
    ) -> Observation {
        self.worlds += 1;
        let mut read_paths = self.closure.clone();
        read_paths.extend(read.iter().map(PathBuf::from));
        let plan = capability::plan(&CapabilityManifest {
            read_paths,
            env: BTreeMap::from([("LC_ALL".into(), "C".into())]),
            aliases: aliases
                .iter()
                .map(|(s, t)| Alias {
                    source: PathBuf::from(s),
                    target: PathBuf::from(t),
                    writable: false,
                })
                .collect(),
            ..Default::default()
        })
        .expect("plan");
        let root = self.base.join(format!("root-{}", self.worlds));
        fs::create_dir_all(&root).unwrap();
        capability::observe_with(&plan, &root, &self.python, &BTreeMap::new(), extra)
            .expect("world")
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        // Materialized files are read-only; their directories are not.
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn write(path: &str, data: &str) {
    fs::create_dir_all(PathBuf::from(path).parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

/// D: a.txt, sub/b.txt, l -> a.txt.  apriv: secret, alpha (same bytes as a.txt).
fn populate(d: &str) {
    write(&format!("{d}/a.txt"), "alpha");
    write(&format!("{d}/sub/b.txt"), "beta");
    std::os::unix::fs::symlink("a.txt", format!("{d}/l")).unwrap();
}

/// What A granted: S, D's handle, and the objects A could see.
struct Granted {
    d: String,
    apriv: String,
    s: String,
    handle: String,
    names: BTreeSet<String>,
    private: BTreeSet<(u64, u64)>,
    store: ObjectStore,
}

fn relative(obs: &Observation, dir: &str) -> BTreeSet<String> {
    obs.objects_within(dir)
        .iter()
        .map(|(p, _, _)| p.strip_prefix(dir).unwrap_or(p).to_string())
        .collect()
}

fn grant_from_a(lab: &mut Lab, d: &str, apriv: &str) -> Granted {
    let a = lab.world(
        &[d, apriv],
        &[],
        &ProbeExtra {
            resolve: vec![d.into()],
            transcribe: vec![d.into()],
            ..Default::default()
        },
    );
    let store = ObjectStore::new();
    let s = snapshot_from(&store, &a.transcript(), d, "provider:world-probe").expect("S");
    Granted {
        d: d.to_string(),
        apriv: apriv.to_string(),
        s,
        handle: object_id(a.resolved(d).unwrap(), IdScheme::Handle).unwrap(),
        names: relative(&a, d),
        private: a
            .objects_within(apriv)
            .iter()
            .map(|(_, dev, ino)| (*dev, *ino))
            .collect(),
        store,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Pin {
    Object,
    Content,
    Both,
}

/// root → A → B, read D, pinned by `pin`; B's use judged on `context`.
fn decide(g: &Granted, pin: Pin, context: &DEvidence) -> Verdict {
    let mut identities = BTreeMap::new();
    if pin != Pin::Content {
        identities.insert(g.d.clone(), g.handle.clone());
    }
    if pin != Pin::Object {
        identities.insert(format!("{}#tree", g.d), content_id(&g.s));
    }
    let mut auth = Authority::new();
    let a = auth.enroll("A");
    let b = auth.enroll("B");
    let grant = |parent, grantor: &str, grantee: &str, depth| Grant {
        parent,
        grantor: grantor.into(),
        grantee: grantee.into(),
        objects: vec![g.d.clone()],
        operations: vec![Op::Read],
        constraints: BTreeMap::new(),
        not_after: 100,
        depth,
        identities: identities.clone(),
    };
    let root = auth.root_grant(grant(Parent::Root, "", &a, 1));
    let child = auth
        .delegate("A", grant(Parent::Grant(root), &a, &b, 0))
        .unwrap();
    let copy = auth.publish();
    let claimed = authority::chain(&copy, &child);
    let use_ = Use {
        world: b.clone(),
        names: vec![cap_name(Op::Read, &format!("{}/a.txt", g.d))],
    };
    let request = Request {
        copies: std::slice::from_ref(&copy),
        claimed: &claimed,
        leaf: &child,
        use_: &use_,
        context,
    };
    authority::authorize(&auth, &auth.anchor(), &auth.witness(), &request).verdict
}

/// One B world over `source` (bound at VIEW), judged under every pin.
struct Seen {
    verdicts: BTreeMap<Pin, Verdict>,
    root: Result<String, String>,
    extra_names: Vec<String>,
    shared_with_private: usize,
    read: String,
    write: String,
}

fn receive(lab: &mut Lab, g: &Granted, source: &str) -> Seen {
    let b = lab.world(
        &[],
        &[(source, VIEW)],
        &ProbeExtra {
            ops: BTreeMap::from([
                (
                    "1-read".into(),
                    ProbeOp::Read {
                        path: format!("{VIEW}/a.txt"),
                    },
                ),
                (
                    "2-write".into(),
                    ProbeOp::Write {
                        path: format!("{VIEW}/a.txt"),
                        data: "x".into(),
                    },
                ),
            ]),
            resolve: vec![VIEW.into()],
            transcribe: vec![VIEW.into()],
        },
    );
    let mut context =
        object_identity::evidence(&b, &[(g.d.clone(), VIEW.into())], IdScheme::Handle);
    let (content, failed) = content_evidence(
        &b,
        &[(format!("{}#tree", g.d), VIEW.into())],
        &ObjectStore::new(),
    );
    context.merge(&content);
    let root = snapshot_from(
        &ObjectStore::new(),
        &b.transcript(),
        VIEW,
        "provider:world-probe",
    )
    .map_err(|e| format!("{e} {failed:?}"));
    let verdicts = [Pin::Object, Pin::Content, Pin::Both]
        .into_iter()
        .map(|p| (p, decide(g, p, &context)))
        .collect();
    Seen {
        verdicts,
        root,
        extra_names: relative(&b, VIEW).difference(&g.names).cloned().collect(),
        shared_with_private: b
            .objects_within(VIEW)
            .iter()
            .filter(|(_, d, i)| g.private.contains(&(*d, *i)))
            .count(),
        read: b.op("1-read").unwrap_or("?").to_string(),
        write: b.op("2-write").unwrap_or("?").to_string(),
    }
}

struct Case {
    name: &'static str,
    /// SnapshotCapability semantics: may B see exactly this view?
    allowed: bool,
    /// Change the host after the grant; return the directory B is given
    /// in the live arm.
    mutate: fn(&Lab, &Granted) -> String,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "1 same content, different path (copy)",
            allowed: true,
            mutate: |lab, g| {
                let e = lab.s("copy-of-d");
                let st = Command::new("cp").args(["-a", &g.d, &e]).status().unwrap();
                assert!(st.success());
                e
            },
        },
        Case {
            name: "2 same path, modified content",
            allowed: false,
            mutate: |_, g| {
                write(&format!("{}/a.txt", g.d), "ALPHA");
                g.d.clone()
            },
        },
        Case {
            name: "3a file added after grant",
            allowed: false,
            mutate: |_, g| {
                write(&format!("{}/new.txt", g.d), "new");
                g.d.clone()
            },
        },
        Case {
            name: "3b empty directory added after grant",
            allowed: false,
            mutate: |_, g| {
                fs::create_dir(format!("{}/secret-plans", g.d)).unwrap();
                g.d.clone()
            },
        },
        Case {
            name: "4a hard link to a private file added",
            allowed: false,
            mutate: |_, g| {
                fs::hard_link(format!("{}/secret", g.apriv), format!("{}/s", g.d)).unwrap();
                g.d.clone()
            },
        },
        Case {
            name: "4b file replaced by a hard link to identical private bytes",
            allowed: false,
            mutate: |_, g| {
                let a = format!("{}/a.txt", g.d);
                fs::remove_file(&a).unwrap();
                fs::hard_link(format!("{}/alpha", g.apriv), &a).unwrap();
                g.d.clone()
            },
        },
        Case {
            name: "5 rename",
            allowed: true,
            mutate: |lab, g| {
                let d2 = lab.s("renamed-d");
                fs::rename(&g.d, &d2).unwrap();
                d2
            },
        },
        Case {
            name: "6a delete and recreate, different content",
            allowed: false,
            mutate: |_, g| {
                fs::remove_dir_all(&g.d).unwrap();
                populate(&g.d);
                write(&format!("{}/a.txt", g.d), "omega");
                g.d.clone()
            },
        },
        Case {
            name: "6b delete and recreate, identical content",
            allowed: true,
            mutate: |_, g| {
                fs::remove_dir_all(&g.d).unwrap();
                populate(&g.d);
                g.d.clone()
            },
        },
    ]
}

fn mark(v: Verdict, allowed: bool) -> String {
    let ok = (v == Verdict::Allow) == allowed;
    format!("{v:?}{}", if ok { "" } else { " ✗" })
}

#[test]
fn directory_capabilities_bound_to_content() {
    let Some(mut lab) = Lab::new() else { return };
    let start = Instant::now();
    let mut wrong: BTreeMap<(&str, Pin), Vec<&str>> = BTreeMap::new();
    for (i, case) in cases().into_iter().enumerate() {
        // Fresh D and private files per case.
        let d = lab.s(&format!("c{i}/d"));
        let apriv = lab.s(&format!("c{i}/apriv"));
        populate(&d);
        write(&format!("{apriv}/secret"), "secret");
        write(&format!("{apriv}/alpha"), "alpha");
        let g = grant_from_a(&mut lab, &d, &apriv);

        let source = (case.mutate)(&lab, &g);
        let live = receive(&mut lab, &g, &source);

        let mat = lab.s(&format!("c{i}/materialized"));
        materialize(&g.store, &g.s, std::path::Path::new(&mat)).expect("materialize S");
        let materialized = receive(&mut lab, &g, &mat);

        println!("\n{}  (B may see it: {})", case.name, case.allowed);
        println!("  S = {}", &g.s[..16]);
        // Ground truth per arm: the live view is the changed D; the
        // materialized view is S, which B may always see.
        for (arm, seen, allowed) in [
            ("live", &live, case.allowed),
            ("materialized", &materialized, true),
        ] {
            let root = match &seen.root {
                Ok(r) if *r == g.s => "= S".to_string(),
                Ok(r) => format!("{} ≠ S", &r[..16]),
                Err(e) => format!("none: {e}"),
            };
            println!(
                "  {arm:<12} root {root}; Object {}, Content {}, Both {}; extra names {:?}; shares {} inode(s) with A's private files; read {}, write {}",
                mark(seen.verdicts[&Pin::Object], allowed),
                mark(seen.verdicts[&Pin::Content], allowed),
                mark(seen.verdicts[&Pin::Both], allowed),
                seen.extra_names,
                seen.shared_with_private,
                &seen.read[..seen.read.len().min(11)],
                seen.write,
            );
            for (pin, v) in &seen.verdicts {
                if (*v == Verdict::Allow) != allowed {
                    wrong.entry((arm, *pin)).or_default().push(case.name);
                }
            }
        }
        // The materialized view is S whatever happened to D, holds no
        // object of A's, and cannot be written.
        assert_eq!(
            materialized.root.as_deref(),
            Ok(g.s.as_str()),
            "{}",
            case.name
        );
        assert!(materialized.extra_names.is_empty());
        assert_eq!(materialized.shared_with_private, 0);
        assert_eq!(materialized.write, "EROFS");
        assert_eq!(materialized.verdicts[&Pin::Content], Verdict::Allow);
    }
    println!("\nwrong verdicts by arm and pin:");
    for ((arm, pin), names) in &wrong {
        println!("  {arm:<12} {pin:?}: {names:?}");
    }
    println!("{} worlds in {:?}", lab.worlds, start.elapsed());

    // Content pins on a live view: right on content, wrong exactly where
    // the view exposes what a git tree does not record.
    assert_eq!(
        wrong
            .get(&("live", Pin::Content))
            .cloned()
            .unwrap_or_default(),
        vec![
            "3b empty directory added after grant",
            "4b file replaced by a hard link to identical private bytes",
        ]
    );
    // Content pins on a materialized view: never wrong. Object pins on a
    // materialized view: always wrong (the copy is a new object).
    assert!(!wrong.contains_key(&("materialized", Pin::Content)));
    assert_eq!(wrong[&("materialized", Pin::Object)].len(), cases().len());
}

/// A store that serves a tampered object cannot be materialized: no
/// world is built from it.
#[test]
fn tampered_snapshot_is_not_materialized() {
    let base = std::env::temp_dir().join(format!("v9r-x1b-t-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let d = base.join("d");
    populate(d.to_str().unwrap());
    let store = ObjectStore::new();
    // Transcribe on the host with the same formats the probe uses.
    let mut t = BTreeMap::new();
    let ds = d.to_str().unwrap().to_string();
    use v9r_core::graph::{Key, Term};
    t.insert(
        Key::new("fs_stat", [ds.as_str()]),
        Term::Map(BTreeMap::from([("nlink".into(), "3".into())])),
    );
    t.insert(
        Key::new("fs_dir", [ds.as_str()]),
        Term::Map(BTreeMap::from([
            ("a.txt".into(), "100644".into()),
            ("l".into(), "120000".into()),
            ("sub".into(), "40000".into()),
        ])),
    );
    let sub = format!("{ds}/sub");
    t.insert(
        Key::new("fs_stat", [sub.as_str()]),
        Term::Map(BTreeMap::from([("nlink".into(), "2".into())])),
    );
    t.insert(
        Key::new("fs_dir", [sub.as_str()]),
        Term::Map(BTreeMap::from([("b.txt".into(), "100644".into())])),
    );
    t.insert(
        Key::new("fs_file", [format!("{ds}/a.txt").as_str()]),
        Term::Bytes(b"alpha".to_vec()),
    );
    t.insert(
        Key::new("fs_file", [format!("{sub}/b.txt").as_str()]),
        Term::Bytes(b"beta".to_vec()),
    );
    t.insert(
        Key::new("fs_link", [format!("{ds}/l").as_str()]),
        Term::Text("a.txt".into()),
    );
    let root = snapshot_from(&store, &t, &ds, "test").unwrap();

    assert!(materialize(&store, &root, &base.join("ok")).is_ok());
    let blob = store
        .ids()
        .into_iter()
        .find(|id| matches!(store.get_verified(id), Ok((k, b)) if k == "blob" && b == b"beta"))
        .unwrap();
    store.overwrite(&blob, b"blob 4\0BETA".to_vec());
    let r = materialize(&store, &root, &base.join("tampered"));
    println!("tampered: {r:?}");
    assert!(r.unwrap_err().contains("do not hash"));
    assert!(!base.join("tampered").exists(), "nothing written");
    let _ = fs::remove_dir_all(&base);
}
