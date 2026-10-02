//! The claim's negative cases, on a filesystem-only fixture.
//!
//! Debloat Phase 1: these guarantees were tested only in suites that need
//! the git domain or `fs_provider` (`tests/graph.rs`, `tests/temporal.rs`,
//! `tests/verifiable.rs`, `tests/provenance.rs`). Each test below ports
//! one of them, named in its comment, onto the evidence the claim
//! actually uses: `RawFsObserver` → `FsSnapshot` (`snapshot`,
//! `snapshot_entries`) → `ObjectStore`, plus a stand-in test runner
//! (`tests(tree)`), as in the demo.
//!
//! Two runtimes, both unchanged:
//! * `graph::runtime`: a declaration judged twice on fresh evidence (the
//!   registry's binding, trust and kind rules);
//! * `temporal::runtime`: a transition judged on snapshots before and
//!   after (pairing, consistency, freshness, hold).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use v9r_core::fs_raw::RawFsObserver;
use v9r_core::graph::{
    self, Answer, Attested, Attestor, EvidenceProvider, Key, Method, Registry, Term, Trust,
};
use v9r_core::kernel::{
    Decision, Invariant, Name, Obligation, Phase, Requirement, Status, Strength, Verdict,
};
use v9r_core::provenance::{explain, Source};
use v9r_core::runtime::{Authorize, RtSubject};
use v9r_core::snapshot::{FsSnapshot, ObjectStore};
use v9r_core::temporal::{
    self, changed, pin, Actor, At, Snapshot, TKey, TObligation, Transition, TransitionInvariant,
};

// ------------------------------------------------------------ fixture

const DIST: &str = "dist";
const ARTIFACT: &str = "artifact_matches_approved";
const TESTS: &str = "tests_passed";

fn snapshot_key(dir: &str) -> Key {
    Key::new("snapshot", [dir])
}

fn tests_key(tree: &str) -> Key {
    Key::new("tests", [tree])
}

struct Env {
    base: PathBuf,
    store: ObjectStore,
}

impl Env {
    /// `dist/` (README, src/lib.txt) and `ws/` (README, src/a.py).
    fn new(tag: &str) -> Env {
        let base = std::env::temp_dir().join(format!("v9r-claim-{tag}-{}", uuid::Uuid::new_v4()));
        for root in ["dist", "ws"] {
            fs::create_dir_all(base.join(root).join("src")).unwrap();
            fs::write(base.join(root).join("README"), "read me\n").unwrap();
        }
        fs::write(base.join("dist/src/lib.txt"), "library\n").unwrap();
        fs::write(base.join("ws/src/a.py"), "a = 1\n").unwrap();
        Env {
            base: fs::canonicalize(base).unwrap(),
            store: ObjectStore::new(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn raw(&self, id: &str) -> RawFsObserver {
        RawFsObserver::new(id, &self.base)
    }

    /// The snapshot verifier and nothing else.
    fn bare(&self) -> Registry {
        let r = Registry::new();
        r.add_verifier(FsSnapshot::new(self.store.clone()));
        r
    }

    /// Raw observer `raw` (attesting) and the snapshot verifier.
    fn observed(&self) -> Registry {
        let r = self.bare();
        r.register(self.raw("raw"), Trust::Attesting);
        r
    }

    fn approve(&self) -> String {
        match self.observed().query(&snapshot_key(DIST)) {
            Some(Term::Id(id)) => id,
            other => panic!("approval: {other:?}"),
        }
    }

    /// Raw observer, snapshot verifier and a test runner that passed
    /// `approved`.
    fn standard(&self, approved: &str) -> Registry {
        let r = self.observed();
        r.register(Ci::new("ci").with(approved, true), Trust::Attesting);
        r
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// A stand-in test runner: `tests(tree)` = passed/failed, for the trees
/// it has results for.
#[derive(Clone)]
struct Ci {
    id: String,
    results: BTreeMap<String, bool>,
}

impl Ci {
    fn new(id: &str) -> Ci {
        Ci {
            id: id.into(),
            results: BTreeMap::new(),
        }
    }
    fn with(mut self, tree: &str, passed: bool) -> Ci {
        self.results.insert(tree.into(), passed);
        self
    }
}

fn outcome(passed: bool) -> Term {
    Term::Text(if passed { "passed" } else { "failed" }.into())
}

impl EvidenceProvider for Ci {
    fn id(&self) -> &str {
        &self.id
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "tests" && key.args.len() == 1
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let passed = *self.results.get(&key.args[0])?;
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    outcome(passed),
                    "test run",
                )))
            })
            .collect()
    }
}

// ------------------------------------------------------------ declarations (graph runtime)

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    approved: String,
}

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

/// `snapshot(dist)` is the approved tree.
struct ArtifactMatches;

impl Invariant<Release, Key, Term> for ArtifactMatches {
    fn id(&self) -> &str {
        ARTIFACT
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![hard(ARTIFACT, snapshot_key(DIST), Term::Id(r.approved.clone()))]
    }
}

/// The tests passed on the approved tree.
struct TestsPassed;

impl Invariant<Release, Key, Term> for TestsPassed {
    fn id(&self) -> &str {
        TESTS
    }
    fn obligations(&self, r: &Release) -> Vec<Obligation<Key, Term>> {
        vec![hard(TESTS, tests_key(&r.approved), outcome(true))]
    }
}

type Rt = graph::GraphRuntime<Release>;

fn declare(registry: &Registry) -> Rt {
    graph::runtime(
        registry.clone(),
        vec![Box::new(ArtifactMatches), Box::new(TestsPassed)],
    )
}

