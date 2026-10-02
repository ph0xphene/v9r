//! Domain-neutral effect runtime.
//!
//! The runtime owns the lifecycle every guarded effect goes through; a
//! domain ([`EffectDomain`]) owns only what is specific to it: how reality
//! is observed, what the effect does, which evidence it can produce and
//! which invariants apply.
//!
//! ```text
//!             ┌──────────────────────────── runtime ───────────────────────────┐
//! proposal ──▶│ PRE obligations (domain) + transitions_accepted + basis        │
//!             │   evaluate(Pre) on evidence from the TRUSTED observation       │
//!             │   Allow ─▶ Authorization (single use, bound to runtime+policy) │
//!             │                                                                │
//! execute  ──▶│ observe() ─▶ re-evaluate the authorization on FRESH evidence   │
//!             │   stale ─▶ refuse; hold if reality ≠ trusted state (drift)     │
//!             │ domain.execute() ─▶ observe() ─▶ domain.conclude() (receipt)   │
//!             │ POST obligations (domain) ─▶ evaluate(Post)                    │
//!             │   Allow ∧ observable ─▶ trusted := after, release output       │
//!             │   otherwise          ─▶ hold (every PRE denied)                │
//!             │                                                                │
//! compensate ▶│ (only for `Compensable` domains) same as execute, without      │
//!             │ authorization; Allow ─▶ release the hold                       │
//!             └────────────────────────────────────────────────────────────────┘
//! ```
//!
//! What the runtime knows: proposals, observations, effects, receipts and
//! outputs are opaque associated types. It adds exactly two obligations of
//! its own:
//!
//! * `transitions_accepted` (PRE): nothing runs on top of a transition
//!   the runtime did not accept. It is backed by the runtime's own
//!   verified fact, in the runtime's own vocabulary ([`RuntimeSubject`]).
//! * `authorization_basis_current` (PRE, re-checked at execution): the
//!   domain's [`EffectDomain::basis`] facts of the observation the
//!   authorization was granted on.
//!
//! Domain facts and runtime facts are evaluated in one kernel decision
//! under the sum types [`RtSubject`] / [`RtValue`].
//!
//! The module depends only on `std`, `serde` and the kernel (a test
//! enforces it).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::kernel::{
    evaluate, Decision, DecisionRecord, EvidenceBase, Fact, Obligation, Phase, Proposed,
    Provenance, Requirement, Semantic, Status, Strength, Verdict, Verified,
};

// ------------------------------------------------------------ vocabulary

/// Facts about the runtime itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSubject {
    /// Whether every transition so far was accepted.
    Transitions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "value", rename_all = "snake_case")]
pub enum RuntimeValue {
    Accepted,
    Unaccepted { reason: String },
}

/// A subject of one decision: the runtime's own, or the domain's.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", content = "subject", rename_all = "snake_case")]
pub enum RtSubject<S> {
    Runtime(RuntimeSubject),
    Domain(S),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", content = "value", rename_all = "snake_case")]
pub enum RtValue<V> {
    Runtime(RuntimeValue),
    Domain(V),
}

// Domain facts print as themselves, so explanations read as before.
impl<S: fmt::Debug> fmt::Debug for RtSubject<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtSubject::Runtime(s) => write!(f, "runtime.{s:?}"),
            RtSubject::Domain(s) => s.fmt(f),
        }
    }
}

impl<V: fmt::Debug> fmt::Debug for RtValue<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtValue::Runtime(v) => write!(f, "runtime.{v:?}"),
            RtValue::Domain(v) => v.fmt(f),
        }
    }
}

type Sub<D> = RtSubject<<D as EffectDomain>::Subject>;
type Val<D> = RtValue<<D as EffectDomain>::Value>;
pub type RtObligation<D> = Obligation<Sub<D>, Val<D>>;
pub type RtDecision<D> = Decision<Sub<D>, Val<D>>;
pub type RtEvidence<D> = EvidenceBase<Sub<D>, Val<D>>;
pub type DomainEvidence<D> = EvidenceBase<<D as EffectDomain>::Subject, <D as EffectDomain>::Value>;
pub type DomainFact<D> = Fact<<D as EffectDomain>::Subject, <D as EffectDomain>::Value>;
pub type DomainObligation<D> = Obligation<<D as EffectDomain>::Subject, <D as EffectDomain>::Value>;

/// Invariant id of the runtime's acceptance obligation.
pub const ACCEPTED: &str = "transitions_accepted";
/// Invariant id of the authorization basis obligations.
pub const BASIS: &str = "authorization_basis_current";

// ------------------------------------------------------------ domain

