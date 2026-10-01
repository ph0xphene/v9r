//! Temporal evidence: state before and after an effect.
//!
//! ```text
//!   snapshot s0 ──▶ effect ──▶ snapshot s1          (ids from one monotonic clock)
//!       │                          │
//!   key@s0 facts               key@s1 facts         (same keys, same providers)
//!       └──────── transition invariants ────────┘   (relations between s0 and s1)
//! ```
//!
//! Providers keep answering *now*. The temporal layer asks them for the
//! watched keys at t0 and at t1 and tags their answers with snapshot ids
//! it owns, so a subject is a key **at** a moment ([`TKey`]):
//!
//! * `key@s<id>`: the key's evidence in that snapshot;
//! * `key@now`: asked live at decision time (timeless facts such as
//!   ancestry or a commit's tree);
//! * `key@current`: the snapshot under check (used for the authorization
//!   basis, whose snapshot is not known when the basis is stated).
//!
//! # Relations between t0 and t1 with the existing kernel forms
//!
//! The kernel compares a subject with a *given* value; it cannot compare
//! two unknown values. A transition invariant therefore reads the
//! verified values of the two snapshots and derives ordinary obligations
//! from them ("the names that changed lie within these scopes", "the ref
//! at t1 is X", "the commit at t1 has passed tests"), and **pins** every
//! value it read (`key@s0 = v0`, `key@s1 = v1`) so the kernel checks
//! that those values really were verified. If a value is missing, the pin
//! is undetermined and so is the decision.
//!
//! # What the layer guarantees
//!
//! * **Identity.** Snapshots are created only here, with fresh ids; no
//!   API accepts a snapshot from outside.
//! * **Pairing.** Every POST judgment includes `transition.ordered`: a
//!   fact, attested here, that `before < effect < after` on the clock.
//! * **Consistency.** A snapshot reads every provider twice; a key whose
//!   two readings differ keeps no value in that snapshot (some provider
//!   saw the world change while the snapshot was taken).
//! * **Freshness.** The basis is every watched key's verified value in
//!   the trusted snapshot, re-checked against a fresh one at execution.
//!
//! Depends only on `std`, `serde`, the kernel, the runtime and the
//! evidence graph (a test enforces it).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::graph::{GraphError, GraphEvidence, Key, Registry, Term};
use crate::kernel::{
    Evidence, EvidenceBase, Fact, Name, Obligation, Phase, Provenance, Requirement, Strength,
    Verified,
};
use crate::runtime::{
    Concluded, DomainEvidence, DomainObligation, EffectDomain, MemoryJournal, Runtime, Stage,
};

// ------------------------------------------------------------ clock and keys

static CLOCK: AtomicU64 = AtomicU64::new(1);

fn tick() -> u64 {
    CLOCK.fetch_add(1, Ordering::SeqCst)
}

pub type SnapshotId = u64;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum At {
    Now,
    Current,
    Snapshot(SnapshotId),
}

/// A key at a moment.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TKey {
    pub at: At,
    pub key: Key,
}

impl TKey {
    pub fn now(key: Key) -> Self {
        Self { at: At::Now, key }
    }

    pub fn at(snapshot: &Snapshot, key: Key) -> Self {
        Self {
            at: At::Snapshot(snapshot.id),
            key,
        }
    }
}

impl fmt::Debug for TKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            At::Now => write!(f, "{:?}@now", self.key),
            At::Current => write!(f, "{:?}@current", self.key),
            At::Snapshot(id) => write!(f, "{:?}@s{id}", self.key),
        }
    }
}

pub type TObligation = Obligation<TKey, Term>;

// ------------------------------------------------------------ snapshots

/// The watched keys' evidence at one moment. Created only by this module.
///
/// ```compile_fail
/// // No snapshot can be built or deserialized from outside.
/// let s: v9r_core::temporal::Snapshot = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone)]
pub struct Snapshot {
    id: SnapshotId,
    evidence: GraphEvidence,
    inconsistent: Vec<Key>,
}

impl Snapshot {
    pub fn id(&self) -> SnapshotId {
        self.id
    }

    /// Keys whose two readings differed; they have no value here.
    pub fn inconsistent(&self) -> &[Key] {
        &self.inconsistent
    }

    /// The single verified value of `key` in this snapshot, if any.
    pub fn verified(&self, key: &Key) -> Option<&Term> {
        let mut values = self.evidence.get(key).iter().filter_map(|e| match e {
            Evidence::Verified { value, .. } => Some(value),
            _ => None,
        });
        let first = values.next()?;
        values.all(|v| v == first).then_some(first)
    }