async fn authorized(rt: &mut Rt, release: Release) -> v9r_core::runtime::Authorization<graph::GraphDomain<Release>> {
    match rt.authorize(release).await.unwrap() {
        Authorize::Allowed(auth) => *auth,
        other => panic!("{}", other.decision()),
    }
}

fn statuses<'a, S, V>(d: &'a Decision<S, V>, invariant: &str) -> Vec<&'a Status> {
    d.findings
        .iter()
        .filter(|f| f.obligation.invariant == invariant)
        .map(|f| &f.status)
        .collect()
}

fn violated<S, V>(d: &Decision<S, V>, invariant: &str) -> bool {
    statuses(d, invariant)
        .iter()
        .any(|s| matches!(s, Status::Violated(_)))
}

fn undetermined<S, V>(d: &Decision<S, V>, invariant: &str) -> bool {
    statuses(d, invariant)
        .iter()
        .any(|s| matches!(s, Status::Undetermined(_)))
}

fn none_violated<S, V>(d: &Decision<S, V>) -> bool {
    d.violated().next().is_none()
}

fn discarded(registry: &Registry, needle: &str) -> bool {
    registry
        .discarded()
        .iter()
        .any(|d| d.reason.contains(needle))
}

#[tokio::test]
async fn honest_release_is_allowed_by_independent_observers() {
    // Ports graph.rs::release_is_allowed_when_three_independent_providers_establish_it.
    let env = Env::new("honest");
    let approved = env.approve();
    let registry = env.standard(&approved);
    let mut rt = declare(&registry);
    let release = Release { approved };
    let auth = authorized(&mut rt, release.clone()).await;
    let report = rt.execute(auth).await.unwrap();
    assert!(report.accepted, "{}", report.decision);
    assert_eq!(report.output, Some(release));
    let text = report.decision.to_string();
    for observer in ["verifier:fs-snapshot", "provider:ci", "trusts provider:raw"] {
        assert!(text.contains(observer), "{text}");
    }
    assert!(registry.discarded().is_empty(), "{:?}", registry.discarded());
}

#[tokio::test]
async fn removing_any_observer_blocks_with_missing_evidence_never_denies() {
    // Ports graph.rs::removing_any_provider_blocks_with_missing_evidence_never_denies.
    let env = Env::new("remove");
    let approved = env.approve();
    let release = Release {
        approved: approved.clone(),
    };
    // Without the filesystem observer: no snapshot.
    let registry = env.bare();
    registry.register(Ci::new("ci").with(&approved, true), Trust::Attesting);
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    assert!(undetermined(result.decision(), ARTIFACT));
    let plan = registry.plan([&Key::new("fs_dir", [DIST])]);
    assert_eq!(plan.unresolved().len(), 1);
    // Without the test runner: no test result.
    let registry = env.observed();
    let result = declare(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    assert!(undetermined(result.decision(), TESTS));
    let plan = registry.plan([&tests_key(&approved)]);
    assert_eq!(plan.unresolved(), vec![&tests_key(&approved)]);
}

#[tokio::test]
async fn verified_negative_evidence_denies() {
    // Ports graph.rs::verified_negative_evidence_denies.
    let env = Env::new("negative");
    let approved = env.approve();
    let release = Release {
        approved: approved.clone(),
    };
    // The tests ran on this tree and failed.
    let registry = env.observed();
    registry.register(Ci::new("ci").with(&approved, false), Trust::Attesting);
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(violated(result.decision(), TESTS));
    // The artifact is no longer the approved tree.
    fs::write(env.path("dist/feature.txt"), "tampered\n").unwrap();
    let result = declare(&env.standard(&approved))
        .authorize(release)
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(violated(result.decision(), ARTIFACT));
}

/// Answers with claims instead of attestations.
struct ClaimingCi(Ci);

impl EvidenceProvider for ClaimingCi {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }
    fn provide(&self, keys: &[&Key], _: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let passed = *self.0.results.get(&key.args[0])?;
                Some(Answer::Proposed {
                    key: (*key).clone(),
                    value: outcome(passed),
                    note: "trust me".into(),
                })
            })
            .collect()
    }
}

#[tokio::test]
async fn claims_instead_of_evidence_block() {
    // Ports graph.rs::git_claims_instead_of_evidence_block and the
    // untrusted-observer half of verifiable.rs::third_party_object_mirror_is_safe_and_third_party_fs_is_not.
    let env = Env::new("claims");
    let approved = env.approve();
    let release = Release {
        approved: approved.clone(),
    };
    // A runner that only claims.
    let registry = env.observed();
    registry.register(ClaimingCi(Ci::new("ci").with(&approved, true)), Trust::Attesting);
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    assert!(result.decision().to_string().contains("proposed"));
    // An honest runner registered as claims-only: downgraded.
    let registry = env.observed();
    registry.register(Ci::new("ci").with(&approved, true), Trust::ClaimsOnly);
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    // The filesystem observer registered as claims-only: the snapshot
    // rests on unvouched inputs, so it is only a claim.
    let registry = env.bare();
    registry.register(env.raw("raw"), Trust::ClaimsOnly);
    registry.register(Ci::new("ci").with(&approved, true), Trust::Attesting);
    let result = declare(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(undetermined(result.decision(), ARTIFACT));
    assert!(result.decision().to_string().contains("unvouched"));
}

/// The filesystem observer, which also attests a test result nobody
/// asked it for.
struct ScopeCreep(RawFsObserver, String);

impl EvidenceProvider for ScopeCreep {
    fn id(&self) -> &str {
        "raw"
    }
    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut out = self.0.provide(keys, attestor);
        out.push(Answer::Verified(attestor.attest(
            tests_key(&self.1),
            outcome(true),
            "volunteered",
        )));
        out
    }
}

