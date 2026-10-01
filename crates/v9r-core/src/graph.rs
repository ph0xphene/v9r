//! Evidence graph: composes independent evidence providers.
//!
//! ```text
//!   invariants (declare required facts as obligations over `Key`/`Term`)
//!        │
//!        ▼
//!   Runtime ── GraphDomain ── Registry ──┬── provider A  (answers kinds a…)
//!                                        ├── provider B  (answers kinds b…)
//!                                        └── provider C  (answers kinds c…)
//! ```
//!
//! * A **key** names a fact: a kind and its arguments, e.g.
//!   `descends(repo, base, commit)`. Values are a small closed set of
//!   [`Term`]s. Kinds are open: any provider may answer any kind.
//! * An **invariant** states the facts it requires as ordinary kernel
//!   obligations over keys. It does not say who answers them.
//! * The **registry** asks every registered provider whether it answers a
//!   key ([`EvidenceProvider::answers`]), collects the answers and merges
//!   them into one evidence base. Several providers may answer the same
//!   key: agreement is fine, disagreement between verified answers is a
//!   contradiction, which the kernel reports as undetermined.
//! * A key no provider answers has no evidence: the obligation is
//!   undetermined and the verdict is Blocked, never Deny.
//!
//! # Attestation
//!
//! Providers vouch for what they observed through an [`Attestor`] the
//! registry hands them for one request. An [`Attested`] answer is bound to
//! that provider and that request: the registry discards it if it comes
//! back from another provider or another request (forwarded or replayed),
//! or if it is about a key that provider was not asked. Whether a
//! provider's attestations count as verified is the host's decision at
//! registration ([`Trust`]); a `ClaimsOnly` provider's answers are kept as
//! proposed claims.
//!
//! Providers know nothing of the lifecycle, authorization, invariants or
//! each other. The module depends only on `std`, `serde`, the kernel and
//! the runtime (a test enforces it).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::kernel::{
    Evidence, EvidenceBase, Fact, Invariant, Obligation, Phase, Proposed, Provenance, Verified,
};
use crate::runtime::{
    Concluded, DomainEvidence, DomainObligation, EffectDomain, MemoryJournal, Runtime,
    RuntimeFault, Stage,
};

// ------------------------------------------------------------ vocabulary

/// A fact key: `kind(arg, …)`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Key {
    pub kind: String,
    pub args: Vec<String>,
}

impl Key {
    pub fn new<A: Into<String>>(kind: &str, args: impl IntoIterator<Item = A>) -> Self {
        Self {
            kind: kind.to_string(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.kind, self.args.join(", "))
    }
}

/// A fact value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "term", content = "value", rename_all = "snake_case")]
pub enum Term {
    Bool(bool),
    /// An identifier (e.g. an object id).
    Id(String),
    /// A digest in a normal form both sides of a comparison agree on.
    Digest(String),
    Text(String),
    /// A set-valued fact (e.g. every ref of a repository, every entry of
    /// a tree), name → value.
    Map(BTreeMap<String, String>),
    Absent,
}

impl fmt::Debug for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Bool(b) => write!(f, "{b}"),
            Term::Id(s) | Term::Digest(s) | Term::Text(s) => write!(f, "{s}"),
            Term::Map(map) => {
                // Display only: a short fingerprint, not a commitment.
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                map.hash(&mut hasher);
                write!(f, "{{{} entries, #{:016x}}}", map.len(), hasher.finish())
            }
            Term::Absent => f.write_str("absent"),
        }
    }
}

pub type GraphFact = Fact<Key, Term>;
pub type GraphEvidence = EvidenceBase<Key, Term>;

// ------------------------------------------------------------ providers

/// An independent source of evidence.
pub trait EvidenceProvider: Send + Sync {
    /// Stable identifier, unique within a registry.
    fn id(&self) -> &str;

    /// Can this provider establish a value for `key`?
    fn answers(&self, key: &Key) -> bool;

    /// Answer `keys` (all of which `answers` accepted). Observed values
    /// are vouched for with `attestor`; anything else is a claim.
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer>;
}

/// How a provider says it established a fact. Everything here is the
/// provider's **claim**: the registry records it but cannot check it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Method {
    /// The procedure, e.g. `cat-file -t`.
    pub name: String,
    /// The definition the value follows (e.g. a digest normal form), if
    /// the value is not self-describing.
    pub definition: Option<String>,
}

