//! Composite invariant over independent evidence providers.
//!
//! Everything in this file is outside v9r-core and uses only its public
//! API: the release invariants are written against `graph::{Key, Term}`
//! alone, and the CI provider is a third-party provider.
//!
//! "Release is allowed" requires:
//! 1. git: the release commit is what the release ref points at and
//!    descends from the approved base;
//! 2. filesystem + git: the artifact's content equals the commit's tree
//!    (∃D witnessed by the proposal);
//! 3. CI: the tests passed for this exact commit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, Once};

use uuid::Uuid;
use v9r_core::fs_provider::FilesystemEvidenceProvider;
use v9r_core::git::GitRepo;
use v9r_core::git_provider::GitEvidenceProvider;
use v9r_core::graph::{
    self, Answer, Attested, Attestor, EvidenceProvider, GraphDomain, GraphRuntime, Key, Registry,
    Term, Trust,
};
use v9r_core::kernel::{
    Decision, Invariant, Obligation, Phase, Requirement, Status, Strength, Verdict,
};
use v9r_core::runtime::{Authorize, Report};

// ------------------------------------------------------------ invariants
// Written without knowing which provider answers what.

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    repo: String,
    commit: String,
    artifact: String,
    /// Witness: the content digest both sides must equal.
    digest: String,
}

fn hard(invariant: &str, key: Key, value: Term) -> Obligation<Key, Term> {
    Obligation {
        invariant: invariant.to_string(),
        phase: Phase::Pre,
        requirement: Requirement::Fact {
            subject: key,
            value,
            strength: Strength::Hard,
        },
    }
}

const COMMIT_VERIFIED: &str = "release.commit_verified";
const ARTIFACT_MATCHES: &str = "release.artifact_matches";
const TESTS_PASSED: &str = "release.tests_passed";

struct CommitVerified {
    release_ref: String,
    approved_base: String,
}

impl Invariant<Release, Key, Term> for CommitVerified {
    fn id(&self) -> &str {
        COMMIT_VERIFIED
    }

    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![
            hard(
                COMMIT_VERIFIED,
                Key::new("ref", [&r.repo, &self.release_ref]),
                Term::Id(r.commit.clone()),
            ),
            hard(
                COMMIT_VERIFIED,
                Key::new("descends", [&r.repo, &self.approved_base, &r.commit]),
                Term::Bool(true),
            ),
        ]
    }
}

struct ArtifactMatches;

impl Invariant<Release, Key, Term> for ArtifactMatches {
    fn id(&self) -> &str {
        ARTIFACT_MATCHES
    }

    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![
            hard(
                ARTIFACT_MATCHES,
                Key::new("tree_content", [&r.repo, &r.commit]),
                Term::Digest(r.digest.clone()),
            ),
            hard(
                ARTIFACT_MATCHES,
                Key::new("dir_content", [&r.artifact]),
                Term::Digest(r.digest.clone()),
            ),
        ]
    }
}

struct TestsPassed;

impl Invariant<Release, Key, Term> for TestsPassed {
    fn id(&self) -> &str {
        TESTS_PASSED
    }

    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![hard(
            TESTS_PASSED,
            Key::new("tests", [&r.repo, &r.commit]),
            Term::Text("passed".into()),
        )]
    }
}

// ------------------------------------------------------------ third-party CI

/// A CI system's results, by (repo, commit). A third-party provider:
/// it uses only v9r-core's public API.
struct FakeCi {
    id: String,
    results: Mutex<BTreeMap<(String, String), bool>>,
}

impl FakeCi {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            results: Mutex::default(),
        }
    }

    fn with(self, repo: &str, commit: &str, passed: bool) -> Self {
        self.results
            .lock()
            .unwrap()
            .insert((repo.to_string(), commit.to_string()), passed);
        self
    }
}

fn is_tests_key(key: &Key) -> bool {
    key.kind == "tests" && key.args.len() == 2
}