/// Where in the lifecycle obligations are being derived.
pub enum Stage<'a, D: EffectDomain + ?Sized> {
    /// Before authority is granted, against the trusted observation.
    Pre {
        proposal: &'a D::Proposal,
        before: &'a D::Observation,
    },
    /// After an authorized effect ran.
    Post {
        proposal: &'a D::Proposal,
        before: &'a D::Observation,
        /// `None` if the result could not be observed.
        after: Option<&'a D::Observation>,
        receipt: &'a D::Receipt,
    },
    /// After a compensation ran.
    Compensated {
        after: Option<&'a D::Observation>,
        receipt: &'a D::Receipt,
    },
}

/// What a domain makes of an effect once its result has been observed.
pub struct Concluded<D: EffectDomain + ?Sized> {
    /// The domain's account of the effect (returned to the caller and
    /// passed to POST obligation derivation and evidence).
    pub receipt: D::Receipt,
    /// Released to the caller only if the POST verdict accepts.
    pub output: Option<D::Output>,
    /// Facts the effect established that stay true (keyed so they cannot
    /// go stale, e.g. by version). Kept for later decisions.
    pub durable: DomainEvidence<D>,
}

/// One kind of reality the runtime can guard.
///
/// A domain is trusted code: its observer attests verified facts. It
/// carries no lifecycle: it never decides whether something may run,
/// whether a result is accepted or what happens after a rejection.
#[allow(async_fn_in_trait)]
pub trait EffectDomain {
    type Subject: Ord + Clone + fmt::Debug;
    type Value: PartialEq + Clone + fmt::Debug;
    /// Plain data from a proposer; carries no authority.
    type Proposal: fmt::Debug;
    /// A snapshot of reality.
    type Observation: Clone;
    /// What `execute` (or `compensate`) hands back before the result is
    /// observed.
    type Effect;
    type Receipt;
    type Output;
    type Error: From<RuntimeFault> + fmt::Display;

    /// Human-readable action label for the journal.
    fn label(&self, proposal: &Self::Proposal) -> String;

    /// Digest of the domain's trusted, immutable policy. Authorizations
    /// are bound to it.
    fn policy_digest(&self) -> String;

    /// Observe reality now.
    async fn observe(&self) -> Result<Self::Observation, Self::Error>;

    /// Facts that identify `observation` for freshness: if reality still
    /// has these values, an authorization granted on `observation` is
    /// still current.
    fn basis(&self, observation: &Self::Observation) -> Vec<DomainFact<Self>>;

    /// The domain's invariants at `stage`.
    fn obligations(&self, stage: Stage<'_, Self>) -> Vec<DomainObligation<Self>>;

    /// Verified evidence about `subjects`, from `observation` and/or a
    /// receipt. Must not run effects.
    async fn evidence(
        &self,
        subjects: &[&Self::Subject],
        observation: Option<&Self::Observation>,
        receipt: Option<&Self::Receipt>,
    ) -> DomainEvidence<Self>;

    /// Perform an authorized proposal. `before` is the fresh observation
    /// its authorization was just re-checked on.
    async fn execute(
        &mut self,
        proposal: &Self::Proposal,
        before: &Self::Observation,
    ) -> Result<Self::Effect, Self::Error>;

    /// Account for an effect, given the observations around it.
    async fn conclude(
        &mut self,
        effect: Self::Effect,
        before: &Self::Observation,
        after: Result<&Self::Observation, String>,
    ) -> Result<Concluded<Self>, Self::Error>;
}

/// A domain that can try to undo rejected transitions. Not every domain
/// can; without it a rejection holds the runtime for good.
#[allow(async_fn_in_trait)]
pub trait Compensable: EffectDomain {
    /// Journal label of a compensation.
    const LABEL: &'static str = "compensate";

    /// Start a compensation from the current reality `before`. Whether it
    /// worked is judged like any effect, by the `Compensated` obligations.
    async fn compensate(&mut self, before: &Self::Observation)
        -> Result<Self::Effect, Self::Error>;
}

/// Where the runtime records its decisions.
#[allow(async_fn_in_trait)]
pub trait Journal {
    type Error;
    async fn record(&self, record: DecisionRecord) -> Result<(), Self::Error>;
}

/// A journal that keeps decisions in memory.
#[derive(Debug, Default)]
pub struct MemoryJournal {
    records: Mutex<Vec<DecisionRecord>>,
}

impl MemoryJournal {
    pub fn records(&self) -> Vec<DecisionRecord> {
        self.records.lock().expect("journal lock").clone()
    }
}

impl Journal for MemoryJournal {
    type Error = std::convert::Infallible;

