//! "Release operation is safe": transition invariants over independent
//! providers, judged on snapshots before and after the effect.
//!
//! Everything here uses only v9r-core's public API. The invariants know
//! keys and values, not providers; the CI provider is third-party.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, Once};

use uuid::Uuid;
use v9r_core::fs_provider::FilesystemEvidenceProvider;
use v9r_core::git::GitRepo;
use v9r_core::git_provider::GitEvidenceProvider;
use v9r_core::graph::{Answer, Attested, Attestor, EvidenceProvider, Key, Registry, Term, Trust};
use v9r_core::kernel::{Decision, Name, Obligation, Phase, Requirement, Status, Verdict};
use v9r_core::runtime::{Authorize, Report};
use v9r_core::temporal::{
    self, changed, pin, require, Actor, TKey, TemporalDomain, TemporalRuntime, Transition,
    TransitionInvariant,
};

// ------------------------------------------------------------ invariants

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReleaseOp {
    /// The commit the release ref is to point at.
    commit: String,
    /// Witness: the content digest of that commit's tree.
    digest: String,
}

const REPO: &str = "repo";
const RELEASE_REF: &str = "refs/heads/release";
const GIT_INV: &str = "release.refs_transition";
const FS_INV: &str = "release.artifact_placement";
const CI_INV: &str = "release.ci_for_result";

fn refs_key() -> Key {
    Key::new("refs", [REPO])
}

fn release_key() -> Key {
    Key::new("ref", [REPO, RELEASE_REF])
}

fn id_of(term: Option<&Term>) -> String {
    match term {
        Some(Term::Id(id)) => id.clone(),
        _ => "unknown".to_string(),
    }
}

/// Only the release ref moves, to the proposed commit, fast-forward.
struct RefsTransition;

impl TransitionInvariant<ReleaseOp> for RefsTransition {
    fn id(&self) -> &str {
        GIT_INV
    }

    fn watches(&self) -> Vec<Key> {
        vec![refs_key(), release_key()]
    }

    fn post(&self, t: &Transition<'_, ReleaseOp>) -> Vec<Obligation<TKey, Term>> {
        let (refs_before, m0) = pin(GIT_INV, t.before, &refs_key());
        let (rel_before, old) = pin(GIT_INV, t.before, &release_key());
        let mut out = vec![refs_before, rel_before];
        let m1 = t.after.and_then(|after| {
            let (pinned, m1) = pin(GIT_INV, after, &refs_key());
            out.push(pinned);
            out.push(require(
                GIT_INV,
                TKey::at(after, release_key()),
                Term::Id(t.proposal.commit.clone()),
            ));
            m1
        });
        out.push(Obligation {
            invariant: GIT_INV.into(),
            phase: Phase::Post,
            requirement: Requirement::Within {
                names: changed(REPO, m0, m1),
                scopes: vec![format!("{REPO}/{RELEASE_REF}")],
            },
        });
        out.push(require(
            GIT_INV,
            TKey::now(Key::new(
                "descends",
                [REPO, &id_of(old), &t.proposal.commit],
            )),
            Term::Bool(true),
        ));
        out
    }
}

/// The only filesystem changes are the artifact and git's own metadata,
/// and the artifact has the content of the released commit.
struct ArtifactPlacement;

const ARTIFACT_DIR: &str = "dist";

impl TransitionInvariant<ReleaseOp> for ArtifactPlacement {
    fn id(&self) -> &str {
        FS_INV
    }

    fn watches(&self) -> Vec<Key> {
        vec![Key::new("entries", ["."])]
    }

    fn post(&self, t: &Transition<'_, ReleaseOp>) -> Vec<Obligation<TKey, Term>> {
        let tree = Key::new("entries", ["."]);
        let (before, e0) = pin(FS_INV, t.before, &tree);
        let mut out = vec![before];
        let e1 = t.after.and_then(|after| {
            let (pinned, e1) = pin(FS_INV, after, &tree);
            out.push(pinned);
            e1
        });
        out.push(Obligation {
            invariant: FS_INV.into(),
            phase: Phase::Post,
            requirement: Requirement::Within {
                names: changed("", e0, e1),
                scopes: vec![ARTIFACT_DIR.into(), format!("{REPO}/.git")],
            },
        });
        out.push(require(
            FS_INV,
            TKey::now(Key::new("dir_content", [ARTIFACT_DIR])),
            Term::Digest(t.proposal.digest.clone()),
        ));
        out.push(require(
            FS_INV,
            TKey::now(Key::new("tree_content", [REPO, &t.proposal.commit])),
            Term::Digest(t.proposal.digest.clone()),
        ));
        out
    }
}