    fn take(registry: &Registry, keys: &[Key]) -> Self {
        let asked: Vec<&Key> = keys.iter().collect();
        let first = registry.collect(&asked);
        let (mut evidence, round) = registry.collect_round(&asked);
        let inconsistent: Vec<Key> = keys
            .iter()
            .filter(|k| !same_readings(first.get(k), evidence.get(k)))
            .cloned()
            .collect();
        evidence.forget(|k| inconsistent.contains(k));
        let id = tick();
        // The kept reading's lineage now names this snapshot.
        registry.assign_snapshot(round, id);
        Self {
            id,
            evidence,
            inconsistent,
        }
    }
}

/// Same classes and values (provenance bases may differ between reads).
fn same_readings(a: &[Evidence<Term>], b: &[Evidence<Term>]) -> bool {
    let shape = |e: &Evidence<Term>| {
        (
            e.class(),
            match e {
                Evidence::Verified { value, .. }
                | Evidence::Semantic { value, .. }
                | Evidence::Proposed { value, .. } => value.clone(),
            },
        )
    };
    let a: Vec<_> = a.iter().map(shape).collect();
    let b: Vec<_> = b.iter().map(shape).collect();
    a.len() == b.len() && a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

// ------------------------------------------------------------ invariants

/// The pair an invariant judges after an effect.
pub struct Transition<'a, P> {
    pub proposal: &'a P,
    pub before: &'a Snapshot,
    /// `None` if the state after the effect could not be observed.
    pub after: Option<&'a Snapshot>,
}

/// An invariant over a state transition.
pub trait TransitionInvariant<P>: Send + Sync {
    fn id(&self) -> &str;

    /// State keys to capture before and after every effect.
    fn watches(&self) -> Vec<Key>;

    /// Obligations before authority is granted.
    fn pre(&self, _proposal: &P, _before: &Snapshot) -> Vec<TObligation> {
        Vec::new()
    }

    /// Obligations on the transition.
    fn post(&self, transition: &Transition<'_, P>) -> Vec<TObligation>;
}

/// A hard POST obligation.
pub fn require(invariant: &str, subject: TKey, value: Term) -> TObligation {
    Obligation {
        invariant: invariant.to_string(),
        phase: Phase::Post,
        requirement: Requirement::Fact {
            subject,
            value,
            strength: Strength::Hard,
        },
    }
}

/// Read `key`'s verified value in `snapshot` and pin it: the obligation
/// holds only if that value really is the snapshot's verified value.
/// Without one, the pin is undetermined.
pub fn pin<'s>(
    invariant: &str,
    snapshot: &'s Snapshot,
    key: &Key,
) -> (TObligation, Option<&'s Term>) {
    let value = snapshot.verified(key);
    (
        require(
            invariant,
            TKey::at(snapshot, key.clone()),
            value.cloned().unwrap_or(Term::Absent),
        ),
        value,
    )
}

/// Names whose value differs between two set-valued facts, under
/// `prefix`. If either side is unknown, everything below `prefix` may
/// have changed.
pub fn changed(prefix: &str, before: Option<&Term>, after: Option<&Term>) -> Vec<Name> {
    let name = |n: &str| {
        if prefix.is_empty() {
            n.to_string()
        } else {
            format!("{prefix}/{n}")
        }
    };
    let (Some(Term::Map(a)), Some(Term::Map(b))) = (before, after) else {
        return vec![Name::UnknownBelow(prefix.to_string())];
    };
    let mut names: Vec<&String> = a.keys().chain(b.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|n| a.get(*n) != b.get(*n))
        .map(|n| Name::Known(name(n)))
        .collect()
}

// ------------------------------------------------------------ domain

/// Performs the effect. It knows nothing of snapshots or evidence.
#[allow(async_fn_in_trait)]
pub trait Actor<P> {
    async fn act(&mut self, proposal: &P) -> Result<String, String>;
}

pub struct Acted<P> {
    proposal: P,
    result: Result<String, String>,
    done: u64,
}

/// The pair of snapshots around one effect, and what the actor said.
#[derive(Clone)]
pub struct TransitionReceipt {
    pub before: Snapshot,
    pub after: Option<Snapshot>,
    /// Clock reading when the effect finished.
    pub done: u64,
    pub result: Result<String, String>,
}

const ORDERED: &str = "transition.ordered";

fn order_key(before: SnapshotId, done: u64, after: Option<SnapshotId>) -> Key {
    Key::new(
        "transition",
        [
            before.to_string(),
            done.to_string(),
            after.map_or("none".to_string(), |a| a.to_string()),
        ],
    )
}

