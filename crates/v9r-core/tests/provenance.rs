//! "Release artifact is valid", explained by the lineage of its evidence,
//! and attacked through provenance.
//!
//! Outside v9r-core: the invariants, the CI provider and every
//! adversarial provider use only the public API.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, Once};

use uuid::Uuid;
use v9r_core::content;
use v9r_core::fs_provider::FilesystemEvidenceProvider;
use v9r_core::git::GitRepo;
use v9r_core::git_provider::GitEvidenceProvider;
use v9r_core::graph::{
    self, Answer, Attested, Attestor, EvidenceProvider, GraphRuntime, Key, Method, Registry, Term,
    Trust,
};
use v9r_core::kernel::{Invariant, Obligation, Phase, Requirement, Strength, Verdict};
use v9r_core::provenance::{explain, Explanation, Source};
use v9r_core::runtime::{Authorize, RtSubject};
use v9r_core::state::ContentHash;

// ------------------------------------------------------------ invariants

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    commit: String,
    /// Witness: the content digest both sides must equal.
    digest: String,
}

const REPO: &str = "repo";
const DIST: &str = "dist";
const COMMIT: &str = "release.commit_exists";
const ARTIFACT: &str = "release.artifact_matches";
const TESTS: &str = "release.tests_passed";

fn hard(invariant: &str, key: Key, value: Term) -> Obligation<Key, Term> {
    Obligation {
        invariant: invariant.into(),
        phase: Phase::Pre,
        requirement: Requirement::Fact {
            subject: key,
            value,
            strength: Strength::Hard,
        },
    }
}

struct CommitExists;
impl Invariant<Release, Key, Term> for CommitExists {
    fn id(&self) -> &str {
        COMMIT
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![hard(
            COMMIT,
            Key::new("commit", [REPO, &r.commit]),
            Term::Bool(true),
        )]
    }
}

struct ArtifactMatches;
impl Invariant<Release, Key, Term> for ArtifactMatches {
    fn id(&self) -> &str {
        ARTIFACT
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![
            hard(
                ARTIFACT,
                Key::new("tree_content", [REPO, &r.commit]),
                Term::Digest(r.digest.clone()),
            ),
            hard(
                ARTIFACT,
                Key::new("dir_content", [DIST]),
                Term::Digest(r.digest.clone()),
            ),
        ]
    }
}

struct TestsPassed;
impl Invariant<Release, Key, Term> for TestsPassed {
    fn id(&self) -> &str {
        TESTS
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![hard(
            TESTS,
            Key::new("tests", [REPO, &r.commit]),
            Term::Text("passed".into()),
        )]
    }
}

// ------------------------------------------------------------ CI (third party)

struct FakeCi {
    passed: String,
}

