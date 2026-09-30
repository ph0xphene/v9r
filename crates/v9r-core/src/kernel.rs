//! Minimal invariant kernel.
//!
//! Domain-agnostic: this module knows nothing about files, processes or
//! hashing. It decides whether a set of obligations is met by a body of
//! evidence, and nothing else. Domain layers (see `crate::policy`) turn
//! proposals and trusted configuration into obligations, and turn their
//! own observations into evidence.
//!
//! # Model
//!
//! * A **fact** is a `(subject, value)` pair. Subjects are keys: two facts
//!   with the same subject and different values contradict each other.
//! * **Evidence** is a fact plus its class:
//!   - [`Verified`] — established by a deterministic observer inside this
//!     crate. Only this crate can construct it; it has no `Deserialize`.
//!   - [`Semantic`] — an interpretation with a confidence. Anyone can
//!     construct one; it never satisfies a hard requirement and never
//!     proves a violation.
//!   - [`Proposed`] — a claim (an agent's, or a record read back from
//!     disk). Informational only.
//!   - Unknown — the absence of evidence for a subject.
//! * An **obligation** is one [`Requirement`] tagged with the invariant
//!   that produced it and a [`Phase`].
//! * An **invariant** turns a context into obligations ([`Invariant`]).
//! * A **decision** evaluates obligations of one phase against evidence.
//!
//! # Decision semantics
//!
//! Each obligation gets a [`Status`]:
//! `Satisfied` (evidence establishes it), `Violated` (verified evidence
//! establishes its negation) or `Undetermined` (neither). The verdict is
//! `Deny` if any obligation is violated, else `Blocked` if any is
//! undetermined, else `Allow`. Unknown never becomes true or false.
//!
//! The requirement language is closed and small: exact-value facts,
//! hierarchical-name containment, and numeric bounds. No quantifiers,
//! no recursion, no user code in rules.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A fact established by a deterministic observer in this crate.
///
/// There is no public constructor and no `Deserialize`, so code outside
/// v9r-core (a semantic oracle, a planner, a deserializer) cannot produce
/// one.
///
/// ```compile_fail
/// let forged = v9r_core::kernel::Verified::attest(1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Verified<T>(T);

impl<T> Verified<T> {
    pub(crate) fn attest(value: T) -> Self {
        Self(value)
    }

    pub fn get(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

/// An interpretation with a confidence in basis points (0..=10000).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Semantic<T> {
    pub value: T,
    pub source: String,
    pub confidence_bp: u16,
}

/// A claim without evidence behind it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposed<T> {
    pub value: T,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Proposed,
    Semantic,
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fact<S, V> {
    pub subject: S,
    pub value: V,
}

/// Which deterministic observer established a verified fact, and on what
/// basis (e.g. an observation digest).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Provenance {
    pub observer: String,
    pub basis: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum Evidence<V> {
    Verified {
        value: V,
        provenance: Provenance,
    },
    Semantic {
        value: V,
        source: String,
        confidence_bp: u16,
    },
    Proposed {
        value: V,
        source: String,
    },
}

impl<V> Evidence<V> {
    pub fn class(&self) -> EvidenceClass {
        match self {
            Evidence::Verified { .. } => EvidenceClass::Verified,
            Evidence::Semantic { .. } => EvidenceClass::Semantic,
            Evidence::Proposed { .. } => EvidenceClass::Proposed,
        }
    }
}

/// Evidence indexed by subject. Adding identical evidence twice is a
/// no-op, so duplicated evidence cannot change a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvidenceBase<S: Ord, V> {
    entries: BTreeMap<S, Vec<Evidence<V>>>,
}