pub struct TemporalDomain<P, A> {
    registry: Registry,
    invariants: Vec<Box<dyn TransitionInvariant<P>>>,
    actor: A,
    watches: Vec<Key>,
    digest: String,
}

impl<P, A> TemporalDomain<P, A> {
    pub fn new(
        registry: Registry,
        invariants: Vec<Box<dyn TransitionInvariant<P>>>,
        actor: A,
    ) -> Self {
        let mut watches: Vec<Key> = invariants.iter().flat_map(|i| i.watches()).collect();
        watches.sort();
        watches.dedup();
        let digest = invariants
            .iter()
            .map(|i| i.id().to_string())
            .collect::<Vec<_>>()
            .join("+");
        Self {
            registry,
            invariants,
            actor,
            watches,
            digest,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::take(&self.registry, &self.watches)
    }
}

/// `snapshot`'s evidence, relabeled to `at`.
fn relabel(snapshot: &Snapshot, at: At) -> EvidenceBase<TKey, Term> {
    snapshot.evidence.map(
        |key| TKey {
            at,
            key: key.clone(),
        },
        Term::clone,
    )
}

impl<P: Clone + fmt::Debug, A: Actor<P>> EffectDomain for TemporalDomain<P, A> {
    type Subject = TKey;
    type Value = Term;
    type Proposal = P;
    type Observation = Snapshot;
    type Effect = Acted<P>;
    type Receipt = TransitionReceipt;
    type Output = P;
    type Error = GraphError;

    fn label(&self, proposal: &P) -> String {
        format!("{proposal:?}")
    }

    fn policy_digest(&self) -> String {
        self.digest.clone()
    }

    async fn observe(&self) -> Result<Snapshot, GraphError> {
        Ok(self.snapshot())
    }

    fn basis(&self, snapshot: &Snapshot) -> Vec<Fact<TKey, Term>> {
        self.watches
            .iter()
            .filter_map(|key| {
                Some(Fact {
                    subject: TKey {
                        at: At::Current,
                        key: key.clone(),
                    },
                    value: snapshot.verified(key)?.clone(),
                })
            })
            .collect()
    }

    fn obligations(&self, stage: Stage<'_, Self>) -> Vec<DomainObligation<Self>> {
        match stage {
            Stage::Pre { proposal, before } => self
                .invariants
                .iter()
                .flat_map(|i| i.pre(proposal, before))
                .collect(),
            Stage::Post {
                proposal,
                before,
                after,
                receipt,
            } => {
                let ordered = require(
                    ORDERED,
                    TKey::now(order_key(before.id, receipt.done, after.map(|a| a.id))),
                    Term::Bool(true),
                );
                let transition = Transition {
                    proposal,
                    before,
                    after,
                };
                std::iter::once(ordered)
                    .chain(self.invariants.iter().flat_map(|i| i.post(&transition)))
                    .collect()
            }
            Stage::Compensated { .. } => Vec::new(),
        }
    }

    async fn evidence(
        &self,
        subjects: &[&TKey],
        observation: Option<&Snapshot>,
        receipt: Option<&TransitionReceipt>,
    ) -> DomainEvidence<Self> {
        let mut evidence = EvidenceBase::new();
        let snapshots: Vec<&Snapshot> = observation
            .into_iter()
            .chain(receipt.map(|r| &r.before))
            .chain(receipt.and_then(|r| r.after.as_ref()))
            .collect();
        let mut live = Vec::new();
        let mut wanted: Vec<SnapshotId> = Vec::new();
        for subject in subjects {
            match subject.at {
                At::Current => {
                    if let Some(current) = observation {
                        evidence.merge(&relabel(current, At::Current));
                    }
                }
                At::Snapshot(id) => wanted.push(id),
                At::Now if subject.key.kind == "transition" => {
                    if let Some(receipt) = receipt {
                        let after = receipt.after.as_ref().map(Snapshot::id);
                        // Only the pairing this receipt actually records.
                        // Nothing observed after: unknown, not false.
                        if let (Some(a), true) = (
                            after,
                            subject.key == order_key(receipt.before.id, receipt.done, after),
                        ) {
                            let ordered = receipt.before.id < receipt.done && receipt.done < a;
                            evidence.add_verified(
                                Verified::attest(Fact {
                                    subject: (*subject).clone(),
                                    value: Term::Bool(ordered),
                                }),
                                Provenance {
                                    observer: "temporal-clock".to_string(),
                                    basis: "snapshot and effect clock readings".to_string(),
                                },
                            );
                        }
                    }
                }
                At::Now => live.push(&subject.key),
            }
        }
        wanted.sort();
        wanted.dedup();
        for id in wanted {
            if let Some(snapshot) = snapshots.iter().find(|s| s.id == id) {
                evidence.merge(&relabel(snapshot, At::Snapshot(id)));
            }
        }
        if !live.is_empty() {
            evidence.merge(
                &self
                    .registry
                    .collect(&live)
                    .map(|key| TKey::now(key.clone()), Term::clone),
            );
        }
        evidence
    }