impl EvidenceProvider for FakeCi {
    fn id(&self) -> &str {
        "ci"
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "tests" && key.args.len() == 2
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter(|k| k.args[1] == self.passed)
            .map(|key| {
                Answer::Verified(
                    attestor
                        .attest(
                            (*key).clone(),
                            Term::Text("passed".into()),
                            "pipeline result",
                        )
                        .observed(format!("pipeline run #7 at {}", key.args[1])),
                )
            })
            .collect()
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

/// Registers the filesystem provider(s) of a scenario.
type AddFs = Box<dyn FnOnce(&Registry)>;

struct Env {
    base: PathBuf,
    work: PathBuf,
    commit: String,
}

impl Env {
    /// `repo` with commit X (README, feature.txt); `dist/` with exactly
    /// X's files.
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("v9r-prov-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        let repo = work.join(REPO);
        fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", REPO]);
        for (file, text) in [("README", "readme\n"), ("feature.txt", "feature\n")] {
            fs::write(repo.join(file), text).unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "release"]);
        let commit = git(&repo, &["rev-parse", "HEAD"]);
        fs::create_dir_all(work.join(DIST)).unwrap();
        fs::write(work.join("dist/README"), "readme\n").unwrap();
        fs::write(work.join("dist/feature.txt"), "feature\n").unwrap();
        Self { base, work, commit }
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

    fn ci(&self) -> FakeCi {
        FakeCi {
            passed: self.commit.clone(),
        }
    }

    /// git, then `fs` (if any), then ci.
    fn registry(&self, fs: Option<AddFs>) -> Registry {
        let registry = Registry::new();
        registry.register(self.git_provider(), Trust::Attesting);
        if let Some(add) = fs {
            add(&registry);
        }
        registry.register(self.ci(), Trust::Attesting);
        registry
    }

    fn honest(&self) -> Registry {
        let fs = self.fs_provider();
        self.registry(Some(Box::new(move |r| {
            r.register(fs, Trust::Attesting);
        })))
    }

    fn release(&self) -> Release {
        let probe = Registry::new();
        probe.register(self.git_provider(), Trust::Attesting);
        let Some(Term::Digest(digest)) =
            probe.query(&Key::new("tree_content", [REPO, self.commit.as_str()]))
        else {
            panic!("no tree digest")
        };
        Release {
            commit: self.commit.clone(),
            digest,
        }
    }

    fn tamper(&self) {
        fs::write(self.work.join("dist/feature.txt"), "tampered\n").unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn define_content(registry: &Registry) {
    registry.define("tree_content", content::DEFINITION);
    registry.define("dir_content", content::DEFINITION);
}

fn runtime(registry: &Registry) -> GraphRuntime<Release> {
    graph::runtime(
        registry.clone(),
        vec![
            Box::new(CommitExists),
            Box::new(ArtifactMatches),
            Box::new(TestsPassed),
        ],
    )
}

fn explained(
    decision: &v9r_core::runtime::RtDecision<v9r_core::graph::GraphDomain<Release>>,
    registry: &Registry,
) -> Explanation {
    explain(decision, registry, |_: &RtSubject<Key>| None)
}

async fn authorize(registry: &Registry, release: Release) -> (Verdict, Explanation) {
    let result = runtime(registry).authorize(release).await.unwrap();
    let explanation = explained(result.decision(), registry);
    (result.verdict(), explanation)
}

/// The direct lineage of the finding about `kind`.
fn lineage_of<'a>(e: &'a Explanation, kind: &str) -> Option<&'a [(usize, graph::Lineage)]> {
    e.findings.iter().find_map(|f| match &f.source {
        Source::Lineage(chain) if chain[0].1.key.kind == kind => Some(chain.as_slice()),
        _ => None,
    })
}

// ------------------------------------------------------------ explanation

#[tokio::test]
async fn release_is_explained_by_the_lineage_of_its_evidence() {
    let env = Env::new("explain");
    let registry = env.honest();
    define_content(&registry);
    let mut rt = runtime(&registry);
    let release = env.release();
    let Authorize::Allowed(auth) = rt.authorize(release.clone()).await.unwrap() else {
        panic!()
    };
    let pre = explained(auth.decision(), &registry);
    // The runtime's own fact is established by the runtime, not a provider.
    assert!(pre.findings.iter().any(|f| matches!(
        &f.source, Source::Layer { observer, .. } if observer == "runtime-verdicts"
    )));
    let report = rt.execute(*auth).await.unwrap();
    assert!(report.accepted);
    let e = explained(&report.decision, &registry);
    println!("{e}");
    assert_eq!(e.verdict, Verdict::Allow);
    assert!(e.problems.is_empty(), "{e}");

    // Who: each fact by the provider that observes it.
    for (kind, provider) in [
        ("commit", "git"),
        ("tree_content", "git"),
        ("dir_content", "fs"),
        ("tests", "ci"),
    ] {
        let chain = lineage_of(&e, kind).unwrap_or_else(|| panic!("{kind}: {e}"));
        assert_eq!(chain[0].1.provider, provider);
        assert!(!chain[0].1.supporting);
    }
    // How: methods and definitions (claimed).
    let tree = lineage_of(&e, "tree_content").unwrap();
    let dir = lineage_of(&e, "dir_content").unwrap();
    assert_eq!(
        tree[0].1.method.definition.as_deref(),
        Some(content::DEFINITION)
    );
    assert_eq!(
        dir[0].1.method.definition.as_deref(),
        Some(content::DEFINITION)
    );
    // Dependencies: tree_content(X) ← commit(X); dir_content ← entries(dist).
    assert_eq!(
        tree[1].1.key,
        Key::new("commit", [REPO, env.commit.as_str()])
    );
    // (commit(X) was also asked, so it is an answer as well as a support.)
    assert_eq!(tree[1].1.request, tree[0].1.request);
    assert_eq!(dir[1].1.key, Key::new("entries", [DIST]));
    assert!(dir[1].1.supporting && dir[1].1.request == dir[0].1.request);
    // When: all from the same collection round (this POST judgment).
    let rounds: Vec<u64> = ["commit", "tree_content", "dir_content", "tests"]
        .iter()
        .map(|k| lineage_of(&e, k).unwrap()[0].1.round)
        .collect();
    assert!(rounds.iter().all(|r| *r == rounds[0]));
    let text = e.to_string();
    for needle in [
        "ALLOW",
        "provider git",
        "provider fs",
        "provider ci",
        "└ depends on",
        "definition v9r-content-manifest/1",
    ] {
        assert!(text.contains(needle), "{needle}\n{text}");
    }
}

// ------------------------------------------------------------ another snapshot

/// Keeps a supporting `entries` attestation from its first request and,
/// later, claims its `dir_content` was derived from it.
struct AnachronisticFs {
    inner: FilesystemEvidenceProvider,
    kept: Mutex<Option<Attested>>,
}

impl EvidenceProvider for AnachronisticFs {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let honest = self.inner.provide(keys, attestor);
        let mut kept = self.kept.lock().unwrap();
        let Some(old) = kept.as_ref() else {
            // First request: answer honestly, keep a spare support.
            for answer in &honest {
                if let Answer::Verified(a) = answer {
                    if a.fact().subject.kind == "entries" {
                        *kept = Some(attestor.attest(
                            a.fact().subject.clone(),
                            a.fact().value.clone(),
                            "tree walk",
                        ));
                    }
                }
            }
            return honest;
        };
        honest
            .into_iter()
            .filter_map(|answer| match answer {
                Answer::Verified(a) if a.fact().subject.kind == "dir_content" => {
                    Some(Answer::Verified(
                        attestor
                            .attest(
                                a.fact().subject.clone(),
                                a.fact().value.clone(),
                                Method::new("content manifest").defined_as(content::DEFINITION),
                            )
                            .depends_on(old),
                    ))
                }
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn evidence_claiming_an_earlier_state_is_refused() {
    let env = Env::new("anachronism");
    let fs = AnachronisticFs {
        inner: env.fs_provider(),
        kept: Mutex::default(),
    };
    let registry = env.registry(Some(Box::new(move |r| {
        r.register(fs, Trust::Attesting);
    })));
    let mut rt = runtime(&registry);
    let Authorize::Allowed(auth) = rt.authorize(env.release()).await.unwrap() else {
        panic!("first request is honest")
    };
    let report = rt.execute(*auth).await.unwrap();
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
        .any(|d| d.key.kind == "dir_content" && d.reason.contains("outside this response")));
}

/// Claims, in free text, to have observed another snapshot.
struct BoastfulFs(FilesystemEvidenceProvider);

impl EvidenceProvider for BoastfulFs {
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
                Answer::Verified(a) => Answer::Verified(a.observed("snapshot s1")),
                other => other,
            })
            .collect()
    }
}

#[tokio::test]
async fn claimed_state_is_shown_as_a_claim_beside_the_established_round() {
    let env = Env::new("boast");
    let fs = BoastfulFs(env.fs_provider());
    let registry = env.registry(Some(Box::new(move |r| {
        r.register(fs, Trust::Attesting);
    })));
    let (verdict, e) = authorize(&registry, env.release()).await;
    // Free-text claims cannot be checked: the fact is accepted…
    assert_eq!(verdict, Verdict::Allow);
    let dir = &lineage_of(&e, "dir_content").unwrap()[0].1;
    // …and its explanation keeps the claim apart from what was established.
    assert!(dir.observed.contains(&"snapshot s1".to_string()));
    assert_eq!(dir.snapshot, None, "no snapshot was established");
}

// ------------------------------------------------------------ stale evidence

/// Answers `dir_content` from what it saw the first time, attested
/// freshly, with its first-seen state as the claimed observation.
struct StaleFs {
    inner: FilesystemEvidenceProvider,
    first: Mutex<Option<(Term, Term, Vec<String>)>>,
}

impl EvidenceProvider for StaleFs {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut first = self.first.lock().unwrap();
        if first.is_none() {
            let honest = self.inner.provide(keys, attestor);
            let mut walk = None;
            let mut digest = None;
            let mut observed = Vec::new();
            for answer in &honest {
                if let Answer::Verified(a) = answer {
                    match a.fact().subject.kind.as_str() {
                        "entries" => walk = Some(a.fact().value.clone()),
                        _ => digest = Some(a.fact().value.clone()),
                    }
                    observed = vec!["tree observed at first request".to_string()];
                }
            }
            *first = walk.zip(digest).map(|(w, d)| (w, d, observed));
            return honest;
        }
        let (walk, digest, observed) = first.clone().unwrap();
        let mut support = attestor.attest(Key::new("entries", [DIST]), walk, "tree walk");
        let mut fact = attestor.attest(
            Key::new("dir_content", [DIST]),
            digest,
            Method::new("content manifest").defined_as(content::DEFINITION),
        );
        for o in observed {
            support = support.observed(o.clone());
            fact = fact.observed(o);
        }
        let fact = fact.depends_on(&support);
        vec![Answer::Verified(support), Answer::Verified(fact)]
    }
}