impl Method {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definition: None,
        }
    }

    pub fn defined_as(mut self, definition: impl Into<String>) -> Self {
        self.definition = Some(definition.into());
        self
    }
}

impl From<&str> for Method {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for Method {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

pub type LineageId = u64;

/// Where a verified fact came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Lineage {
    pub id: LineageId,
    pub key: Key,
    pub value: Term,
    // Established by the registry.
    pub provider: String,
    pub request: u64,
    /// The collection round it was gathered in.
    pub round: u64,
    /// The snapshot that round belongs to, if a temporal layer kept it.
    pub snapshot: Option<u64>,
    /// Gathered only to support another fact (not asked for).
    pub supporting: bool,
    /// Facts it depends on: claimed by the provider, *checked* by the
    /// registry to come from the same response.
    pub depends_on: Vec<LineageId>,
    // Claimed by the provider.
    pub method: Method,
    /// The state it says it observed (e.g. a tree digest).
    pub observed: Vec<String>,
}

static NEXT_LINEAGE: AtomicU64 = AtomicU64::new(1);

/// The kernel provenance basis that carries a lineage id.
pub fn lineage_token(id: LineageId) -> String {
    format!("lineage:L{id}")
}

/// Lineage ids mentioned in a text (e.g. a kernel finding's reason).
pub fn lineage_ids(text: &str) -> Vec<LineageId> {
    text.match_indices("lineage:L")
        .filter_map(|(i, m)| {
            let digits: String = text[i + m.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
        .collect()
}

/// Lets one provider vouch for facts during one request.
pub struct Attestor {
    provider: String,
    request: u64,
}

impl Attestor {
    /// Vouch that `key` has `value`, established by `method`.
    pub fn attest(&self, key: Key, value: Term, method: impl Into<Method>) -> Attested {
        Attested {
            provider: self.provider.clone(),
            request: self.request,
            id: NEXT_LINEAGE.fetch_add(1, Ordering::Relaxed),
            fact: Verified::attest(Fact {
                subject: key,
                value,
            }),
            method: method.into(),
            observed: Vec::new(),
            depends_on: Vec::new(),
        }
    }
}

/// A fact a provider vouched for. Opaque: no `Clone`, no `Deserialize`.
pub struct Attested {
    provider: String,
    request: u64,
    id: LineageId,
    fact: Verified<GraphFact>,
    method: Method,
    observed: Vec<String>,
    depends_on: Vec<LineageId>,
}

impl Attested {
    pub fn fact(&self) -> &GraphFact {
        self.fact.get()
    }

    pub fn id(&self) -> LineageId {
        self.id
    }

    /// Claim the state this was observed in.
    pub fn observed(mut self, state: impl Into<String>) -> Self {
        self.observed.push(state.into());
        self
    }

    /// Claim this was derived from `other`, which must be returned in the
    /// same response.
    pub fn depends_on(mut self, other: &Attested) -> Self {
        self.depends_on.push(other.id);
        self
    }
}

pub enum Answer {
    Verified(Attested),
    Proposed { key: Key, value: Term, note: String },
}

/// Whether a provider's attestations count as verified evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    Attesting,
    /// Everything it says is kept as a proposed claim.
    ClaimsOnly,
}

/// An answer the registry did not use as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discarded {
    pub provider: String,
    pub key: Key,
    pub reason: String,
}

/// Which providers would be asked for which keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub routes: BTreeMap<Key, Vec<String>>,
}

impl Plan {
    /// Keys no registered provider answers.
    pub fn unresolved(&self) -> Vec<&Key> {
        self.routes
            .iter()
            .filter(|(_, providers)| providers.is_empty())
            .map(|(key, _)| key)
            .collect()
    }
}

/// What the host declares about a kind.
#[derive(Clone, Debug, Default)]
struct KindRules {
    /// Answers must claim this definition.
    definition: Option<String>,
    /// Only these providers may answer.
    observers: Option<Vec<String>>,
}

#[derive(Default)]
struct Inner {
    providers: Vec<(Arc<dyn EvidenceProvider>, Trust)>,
    discarded: Vec<Discarded>,
    lineage: BTreeMap<LineageId, Lineage>,
    kinds: BTreeMap<String, KindRules>,
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
static NEXT_ROUND: AtomicU64 = AtomicU64::new(1);

/// The set of registered providers. Cloning shares it. Whoever holds it
/// decides which providers are trusted: it is privileged.
#[derive(Clone, Default)]
pub struct Registry(Arc<Mutex<Inner>>);

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().expect("registry lock")
    }

