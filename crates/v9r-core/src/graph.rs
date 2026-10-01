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
    Absent,
}

impl fmt::Debug for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Bool(b) => write!(f, "{b}"),
            Term::Id(s) | Term::Digest(s) | Term::Text(s) => write!(f, "{s}"),
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

/// Lets one provider vouch for facts during one request.
pub struct Attestor {
    provider: String,
    request: u64,
}

impl Attestor {
    /// Vouch that `key` has `value`, observed on `basis`.
    pub fn attest(&self, key: Key, value: Term, basis: impl Into<String>) -> Attested {
        Attested {
            provider: self.provider.clone(),
            request: self.request,
            fact: Verified::attest(Fact {
                subject: key,
                value,
            }),
            basis: basis.into(),
        }
    }
}

/// A fact a provider vouched for. Opaque: no `Clone`, no `Deserialize`.
pub struct Attested {
    provider: String,
    request: u64,
    fact: Verified<GraphFact>,
    basis: String,
}

impl Attested {
    pub fn fact(&self) -> &GraphFact {
        self.fact.get()
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

#[derive(Default)]
struct Inner {
    providers: Vec<(Arc<dyn EvidenceProvider>, Trust)>,
    discarded: Vec<Discarded>,
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

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
        // Never call providers under the lock.
        let providers: Vec<_> = self.inner().providers.clone();
        let mut evidence = GraphEvidence::new();
        let mut discarded = Vec::new();
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
            for answer in provider.provide(&asked, &attestor) {
                let mut discard = |key: &Key, reason: &str| {
                    discarded.push(Discarded {
                        provider: id.clone(),
                        key: key.clone(),
                        reason: reason.to_string(),
                    })
                };
                match answer {
                    Answer::Verified(attested) => {
                        let key = &attested.fact().subject;
                        if attested.provider != id || attested.request != attestor.request {
                            discard(key, "attestation from another provider or request");
                        } else if !asked.contains(&key) {
                            discard(key, "not asked of this provider");
                        } else if trust == Trust::ClaimsOnly {
                            let fact = attested.fact.into_inner();
                            evidence.add_proposed(Proposed {
                                value: fact,
                                source: format!("provider:{id} (claims only)"),
                            });
                        } else {
                            evidence.add_verified(
                                attested.fact,
                                Provenance {
                                    observer: format!("provider:{id}"),
                                    basis: attested.basis,
                                },
                            );
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
                            discard(&key, "not asked of this provider");
                        }
                    }
                }
            }
        }
        self.inner().discarded.extend(discarded);
        evidence
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