#[tokio::test]
async fn stale_evidence_attested_freshly_is_believed() {
    let env = Env::new("stale");
    let fs = StaleFs {
        inner: env.fs_provider(),
        first: Mutex::default(),
    };
    let registry = env.registry(Some(Box::new(move |r| {
        r.register(fs, Trust::Attesting);
    })));
    define_content(&registry);
    let mut rt = runtime(&registry);
    let Authorize::Allowed(auth) = rt.authorize(env.release()).await.unwrap() else {
        panic!()
    };
    env.tamper();
    let report = rt.execute(*auth).await.unwrap();
    // A false ALLOW on a tampered artifact, with a consistent-looking
    // explanation: every check provenance can make passes.
    assert!(report.accepted, "{}", report.decision);
    let e = explained(&report.decision, &registry);
    assert!(e.problems.is_empty(), "{e}");
    let dir = lineage_of(&e, "dir_content").unwrap();
    assert_eq!(dir[1].1.key.kind, "entries", "it even shows its work");
    assert_eq!(dir[0].1.observed, ["tree observed at first request"]);
}

// ------------------------------------------------------------ definitions

/// Computes "the digest of dist" as sha256 over its files' bytes, in name
/// order: a different definition of the same kind of fact.
struct TarFs {
    root: PathBuf,
}