    async fn record(&self, record: DecisionRecord) -> Result<(), Self::Error> {
        self.records.lock().expect("journal lock").push(record);
        Ok(())
    }
}

/// Lifecycle errors that are the runtime's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeFault {
    /// The authorization was minted by another runtime or under another
    /// policy.
    ForeignAuthorization,
}

impl fmt::Display for RuntimeFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeFault::ForeignAuthorization => {
                f.write_str("authorization belongs to another runtime or policy")
            }
        }
    }
}

// ------------------------------------------------------------ authority

/// Single-use permission to execute one proposal on one observed state
/// under one policy. Private fields, no `Clone`, no `Deserialize`;
/// [`Runtime::execute`] consumes it.
pub struct Authorization<D: EffectDomain> {
    runtime: u64,
    policy: String,
    proposal: D::Proposal,
    obligations: Vec<RtObligation<D>>,
    decision: RtDecision<D>,
}

impl<D: EffectDomain> Authorization<D> {
    pub fn proposal(&self) -> &D::Proposal {
        &self.proposal
    }

    pub fn decision(&self) -> &RtDecision<D> {
        &self.decision
    }
}

impl<D: EffectDomain> fmt::Debug for Authorization<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authorization")
            .field("proposal", &self.proposal)
            .field("decision", &self.decision)
            .finish_non_exhaustive()
    }
}

pub enum Authorize<D: EffectDomain> {
    Allowed(Box<Authorization<D>>),
    Denied(RtDecision<D>),
    Blocked(RtDecision<D>),
}

impl<D: EffectDomain> Authorize<D> {
    pub fn verdict(&self) -> Verdict {
        match self {
            Authorize::Allowed(_) => Verdict::Allow,
            Authorize::Denied(_) => Verdict::Deny,
            Authorize::Blocked(_) => Verdict::Blocked,
        }
    }

    pub fn decision(&self) -> &RtDecision<D> {
        match self {
            Authorize::Allowed(auth) => &auth.decision,
            Authorize::Denied(d) | Authorize::Blocked(d) => d,
        }
    }
}

impl<D: EffectDomain> fmt::Debug for Authorize<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Authorize::Allowed(auth) => f.debug_tuple("Allowed").field(auth).finish(),
            Authorize::Denied(d) => f.debug_tuple("Denied").field(d).finish(),
            Authorize::Blocked(d) => f.debug_tuple("Blocked").field(d).finish(),
        }
    }
}

/// What happened to an authorized proposal (or a compensation).
pub struct Report<D: EffectDomain> {
    /// False if execution was refused (stale authorization).
    pub executed: bool,
    pub receipt: Option<D::Receipt>,
    /// The domain's output, only if the transition was accepted.
    pub output: Option<D::Output>,
    pub accepted: bool,
    /// POST decision if the effect ran; otherwise the execution-time
    /// re-evaluation that refused it.
    pub decision: RtDecision<D>,
}

// ------------------------------------------------------------ runtime

static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

pub struct Runtime<D: EffectDomain, J: Journal> {
    id: u64,
    domain: D,
    journal: J,
    /// The last observation the runtime accepted.
    trusted: D::Observation,
    /// Durable domain evidence (effect-established, semantic, proposed).
    durable: DomainEvidence<D>,
    unaccepted: Option<String>,
}