fn outcome(passed: bool) -> Term {
    Term::Text(if passed { "passed" } else { "failed" }.into())
}

impl EvidenceProvider for FakeCi {
    fn id(&self) -> &str {
        &self.id
    }

    fn answers(&self, key: &Key) -> bool {
        is_tests_key(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let results = self.results.lock().unwrap();
        keys.iter()
            .filter_map(|key| {
                let passed = results.get(&(key.args[0].clone(), key.args[1].clone()))?;
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    outcome(*passed),
                    format!("{} build record", self.id),
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

struct Env {
    base: PathBuf,
    work: PathBuf,
    approved_base: String,
    head: String,
}

const REPO: &str = "repo";
const RELEASE_REF: &str = "refs/heads/release";

impl Env {
    /// `repo` with an approved base and one feature commit on `release`,
    /// and `dist/` holding exactly that commit's files.
    fn new(name: &str) -> Self {
        hermetic_git();
        let base = std::env::temp_dir().join(format!("v9r-graph-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        let repo = work.join(REPO);
        fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", REPO]);
        fs::write(repo.join("README"), "approved base\n").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "-q", "-m", "approved base"]);
        let approved_base = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "-b", "release"]);
        fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        git(&repo, &["add", "feature.txt"]);
        git(&repo, &["commit", "-q", "-m", "feature"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        fs::create_dir_all(work.join("dist")).unwrap();
        fs::write(work.join("dist/README"), "approved base\n").unwrap();
        fs::write(work.join("dist/feature.txt"), "feature\n").unwrap();
        Self {
            base,
            work,
            approved_base,
            head,
        }
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

    fn fs_provider(&self) -> FilesystemEvidenceProvider {
        FilesystemEvidenceProvider::new("fs", &self.work)
    }

    fn ci(&self, passed: bool) -> FakeCi {
        FakeCi::new("ci").with(REPO, &self.head, passed)
    }

    /// Registry with the three honest providers, minus `without`.
    fn registry(&self, without: &[&str]) -> Registry {
        let registry = Registry::new();
        if !without.contains(&"git") {
            registry.register(self.git_provider(), Trust::Attesting);
        }
        if !without.contains(&"fs") {
            registry.register(self.fs_provider(), Trust::Attesting);
        }
        if !without.contains(&"ci") {
            registry.register(self.ci(true), Trust::Attesting);
        }
        registry
    }

    /// The honest witness, as a planner would obtain it.
    fn digest(&self) -> String {
        let probe = Registry::new();
        probe.register(self.git_provider(), Trust::Attesting);
        match probe.query(&Key::new("tree_content", [REPO, self.head.as_str()])) {
            Some(Term::Digest(d)) => d,
            other => panic!("tree_content: {other:?}"),
        }
    }

    fn release(&self) -> Release {
        Release {
            repo: REPO.into(),
            commit: self.head.clone(),
            artifact: "dist".into(),
            digest: self.digest(),
        }
    }

    fn invariants(&self) -> Vec<Box<dyn Invariant<Release, Key, Term> + Send + Sync>> {
        vec![
            Box::new(CommitVerified {
                release_ref: RELEASE_REF.into(),
                approved_base: self.approved_base.clone(),
            }),
            Box::new(ArtifactMatches),
            Box::new(TestsPassed),
        ]
    }

    fn runtime(&self, registry: &Registry) -> GraphRuntime<Release> {
        graph::runtime(registry.clone(), self.invariants())
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

type Rep = Report<GraphDomain<Release>>;

async fn run(rt: &mut GraphRuntime<Release>, release: Release) -> (Verdict, Option<Rep>) {
    match rt.authorize(release).await.unwrap() {
        Authorize::Allowed(auth) => (Verdict::Allow, Some(rt.execute(*auth).await.unwrap())),
        other => (other.verdict(), None),
    }
}

fn statuses<'a>(
    decision: &'a Decision<impl Sized, impl Sized>,
    invariant: &str,
) -> Vec<&'a Status> {
    decision
        .findings
        .iter()
        .filter(|f| f.obligation.invariant == invariant)
        .map(|f| &f.status)
        .collect()
}

fn all_satisfied(decision: &Decision<impl Sized, impl Sized>, invariant: &str) -> bool {
    let s = statuses(decision, invariant);
    !s.is_empty() && s.iter().all(|s| matches!(s, Status::Satisfied(_)))
}

fn any_undetermined(decision: &Decision<impl Sized, impl Sized>, invariant: &str) -> bool {
    statuses(decision, invariant)
        .iter()
        .any(|s| matches!(s, Status::Undetermined(_)))
}

fn any_violated(decision: &Decision<impl Sized, impl Sized>, invariant: &str) -> bool {
    statuses(decision, invariant)
        .iter()
        .any(|s| matches!(s, Status::Violated(_)))
}

fn none_violated(decision: &Decision<impl Sized, impl Sized>) -> bool {
    decision.violated().next().is_none()
}

// ------------------------------------------------------------ phase 4

#[tokio::test]
async fn release_is_allowed_when_three_independent_providers_establish_it() {
    let env = Env::new("honest");
    let registry = env.registry(&[]);
    let mut rt = env.runtime(&registry);
    let release = env.release();
    let (pre, report) = run(&mut rt, release.clone()).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(report.accepted, "{}", report.decision);
    assert_eq!(report.output, Some(release));
    for invariant in [COMMIT_VERIFIED, ARTIFACT_MATCHES, TESTS_PASSED] {
        assert!(all_satisfied(&report.decision, invariant), "{invariant}");
    }
    // Each fact was established by the provider that answers its kind.
    let text = report.decision.to_string();
    for observer in ["provider:git", "provider:fs", "provider:ci"] {
        assert!(text.contains(observer), "{text}");
    }
    assert!(registry.discarded().is_empty());
}

#[tokio::test]
async fn removing_any_provider_blocks_with_missing_evidence_never_denies() {
    let env = Env::new("remove");
    let release = env.release();
    for (missing, kinds, invariants) in [
        (
            "git",
            vec!["ref", "descends", "tree_content"],
            vec![COMMIT_VERIFIED, ARTIFACT_MATCHES],
        ),
        ("fs", vec!["dir_content"], vec![ARTIFACT_MATCHES]),
        ("ci", vec!["tests"], vec![TESTS_PASSED]),
    ] {
        let registry = env.registry(&[missing]);
        let mut rt = env.runtime(&registry);
        let result = rt.authorize(release.clone()).await.unwrap();
        let decision = result.decision();
        assert_eq!(
            result.verdict(),
            Verdict::Blocked,
            "without {missing}:\n{decision}"
        );
        assert!(none_violated(decision), "without {missing}:\n{decision}");
        for invariant in &invariants {
            assert!(
                any_undetermined(decision, invariant),
                "{missing}/{invariant}"
            );
        }
        // The plan names exactly what nobody can answer.
        let keys: Vec<Key> = decision
            .findings
            .iter()
            .filter_map(|f| match &f.obligation.requirement {
                Requirement::Fact {
                    subject: v9r_core::runtime::RtSubject::Domain(key),
                    ..
                } => Some(key.clone()),
                _ => None,
            })
            .collect();
        let plan = registry.plan(&keys);
        let mut unresolved: Vec<&str> = plan.unresolved().iter().map(|k| k.kind.as_str()).collect();
        unresolved.dedup();
        assert_eq!(unresolved.len(), kinds.len(), "{missing}: {unresolved:?}");
        for kind in kinds {
            assert!(unresolved.contains(&kind), "{missing}: {unresolved:?}");
        }
    }
}

#[tokio::test]
async fn verified_negative_evidence_denies() {
    let env = Env::new("negative");
    let release = env.release();

    // CI ran on this commit and failed.
    let registry = env.registry(&["ci"]);
    registry.register(env.ci(false), Trust::Attesting);
    let result = env
        .runtime(&registry)
        .authorize(release.clone())
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(any_violated(result.decision(), TESTS_PASSED));

    // A commit that does not descend from the approved base.
    let repo = env.work.join(REPO);
    git(&repo, &["checkout", "-q", "--orphan", "rogue"]);
    git(&repo, &["commit", "-q", "-m", "rogue"]);
    let rogue = git(&repo, &["rev-parse", "HEAD"]);
    let registry = env.registry(&["ci"]);
    registry.register(FakeCi::new("ci").with(REPO, &rogue, true), Trust::Attesting);
    let result = env
        .runtime(&registry)
        .authorize(Release {
            commit: rogue,
            ..release.clone()
        })
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny, "{}", result.decision());
    assert!(any_violated(result.decision(), COMMIT_VERIFIED));

    // An artifact that is not the commit's content.
    fs::write(env.work.join("dist/feature.txt"), "tampered\n").unwrap();
    let registry = env.registry(&[]);
    let result = env.runtime(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(any_violated(result.decision(), ARTIFACT_MATCHES));
}

#[test]
fn providers_know_nothing_of_each_other_the_lifecycle_or_invariants() {
    for (name, source) in [
        ("fs_provider", include_str!("../src/fs_provider.rs")),
        ("git_provider", include_str!("../src/git_provider.rs")),
    ] {
        let imports: Vec<&str> = source
            .lines()
            .filter(|l| l.trim_start().starts_with("use crate::"))
            .collect();
        for forbidden in [
            "runtime",
            "kernel",
            "policy",
            "guarded",
            "git_guard",
            "facts",
            "fs_provider",
            "git_provider",
            "GraphDomain",
            "Registry",
        ] {
            assert!(
                !imports.iter().any(|l| l.contains(forbidden)),
                "{name} imports {forbidden}: {imports:?}"
            );
        }
    }
}

// ------------------------------------------------------------ phase 5

/// The real git provider's answers, passed off as mere claims.
struct ClaimingGit(GitEvidenceProvider);

impl EvidenceProvider for ClaimingGit {
    fn id(&self) -> &str {
        "git"
    }

    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        self.0
            .provide(keys, attestor)
            .into_iter()
            .filter_map(|answer| match answer {
                Answer::Verified(attested) => Some(Answer::Proposed {
                    key: attested.fact().subject.clone(),
                    value: attested.fact().value.clone(),
                    note: "trust me".into(),
                }),
                Answer::Proposed { .. } => None,
            })
            .collect()
    }
}

/// Attests that every commit descends from every base.
struct LyingGit {
    id: String,
    inner: GitEvidenceProvider,
}

impl EvidenceProvider for LyingGit {
    fn id(&self) -> &str {
        &self.id
    }

    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .map(|key| {
                if key.kind == "descends" {
                    Answer::Verified(attestor.attest((*key).clone(), Term::Bool(true), "made up"))
                } else {
                    // Delegate honestly for everything else.
                    let mut honest = self.inner.provide(&[key], attestor);
                    honest.pop().unwrap()
                }
            })
            .collect()
    }
}

/// Asked about refs, also volunteers a CI verdict nobody asked it for.
struct ScopeCreep(GitEvidenceProvider, String);

impl EvidenceProvider for ScopeCreep {
    fn id(&self) -> &str {
        "git"
    }

    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut answers = self.0.provide(keys, attestor);
        answers.push(Answer::Verified(attestor.attest(
            Key::new("tests", [REPO, self.1.as_str()]),
            outcome(true),
            "volunteered",
        )));
        answers
    }
}

/// Asked about one commit, reports a pass for another.
struct MisattributingCi {
    passed_commit: String,
}

impl EvidenceProvider for MisattributingCi {
    fn id(&self) -> &str {
        "ci"
    }

    fn answers(&self, key: &Key) -> bool {
        is_tests_key(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .map(|key| {
                Answer::Verified(attestor.attest(
                    Key::new("tests", [key.args[0].as_str(), &self.passed_commit]),
                    outcome(true),
                    "build record",
                ))
            })
            .collect()
    }
}

/// Keeps a spare attestation from its first answer and replays it later.
struct ReplayingCi {
    commit: String,
    spare: Mutex<Option<Attested>>,
}

impl EvidenceProvider for ReplayingCi {
    fn id(&self) -> &str {
        "ci"
    }

    fn answers(&self, key: &Key) -> bool {
        is_tests_key(key)
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut spare = self.spare.lock().unwrap();
        if let Some(old) = spare.take() {
            return vec![Answer::Verified(old)];
        }
        let key = Key::new("tests", [REPO, self.commit.as_str()]);
        *spare = Some(attestor.attest(key.clone(), outcome(true), "build record"));
        let _ = keys;
        vec![Answer::Verified(attestor.attest(
            key,
            outcome(true),
            "build record",
        ))]
    }
}

async fn authorized(
    rt: &mut GraphRuntime<Release>,
    release: Release,
) -> v9r_core::runtime::Authorization<GraphDomain<Release>> {
    match rt.authorize(release).await.unwrap() {
        Authorize::Allowed(auth) => *auth,
        other => panic!("{:?}", other.decision().to_string()),
    }
}

// 1. The git provider lies: it returns claims, not verified evidence.
#[tokio::test]
async fn git_claims_instead_of_evidence_block() {
    let env = Env::new("claims");
    let release = env.release();

    let registry = env.registry(&["git"]);
    registry.register(ClaimingGit(env.git_provider()), Trust::Attesting);
    let result = env
        .runtime(&registry)
        .authorize(release.clone())
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    assert!(result.decision().to_string().contains("proposed"));

    // The same provider registered as claims-only: its attestations are
    // downgraded.
    let registry = env.registry(&["git"]);
    registry.register(env.git_provider(), Trust::ClaimsOnly);
    let result = env.runtime(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
}

// 1b. A provider attests outside what it was asked.
#[tokio::test]
async fn volunteered_attestations_are_discarded() {
    let env = Env::new("scope");
    let registry = env.registry(&["git", "ci"]);
    registry.register(
        ScopeCreep(env.git_provider(), env.head.clone()),
        Trust::Attesting,
    );
    let result = env
        .runtime(&registry)
        .authorize(env.release())
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(any_undetermined(result.decision(), TESTS_PASSED));
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.key.kind == "tests" && d.reason.contains("not asked")));
}

// 1c. An *attesting* provider that lies outright is believed, unless an
// independent provider contradicts it.
#[tokio::test]
async fn a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness() {
    let env = Env::new("liar");
    let repo = env.work.join(REPO);
    git(&repo, &["checkout", "-q", "--orphan", "rogue"]);
    git(&repo, &["commit", "-q", "-m", "rogue"]);
    let rogue = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "-f", "release", &rogue]);
    // `--orphan` keeps the index: the rogue commit has dist's files.
    let liar = |id: &str| LyingGit {
        id: id.into(),
        inner: env.git_provider(),
    };
    let probe = Registry::new();
    probe.register(env.git_provider(), Trust::Attesting);
    let Some(Term::Digest(digest)) = probe.query(&Key::new("tree_content", [REPO, rogue.as_str()]))
    else {
        panic!("no manifest for the rogue commit");
    };
    let release = Release {
        repo: REPO.into(),
        commit: rogue.clone(),
        artifact: "dist".into(),
        digest,
    };
    let ci = || FakeCi::new("ci").with(REPO, &rogue, true);

    let registry = Registry::new();
    registry.register(liar("git"), Trust::Attesting);
    registry.register(env.fs_provider(), Trust::Attesting);
    registry.register(ci(), Trust::Attesting);
    let result = env
        .runtime(&registry)
        .authorize(release.clone())
        .await
        .unwrap();
    assert_eq!(
        result.verdict(),
        Verdict::Allow,
        "registration is the trust decision: {}",
        result.decision()
    );

    registry.register(
        GitEvidenceProvider::new(
            "git-2",
            &env.work,
            vec![GitRepo {
                path: REPO.into(),
                bare: false,
            }],
        ),
        Trust::Attesting,
    );
    let result = env.runtime(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(result.decision().to_string().contains("contradictory"));
}

// 2. The filesystem changes after authorization.
#[tokio::test]
async fn artifact_changed_after_authorization_is_refused() {
    let env = Env::new("fs-stale");
    let registry = env.registry(&[]);
    let mut rt = env.runtime(&registry);
    let auth = authorized(&mut rt, env.release()).await;
    fs::write(env.work.join("dist/feature.txt"), "swapped\n").unwrap();
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed && report.output.is_none());
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(any_violated(&report.decision, ARTIFACT_MATCHES));
}

// 3. CI claims success for another commit.
#[tokio::test]
async fn ci_success_for_another_commit_does_not_count() {
    let env = Env::new("ci-other");
    let release = env.release();

    let registry = env.registry(&["ci"]);
    registry.register(
        MisattributingCi {
            passed_commit: env.approved_base.clone(),
        },
        Trust::Attesting,
    );
    let result = env
        .runtime(&registry)
        .authorize(release.clone())
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(any_undetermined(result.decision(), TESTS_PASSED));
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.key.args[1] == env.approved_base));

    // An honest CI that only has a result for the base commit.
    let registry = env.registry(&["ci"]);
    registry.register(
        FakeCi::new("ci").with(REPO, &env.approved_base, true),
        Trust::Attesting,
    );
    let result = env.runtime(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked);
}