impl EvidenceProvider for TarFs {
    fn id(&self) -> &str {
        "tar-fs"
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "dir_content" && key.args.len() == 1
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .map(|key| {
                let dir = self.root.join(&key.args[0]);
                let mut names: Vec<_> = fs::read_dir(&dir)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                names.sort();
                let mut bytes = Vec::new();
                for name in names {
                    bytes.extend(fs::read(dir.join(name)).unwrap());
                }
                Answer::Verified(attestor.attest(
                    (*key).clone(),
                    Term::Digest(ContentHash::of(&bytes).to_string()),
                    Method::new("sha256 of concatenated files").defined_as("concat-sha256/1"),
                ))
            })
            .collect()
    }
}

fn with_tar(env: &Env, honest_fs_too: bool) -> Registry {
    let tar = TarFs {
        root: env.work.clone(),
    };
    let fs = env.fs_provider();
    env.registry(Some(Box::new(move |r| {
        if honest_fs_too {
            r.register(fs, Trust::Attesting);
        }
        r.register(tar, Trust::Attesting);
    })))
}

#[tokio::test]
async fn different_definitions_of_the_same_digest_do_not_combine_silently() {
    let env = Env::new("definitions");
    let release = env.release();

    // Undeclared: the comparison happens and DENIES an artifact that is in
    // fact correct; only the explanation shows why.
    let (verdict, e) = authorize(&with_tar(&env, false), release.clone()).await;
    assert_eq!(verdict, Verdict::Deny, "{e}");
    assert!(
        e.problems
            .iter()
            .any(|p| p.contains("different definitions")),
        "{e}"
    );

    // Declared: the foreign definition is refused; the comparison cannot
    // be made, so BLOCKED, not DENY.
    let registry = with_tar(&env, false);
    define_content(&registry);
    let (verdict, e) = authorize(&registry, release.clone()).await;
    assert_eq!(verdict, Verdict::Blocked, "{e}");
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.provider == "tar-fs" && d.reason.contains("definition mismatch")));

    // Declared, with a conforming provider as well: it alone is used.
    let registry = with_tar(&env, true);
    define_content(&registry);
    let (verdict, e) = authorize(&registry, release.clone()).await;
    assert_eq!(verdict, Verdict::Allow, "{e}");
    assert_eq!(lineage_of(&e, "dir_content").unwrap()[0].1.provider, "fs");

    // Undeclared, both providers: contradictory verified values. BLOCKED,
    // and the kernel's reason carries no provenance to explain it.
    let (verdict, e) = authorize(&with_tar(&env, true), release).await;
    assert_eq!(verdict, Verdict::Blocked, "{e}");
    let finding = e
        .findings
        .iter()
        .find(|f| f.statement.starts_with("dir_content"))
        .unwrap();
    assert_eq!(finding.source, Source::None);
}