/// The tests passed for whatever the release ref points at afterwards.
struct CiForResult;

impl TransitionInvariant<ReleaseOp> for CiForResult {
    fn id(&self) -> &str {
        CI_INV
    }

    fn watches(&self) -> Vec<Key> {
        vec![release_key()]
    }

    fn post(&self, t: &Transition<'_, ReleaseOp>) -> Vec<Obligation<TKey, Term>> {
        let Some(after) = t.after else {
            return vec![require(
                CI_INV,
                TKey::now(Key::new("tests", [REPO, "unknown"])),
                Term::Text("passed".into()),
            )];
        };
        let (pinned, result) = pin(CI_INV, after, &release_key());
        vec![
            pinned,
            require(
                CI_INV,
                TKey::now(Key::new("tests", [REPO, &id_of(result)])),
                Term::Text("passed".into()),
            ),
        ]
    }
}

fn invariants() -> Vec<Box<dyn TransitionInvariant<ReleaseOp>>> {
    vec![
        Box::new(RefsTransition),
        Box::new(ArtifactPlacement),
        Box::new(CiForResult),
    ]
}

// ------------------------------------------------------------ providers

/// Third-party CI: results by (repo, commit).
struct FakeCi(BTreeMap<String, bool>);

impl EvidenceProvider for FakeCi {
    fn id(&self) -> &str {
        "ci"
    }

    fn answers(&self, key: &Key) -> bool {
        key.kind == "tests" && key.args.len() == 2
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let passed = self.0.get(&key.args[1])?;
                let text = if *passed { "passed" } else { "failed" };
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    Term::Text(text.into()),
                    "build record",
                )))
            })
            .collect()
    }
}

// ------------------------------------------------------------ environment

static HERMETIC: Once = Once::new();

fn hermetic_git() {
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
}