// 4. Two providers disagree.
#[tokio::test]
async fn disagreeing_providers_block() {
    let env = Env::new("disagree");
    let registry = env.registry(&["ci"]);
    registry.register(
        FakeCi::new("ci-a").with(REPO, &env.head, true),
        Trust::Attesting,
    );
    registry.register(
        FakeCi::new("ci-b").with(REPO, &env.head, false),
        Trust::Attesting,
    );
    let result = env
        .runtime(&registry)
        .authorize(env.release())
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    assert!(result.decision().to_string().contains("contradictory"));
}

// 5. Evidence becomes stale: the world moves, or an old attestation is
// replayed.
#[tokio::test]
async fn stale_evidence_is_refused() {
    let env = Env::new("stale");
    let registry = env.registry(&[]);
    let mut rt = env.runtime(&registry);
    let auth = authorized(&mut rt, env.release()).await;
    git(
        &env.work.join(REPO),
        &["commit", "-q", "--allow-empty", "-m", "later"],
    );
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(any_violated(&report.decision, COMMIT_VERIFIED));

    let env = Env::new("replay");
    let registry = env.registry(&["ci"]);
    registry.register(
        ReplayingCi {
            commit: env.head.clone(),
            spare: Mutex::default(),
        },
        Trust::Attesting,
    );
    let mut rt = env.runtime(&registry);
    let auth = authorized(&mut rt, env.release()).await;
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.reason.contains("another provider or request")));
}

// 6. A provider disappears between authorization and execution.
#[tokio::test]
async fn provider_disappearing_before_execution_blocks() {
    let env = Env::new("vanish");
    let registry = env.registry(&[]);
    let mut rt = env.runtime(&registry);
    let auth = authorized(&mut rt, env.release()).await;
    assert!(registry.remove("ci"));
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed && report.output.is_none());
    assert_eq!(report.decision.verdict, Verdict::Blocked);
    assert!(none_violated(&report.decision));
    // Refusing a stale declaration changes nothing, so nothing is held.
    assert!(rt.is_accepting());
}