// ------------------------------------------------------------ copied evidence

/// A CI system that also "reports" the artifact digest, copied from the
/// filesystem domain's earlier report rather than observed.
struct CopyingCi {
    passed: String,
    copied_digest: String,
}

impl EvidenceProvider for CopyingCi {
    fn id(&self) -> &str {
        "ci"
    }
    fn answers(&self, key: &Key) -> bool {
        (key.kind == "tests" && key.args.len() == 2) || key.kind == "dir_content"
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let (value, method) = match key.kind.as_str() {
                    "tests" if key.args[1] == self.passed => {
                        (Term::Text("passed".into()), Method::new("pipeline result"))
                    }
                    "dir_content" => (
                        Term::Digest(self.copied_digest.clone()),
                        Method::new("artifact digest from build log")
                            .defined_as(content::DEFINITION),
                    ),
                    _ => return None,
                };
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    value,
                    method,
                )))
            })
            .collect()
    }
}

#[tokio::test]
async fn evidence_copied_between_domains_is_caught_only_by_declared_observers() {
    let env = Env::new("copied");
    let release = env.release();
    env.tamper();
    let copying = |registry: &Registry| {
        registry.register(
            CopyingCi {
                passed: env.commit.clone(),
                copied_digest: release.digest.clone(),
            },
            Trust::Attesting,
        );
    };

    // No filesystem observer at all: the copy is believed.
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::Attesting);
    copying(&registry);
    define_content(&registry);
    let (verdict, e) = authorize(&registry, release.clone()).await;
    assert_eq!(
        verdict,
        Verdict::Allow,
        "false ALLOW on a tampered artifact: {e}"
    );
    assert_eq!(lineage_of(&e, "dir_content").unwrap()[0].1.provider, "ci");

    // Declared observers: the copy is refused, the fact is missing.
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::Attesting);
    copying(&registry);
    define_content(&registry);
    registry.restrict("dir_content", &["fs"]);
    let (verdict, _) = authorize(&registry, release.clone()).await;
    assert_eq!(verdict, Verdict::Blocked);
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.provider == "ci" && d.reason.contains("not an observer")));

    // Declared observers and the real one: the tampering is seen.
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::Attesting);
    registry.register(env.fs_provider(), Trust::Attesting);
    copying(&registry);
    define_content(&registry);
    registry.restrict("dir_content", &["fs"]);
    let (verdict, e) = authorize(&registry, release).await;
    assert_eq!(verdict, Verdict::Deny, "{e}");
}

#[test]
fn lineage_tokens_round_trip_through_kernel_text() {
    assert_eq!(
        graph::lineage_ids("x (lineage:L12); y lineage:L3"),
        vec![12, 3]
    );
    assert_eq!(graph::lineage_token(7), "lineage:L7");
}