    /// Register a provider. Refused (false) if its id is taken.
    pub fn register(&self, provider: impl EvidenceProvider + 'static, trust: Trust) -> bool {
        let mut inner = self.inner();
        if inner.providers.iter().any(|(p, _)| p.id() == provider.id()) {
            return false;
        }
        inner.providers.push((Arc::new(provider), trust));
        true
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut inner = self.inner();
        let before = inner.providers.len();
        inner.providers.retain(|(p, _)| p.id() != id);
        inner.providers.len() != before
    }

    /// Declare the definition answers of `kind` must follow. Answers that
    /// claim another (or none) are discarded: values under different
    /// definitions must not meet in one decision.
    pub fn define(&self, kind: &str, definition: &str) {
        self.inner()
            .kinds
            .entry(kind.to_string())
            .or_default()
            .definition = Some(definition.to_string());
    }

    /// Declare which providers may answer `kind`.
    pub fn restrict(&self, kind: &str, observers: &[&str]) {
        self.inner()
            .kinds
            .entry(kind.to_string())
            .or_default()
            .observers = Some(observers.iter().map(|s| s.to_string()).collect());
    }

    pub fn plan<'a>(&self, keys: impl IntoIterator<Item = &'a Key>) -> Plan {
        let providers: Vec<_> = self.inner().providers.clone();
        Plan {
            routes: keys
                .into_iter()
                .map(|key| {
                    let ids = providers
                        .iter()
                        .filter(|(p, _)| p.answers(key))
                        .map(|(p, _)| p.id().to_string())
                        .collect();
                    (key.clone(), ids)
                })
                .collect(),
        }
    }

    /// Every answer discarded so far.
    pub fn discarded(&self) -> Vec<Discarded> {
        self.inner().discarded.clone()
    }

    pub fn lineage(&self, id: LineageId) -> Option<Lineage> {
        self.inner().lineage.get(&id).cloned()
    }

    /// Record that `round` was kept as snapshot `snapshot`.
    pub fn assign_snapshot(&self, round: u64, snapshot: u64) {
        for lineage in self.inner().lineage.values_mut() {
            if lineage.round == round {
                lineage.snapshot = Some(snapshot);
            }
        }
    }

    /// The single verified value for `key` right now, if there is one.
    /// Read-only; the answer is data, not authority.
    pub fn query(&self, key: &Key) -> Option<Term> {
        let evidence = self.collect(&[key]);
        let mut verified = evidence.get(key).iter().filter_map(|e| match e {
            Evidence::Verified { value, .. } => Some(value.clone()),
            _ => None,
        });
        let first = verified.next()?;
        verified.all(|v| v == first).then_some(first)
    }

    /// Ask every provider that answers any of `keys`, and merge.
    pub fn collect(&self, keys: &[&Key]) -> GraphEvidence {
        self.collect_round(keys).0
    }

    /// Like [`Registry::collect`], also returning the round id.
    pub fn collect_round(&self, keys: &[&Key]) -> (GraphEvidence, u64) {
        let round = NEXT_ROUND.fetch_add(1, Ordering::Relaxed);
        // Never call providers under the lock.
        let (providers, kinds) = {
            let inner = self.inner();
            (inner.providers.clone(), inner.kinds.clone())
        };
        let mut evidence = GraphEvidence::new();
        let mut discarded = Vec::new();
        let mut lineage = Vec::new();
        for (provider, trust) in providers {
            let id = provider.id().to_string();
            let asked: Vec<&Key> = keys
                .iter()
                .copied()
                .filter(|k| provider.answers(k))
                .collect();
            if asked.is_empty() {
                continue;
            }
            let attestor = Attestor {
                provider: id.clone(),
                request: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            };
            let mut discard = |key: &Key, reason: String| {
                discarded.push(Discarded {
                    provider: id.clone(),
                    key: key.clone(),
                    reason,
                })
            };
            // Pass 1: bind to this provider and request; apply kind rules.
            let mut valid: BTreeMap<LineageId, Attested> = BTreeMap::new();
            for answer in provider.provide(&asked, &attestor) {
                match answer {
                    Answer::Verified(attested) => {
                        let key = attested.fact().subject.clone();
                        if attested.provider != id || attested.request != attestor.request {
                            discard(&key, "attestation from another provider or request".into());
                        } else if let Some(reason) = kind_violation(&kinds, &id, &attested) {
                            discard(&key, reason);
                        } else {
                            valid.insert(attested.id, attested);
                        }
                    }
                    Answer::Proposed { key, value, note } => {
                        if asked.contains(&&key) {
                            evidence.add_proposed(Proposed {
                                value: Fact {
                                    subject: key,
                                    value,
                                },
                                source: format!("provider:{id}: {note}"),
                            });
                        } else {
                            discard(&key, "not asked of this provider".into());
                        }
                    }
                }
            }
            // Pass 2: dependencies must lie within this response.
            let foreign: Vec<LineageId> = valid
                .values()
                .filter(|a| a.depends_on.iter().any(|d| !valid.contains_key(d)))
                .map(|a| a.id)
                .collect();
            for lid in foreign {
                let a = valid.remove(&lid).expect("present");
                discard(
                    &a.fact().subject,
                    "depends on evidence outside this response (another request or snapshot)"
                        .into(),
                );
            }
            // Answers to asked keys, and whatever they (transitively)
            // depend on as support.
            let mut keep: Vec<LineageId> = valid
                .values()
                .filter(|a| asked.contains(&&a.fact().subject))
                .map(|a| a.id)
                .collect();
            let mut i = 0;
            while i < keep.len() {
                let deps = valid
                    .get(&keep[i])
                    .map(|a| a.depends_on.clone())
                    .unwrap_or_default();
                for dep in deps {
                    if !keep.contains(&dep) && valid.contains_key(&dep) {
                        keep.push(dep);
                    }
                }
                i += 1;
            }
            for (lid, attested) in valid {
                let key = attested.fact().subject.clone();
                if !keep.contains(&lid) {
                    discard(&key, "not asked of this provider".into());
                    continue;
                }
                let supporting = !asked.contains(&&key);
                lineage.push(Lineage {
                    id: lid,
                    value: attested.fact().value.clone(),
                    key,
                    provider: id.clone(),
                    request: attested.request,
                    round,
                    snapshot: None,
                    supporting,
                    depends_on: attested.depends_on,
                    method: attested.method,
                    observed: attested.observed,
                });
                if supporting {
                    continue;
                }
                if trust == Trust::ClaimsOnly {
                    evidence.add_proposed(Proposed {
                        value: attested.fact.into_inner(),
                        source: format!("provider:{id} (claims only) {}", lineage_token(lid)),
                    });
                } else {
                    evidence.add_verified(
                        attested.fact,
                        Provenance {
                            observer: format!("provider:{id}"),
                            basis: lineage_token(lid),
                        },
                    );
                }
            }
        }
        let mut inner = self.inner();
        inner.discarded.extend(discarded);
        inner.lineage.extend(lineage.into_iter().map(|l| (l.id, l)));
        (evidence, round)
    }
}

