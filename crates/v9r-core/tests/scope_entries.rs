//! The scope invariant over `snapshot_entries` (FsSnapshot): which
//! changes inside and outside a declared scope it allows, denies or
//! blocks.
//!
//! Debloat Phase 1 replaced `fs_provider`'s `entries` with
//! `snapshot_entries`. Before `fs_provider` was removed, this file ran
//! both sources side by side on every case below (same runtime, same
//! registry as State vs Causality v0, a fresh workspace per case) and
//! required the new source to reject everything the old one rejected:
//!
//! | change | in scope | `entries` (fs_provider) | `snapshot_entries` |
//! |---|---|---|---|
//! | nothing; modify / add / delete a src file; empty dir in src | yes | Allow | Allow |
//! | modify / add / add hidden / delete a file outside | no | Deny | Deny |
//! | add a file in a nested dir outside | no | Deny | Deny |
//! | add / delete an empty dir outside | no | Deny | Deny |
//! | add / retarget a symlink outside | no | Deny | Deny |
//! | replace a file by a directory outside | no | Deny | Deny |
//! | `chmod +x` a file outside | no | **Allow** | **Deny** |
//! | make a file outside unreadable | no | Blocked | Blocked |
//! | create a FIFO outside | no | **Deny** | **Blocked** |
//!
//! The two differences: the old source ignored permission bits (a false
//! Allow); the new one cannot represent a FIFO in a snapshot, so it
//! blocks where the old one denied. Both still reject. The cases now pin
//! the new source's verdicts.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use v9r_core::fs_raw::RawFsObserver;
use v9r_core::graph::{Key, Registry, Trust};
use v9r_core::kernel::{Obligation, Phase, Requirement, Verdict};
use v9r_core::runtime::Authorize;
use v9r_core::snapshot::{FsSnapshot, ObjectStore};
use v9r_core::temporal::{self, changed, pin, Actor, TObligation, Transition, TransitionInvariant};

#[derive(Clone, Debug)]
struct Task;