    async fn execute(&mut self, proposal: &P, _before: &Snapshot) -> Result<Acted<P>, GraphError> {
        let result = self.actor.act(proposal).await;
        Ok(Acted {
            proposal: proposal.clone(),
            result,
            done: tick(),
        })
    }

    async fn conclude(
        &mut self,
        acted: Acted<P>,
        before: &Snapshot,
        after: Result<&Snapshot, String>,
    ) -> Result<Concluded<Self>, GraphError> {
        let output = acted.result.is_ok().then_some(acted.proposal);
        Ok(Concluded {
            receipt: TransitionReceipt {
                before: before.clone(),
                after: after.ok().cloned(),
                done: acted.done,
                result: acted.result,
            },
            output,
            durable: EvidenceBase::new(),
        })
    }
}

pub type TemporalRuntime<P, A> = Runtime<TemporalDomain<P, A>, MemoryJournal>;

/// A runtime guarding `actor`'s effects with transition invariants over
/// `registry`. The baseline snapshot is taken now.
pub fn runtime<P: Clone + fmt::Debug, A: Actor<P>>(
    registry: Registry,
    invariants: Vec<Box<dyn TransitionInvariant<P>>>,
    actor: A,
) -> TemporalRuntime<P, A> {
    let domain = TemporalDomain::new(registry, invariants, actor);
    let baseline = domain.snapshot();
    Runtime::new(domain, MemoryJournal::default(), baseline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{evaluate, Verdict};

    #[test]
    fn temporal_depends_on_no_domain_module() {
        let source = include_str!("temporal.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for line in source
            .lines()
            .filter(|l| l.trim_start().starts_with("use "))
        {
            assert!(
                [
                    "std::",
                    "serde::",
                    "crate::kernel::",
                    "crate::runtime::",
                    "crate::graph::"
                ]
                .iter()
                .any(|allowed| line.contains(allowed)),
                "{line}"
            );
        }
        let code = source
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
            "ref",
        ] {
            assert!(
                !code
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|t| t == word),
                "temporal code mentions `{word}`"
            );
        }
    }

    struct Idle;

    impl Actor<()> for Idle {
        async fn act(&mut self, _: &()) -> Result<String, String> {
            Ok(String::new())
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot::take(&Registry::new(), &[])
    }

    async fn ordered_verdict(before: &Snapshot, done: u64, after: Option<&Snapshot>) -> Verdict {
        let domain = TemporalDomain::<(), Idle>::new(Registry::new(), Vec::new(), Idle);
        let receipt = TransitionReceipt {
            before: before.clone(),
            after: after.cloned(),
            done,
            result: Ok(String::new()),
        };
        let obligations = domain.obligations(Stage::Post {
            proposal: &(),
            before,
            after,
            receipt: &receipt,
        });
        let subjects: Vec<&TKey> = obligations
            .iter()
            .filter_map(|o| match &o.requirement {
                Requirement::Fact { subject, .. } => Some(subject),
                _ => None,
            })
            .collect();
        let evidence = domain.evidence(&subjects, after, Some(&receipt)).await;
        evaluate(Phase::Post, &obligations, &evidence).verdict
    }

    #[tokio::test]
    async fn pairing_must_follow_the_clock() {
        let s0 = snapshot();
        let done = tick();
        let s1 = snapshot();
        assert_eq!(ordered_verdict(&s0, done, Some(&s1)).await, Verdict::Allow);
        // Before replayed as after.
        assert_eq!(ordered_verdict(&s0, done, Some(&s0)).await, Verdict::Deny);
        // After taken before the effect finished.
        let late = tick();
        assert_eq!(ordered_verdict(&s0, late, Some(&s1)).await, Verdict::Deny);
        // Swapped.
        assert_eq!(ordered_verdict(&s1, done, Some(&s0)).await, Verdict::Deny);
        // Nothing observed after: unknown.
        assert_eq!(ordered_verdict(&s0, done, None).await, Verdict::Blocked);
    }
}