impl<S: Ord, V> Default for EvidenceBase<S, V> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<S: Ord + Clone, V: PartialEq + Clone> EvidenceBase<S, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_verified(&mut self, fact: Verified<Fact<S, V>>, provenance: Provenance) {
        let fact = fact.into_inner();
        self.insert(
            fact.subject,
            Evidence::Verified {
                value: fact.value,
                provenance,
            },
        );
    }

    pub fn add_semantic(&mut self, fact: Semantic<Fact<S, V>>) {
        self.insert(
            fact.value.subject,
            Evidence::Semantic {
                value: fact.value.value,
                source: fact.source,
                confidence_bp: fact.confidence_bp.min(10_000),
            },
        );
    }

    pub fn add_proposed(&mut self, fact: Proposed<Fact<S, V>>) {
        self.insert(
            fact.value.subject,
            Evidence::Proposed {
                value: fact.value.value,
                source: fact.source,
            },
        );
    }

    /// Add everything from `other` (verified entries stay verified: they
    /// were attested when first added).
    pub fn merge(&mut self, other: &Self) {
        for (subject, evidence) in &other.entries {
            for item in evidence {
                self.insert(subject.clone(), item.clone());
            }
        }
    }

    /// Drop all evidence about subjects matching `pred` (used when the
    /// state they describe has been superseded).
    pub fn forget(&mut self, mut pred: impl FnMut(&S) -> bool) {
        self.entries.retain(|subject, _| !pred(subject));
    }

    pub fn get(&self, subject: &S) -> &[Evidence<V>] {
        self.entries.get(subject).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn entries(&self) -> &BTreeMap<S, Vec<Evidence<V>>> {
        &self.entries
    }

    fn insert(&mut self, subject: S, evidence: Evidence<V>) {
        let slot = self.entries.entry(subject).or_default();
        if !slot.contains(&evidence) {
            slot.push(evidence);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    /// Only verified evidence satisfies it.
    Hard,
    /// Verified evidence, or semantic evidence at or above the threshold.
    Soft { min_confidence_bp: u16 },
}

/// A member of a hierarchical `/`-separated namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Name {
    Known(String),
    /// Something below this name may be a member; exactly what is unknown.
    UnknownBelow(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "requirement", rename_all = "snake_case")]
pub enum Requirement<S, V> {
    /// The subject must have exactly this value.
    Fact {
        subject: S,
        value: V,
        strength: Strength,
    },
    /// Every name must lie within one of the scopes (component-bounded
    /// prefix; the empty scope contains everything).
    Within {
        names: Vec<Name>,
        scopes: Vec<String>,
    },
    /// A quantity must not exceed a bound.
    AtMost {
        quantity: String,
        value: u64,
        limit: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Must hold before execution authority is granted.
    Pre,
    /// Judged after execution, from evidence about what happened.
    Post,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation<S, V> {
    pub invariant: String,
    pub phase: Phase,
    pub requirement: Requirement<S, V>,
}

/// Turns a domain context into obligations. Implementations must be
/// deterministic and must not execute anything.
pub trait Invariant<C, S, V> {
    fn id(&self) -> &str;
    fn obligations(&self, context: &C) -> Vec<Obligation<S, V>>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "because", rename_all = "snake_case")]
pub enum Status {
    Satisfied(String),
    Violated(String),
    Undetermined(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Deny,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding<S, V> {
    pub obligation: Obligation<S, V>,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision<S, V> {
    pub phase: Phase,
    pub verdict: Verdict,
    pub findings: Vec<Finding<S, V>>,
}

impl<S, V> Decision<S, V> {
    /// Findings with the given kind of status.
    pub fn violated(&self) -> impl Iterator<Item = &Finding<S, V>> {
        self.findings
            .iter()
            .filter(|f| matches!(f.status, Status::Violated(_)))
    }

    pub fn undetermined(&self) -> impl Iterator<Item = &Finding<S, V>> {
        self.findings
            .iter()
            .filter(|f| matches!(f.status, Status::Undetermined(_)))
    }
}

/// Domain-free summary of a decision, for the trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub action: String,
    pub phase: Phase,
    pub verdict: Verdict,
    pub findings: Vec<FindingRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRecord {
    pub invariant: String,
    pub status: Status,
}

impl<S, V> Decision<S, V> {
    pub fn record(&self, action: impl Into<String>) -> DecisionRecord {
        DecisionRecord {
            action: action.into(),
            phase: self.phase,
            verdict: self.verdict,
            findings: self
                .findings
                .iter()
                .map(|f| FindingRecord {
                    invariant: f.obligation.invariant.clone(),
                    status: f.status.clone(),
                })
                .collect(),
        }
    }
}

impl<S, V> fmt::Display for Decision<S, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} {:?}", self.phase, self.verdict)?;
        for finding in &self.findings {
            let (tag, because) = match &finding.status {
                Status::Satisfied(b) => ("ok", b),
                Status::Violated(b) => ("VIOLATED", b),
                Status::Undetermined(b) => ("UNDETERMINED", b),
            };
            write!(f, "\n  [{tag}] {}: {because}", finding.obligation.invariant)?;
        }
        Ok(())
    }
}

/// Evaluate the obligations of `phase` against `evidence`. Obligations of
/// the other phase are ignored. Pure and deterministic.
pub fn evaluate<S, V>(
    phase: Phase,
    obligations: &[Obligation<S, V>],
    evidence: &EvidenceBase<S, V>,
) -> Decision<S, V>
where
    S: Ord + Clone + fmt::Debug,
    V: PartialEq + Clone + fmt::Debug,
{
    let findings: Vec<Finding<S, V>> = obligations
        .iter()
        .filter(|o| o.phase == phase)
        .map(|o| Finding {
            obligation: o.clone(),
            status: check(&o.requirement, evidence),
        })
        .collect();
    let verdict = if findings
        .iter()
        .any(|f| matches!(f.status, Status::Violated(_)))
    {
        Verdict::Deny
    } else if findings
        .iter()
        .any(|f| matches!(f.status, Status::Undetermined(_)))
    {
        Verdict::Blocked
    } else {
        Verdict::Allow
    };
    Decision {
        phase,
        verdict,
        findings,
    }
}

fn check<S, V>(requirement: &Requirement<S, V>, evidence: &EvidenceBase<S, V>) -> Status
where
    S: Ord + Clone + fmt::Debug,
    V: PartialEq + Clone + fmt::Debug,
{
    match requirement {
        Requirement::Fact {
            subject,
            value,
            strength,
        } => check_fact(subject, value, *strength, evidence.get(subject)),
        Requirement::Within { names, scopes } => check_within(names, scopes),
        Requirement::AtMost {
            quantity,
            value,
            limit,
        } => {
            if value <= limit {
                Status::Satisfied(format!("{quantity} = {value} <= {limit}"))
            } else {
                Status::Violated(format!("{quantity} = {value} exceeds {limit}"))
            }
        }
    }
}

fn check_fact<S: fmt::Debug, V: PartialEq + fmt::Debug>(
    subject: &S,
    required: &V,
    strength: Strength,
    evidence: &[Evidence<V>],
) -> Status {
    let mut verified: Vec<(&V, &Provenance)> = Vec::new();
    for item in evidence {
        if let Evidence::Verified { value, provenance } = item {
            if !verified.iter().any(|(v, _)| *v == value) {
                verified.push((value, provenance));
            }
        }
    }
    match verified.as_slice() {
        [(value, provenance)] if *value == required => {
            return Status::Satisfied(format!(
                "{subject:?} = {value:?}, verified by {} ({})",
                provenance.observer, provenance.basis
            ))
        }
        [(value, provenance)] => {
            return Status::Violated(format!(
                "{subject:?} = {value:?}, verified by {} ({}); required {required:?}",
                provenance.observer, provenance.basis
            ))
        }
        [] => {}
        many => {
            let values: Vec<_> = many.iter().map(|(v, _)| v).collect();
            return Status::Undetermined(format!(
                "contradictory verified evidence for {subject:?}: {values:?}"
            ));
        }
    }

    if let Strength::Soft { min_confidence_bp } = strength {
        let semantic: Vec<(&V, u16, &String)> = evidence
            .iter()
            .filter_map(|item| match item {
                Evidence::Semantic {
                    value,
                    source,
                    confidence_bp,
                } if *confidence_bp >= min_confidence_bp => Some((value, *confidence_bp, source)),
                _ => None,
            })
            .collect();
        let agrees = semantic.iter().all(|(value, _, _)| *value == required);
        if let (Some((_, confidence, source)), true) = (semantic.first(), agrees) {
            return Status::Satisfied(format!(
                "{subject:?} = {required:?} per {source} at {confidence}bp (soft requirement)"
            ));
        }
    }

    let weaker: Vec<String> = evidence
        .iter()
        .map(|item| match item {
            Evidence::Semantic {
                value,
                source,
                confidence_bp,
            } => format!("semantic {value:?} from {source} at {confidence_bp}bp"),
            Evidence::Proposed { value, source } => format!("proposed {value:?} by {source}"),
            Evidence::Verified { .. } => unreachable!("handled above"),
        })
        .collect();
    if weaker.is_empty() {
        Status::Undetermined(format!("no evidence for {subject:?}"))
    } else {
        Status::Undetermined(format!(
            "no verified evidence for {subject:?} (have: {})",
            weaker.join("; ")
        ))
    }
}

fn check_within(names: &[Name], scopes: &[String]) -> Status {
    let contained = |name: &str| scopes.iter().any(|scope| name_within(name, scope));
    let outside: Vec<&str> = names
        .iter()
        .filter_map(|n| match n {
            Name::Known(name) if !contained(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if !outside.is_empty() {
        return Status::Violated(format!("outside {scopes:?}: {outside:?}"));
    }
    let unknown: Vec<&str> = names
        .iter()
        .filter_map(|n| match n {
            Name::UnknownBelow(name) if !contained(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if !unknown.is_empty() {
        return Status::Undetermined(format!(
            "unknown members below {unknown:?} may lie outside {scopes:?}"
        ));
    }
    Status::Satisfied(format!("{} name(s) within {scopes:?}", names.len()))
}

/// Component-bounded containment on `/`-separated names.
pub fn name_within(name: &str, scope: &str) -> bool {
    scope.is_empty()
        || name == scope
        || (name.len() > scope.len()
            && name.starts_with(scope)
            && name.as_bytes()[scope.len()] == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    type S = &'static str;
    type V = &'static str;

    fn provenance() -> Provenance {
        Provenance {
            observer: "test".to_string(),
            basis: "unit".to_string(),
        }
    }

    fn verified(subject: S, value: V) -> Verified<Fact<S, V>> {
        Verified::attest(Fact { subject, value })
    }

    fn semantic(subject: S, value: V, confidence_bp: u16) -> Semantic<Fact<S, V>> {
        Semantic {
            value: Fact { subject, value },
            source: "oracle".to_string(),
            confidence_bp,
        }
    }

    fn hard(subject: S, value: V) -> Vec<Obligation<S, V>> {
        vec![Obligation {
            invariant: "deploy.requires_tests".to_string(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject,
                value,
                strength: Strength::Hard,
            },
        }]
    }

    fn verdict(obligations: &[Obligation<S, V>], evidence: &EvidenceBase<S, V>) -> Verdict {
        evaluate(Phase::Pre, obligations, evidence).verdict
    }

    const TESTS_V1: S = "tests_passed(v1)";

    #[test]
    fn verified_fact_satisfies_hard_obligation() {
        let mut evidence = EvidenceBase::new();
        evidence.add_verified(verified(TESTS_V1, "yes"), provenance());
        assert_eq!(verdict(&hard(TESTS_V1, "yes"), &evidence), Verdict::Allow);
    }

    #[test]
    fn semantic_fact_cannot_satisfy_hard_obligation_even_at_full_confidence() {
        let mut evidence = EvidenceBase::new();
        evidence.add_semantic(semantic(TESTS_V1, "yes", 9_900));
        evidence.add_semantic(semantic(TESTS_V1, "yes", 10_000));
        let decision = evaluate(Phase::Pre, &hard(TESTS_V1, "yes"), &evidence);
        assert_eq!(decision.verdict, Verdict::Blocked);
        let Status::Undetermined(because) = &decision.findings[0].status else {
            panic!("{decision}")
        };
        assert!(because.contains("no verified evidence"), "{because}");
    }

    #[test]
    fn unknown_blocks_rather_than_denies() {
        let evidence = EvidenceBase::new();
        assert_eq!(verdict(&hard(TESTS_V1, "yes"), &evidence), Verdict::Blocked);
    }

    #[test]
    fn contradicting_verified_evidence_denies() {
        let mut evidence = EvidenceBase::new();
        evidence.add_verified(verified(TESTS_V1, "no"), provenance());
        assert_eq!(verdict(&hard(TESTS_V1, "yes"), &evidence), Verdict::Deny);
    }

    #[test]
    fn semantic_evidence_never_proves_a_violation() {
        let mut evidence = EvidenceBase::new();
        evidence.add_semantic(semantic(TESTS_V1, "no", 10_000));
        assert_eq!(verdict(&hard(TESTS_V1, "yes"), &evidence), Verdict::Blocked);
    }

    #[test]
    fn mutually_contradictory_verified_facts_block() {
        let mut evidence = EvidenceBase::new();
        evidence.add_verified(verified(TESTS_V1, "yes"), provenance());
        evidence.add_verified(verified(TESTS_V1, "no"), provenance());
        let decision = evaluate(Phase::Pre, &hard(TESTS_V1, "yes"), &evidence);
        assert_eq!(decision.verdict, Verdict::Blocked, "{decision}");
    }

    #[test]
    fn proposed_claims_are_informational_only() {
        let mut evidence = EvidenceBase::new();
        evidence.add_proposed(Proposed {
            value: Fact {
                subject: TESTS_V1,
                value: "yes",
            },
            source: "agent".to_string(),
        });
        assert_eq!(verdict(&hard(TESTS_V1, "yes"), &evidence), Verdict::Blocked);
    }

    #[test]
    fn soft_obligation_accepts_confident_agreeing_semantics_only() {
        let soft = vec![Obligation {
            invariant: "soft".to_string(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: TESTS_V1,
                value: "yes",
                strength: Strength::Soft {
                    min_confidence_bp: 9_000,
                },
            },
        }];
        let mut evidence = EvidenceBase::new();
        evidence.add_semantic(semantic(TESTS_V1, "yes", 8_000));
        assert_eq!(verdict(&soft, &evidence), Verdict::Blocked);
        evidence.add_semantic(semantic(TESTS_V1, "yes", 9_500));
        assert_eq!(verdict(&soft, &evidence), Verdict::Allow);
        evidence.add_semantic(semantic(TESTS_V1, "no", 9_100));
        assert_eq!(verdict(&soft, &evidence), Verdict::Blocked);
    }

    #[test]
    fn duplicated_evidence_is_idempotent() {
        let mut once = EvidenceBase::new();
        once.add_verified(verified(TESTS_V1, "yes"), provenance());
        let mut twice = once.clone();
        twice.add_verified(verified(TESTS_V1, "yes"), provenance());
        twice.merge(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deny_dominates_blocked_and_phases_are_separate() {
        let mut obligations = hard(TESTS_V1, "yes");
        obligations.push(Obligation {
            invariant: "budget".to_string(),
            phase: Phase::Pre,
            requirement: Requirement::AtMost {
                quantity: "steps".to_string(),
                value: 9,
                limit: 8,
            },
        });
        obligations.push(Obligation {
            invariant: "post-only".to_string(),
            phase: Phase::Post,
            requirement: Requirement::AtMost {
                quantity: "x".to_string(),
                value: 0,
                limit: 0,
            },
        });
        let decision = evaluate(Phase::Pre, &obligations, &EvidenceBase::new());
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.findings.len(), 2);
        assert_eq!(decision.violated().count(), 1);
        assert_eq!(decision.undetermined().count(), 1);
    }

    #[test]
    fn within_is_three_valued() {
        let within =
            |names: Vec<Name>| check_within(&names, &["out".to_string(), "logs/app".to_string()]);
        assert!(matches!(
            within(vec![
                Name::Known("out/a".into()),
                Name::Known("logs/app".into())
            ]),
            Status::Satisfied(_)
        ));
        assert!(matches!(
            within(vec![Name::Known("output".into())]),
            Status::Violated(_)
        ));
        // Unknown below an in-scope name is still fully in scope.
        assert!(matches!(
            within(vec![Name::UnknownBelow("out/cache".into())]),
            Status::Satisfied(_)
        ));
        // Unknown below an out-of-scope name might be anything.
        assert!(matches!(
            within(vec![Name::UnknownBelow("logs".into())]),
            Status::Undetermined(_)
        ));
        // A known violation wins over unknowns.
        assert!(matches!(
            within(vec![
                Name::UnknownBelow("logs".into()),
                Name::Known("x".into())
            ]),
            Status::Violated(_)
        ));
        assert!(matches!(
            check_within(&[Name::Known("anything".into())], &[String::new()]),
            Status::Satisfied(_)
        ));
    }

    #[test]
    fn kernel_depends_on_no_domain_module() {
        // Architecture guard: the kernel must stay domain-agnostic.
        let source = include_str!("kernel.rs");
        let imports: Vec<&str> = source
            .lines()
            .filter(|line| line.trim_start().starts_with("use "))
            .filter(|line| !line.contains("super::"))
            .collect();
        assert!(
            imports
                .iter()
                .all(|line| line.contains("std::") || line.contains("serde::")),
            "{imports:?}"
        );
    }

    #[test]
    fn evaluation_is_deterministic() {
        let mut evidence = EvidenceBase::new();
        evidence.add_semantic(semantic("b", "1", 5));
        evidence.add_verified(verified("a", "1"), provenance());
        let obligations = [hard("a", "1"), hard("b", "1"), hard("c", "1")].concat();
        let first = evaluate(Phase::Pre, &obligations, &evidence);
        let again = evaluate(Phase::Pre, &obligations, &evidence.clone());
        assert_eq!(first, again);
        assert_eq!(first.to_string(), again.to_string());
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&again).unwrap()
        );
    }
}