#[tokio::test]
async fn volunteered_attestations_are_discarded() {
    // Ports graph.rs::volunteered_attestations_are_discarded.
    let env = Env::new("scope");
    let approved = env.approve();
    let registry = env.bare();
    registry.register(ScopeCreep(env.raw("raw"), approved.clone()), Trust::Attesting);
    let result = declare(&registry)
        .authorize(Release { approved })
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(undetermined(result.decision(), TESTS));
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.key.kind == "tests" && d.reason.contains("not asked")));
}

#[tokio::test]
async fn a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness() {
    // Ports graph.rs::a_trusted_liar_is_believed_alone_and_blocked_by_an_independent_witness.
    // The tests really fail; the registered runner says they passed.
    let env = Env::new("liar");
    let approved = env.approve();
    let release = Release {
        approved: approved.clone(),
    };
    let registry = env.observed();
    registry.register(Ci::new("ci").with(&approved, true), Trust::Attesting);
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(
        result.verdict(),
        Verdict::Allow,
        "registration is the trust decision: {}",
        result.decision()
    );
    registry.register(Ci::new("ci-2").with(&approved, false), Trust::Attesting);
    let result = declare(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(result.decision().to_string().contains("contradictory"));
}

#[tokio::test]
async fn artifact_changed_after_authorization_is_refused() {
    // Ports graph.rs::artifact_changed_after_authorization_is_refused and
    // the first half of graph.rs::stale_evidence_is_refused.
    let env = Env::new("changed");
    let approved = env.approve();
    let registry = env.standard(&approved);
    let mut rt = declare(&registry);
    let auth = authorized(&mut rt, Release { approved }).await;
    fs::write(env.path("dist/feature.txt"), "swapped\n").unwrap();
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed && report.output.is_none());
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(violated(&report.decision, ARTIFACT));
}

/// Asked about one tree, attests a result for another.
struct MisattributingCi {
    passed_tree: String,
}

impl EvidenceProvider for MisattributingCi {
    fn id(&self) -> &str {
        "ci"
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "tests"
    }
    fn provide(&self, _: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        vec![Answer::Verified(attestor.attest(
            tests_key(&self.passed_tree),
            outcome(true),
            "build record",
        ))]
    }
}

#[tokio::test]
async fn test_success_for_another_tree_does_not_count() {
    // Ports graph.rs::ci_success_for_another_commit_does_not_count.
    let env = Env::new("other-tree");
    let old = env.approve();
    fs::write(env.path("dist/src/lib.txt"), "library v2\n").unwrap();
    let approved = env.approve();
    assert_ne!(old, approved);
    let release = Release {
        approved: approved.clone(),
    };
    let registry = env.observed();
    registry.register(
        MisattributingCi {
            passed_tree: old.clone(),
        },
        Trust::Attesting,
    );
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(undetermined(result.decision(), TESTS));
    assert!(registry.discarded().iter().any(|d| d.key.args[0] == old));
    // An honest runner that only has a result for the old tree.
    let registry = env.observed();
    registry.register(Ci::new("ci").with(&old, true), Trust::Attesting);
    let result = declare(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked);
}

#[tokio::test]
async fn disagreeing_observers_block() {
    // Ports graph.rs::disagreeing_providers_block.
    let env = Env::new("disagree");
    let approved = env.approve();
    let registry = env.observed();
    registry.register(Ci::new("ci-a").with(&approved, true), Trust::Attesting);
    registry.register(Ci::new("ci-b").with(&approved, false), Trust::Attesting);
    let result = declare(&registry)
        .authorize(Release { approved })
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(none_violated(result.decision()));
    assert!(result.decision().to_string().contains("contradictory"));
}

/// Keeps spare attestations from its first request and answers later
/// requests with them.
struct ReplayingCi {
    inner: Ci,
    spare: Mutex<Vec<Attested>>,
}

impl EvidenceProvider for ReplayingCi {
    fn id(&self) -> &str {
        "ci"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut spare = self.spare.lock().unwrap();
        if let Some(old) = spare.pop() {
            return vec![Answer::Verified(old)];
        }
        let fresh = self.inner.provide(keys, attestor);
        for key in keys {
            for _ in 0..4 {
                spare.push(attestor.attest((*key).clone(), outcome(true), "test run"));
            }
        }
        fresh
    }
}

#[tokio::test]
async fn replayed_attestations_are_refused() {
    // Ports the second half of graph.rs::stale_evidence_is_refused.
    let env = Env::new("replay-ci");
    let approved = env.approve();
    let registry = env.observed();
    registry.register(
        ReplayingCi {
            inner: Ci::new("ci").with(&approved, true),
            spare: Mutex::default(),
        },
        Trust::Attesting,
    );
    let mut rt = declare(&registry);
    let auth = authorized(&mut rt, Release { approved }).await;
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.decision.verdict, Verdict::Blocked, "{}", report.decision);
    assert!(discarded(&registry, "another provider or request"));
}

#[tokio::test]
async fn observer_disappearing_before_execution_blocks() {
    // Ports graph.rs::provider_disappearing_before_execution_blocks.
    let env = Env::new("vanish");
    let approved = env.approve();
    let registry = env.standard(&approved);
    let mut rt = declare(&registry);
    let auth = authorized(&mut rt, Release { approved }).await;
    assert!(registry.remove("ci"));
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed && report.output.is_none());
    assert_eq!(report.decision.verdict, Verdict::Blocked);
    assert!(none_violated(&report.decision));
    // Refusing a stale declaration changes nothing, so nothing is held.
    assert!(rt.is_accepting());
}