impl<D, J> Runtime<D, J>
where
    D: EffectDomain,
    J: Journal,
    D::Error: From<J::Error>,
{
    /// `baseline` is the initial trusted observation, supplied by the
    /// privileged code that set the domain up.
    pub fn new(domain: D, journal: J, baseline: D::Observation) -> Self {
        Self {
            id: NEXT_RUNTIME.fetch_add(1, Ordering::Relaxed),
            domain,
            journal,
            trusted: baseline,
            durable: EvidenceBase::new(),
            unaccepted: None,
        }
    }

    pub fn domain(&self) -> &D {
        &self.domain
    }

    pub fn journal(&self) -> &J {
        &self.journal
    }

    /// The last accepted observation.
    pub fn trusted(&self) -> &D::Observation {
        &self.trusted
    }

    pub fn is_accepting(&self) -> bool {
        self.unaccepted.is_none()
    }

    /// Record semantic evidence. It can satisfy only `Soft` obligations.
    pub fn add_semantic(&mut self, fact: Semantic<DomainFact<D>>) {
        self.durable.add_semantic(fact);
    }

    /// Record a claim. Informational only.
    pub fn add_proposed(&mut self, fact: Proposed<DomainFact<D>>) {
        self.durable.add_proposed(fact);
    }

    /// Derive and evaluate PRE obligations against the trusted state.
    /// Only `Allow` yields authority.
    pub async fn authorize(&mut self, proposal: D::Proposal) -> Result<Authorize<D>, D::Error> {
        let label = self.domain.label(&proposal);
        let before = &self.trusted;
        let mut obligations: Vec<RtObligation<D>> = self
            .domain
            .obligations(Stage::Pre {
                proposal: &proposal,
                before,
            })
            .into_iter()
            .map(lift)
            .collect();
        obligations.push(accepted_obligation());
        obligations.extend(self.basis(before));
        let evidence = self.evidence(&obligations, Some(before), None).await;
        let decision = evaluate(Phase::Pre, &obligations, &evidence);
        self.log(&label, &decision).await?;
        Ok(match decision.verdict {
            Verdict::Allow => Authorize::Allowed(Box::new(Authorization {
                runtime: self.id,
                policy: self.domain.policy_digest(),
                proposal,
                obligations,
                decision,
            })),
            Verdict::Deny => Authorize::Denied(decision),
            Verdict::Blocked => Authorize::Blocked(decision),
        })
    }

    /// Re-check an authorization on fresh evidence, execute it, observe
    /// the result and judge it.
    pub async fn execute(&mut self, auth: Authorization<D>) -> Result<Report<D>, D::Error> {
        if auth.runtime != self.id || auth.policy != self.domain.policy_digest() {
            return Err(RuntimeFault::ForeignAuthorization.into());
        }
        let label = self.domain.label(&auth.proposal);
        let before = self.domain.observe().await?;
        let evidence = self.evidence(&auth.obligations, Some(&before), None).await;
        let fresh = evaluate(Phase::Pre, &auth.obligations, &evidence);
        if fresh.verdict != Verdict::Allow {
            self.log(&label, &fresh).await?;
            // A stale authorization alone is not a reason to hold: an
            // accepted step may have superseded its basis. Drift of reality
            // away from the trusted state is.
            let drift = self.basis(&self.trusted);
            let evidence = self.evidence(&drift, Some(&before), None).await;
            if evaluate(Phase::Pre, &drift, &evidence).verdict != Verdict::Allow {
                self.unaccepted
                    .get_or_insert_with(|| "state changed outside authorized actions".to_string());
            }
            return Ok(Report {
                executed: false,
                receipt: None,
                output: None,
                accepted: false,
                decision: fresh,
            });
        }
        let effect = self.domain.execute(&auth.proposal, &before).await?;
        self.settle(Some(&auth.proposal), &label, effect, &before)
            .await
    }

    async fn settle(
        &mut self,
        proposal: Option<&D::Proposal>,
        label: &str,
        effect: D::Effect,
        before: &D::Observation,
    ) -> Result<Report<D>, D::Error> {
        let after = self.domain.observe().await;
        let concluded = self
            .domain
            .conclude(effect, before, after.as_ref().map_err(ToString::to_string))
            .await?;
        self.durable.merge(&concluded.durable);
        let after = after.ok();
        let stage = match proposal {
            Some(proposal) => Stage::Post {
                proposal,
                before,
                after: after.as_ref(),
                receipt: &concluded.receipt,
            },
            None => Stage::Compensated {
                after: after.as_ref(),
                receipt: &concluded.receipt,
            },
        };
        let obligations: Vec<RtObligation<D>> = self
            .domain
            .obligations(stage)
            .into_iter()
            .map(lift)
            .collect();
        let evidence = self
            .evidence(&obligations, after.as_ref(), Some(&concluded.receipt))
            .await;
        let decision = evaluate(Phase::Post, &obligations, &evidence);
        self.log(label, &decision).await?;
        let accepted = match after {
            Some(after) if decision.verdict == Verdict::Allow => {
                self.trusted = after;
                if proposal.is_none() {
                    self.unaccepted = None;
                }
                true
            }
            Some(_) => {
                self.unaccepted = Some(rejection(&decision));
                false
            }
            None => {
                self.unaccepted = Some("post-state unobservable".to_string());
                false
            }
        };
        Ok(Report {
            executed: true,
            receipt: Some(concluded.receipt),
            output: if accepted { concluded.output } else { None },
            accepted,
            decision,
        })
    }

    fn basis(&self, observation: &D::Observation) -> Vec<RtObligation<D>> {
        self.domain
            .basis(observation)
            .into_iter()
            .map(|fact| Obligation {
                invariant: BASIS.to_string(),
                phase: Phase::Pre,
                requirement: Requirement::Fact {
                    subject: RtSubject::Domain(fact.subject),
                    value: RtValue::Domain(fact.value),
                    strength: Strength::Hard,
                },
            })
            .collect()
    }

    /// Durable evidence, the domain's fresh evidence for every domain
    /// subject the obligations mention, and the runtime's own facts.
    async fn evidence(
        &self,
        obligations: &[RtObligation<D>],
        observation: Option<&D::Observation>,
        receipt: Option<&D::Receipt>,
    ) -> RtEvidence<D> {
        let mut domain_subjects = Vec::new();
        let mut runtime_subjects = Vec::new();
        for obligation in obligations {
            if let Requirement::Fact { subject, .. } = &obligation.requirement {
                match subject {
                    RtSubject::Domain(s) => domain_subjects.push(s),
                    RtSubject::Runtime(s) => runtime_subjects.push(s),
                }
            }
        }
        let mut domain = self.durable.clone();
        domain.merge(
            &self
                .domain
                .evidence(&domain_subjects, observation, receipt)
                .await,
        );
        let mut evidence = domain.map(
            |s| RtSubject::Domain(s.clone()),
            |v| RtValue::Domain(v.clone()),
        );
        if runtime_subjects.contains(&&RuntimeSubject::Transitions) {
            let value = match &self.unaccepted {
                None => RuntimeValue::Accepted,
                Some(reason) => RuntimeValue::Unaccepted {
                    reason: reason.clone(),
                },
            };
            evidence.add_verified(
                Verified::attest(Fact {
                    subject: RtSubject::Runtime(RuntimeSubject::Transitions),
                    value: RtValue::Runtime(value),
                }),
                Provenance {
                    observer: "runtime-verdicts".to_string(),
                    basis: "post-execution decisions of this runtime".to_string(),
                },
            );
        }
        evidence
    }

    async fn log(&self, action: &str, decision: &RtDecision<D>) -> Result<(), D::Error> {
        self.journal.record(decision.record(action)).await?;
        Ok(())
    }
}