fn git(dir: &Path, args: &[&str]) -> String {
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

/// Extra registrations, between git and fs.
type Extra = Vec<Box<dyn FnOnce(&Registry)>>;

#[derive(Clone)]
struct Env {
    base: PathBuf,
    work: PathBuf,
    /// release ref at t0 (the approved base).
    x0: String,
    /// the candidate to release (descends from x0).
    x1: String,
    /// a sibling of x1 (also descends from x0).
    x2: String,
}

impl Env {
    fn new(name: &str) -> Self {
        hermetic_git();
        let base = std::env::temp_dir().join(format!("v9r-temporal-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        let repo = work.join(REPO);
        fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", REPO]);
        fs::write(repo.join("README"), "approved base\n").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let x0 = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["branch", "release"]);
        git(&repo, &["checkout", "-q", "-b", "candidate"]);
        fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        git(&repo, &["add", "feature.txt"]);
        git(&repo, &["commit", "-q", "-m", "feature"]);
        let x1 = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "-b", "other", &x0]);
        fs::write(repo.join("other.txt"), "other\n").unwrap();
        git(&repo, &["add", "other.txt"]);
        git(&repo, &["commit", "-q", "-m", "other"]);
        let x2 = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "release"]);
        Self {
            base,
            work,
            x0,
            x1,
            x2,
        }
    }

    fn repo(&self) -> PathBuf {
        self.work.join(REPO)
    }

    fn git_provider(&self) -> GitEvidenceProvider {
        GitEvidenceProvider::new(
            "git",
            &self.work,
            vec![GitRepo {
                path: REPO.into(),
                bare: false,
            }],
        )
    }

    fn ci(&self) -> FakeCi {
        FakeCi(BTreeMap::from([(self.x1.clone(), true)]))
    }

    /// git, then `extra` (in order), then fs, then ci.
    fn registry_with(&self, git: impl EvidenceProvider + 'static, extra: Extra) -> Registry {
        let registry = Registry::new();
        registry.register(git, Trust::Attesting);
        for add in extra {
            add(&registry);
        }
        registry.register(
            FilesystemEvidenceProvider::new("fs", &self.work),
            Trust::Attesting,
        );
        registry.register(self.ci(), Trust::Attesting);
        registry
    }

    fn registry(&self) -> Registry {
        self.registry_with(self.git_provider(), Vec::new())
    }

    fn op(&self) -> ReleaseOp {
        let probe = Registry::new();
        probe.register(self.git_provider(), Trust::Attesting);
        let Some(Term::Digest(digest)) =
            probe.query(&Key::new("tree_content", [REPO, self.x1.as_str()]))
        else {
            panic!("no tree digest")
        };
        ReleaseOp {
            commit: self.x1.clone(),
            digest,
        }
    }

    /// The honest release: move the ref, write the artifact.
    fn release(&self, op: &ReleaseOp) {
        git(
            &self.repo(),
            &["update-ref", RELEASE_REF, &op.commit, &self.x0],
        );
        let dist = self.work.join(ARTIFACT_DIR);
        fs::create_dir_all(&dist).unwrap();
        fs::write(dist.join("README"), "approved base\n").unwrap();
        fs::write(dist.join("feature.txt"), "feature\n").unwrap();
    }

    fn release_ref(&self) -> String {
        git(&self.repo(), &["rev-parse", RELEASE_REF])
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Clones share the directory; whichever drops first removes it.
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// An actor from a closure.
struct Act<F>(F);

impl<F: FnMut(&ReleaseOp) -> Result<String, String>> Actor<ReleaseOp> for Act<F> {
    async fn act(&mut self, op: &ReleaseOp) -> Result<String, String> {
        (self.0)(op)
    }
}

type Rt<F> = TemporalRuntime<ReleaseOp, Act<F>>;
type Rep<F> = Report<TemporalDomain<ReleaseOp, Act<F>>>;

fn guard<F: FnMut(&ReleaseOp) -> Result<String, String>>(registry: &Registry, actor: F) -> Rt<F> {
    temporal::runtime(registry.clone(), invariants(), Act(actor))
}

async fn run<F: FnMut(&ReleaseOp) -> Result<String, String>>(
    rt: &mut Rt<F>,
    op: ReleaseOp,
) -> (Verdict, Option<Rep<F>>) {
    match rt.authorize(op).await.unwrap() {
        Authorize::Allowed(auth) => (Verdict::Allow, Some(rt.execute(*auth).await.unwrap())),
        other => (other.verdict(), None),
    }
}

fn statuses<'a>(d: &'a Decision<impl Sized, impl Sized>, invariant: &str) -> Vec<&'a Status> {
    d.findings
        .iter()
        .filter(|f| f.obligation.invariant == invariant)
        .map(|f| &f.status)
        .collect()
}

fn violated(d: &Decision<impl Sized, impl Sized>, invariant: &str) -> bool {
    statuses(d, invariant)
        .iter()
        .any(|s| matches!(s, Status::Violated(_)))
}

fn undetermined(d: &Decision<impl Sized, impl Sized>, invariant: &str) -> bool {
    statuses(d, invariant)
        .iter()
        .any(|s| matches!(s, Status::Undetermined(_)))
}

fn satisfied(d: &Decision<impl Sized, impl Sized>, invariant: &str) -> bool {
    let s = statuses(d, invariant);
    !s.is_empty() && s.iter().all(|s| matches!(s, Status::Satisfied(_)))
}

// ------------------------------------------------------------ the invariant

#[tokio::test]
async fn honest_release_operation_is_safe() {
    let env = Env::new("honest");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        Ok("released".into())
    });
    let op = env.op();
    let (pre, report) = run(&mut rt, op.clone()).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Allow,
        "{}",
        report.decision
    );
    assert!(report.accepted);
    assert_eq!(report.output, Some(op));
    for invariant in ["transition.ordered", GIT_INV, FS_INV, CI_INV] {
        assert!(satisfied(&report.decision, invariant), "{invariant}");
    }
    assert_eq!(env.release_ref(), env.x1);
}

#[tokio::test]
async fn moving_a_protected_ref_is_denied() {
    let env = Env::new("protected");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        git(&e.repo(), &["update-ref", "refs/heads/main", &e.x2]);
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Deny,
        "{}",
        report.decision
    );
    assert!(violated(&report.decision, GIT_INV));
    assert!(report.decision.to_string().contains("repo/refs/heads/main"));
    assert!(!rt.is_accepting());
}