struct WithinScope(&'static str);

impl TransitionInvariant<Task> for WithinScope {
    fn id(&self) -> &str {
        "I2.changes_within_scope"
    }
    fn watches(&self) -> Vec<Key> {
        vec![Key::new(self.0, ["ws"])]
    }
    fn post(&self, t: &Transition<'_, Task>) -> Vec<TObligation> {
        let k = Key::new(self.0, ["ws"]);
        let (p0, e0) = pin(self.id(), t.before, &k);
        let mut out = vec![p0];
        let e1 = t.after.and_then(|a| {
            let (p1, e1) = pin(self.id(), a, &k);
            out.push(p1);
            e1
        });
        out.push(Obligation {
            invariant: self.id().into(),
            phase: Phase::Post,
            requirement: Requirement::Within {
                names: changed("", e0, e1),
                scopes: vec!["ws/src".into()],
            },
        });
        out
    }
}

type Mutation = fn(&PathBuf);

struct Agent {
    ws: PathBuf,
    mutate: Mutation,
}

impl Actor<Task> for Agent {
    async fn act(&mut self, _: &Task) -> Result<String, String> {
        (self.mutate)(&self.ws);
        Ok(String::new())
    }
}

fn mkfifo(path: &std::path::Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

/// (name, verdict, mutation). In-scope changes are the `Allow` cases.
fn cases() -> Vec<(&'static str, Verdict, Mutation)> {
    vec![
        ("nothing", Verdict::Allow, |_| {}),
        ("modify src file", Verdict::Allow, |ws| fs::write(ws.join("src/a.py"), "new\n").unwrap()),
        ("add src file", Verdict::Allow, |ws| fs::write(ws.join("src/b.py"), "b\n").unwrap()),
        ("delete src file", Verdict::Allow, |ws| fs::remove_file(ws.join("src/a.py")).unwrap()),
        ("add empty dir in src", Verdict::Allow, |ws| fs::create_dir(ws.join("src/d")).unwrap()),
        ("modify file outside", Verdict::Deny, |ws| fs::write(ws.join("README"), "x\n").unwrap()),
        ("add file outside", Verdict::Deny, |ws| fs::write(ws.join("NEW"), "x\n").unwrap()),
        ("add hidden file outside", Verdict::Deny, |ws| fs::write(ws.join(".hidden"), "x\n").unwrap()),
        ("delete file outside", Verdict::Deny, |ws| fs::remove_file(ws.join("README")).unwrap()),
        ("add file in nested dir outside", Verdict::Deny, |ws| {
            fs::write(ws.join("tests/t2.py"), "x\n").unwrap()
        }),
        ("add empty dir outside", Verdict::Deny, |ws| fs::create_dir(ws.join("empty")).unwrap()),
        ("delete empty dir outside", Verdict::Deny, |ws| fs::remove_dir(ws.join("keep")).unwrap()),
        ("add symlink outside", Verdict::Deny, |ws| {
            std::os::unix::fs::symlink("src/a.py", ws.join("link2")).unwrap()
        }),
        ("retarget symlink outside", Verdict::Deny, |ws| {
            fs::remove_file(ws.join("link")).unwrap();
            std::os::unix::fs::symlink("README", ws.join("link")).unwrap();
        }),
        ("replace file by dir outside", Verdict::Deny, |ws| {
            fs::remove_file(ws.join("README")).unwrap();
            fs::create_dir(ws.join("README")).unwrap();
            fs::write(ws.join("README/x"), "x\n").unwrap();
        }),
        ("chmod +x file outside", Verdict::Deny, |ws| {
            fs::set_permissions(ws.join("README"), fs::Permissions::from_mode(0o755)).unwrap()
        }),
        ("make file outside unreadable", Verdict::Blocked, |ws| {
            fs::set_permissions(ws.join("README"), fs::Permissions::from_mode(0o000)).unwrap()
        }),
        ("create fifo outside", Verdict::Blocked, |ws| mkfifo(&ws.join("pipe"))),
    ]
}

struct Lab {
    base: PathBuf,
    store: ObjectStore,
}

impl Lab {
    fn new() -> Lab {
        let base = std::env::temp_dir().join(format!("v9r-entries-{}", uuid::Uuid::new_v4()));
        let ws = base.join("ws");
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::create_dir_all(ws.join("tests")).unwrap();
        fs::create_dir_all(ws.join("keep")).unwrap();
        fs::write(ws.join("src/a.py"), "a\n").unwrap();
        fs::write(ws.join("tests/t.py"), "t\n").unwrap();
        fs::write(ws.join("README"), "read me\n").unwrap();
        std::os::unix::fs::symlink("src/a.py", ws.join("link")).unwrap();
        Lab {
            base: fs::canonicalize(base).unwrap(),
            store: ObjectStore::new(),
        }
    }

    fn registry(&self) -> Registry {
        let r = Registry::new();
        r.register(RawFsObserver::new("raw", &self.base), Trust::Attesting);
        for kind in ["fs_file", "fs_link"] {
            r.restrict(kind, &["raw"]);
        }
        r.add_verifier(FsSnapshot::new(self.store.clone()));
        r
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = std::process::Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.base)
            .status();
        let _ = fs::remove_dir_all(&self.base);
    }
}

async fn verdict(mutate: Mutation) -> (Verdict, String) {
    let lab = Lab::new();
    let agent = Agent {
        ws: lab.base.join("ws"),
        mutate,
    };
    let mut rt = temporal::runtime(lab.registry(), vec![Box::new(WithinScope("snapshot_entries"))], agent);
    let Authorize::Allowed(auth) = rt.authorize(Task).await.unwrap() else {
        panic!("PRE")
    };
    let report = rt.execute(*auth).await.unwrap();
    assert!(report.executed);
    (report.decision.verdict, report.decision.to_string())
}

#[tokio::test]
async fn scope_over_snapshot_entries() {
    for (name, expected, mutate) in cases() {
        let (got, text) = verdict(mutate).await;
        assert_eq!(got, expected, "{name}\n{text}");
    }
}