impl<D, J> Runtime<D, J>
where
    D: Compensable,
    J: Journal,
    D::Error: From<J::Error>,
{
    /// Try to undo the rejected transitions and judge the result. On
    /// `Allow` the hold is released and the observed result becomes the
    /// trusted state. Needs no authorization: it is the runtime's own
    /// recovery action, and the domain's `Compensated` obligations decide
    /// whether it worked.
    pub async fn compensate(&mut self) -> Result<Report<D>, D::Error> {
        let before = self.domain.observe().await?;
        let effect = self.domain.compensate(&before).await?;
        self.settle(None, D::LABEL, effect, &before).await
    }
}

fn lift<S, V>(obligation: Obligation<S, V>) -> Obligation<RtSubject<S>, RtValue<V>> {
    Obligation {
        invariant: obligation.invariant,
        phase: obligation.phase,
        requirement: match obligation.requirement {
            Requirement::Fact {
                subject,
                value,
                strength,
            } => Requirement::Fact {
                subject: RtSubject::Domain(subject),
                value: RtValue::Domain(value),
                strength,
            },
            Requirement::Within { names, scopes } => Requirement::Within { names, scopes },
            Requirement::AtMost {
                quantity,
                value,
                limit,
            } => Requirement::AtMost {
                quantity,
                value,
                limit,
            },
        },
    }
}

fn accepted_obligation<S, V>() -> Obligation<RtSubject<S>, RtValue<V>> {
    Obligation {
        invariant: ACCEPTED.to_string(),
        phase: Phase::Pre,
        requirement: Requirement::Fact {
            subject: RtSubject::Runtime(RuntimeSubject::Transitions),
            value: RtValue::Runtime(RuntimeValue::Accepted),
            strength: Strength::Hard,
        },
    }
}

fn rejection<S, V>(decision: &Decision<S, V>) -> String {
    let reasons: Vec<&str> = decision
        .findings
        .iter()
        .filter(|f| !matches!(f.status, Status::Satisfied(_)))
        .map(|f| f.obligation.invariant.as_str())
        .collect();
    format!(
        "post-execution verdict {:?}: {}",
        decision.verdict,
        reasons.join(", ")
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_depends_on_no_domain_module() {
        // Architecture guard: the runtime may use the kernel, nothing else
        // from this crate.
        let source = include_str!("runtime.rs")
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
                || line.contains("crate::kernel::")),
            "{imports:?}"
        );
        // Domain vocabulary must not appear in code (comments may explain).
        let code: String = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        for word in ["path", "file", "commit", "git", "workspace", "ref", "refs"] {
            assert!(
                !code
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|t| t == word),
                "runtime code mentions `{word}`"
            );
        }
    }
}