#[tokio::test]
async fn releasing_an_unexpected_commit_is_denied() {
    let env = Env::new("unexpected");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        git(&e.repo(), &["update-ref", RELEASE_REF, &e.x2]);
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Deny,
        "{}",
        report.decision
    );
    assert!(violated(&report.decision, GIT_INV));
    // CI is judged on the commit actually released (x2), which has no result.
    assert!(undetermined(&report.decision, CI_INV));
}

#[tokio::test]
async fn artifact_outside_the_allowed_location_is_denied() {
    let env = Env::new("placement");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        fs::write(e.work.join("stray.txt"), "x").unwrap();
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Deny,
        "{}",
        report.decision
    );
    assert!(violated(&report.decision, FS_INV));
    assert!(report.decision.to_string().contains("stray.txt"));
}

#[tokio::test]
async fn ci_must_cover_the_resulting_commit() {
    let env = Env::new("ci");
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::Attesting);
    registry.register(
        FilesystemEvidenceProvider::new("fs", &env.work),
        Trust::Attesting,
    );
    registry.register(
        FakeCi(BTreeMap::from([(env.x0.clone(), true)])),
        Trust::Attesting,
    );
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    assert!(undetermined(&report.decision, CI_INV));
    assert!(
        !rt.is_accepting(),
        "the effect happened and is not accepted"
    );
}

// ------------------------------------------------------------ adversarial

/// Captures spare attestations of `refs` at its `capture`-th request and
/// replays them from its `replay`-th request on: the t0 refs table is
/// presented as the t1 one.
struct ReplayingGit {
    inner: GitEvidenceProvider,
    capture: usize,
    replay: usize,
    calls: Mutex<usize>,
    spares: Mutex<Vec<Attested>>,
}

impl EvidenceProvider for ReplayingGit {
    fn id(&self) -> &str {
        "git"
    }

    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut calls = self.calls.lock().unwrap();
        let wants_refs = keys.iter().any(|k| k.kind == "refs");
        if wants_refs {
            *calls += 1;
        }
        let mut out = Vec::new();
        for key in keys {
            if key.kind == "refs" && *calls >= self.replay {
                if let Some(old) = self.spares.lock().unwrap().pop() {
                    out.push(Answer::Verified(old));
                    continue;
                }
            }
            let answers = self.inner.provide(&[key], attestor);
            if key.kind == "refs" && *calls == self.capture {
                if let Some(Answer::Verified(a)) = answers.first() {
                    let value = a.fact().value.clone();
                    let mut spares = self.spares.lock().unwrap();
                    for _ in 0..8 {
                        spares.push(attestor.attest((*key).clone(), value.clone(), "for-each-ref"));
                    }
                }
            }
            out.extend(answers);
        }
        out
    }
}

// Requests for `refs`: baseline 1-2, t0 (execution freshness) 3-4, t1 5-6.

#[tokio::test]
async fn before_snapshot_replayed_as_after_is_not_believed() {
    let env = Env::new("replay");
    let registry = env.registry_with(
        ReplayingGit {
            inner: env.git_provider(),
            capture: 3,
            replay: 5,
            calls: Mutex::new(0),
            spares: Mutex::default(),
        },
        Vec::new(),
    );
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        git(&e.repo(), &["update-ref", "refs/heads/main", &e.x2]);
        Ok(String::new())
    });
    let (pre, report) = run(&mut rt, env.op()).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(report.executed);
    // Had the replay been believed, "no protected ref changed" would hold.
    assert_ne!(
        report.decision.verdict,
        Verdict::Allow,
        "{}",
        report.decision
    );
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    assert!(undetermined(&report.decision, GIT_INV));
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.key.kind == "refs" && d.reason.contains("another provider or request")));
    assert!(!rt.is_accepting());
}

/// Simulates a concurrent writer: on its `fire`-th request it moves
/// `main`, between the git and fs providers' readings.
struct Interloper {
    repo: PathBuf,
    to: String,
    fire: usize,
    calls: Mutex<usize>,
}

impl EvidenceProvider for Interloper {
    fn id(&self) -> &str {
        "interloper"
    }