/// Why the host's declarations for the attested kind refuse it, if they do.
fn kind_violation(
    kinds: &BTreeMap<String, KindRules>,
    provider: &str,
    attested: &Attested,
) -> Option<String> {
    let rules = kinds.get(&attested.fact().subject.kind)?;
    if let Some(observers) = &rules.observers {
        if !observers.iter().any(|o| o == provider) {
            return Some(format!("{provider} is not an observer of this kind"));
        }
    }
    match (&rules.definition, &attested.method.definition) {
        (Some(required), Some(claimed)) if required != claimed => Some(format!(
            "definition mismatch: kind requires {required}, answer claims {claimed}"
        )),
        (Some(required), None) => Some(format!(
            "definition mismatch: kind requires {required}, answer claims none"
        )),
        _ => None,
    }
}

// ------------------------------------------------------------ domain

/// A declaration judged by invariants over the graph: nothing runs; the
/// declaration itself is the output, released only if accepted.
///
/// Freshness is by re-evaluation: the runtime re-judges every PRE
/// obligation on freshly collected evidence before "executing", and the
/// declaration is judged once more (POST) before it is released. There is
/// no snapshot to drift from, so the basis is empty.
pub struct GraphDomain<P> {
    registry: Registry,
    invariants: Vec<Box<dyn Invariant<P, Key, Term> + Send + Sync>>,
    digest: String,
}