/// The filesystem observer, replaying the directory listings of its
/// first request (bound to that request) in every later one.
struct ReplayingRaw {
    inner: RawFsObserver,
    spares: Mutex<BTreeMap<Key, Vec<Attested>>>,
    armed: Arc<AtomicBool>,
}

impl ReplayingRaw {
    fn new(inner: RawFsObserver, armed: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            spares: Mutex::default(),
            armed,
        }
    }
}

impl EvidenceProvider for ReplayingRaw {
    fn id(&self) -> &str {
        "raw"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut spares = self.spares.lock().unwrap();
        let mut out = Vec::new();
        for key in keys {
            if self.armed.load(Ordering::SeqCst) {
                if let Some(old) = spares.get_mut(*key).and_then(Vec::pop) {
                    out.push(Answer::Verified(old));
                    continue;
                }
            }
            for answer in self.inner.provide(&[key], attestor) {
                if let Answer::Verified(a) = &answer {
                    if a.fact().subject.kind == "fs_dir" && !spares.contains_key(*key) {
                        let copies = (0..16)
                            .map(|_| {
                                attestor.attest(
                                    a.fact().subject.clone(),
                                    a.fact().value.clone(),
                                    "getdents",
                                )
                            })
                            .collect();
                        spares.insert((*key).clone(), copies);
                    }
                }
                out.push(answer);
            }
        }
        out
    }
}

#[tokio::test]
async fn replayed_raw_observation_fails_binding() {
    // Ports verifiable.rs::replayed_raw_observation_fails_temporal_binding.
    let env = Env::new("replay-raw");
    let approved = env.approve();
    let registry = env.bare();
    let armed = Arc::new(AtomicBool::new(true));
    registry.register(ReplayingRaw::new(env.raw("raw"), armed), Trust::Attesting);
    registry.register(Ci::new("ci").with(&approved, true), Trust::Attesting);
    let mut rt = declare(&registry);
    let auth = authorized(&mut rt, Release { approved }).await;
    fs::write(env.path("dist/src/lib.txt"), "backdoor\n").unwrap();
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.decision.verdict, Verdict::Blocked, "{}", report.decision);
    assert!(discarded(&registry, "attestation from another provider or request"));
}

/// The filesystem observer, re-attesting freshly the file contents it saw
/// first: hidden state, not replay.
struct CachingRaw {
    inner: RawFsObserver,
    cache: Mutex<BTreeMap<Key, Term>>,
}