    fn answers(&self, key: &Key) -> bool {
        key.kind == "refs"
    }

    fn provide(&self, _: &[&Key], _: &Attestor) -> Vec<Answer> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == self.fire {
            git(&self.repo, &["update-ref", "refs/heads/main", &self.to]);
        }
        Vec::new()
    }
}

#[tokio::test]
async fn providers_observing_different_versions_block() {
    let env = Env::new("versions");
    let interloper = Interloper {
        repo: env.repo(),
        to: env.x2.clone(),
        fire: 5, // first reading of the t1 snapshot
        calls: Mutex::new(0),
    };
    let registry = env.registry_with(
        env.git_provider(),
        vec![Box::new(move |r: &Registry| {
            r.register(interloper, Trust::Attesting);
        })],
    );
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    let receipt = report.receipt.unwrap();
    let after = receipt.after.unwrap();
    assert!(after.inconsistent().contains(&refs_key()));
    assert!(undetermined(&report.decision, GIT_INV));
}

#[tokio::test]
async fn provider_unavailable_after_effect_blocks_and_holds() {
    let env = Env::new("unavailable");
    let registry = env.registry();
    let e = env.clone();
    let handle = registry.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        handle.remove("fs");
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert!(report.executed);
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    assert!(undetermined(&report.decision, FS_INV));
    assert!(report.output.is_none());
    assert!(!rt.is_accepting());
    assert_eq!(env.release_ref(), env.x1, "the effect did happen");
}

#[tokio::test]
async fn external_modification_between_authorization_and_execution_refuses() {
    let env = Env::new("between");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        Ok(String::new())
    });
    let Authorize::Allowed(auth) = rt.authorize(env.op()).await.unwrap() else {
        panic!()
    };
    git(&env.repo(), &["update-ref", "refs/heads/main", &env.x2]);
    let report = rt.execute(*auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(env.release_ref(), env.x0, "nothing ran");
    assert!(!rt.is_accepting(), "drift from the trusted snapshot");
}

#[tokio::test]
async fn external_modification_during_the_effect_is_attributed_to_it() {
    let env = Env::new("during");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        // Someone else, at the same time.
        git(&e.repo(), &["update-ref", "refs/heads/main", &e.x2]);
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    assert_eq!(report.unwrap().decision.verdict, Verdict::Deny);
}

#[tokio::test]
async fn external_write_inside_allowed_scope_is_indistinguishable() {
    // Documents a limit: transition invariants judge what changed, not who
    // changed it.
    let env = Env::new("benign");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        fs::write(e.repo().join(".git/description"), "someone else\n").unwrap();
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    assert_eq!(report.unwrap().decision.verdict, Verdict::Allow);
}

#[test]
fn transition_vocabulary_uses_existing_requirement_forms() {
    // `changed` yields ordinary names for Within; unknown sides widen.
    let a = Term::Map(BTreeMap::from([("x".into(), "1".into())]));
    let b = Term::Map(BTreeMap::from([
        ("x".into(), "2".into()),
        ("y".into(), "1".into()),
    ]));
    assert_eq!(
        changed("r", Some(&a), Some(&b)),
        vec![Name::Known("r/x".into()), Name::Known("r/y".into())]
    );
    assert_eq!(
        changed("r", Some(&a), None),
        vec![Name::UnknownBelow("r".into())]
    );
}

/// Keeps the `refs` table it saw at its `capture`-th request and, from
/// then on, attests that remembered table *freshly*: hidden state, not
/// replay. Nothing in the attestation can tell.
struct CachingGit {
    inner: GitEvidenceProvider,
    capture: usize,
    calls: Mutex<usize>,
    cached: Mutex<Option<Term>>,
}

impl EvidenceProvider for CachingGit {
    fn id(&self) -> &str {
        "git"
    }

    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut calls = self.calls.lock().unwrap();
        if keys.iter().any(|k| k.kind == "refs") {
            *calls += 1;
        }
        let mut cached = self.cached.lock().unwrap();
        let mut out = Vec::new();
        for key in keys {
            if key.kind == "refs" {
                if let Some(old) = cached.as_ref() {
                    out.push(Answer::Verified(attestor.attest(
                        (*key).clone(),
                        old.clone(),
                        "cache",
                    )));
                    continue;
                }
            }
            let answers = self.inner.provide(&[key], attestor);
            if key.kind == "refs" && *calls == self.capture {
                if let Some(Answer::Verified(a)) = answers.first() {
                    *cached = Some(a.fact().value.clone());
                }
            }
            out.extend(answers);
        }
        out
    }
}