impl<P> GraphDomain<P> {
    pub fn new(
        registry: Registry,
        invariants: Vec<Box<dyn Invariant<P, Key, Term> + Send + Sync>>,
    ) -> Self {
        let digest = invariants
            .iter()
            .map(|i| i.id().to_string())
            .collect::<Vec<_>>()
            .join("+");
        Self {
            registry,
            invariants,
            digest,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    fn derive(&self, proposal: &P) -> Vec<DomainObligation<Self>>
    where
        P: Clone + fmt::Debug,
    {
        self.invariants
            .iter()
            .flat_map(|i| i.obligations(proposal))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    Runtime(RuntimeFault),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::Runtime(fault) => fault.fmt(f),
        }
    }
}

impl From<RuntimeFault> for GraphError {
    fn from(fault: RuntimeFault) -> Self {
        GraphError::Runtime(fault)
    }
}

impl From<Infallible> for GraphError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

impl<P: Clone + fmt::Debug> EffectDomain for GraphDomain<P> {
    type Subject = Key;
    type Value = Term;
    type Proposal = P;
    type Observation = ();
    type Effect = P;
    type Receipt = ();
    type Output = P;
    type Error = GraphError;

    fn label(&self, proposal: &P) -> String {
        format!("{proposal:?}")
    }

    fn policy_digest(&self) -> String {
        self.digest.clone()
    }

    async fn observe(&self) -> Result<(), GraphError> {
        Ok(())
    }

    fn basis(&self, _: &()) -> Vec<GraphFact> {
        Vec::new()
    }

    fn obligations(&self, stage: Stage<'_, Self>) -> Vec<DomainObligation<Self>> {
        match stage {
            Stage::Pre { proposal, .. } => self
                .derive(proposal)
                .into_iter()
                .filter(|o| o.phase == Phase::Pre)
                .collect(),
            // Nothing ran: the declaration is judged again, as POST.
            Stage::Post { proposal, .. } => self
                .derive(proposal)
                .into_iter()
                .map(|o| Obligation {
                    phase: Phase::Post,
                    ..o
                })
                .collect(),
            Stage::Compensated { .. } => Vec::new(),
        }
    }

    async fn evidence(
        &self,
        subjects: &[&Key],
        _: Option<&()>,
        _: Option<&()>,
    ) -> DomainEvidence<Self> {
        self.registry.collect(subjects)
    }

    async fn execute(&mut self, proposal: &P, _: &()) -> Result<P, GraphError> {
        Ok(proposal.clone())
    }

    async fn conclude(
        &mut self,
        declaration: P,
        _: &(),
        _: Result<&(), String>,
    ) -> Result<Concluded<Self>, GraphError> {
        Ok(Concluded {
            receipt: (),
            output: Some(declaration),
            durable: EvidenceBase::new(),
        })
    }
}

pub type GraphRuntime<P> = Runtime<GraphDomain<P>, MemoryJournal>;

/// A runtime judging declarations of type `P` over `registry`.
pub fn runtime<P: Clone + fmt::Debug>(
    registry: Registry,
    invariants: Vec<Box<dyn Invariant<P, Key, Term> + Send + Sync>>,
) -> GraphRuntime<P> {
    Runtime::new(
        GraphDomain::new(registry, invariants),
        MemoryJournal::default(),
        (),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn graph_depends_on_no_domain_module() {
        let source = include_str!("graph.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let imports: Vec<&str> = source
            .lines()
            .filter(|line| line.trim_start().starts_with("use "))
            .collect();
        assert!(
            imports.iter().all(|line| line.contains("std::")
                || line.contains("serde::")
                || line.contains("crate::kernel::")
                || line.contains("crate::runtime::")),
            "{imports:?}"
        );
        let code: String = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        for word in [
            "path",
            "file",
            "commit",
            "git",
            "workspace",
            "ci",
            "artifact",
        ] {
            assert!(
                !code
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|t| t == word),
                "graph code mentions `{word}`"
            );
        }
    }
}