impl EvidenceProvider for CachingRaw {
    fn id(&self) -> &str {
        "raw"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut cache = self.cache.lock().unwrap();
        let mut out = Vec::new();
        for key in keys {
            if let Some(old) = cache.get(*key) {
                out.push(Answer::Verified(attestor.attest((*key).clone(), old.clone(), "read")));
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

#[tokio::test]
async fn stale_raw_content_attested_freshly_is_believed() {
    // Ports verifiable.rs::stale_raw_content_attested_freshly_is_believed.
    // A limit, kept as a test: binding checks requests, not the present.
    let tamper_after_authorization = |caching: bool| async move {
        let env = Env::new("cache");
        let approved = env.approve();
        let registry = env.bare();
        if caching {
            registry.register(
                CachingRaw {
                    inner: env.raw("raw"),
                    cache: Mutex::default(),
                },
                Trust::Attesting,
            );
        } else {
            registry.register(env.raw("raw"), Trust::Attesting);
        }
        registry.register(Ci::new("ci").with(&approved, true), Trust::Attesting);
        let mut rt = declare(&registry);
        let auth = authorized(&mut rt, Release { approved }).await;
        fs::write(env.path("dist/src/lib.txt"), "backdoor\n").unwrap();
        let report = rt.execute(auth).await.unwrap();
        (report.executed, report.decision.verdict)
    };
    assert_eq!(tamper_after_authorization(false).await, (false, Verdict::Deny));
    assert_eq!(
        tamper_after_authorization(true).await,
        (true, Verdict::Allow),
        "false ALLOW: a caching observer is believed"
    );
}

// ------------------------------------------------------------ transitions (temporal runtime)

#[derive(Clone, Debug)]
struct Edit;

const INPUT: &str = "I1.input_is_approved";
const SCOPE: &str = "I2.changes_within_scope";
const RESULT: &str = "I3.result_identified";
const BYTES: &str = "I4.output_bytes";

fn entries_key() -> Key {
    Key::new("snapshot_entries", ["ws"])
}

struct InputApproved(String);

impl TransitionInvariant<Edit> for InputApproved {
    fn id(&self) -> &str {
        INPUT
    }
    fn watches(&self) -> Vec<Key> {
        vec![snapshot_key("ws")]
    }
    fn pre(&self, _: &Edit, _: &Snapshot) -> Vec<TObligation> {
        vec![Obligation {
            invariant: INPUT.into(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: TKey {
                    at: At::Current,
                    key: snapshot_key("ws"),
                },
                value: Term::Id(self.0.clone()),
                strength: Strength::Hard,
            },
        }]
    }
    fn post(&self, t: &Transition<'_, Edit>) -> Vec<TObligation> {
        vec![temporal::require(
            INPUT,
            TKey::at(t.before, snapshot_key("ws")),
            Term::Id(self.0.clone()),
        )]
    }
}

struct WithinSrc;

impl TransitionInvariant<Edit> for WithinSrc {
    fn id(&self) -> &str {
        SCOPE
    }
    fn watches(&self) -> Vec<Key> {
        vec![entries_key()]
    }
    fn post(&self, t: &Transition<'_, Edit>) -> Vec<TObligation> {
        let (p0, e0) = pin(SCOPE, t.before, &entries_key());
        let mut out = vec![p0];
        let e1 = t.after.and_then(|a| {
            let (p1, e1) = pin(SCOPE, a, &entries_key());
            out.push(p1);
            e1
        });
        out.push(Obligation {
            invariant: SCOPE.into(),
            phase: Phase::Post,
            requirement: Requirement::Within {
                names: changed("", e0, e1),
                scopes: vec!["ws/src".into()],
            },
        });
        out
    }
}

/// The resulting state has an identity.
struct ResultIdentified;

impl TransitionInvariant<Edit> for ResultIdentified {
    fn id(&self) -> &str {
        RESULT
    }
    fn watches(&self) -> Vec<Key> {
        vec![snapshot_key("ws")]
    }
    fn post(&self, t: &Transition<'_, Edit>) -> Vec<TObligation> {
        match t.after {
            Some(after) => vec![pin(RESULT, after, &snapshot_key("ws")).0],
            None => vec![temporal::require(
                RESULT,
                TKey::now(snapshot_key("unobserved")),
                Term::Bool(true),
            )],
        }
    }
}

/// `ws/src/a.py` read directly by the observer, before and after (provider
/// facts at snapshots, so they carry lineage).
struct OutputBytes;

impl TransitionInvariant<Edit> for OutputBytes {
    fn id(&self) -> &str {
        BYTES
    }
    fn watches(&self) -> Vec<Key> {
        vec![Key::new("fs_file", ["ws/src/a.py"])]
    }
    fn post(&self, t: &Transition<'_, Edit>) -> Vec<TObligation> {
        let k = Key::new("fs_file", ["ws/src/a.py"]);
        let mut out = vec![pin(BYTES, t.before, &k).0];
        if let Some(after) = t.after {
            out.push(pin(BYTES, after, &k).0);
        }
        out
    }
}

fn transition_invariants(s0: &str) -> Vec<Box<dyn TransitionInvariant<Edit>>> {
    vec![
        Box::new(InputApproved(s0.into())),
        Box::new(WithinSrc),
        Box::new(ResultIdentified),
        Box::new(OutputBytes),
    ]
}

/// Runs a closure as the effect.
struct Doing<F>(F);

impl<F: FnMut() -> Result<String, String>> Actor<Edit> for Doing<F> {
    async fn act(&mut self, _: &Edit) -> Result<String, String> {
        (self.0)()
    }
}

impl Env {
    fn s0(&self, registry: &Registry) -> String {
        match registry.query(&snapshot_key("ws")) {
            Some(Term::Id(id)) => id,
            other => panic!("S0: {other:?}"),
        }
    }

    fn guard<F: FnMut() -> Result<String, String>>(
        &self,
        registry: &Registry,
        effect: F,
    ) -> temporal::TemporalRuntime<Edit, Doing<F>> {
        let s0 = self.s0(&self.observed());
        temporal::runtime(registry.clone(), transition_invariants(&s0), Doing(effect))
    }
}

async fn transition<F: FnMut() -> Result<String, String>>(
    rt: &mut temporal::TemporalRuntime<Edit, Doing<F>>,
) -> v9r_core::runtime::Report<temporal::TemporalDomain<Edit, Doing<F>>> {
    let auth = match rt.authorize(Edit).await.unwrap() {
        Authorize::Allowed(auth) => auth,
        other => panic!("PRE: {}", other.decision()),
    };
    rt.execute(*auth).await.unwrap()
}

fn write(path: PathBuf, content: &'static str) -> impl FnMut() -> Result<String, String> {
    move || {
        fs::write(&path, content).map_err(|e| e.to_string())?;
        Ok(String::new())
    }
}

#[tokio::test]
async fn honest_transition_is_accepted() {
    let env = Env::new("t-honest");
    let registry = env.observed();
    let mut rt = env.guard(&registry, write(env.path("ws/src/a.py"), "a = 2\n"));
    let report = transition(&mut rt).await;
    assert!(report.accepted, "{}", report.decision);
    assert!(rt.is_accepting());
}

#[tokio::test]
async fn before_snapshot_replayed_as_after_is_not_believed() {
    // Ports temporal.rs::before_snapshot_replayed_as_after_is_not_believed.
    // The effect changes a file outside the scope; were the replayed
    // (before) listings believed, nothing would appear to have changed.
    let env = Env::new("t-replay");
    let armed = Arc::new(AtomicBool::new(false));
    let registry = env.bare();
    registry.register(ReplayingRaw::new(env.raw("raw"), armed.clone()), Trust::Attesting);
    let readme = env.path("ws/NEW");
    let mut rt = env.guard(&registry, move || {
        fs::write(&readme, "outside\n").map_err(|e| e.to_string())?;
        armed.store(true, Ordering::SeqCst);
        Ok(String::new())
    });
    let report = transition(&mut rt).await;
    assert!(report.executed);
    assert_ne!(report.decision.verdict, Verdict::Allow, "{}", report.decision);
    assert_eq!(report.decision.verdict, Verdict::Blocked, "{}", report.decision);
    assert!(undetermined(&report.decision, SCOPE));
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.key.kind == "fs_dir" && d.reason.contains("another provider or request")));
    assert!(!rt.is_accepting());
}

/// A concurrent writer: once armed, it changes `ws/src/a.py` the first
/// time it is asked for file contents (the after-snapshot's first
/// reading), after the honest observer (asked first) has read them.
struct Interloper {
    path: PathBuf,
    armed: Arc<AtomicBool>,
}

impl EvidenceProvider for Interloper {
    fn id(&self) -> &str {
        "interloper"
    }
    fn answers(&self, key: &Key) -> bool {
        key.kind == "fs_file"
    }
    fn provide(&self, _: &[&Key], _: &Attestor) -> Vec<Answer> {
        if self.armed.swap(false, Ordering::SeqCst) {
            fs::write(&self.path, "a = 3\n").unwrap();
        }
        Vec::new()
    }
}

#[tokio::test]
async fn observers_reading_different_versions_block() {
    // Ports temporal.rs::providers_observing_different_versions_block.
    let env = Env::new("t-versions");
    let armed = Arc::new(AtomicBool::new(false));
    let registry = env.observed();
    registry.register(
        Interloper {
            path: env.path("ws/src/a.py"),
            armed: armed.clone(),
        },
        Trust::Attesting,
    );
    let target = env.path("ws/src/a.py");
    let mut rt = env.guard(&registry, move || {
        fs::write(&target, "a = 2\n").map_err(|e| e.to_string())?;
        armed.store(true, Ordering::SeqCst);
        Ok(String::new())
    });
    let report = transition(&mut rt).await;
    assert_eq!(report.decision.verdict, Verdict::Blocked, "{}", report.decision);
    // The two readings of the after-snapshot disagree on the file the
    // interloper changed between them: it keeps no value there.
    let after = report.receipt.unwrap().after.unwrap();
    let file = Key::new("fs_file", ["ws/src/a.py"]);
    assert!(after.inconsistent().contains(&file), "{:?}", after.inconsistent());
    assert!(after.verified(&file).is_none());
    assert!(undetermined(&report.decision, BYTES));
    assert!(!rt.is_accepting());
}

#[tokio::test]
async fn observer_unavailable_after_effect_blocks_and_holds() {
    // Ports temporal.rs::provider_unavailable_after_effect_blocks_and_holds.
    let env = Env::new("t-unavailable");
    let registry = env.observed();
    let handle = registry.clone();
    let target = env.path("ws/src/a.py");
    let mut rt = env.guard(&registry, move || {
        fs::write(&target, "a = 2\n").map_err(|e| e.to_string())?;
        handle.remove("raw");
        Ok(String::new())
    });
    let report = transition(&mut rt).await;
    assert!(report.executed);
    assert_eq!(report.decision.verdict, Verdict::Blocked, "{}", report.decision);
    assert!(undetermined(&report.decision, SCOPE));
    assert!(report.output.is_none());
    assert!(!rt.is_accepting());
    assert_eq!(fs::read_to_string(env.path("ws/src/a.py")).unwrap(), "a = 2\n", "the effect did happen");
}

#[tokio::test]
async fn external_modification_between_authorization_and_execution_refuses() {
    // Ports temporal.rs::external_modification_between_authorization_and_execution_refuses.
    let env = Env::new("t-between");
    let registry = env.observed();
    let mut rt = env.guard(&registry, write(env.path("ws/src/a.py"), "a = 2\n"));
    let Authorize::Allowed(auth) = rt.authorize(Edit).await.unwrap() else {
        panic!()
    };
    // Someone else, inside the scope, before the effect runs.
    fs::write(env.path("ws/src/a.py"), "a = 9\n").unwrap();
    let report = rt.execute(*auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(fs::read_to_string(env.path("ws/src/a.py")).unwrap(), "a = 9\n", "nothing ran");
    assert!(!rt.is_accepting(), "drift from the trusted snapshot");
}

#[tokio::test]
async fn external_modification_during_the_effect_is_attributed_to_it() {
    // Ports temporal.rs::external_modification_during_the_effect_is_attributed_to_it.
    let env = Env::new("t-during");
    let registry = env.observed();
    let (target, other) = (env.path("ws/src/a.py"), env.path("ws/README"));
    let mut rt = env.guard(&registry, move || {
        fs::write(&target, "a = 2\n").map_err(|e| e.to_string())?;
        // Someone else, at the same time, outside the scope.
        fs::write(&other, "changed\n").map_err(|e| e.to_string())?;
        Ok(String::new())
    });
    let report = transition(&mut rt).await;
    assert_eq!(report.decision.verdict, Verdict::Deny, "{}", report.decision);
    assert!(violated(&report.decision, SCOPE));
}

#[test]
fn transition_vocabulary_uses_existing_requirement_forms() {
    // Ports temporal.rs::transition_vocabulary_uses_existing_requirement_forms.
    let a = Term::Map(BTreeMap::from([("x".into(), "1".into())]));
    let b = Term::Map(BTreeMap::from([("x".into(), "2".into()), ("y".into(), "1".into())]));
    assert_eq!(
        changed("r", Some(&a), Some(&b)),
        vec![Name::Known("r/x".into()), Name::Known("r/y".into())]
    );
    assert_eq!(changed("r", Some(&a), None), vec![Name::UnknownBelow("r".into())]);
}

#[tokio::test]
async fn stale_state_attested_freshly_is_believed_unless_independently_witnessed() {
    // Ports temporal.rs::stale_state_attested_freshly_is_believed_unless_independently_witnessed.
    for witnessed in [false, true] {
        let env = Env::new("t-caching");
        let registry = env.bare();
        registry.register(
            CachingRaw {
                inner: env.raw("raw"),
                cache: Mutex::default(),
            },
            Trust::Attesting,
        );
        if witnessed {
            registry.register(env.raw("raw-2"), Trust::Attesting);
        }
        // Outside the scope: an honest observer would see it.
        let mut rt = env.guard(&registry, write(env.path("ws/README"), "changed\n"));
        let report = transition(&mut rt).await;
        let decision = report.decision;
        if witnessed {
            assert_eq!(decision.verdict, Verdict::Blocked, "{decision}");
            assert!(discarded(&registry, "contradictory observations"));
        } else {
            assert_eq!(decision.verdict, Verdict::Allow, "false ALLOW: {decision}");
        }
    }
}

// ------------------------------------------------------------ provenance

fn snapshot_of(subject: &RtSubject<TKey>) -> Option<u64> {
    match subject {
        RtSubject::Domain(TKey {
            at: At::Snapshot(id),
            ..
        }) => Some(*id),
        _ => None,
    }
}

#[tokio::test]
async fn snapshot_evidence_carries_its_snapshot_in_lineage() {
    // Ports temporal.rs::snapshot_evidence_carries_its_snapshot_in_lineage.
    let env = Env::new("t-lineage");
    let registry = env.observed();
    let mut rt = env.guard(&registry, write(env.path("ws/src/a.py"), "a = 2\n"));
    let report = transition(&mut rt).await;
    assert!(report.accepted, "{}", report.decision);
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
    assert!(snapshots.contains(&Some(s0)) && snapshots.contains(&Some(s1)), "{snapshots:?}");
    assert!(snapshots.iter().all(|s| *s == Some(s0) || *s == Some(s1)));
    // Derived snapshot facts are explained by the verifier, a layer.
    assert!(explanation.findings.iter().any(|f| matches!(
        &f.source, Source::Layer { observer, .. } if observer == "verifier:fs-snapshot"
    )));

    // Evidence moved between moments: relabel a provider finding from s0
    // to s1 (as a faulty layer might). The audit names it.
    let mut moved = report.decision.clone();
    let finding = moved
        .findings
        .iter_mut()
        .find(|f| {
            f.obligation.invariant == BYTES
                && matches!(&f.obligation.requirement, Requirement::Fact { subject, .. }
                    if snapshot_of(subject) == Some(s0))
        })
        .unwrap();
    if let Requirement::Fact { subject, .. } = &mut finding.obligation.requirement {
        *subject = RtSubject::Domain(TKey {
            at: At::Snapshot(s1),
            key: Key::new("fs_file", ["ws/src/a.py"]),
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

fn explained(
    decision: &v9r_core::runtime::RtDecision<graph::GraphDomain<Release>>,
    registry: &Registry,
) -> v9r_core::provenance::Explanation {
    explain(decision, registry, |_: &RtSubject<Key>| None)
}

fn lineage_of<'a>(
    e: &'a v9r_core::provenance::Explanation,
    kind: &str,
) -> Option<&'a [(usize, graph::Lineage)]> {
    e.findings.iter().find_map(|f| match &f.source {
        Source::Lineage(chain) if chain[0].1.key.kind == kind => Some(chain.as_slice()),
        _ => None,
    })
}

#[tokio::test]
async fn release_is_explained_by_the_lineage_of_its_evidence() {
    // Ports provenance.rs::release_is_explained_by_the_lineage_of_its_evidence.
    let env = Env::new("explain");
    let approved = env.approve();
    let registry = env.standard(&approved);
    let mut rt = declare(&registry);
    let Authorize::Allowed(auth) = rt.authorize(Release { approved }).await.unwrap() else {
        panic!()
    };
    let pre = explained(auth.decision(), &registry);
    assert!(pre.findings.iter().any(|f| matches!(
        &f.source, Source::Layer { observer, .. } if observer == "runtime-verdicts"
    )));
    let report = rt.execute(*auth).await.unwrap();
    assert!(report.accepted);
    let e = explained(&report.decision, &registry);
    assert_eq!(e.verdict, Verdict::Allow);
    assert!(e.problems.is_empty(), "{e}");
    // Who: the test result by the runner that observed it.
    let tests = lineage_of(&e, "tests").unwrap_or_else(|| panic!("{e}"));
    assert_eq!(tests[0].1.provider, "ci");
    assert!(!tests[0].1.supporting);
    // The snapshot by the verifier, a layer. (Its basis, "… input(s);
    // trusts provider:raw", is cut at the first ')' by `explain`: the
    // known limit of explanations parsed from kernel text.)
    assert!(e.findings.iter().any(|f| matches!(
        &f.source, Source::Layer { observer, .. } if observer == "verifier:fs-snapshot"
    )));
    assert!(report.decision.to_string().contains("trusts provider:raw"));
    let text = e.to_string();
    for needle in ["ALLOW", "provider ci"] {
        assert!(text.contains(needle), "{needle}\n{text}");
    }
}

/// Keeps a spare attestation from its first request; later, attests a
/// fresh result that claims to depend on it.
struct AnachronisticCi {
    inner: Ci,
    kept: Mutex<Option<Attested>>,
}

impl EvidenceProvider for AnachronisticCi {
    fn id(&self) -> &str {
        "ci"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let honest = self.inner.provide(keys, attestor);
        let mut kept = self.kept.lock().unwrap();
        let Some(old) = kept.as_ref() else {
            *kept = keys
                .first()
                .map(|k| attestor.attest((*k).clone(), outcome(true), "test run"));
            return honest;
        };
        honest
            .into_iter()
            .filter_map(|answer| match answer {
                Answer::Verified(a) => Some(Answer::Verified(
                    attestor
                        .attest(a.fact().subject.clone(), a.fact().value.clone(), "test run")
                        .depends_on(old),
                )),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn evidence_claiming_an_earlier_state_is_refused() {
    // Ports provenance.rs::evidence_claiming_an_earlier_state_is_refused.
    let env = Env::new("anachronism");
    let approved = env.approve();
    let registry = env.observed();
    registry.register(
        AnachronisticCi {
            inner: Ci::new("ci").with(&approved, true),
            kept: Mutex::default(),
        },
        Trust::Attesting,
    );
    let mut rt = declare(&registry);
    let auth = authorized(&mut rt, Release { approved }).await;
    let report = rt.execute(auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.decision.verdict, Verdict::Blocked, "{}", report.decision);
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.key.kind == "tests" && d.reason.contains("outside this response")));
}

/// Claims, in free text, to have observed another snapshot.
struct BoastfulCi(Ci);

impl EvidenceProvider for BoastfulCi {
    fn id(&self) -> &str {
        "ci"
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
    // Ports provenance.rs::claimed_state_is_shown_as_a_claim_beside_the_established_round.
    let env = Env::new("boast");
    let approved = env.approve();
    let registry = env.observed();
    registry.register(BoastfulCi(Ci::new("ci").with(&approved, true)), Trust::Attesting);
    let result = declare(&registry)
        .authorize(Release { approved })
        .await
        .unwrap();
    let e = explained(result.decision(), &registry);
    // Free-text claims cannot be checked: the fact is accepted…
    assert_eq!(e.verdict, Verdict::Allow, "{e}");
    let tests = &lineage_of(&e, "tests").unwrap()[0].1;
    // …and its explanation keeps the claim apart from what was established.
    assert!(tests.observed.contains(&"snapshot s1".to_string()));
    assert_eq!(tests.snapshot, None, "no snapshot was established");
}

/// A test runner that also "reports" a file's content, copied from an
/// earlier report rather than observed.
struct CopyingCi {
    inner: Ci,
    copied: Vec<u8>,
}

impl EvidenceProvider for CopyingCi {
    fn id(&self) -> &str {
        "ci"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key) || key.kind == "fs_file"
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut out = Vec::new();
        for key in keys {
            if key.kind == "fs_file" {
                out.push(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    Term::Bytes(self.copied.clone()),
                    "build log",
                )));
            } else {
                out.extend(self.inner.provide(&[key], attestor));
            }
        }
        out
    }
}

/// `fs_file(dist/README)` is the approved content.
struct ReadmeUnchanged;

impl Invariant<Release, Key, Term> for ReadmeUnchanged {
    fn id(&self) -> &str {
        "readme_unchanged"
    }
    fn obligations(&self, _: &Release) -> Vec<Obligation<Key, Term>> {
        vec![hard(
            "readme_unchanged",
            Key::new("fs_file", ["dist/README"]),
            Term::Bytes(b"read me\n".to_vec()),
        )]
    }
}

#[tokio::test]
async fn evidence_copied_between_domains_is_caught_only_by_declared_observers() {
    // Ports provenance.rs::evidence_copied_between_domains_is_caught_only_by_declared_observers.
    // This is what `restrict(fs_file, [raw])` in the demo is for.
    let env = Env::new("copied");
    let approved = env.approve();
    fs::write(env.path("dist/README"), "tampered\n").unwrap();
    let release = Release {
        approved: approved.clone(),
    };
    let judge = |registry: &Registry| {
        graph::runtime(
            registry.clone(),
            vec![Box::new(ReadmeUnchanged), Box::new(TestsPassed)],
        )
    };
    let copying = || CopyingCi {
        inner: Ci::new("ci").with(&approved, true),
        copied: b"read me\n".to_vec(),
    };

    // No filesystem observer at all: the copy is believed.
    let registry = Registry::new();
    registry.register(copying(), Trust::Attesting);
    let result = judge(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Allow, "false ALLOW: {}", result.decision());

    // Declared observers: the copy is refused, the fact is missing.
    let registry = Registry::new();
    registry.register(copying(), Trust::Attesting);
    registry.restrict("fs_file", &["raw"]);
    let result = judge(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked);
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.provider == "ci" && d.reason.contains("not an observer")));

    // Declared observers and the real one: the tampering is seen.
    let registry = Registry::new();
    registry.register(env.raw("raw"), Trust::Attesting);
    registry.register(copying(), Trust::Attesting);
    registry.restrict("fs_file", &["raw"]);
    let result = judge(&registry).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny, "{}", result.decision());
}

/// Reports test results under a definition of its own.
struct ForeignCi(Ci, &'static str);

impl EvidenceProvider for ForeignCi {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn answers(&self, key: &Key) -> bool {
        self.0.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let passed = *self.0.results.get(&key.args[0])?;
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    outcome(passed),
                    Method::new("test run").defined_as(self.1),
                )))
            })
            .collect()
    }
}

#[tokio::test]
async fn a_declared_definition_refuses_foreign_answers() {
    // Ports the declared-definition part of
    // provenance.rs::different_definitions_of_the_same_digest_do_not_combine_silently.
    let env = Env::new("definition");
    let approved = env.approve();
    let release = Release {
        approved: approved.clone(),
    };
    let with = |definition: &'static str| {
        let registry = env.observed();
        registry.define("tests", "runner/1");
        registry.register(ForeignCi(Ci::new("ci").with(&approved, true), definition), Trust::Attesting);
        registry
    };
    let registry = with("other-runner/7");
    let result = declare(&registry).authorize(release.clone()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(registry
        .discarded()
        .iter()
        .any(|d| d.provider == "ci" && d.reason.contains("definition mismatch")));
    let result = declare(&with("runner/1")).authorize(release).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Allow, "{}", result.decision());
}

#[test]
fn lineage_tokens_round_trip_through_kernel_text() {
    // Ports provenance.rs::lineage_tokens_round_trip_through_kernel_text.
    assert_eq!(graph::lineage_ids("x (lineage:L12); y lineage:L3"), vec![12, 3]);
    assert_eq!(graph::lineage_token(7), "lineage:L7");
}