#[tokio::test]
async fn stale_state_attested_freshly_is_believed_unless_independently_witnessed() {
    // The limit of "providers must not keep hidden state": the layer can
    // bind attestations to requests, not values to the present.
    for witnessed in [false, true] {
        let env = Env::new("caching");
        let caching = CachingGit {
            inner: env.git_provider(),
            capture: 3,
            calls: Mutex::new(0),
            cached: Mutex::default(),
        };
        let second = GitEvidenceProvider::new(
            "git-2",
            &env.work,
            vec![GitRepo {
                path: REPO.into(),
                bare: false,
            }],
        );
        let extra: Extra = if witnessed {
            vec![Box::new(move |r: &Registry| {
                r.register(second, Trust::Attesting);
            })]
        } else {
            Vec::new()
        };
        let registry = env.registry_with(caching, extra);
        let e = env.clone();
        let mut rt = guard(&registry, move |op| {
            e.release(op);
            git(&e.repo(), &["update-ref", "refs/heads/main", &e.x2]);
            Ok(String::new())
        });
        let (_, report) = run(&mut rt, env.op()).await;
        let decision = report.unwrap().decision;
        if witnessed {
            assert_eq!(decision.verdict, Verdict::Blocked, "{decision}");
            assert!(decision.to_string().contains("contradictory"));
        } else {
            assert_eq!(decision.verdict, Verdict::Allow, "false ALLOW: {decision}");
        }
    }
}

// ------------------------------------------------------------ provenance

fn snapshot_of(subject: &v9r_core::runtime::RtSubject<TKey>) -> Option<u64> {
    match subject {
        v9r_core::runtime::RtSubject::Domain(TKey {
            at: v9r_core::temporal::At::Snapshot(id),
            ..
        }) => Some(*id),
        _ => None,
    }
}

#[tokio::test]
async fn snapshot_evidence_carries_its_snapshot_in_lineage() {
    use v9r_core::provenance::{explain, Source};
    let env = Env::new("lineage");
    let registry = env.registry();
    let e = env.clone();
    let mut rt = guard(&registry, move |op| {
        e.release(op);
        Ok(String::new())
    });
    let (_, report) = run(&mut rt, env.op()).await;
    let report = report.unwrap();
    assert!(report.accepted);
    let receipt = report.receipt.as_ref().unwrap();
    let (s0, s1) = (receipt.before.id(), receipt.after.as_ref().unwrap().id());

    let explanation = explain(&report.decision, &registry, snapshot_of);
    assert!(explanation.problems.is_empty(), "{explanation}");
    let snapshots: Vec<Option<u64>> = explanation
        .findings
        .iter()
        .filter_map(|f| match &f.source {
            Source::Lineage(chain) if f.statement.contains("@s") => Some(chain[0].1.snapshot),
            _ => None,
        })
        .collect();
    assert!(snapshots.contains(&Some(s0)) && snapshots.contains(&Some(s1)));
    assert!(snapshots.iter().all(|s| *s == Some(s0) || *s == Some(s1)));

    // Evidence moved between moments: relabel one finding's subject from
    // s0 to s1 (as a faulty layer might). The audit names it.
    let mut moved = report.decision.clone();
    let finding = moved
        .findings
        .iter_mut()
        .find(|f| {
            matches!(&f.obligation.requirement, Requirement::Fact { subject, .. }
                if snapshot_of(subject) == Some(s0))
        })
        .unwrap();
    if let Requirement::Fact { subject, .. } = &mut finding.obligation.requirement {
        *subject = v9r_core::runtime::RtSubject::Domain(TKey {
            at: v9r_core::temporal::At::Snapshot(s1),
            key: refs_key(),
        });
    }
    let audit = explain(&moved, &registry, snapshot_of);
    assert!(
        audit
            .problems
            .iter()
            .any(|p| p.contains(&format!("evidence for snapshot {s1}"))),
        "{audit}"
    );
}
