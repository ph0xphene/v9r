//! Guarded execution: the narrow path where proposals must pass the
//! invariant kernel before they get execution authority.
//!
//! ```text
//! ActionProposal ──authorize──▶ PRE obligations ──kernel──▶ Allow ─▶ Authorization
//!                                                          Deny / Blocked (nothing runs)
//! Authorization ──execute──▶ freshness check ─▶ run ─▶ receipt ─▶ POST obligations ──kernel──▶ verdict
//! ```
//!
//! * A proposal is plain data with no authority. Only
//!   [`GuardedTask::authorize`] creates an [`Authorization`]: it has
//!   private fields, no `Clone`, no `Deserialize`, and
//!   [`GuardedTask::execute`] consumes it, so it is single-use and cannot
//!   be forged, replayed or copied.
//! * An authorization is bound to the workspace version it was granted
//!   on. If the workspace changed before execution, execution is refused
//!   and the change is treated as unaccepted.
//! * A POST verdict other than `Allow` marks the workspace unaccepted:
//!   every later proposal is denied (`workspace_accepted`) until
//!   [`GuardedTask::rollback`] restores the checkpoint and the kernel
//!   accepts the restoration.
//! * The policy is fixed at [`GuardedTask::start`]; nothing on this path
//!   can change it.
//!
//! The legacy paths (`run_task_step`, `run_observed_step`, the adapter
//! loop) are untouched and do not consult the kernel.
//!
//! ```compile_fail
//! // Authorizations cannot be deserialized…
//! let a: v9r_core::guarded::Authorization = serde_json::from_str("{}").unwrap();
//! ```
//!
//! ```compile_fail
//! // …or copied for replay.
//! fn replay(a: &v9r_core::guarded::Authorization) -> v9r_core::guarded::Authorization {
//!     a.clone()
//! }
//! ```
//!
//! ```compile_fail
//! // Evidence bases cannot be deserialized into existence.
//! let e: v9r_core::facts::RuntimeEvidence = serde_json::from_str("{}").unwrap();
//! ```
//!
//! ```no_run
//! // Control: proposals, semantic evidence and records are plain data.
//! use v9r_core::{effect::ReceiptRecord, facts::RuntimeFact, kernel::Semantic, policy::ActionProposal};
//! let _: ActionProposal = serde_json::from_str("{}").unwrap();
//! let _: Semantic<RuntimeFact> = serde_json::from_str("{}").unwrap();
//! let _: ReceiptRecord = serde_json::from_str("{}").unwrap();
//! ```

use std::sync::Arc;

use uuid::Uuid;

use crate::bundle::{self, BundleError};
use crate::effect::{EffectReceipt, Observation, ReceiptRecord, SemanticOracle};
use crate::execution::{self, ExecutionError, StepOutput};
use crate::facts::{self, RuntimeEvidence, RuntimeFact, Subject, Value};
use crate::kernel::{
    evaluate, Decision, Obligation, Phase, Requirement, Semantic, Strength, Verdict,
};
use crate::manifest::normalize_path;
use crate::policy::{ActionProposal, Context, Policy, ProposedAction, Stage};
use crate::state::ContentHash;
use crate::task::Task;
use crate::trace::{TaskEvent, TraceError, TraceLogger};
use crate::trusted::{StateRoot, TrustError};
use crate::vfs::{self, CheckpointId, TransactionError};

pub type RuntimeDecision = Decision<Subject, Value>;

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("transaction: {0}")]
    Transaction(#[from] TransactionError),
    #[error("execution: {0}")]
    Execution(#[from] ExecutionError),
    #[error("trace: {0}")]
    Trace(#[from] TraceError),
    #[error("trusted state: {0}")]
    Trust(#[from] TrustError),
    #[error("authorization belongs to another task or policy")]
    ForeignAuthorization,
}

pub type Result<T> = std::result::Result<T, GuardError>;

/// Single-use permission to execute one proposal on one workspace
/// version under one policy.
#[derive(Debug)]
pub struct Authorization {
    task_id: Uuid,
    policy: ContentHash,
    basis: ContentHash,
    proposal: ActionProposal,
    decision: RuntimeDecision,
}

impl Authorization {
    pub fn proposal(&self) -> &ActionProposal {
        &self.proposal
    }

    pub fn decision(&self) -> &RuntimeDecision {
        &self.decision
    }

    /// Workspace version the authorization was granted on.
    pub fn basis(&self) -> ContentHash {
        self.basis
    }
}

#[derive(Debug)]
pub enum Authorize {
    Allowed(Box<Authorization>),
    Denied(RuntimeDecision),
    Blocked(RuntimeDecision),
}

impl Authorize {
    pub fn verdict(&self) -> Verdict {
        match self {
            Authorize::Allowed(_) => Verdict::Allow,
            Authorize::Denied(_) => Verdict::Deny,
            Authorize::Blocked(_) => Verdict::Blocked,
        }
    }

    pub fn decision(&self) -> &RuntimeDecision {
        match self {
            Authorize::Allowed(auth) => &auth.decision,
            Authorize::Denied(d) | Authorize::Blocked(d) => d,
        }
    }
}

/// What happened to an authorized action.
#[derive(Debug)]
pub struct StepReport {
    /// False if execution was refused (stale authorization).
    pub executed: bool,
    /// The command's own report, for command actions that ran.
    pub result: Option<std::result::Result<StepOutput, ExecutionError>>,
    pub receipt: Option<EffectReceipt>,
    pub bundle: Option<Vec<u8>>,
    /// The judging decision: POST if the action ran, otherwise the PRE
    /// decision that refused it.
    pub decision: RuntimeDecision,
}

pub struct GuardedTask {
    task: Task,
    policy: Arc<Policy>,
    trace: TraceLogger,
    checkpoint: CheckpointId,
    checkpoint_observation: Observation,
    current: Observation,
    durable: RuntimeEvidence,
    unaccepted: Option<String>,
}

impl GuardedTask {
    /// Register the task under `root`, open its trusted trace and take the
    /// checkpoint every later rollback returns to.
    pub async fn start(mut task: Task, policy: Arc<Policy>, root: StateRoot) -> Result<Self> {
        task.manifest = policy.manifest().clone();
        vfs::register_task_with_state(task.id, task.workdir.clone(), root.clone())?;
        root.ensure_task_dir(task.id)?;
        let trace = TraceLogger::for_task(&root, task.id).await?;
        trace
            .log_event(TaskEvent::task_started(task.manifest.clone()))
            .await?;
        let (checkpoint, workdir, state) = vfs::checkpoint_with_state(task.id, &trace).await?;
        let observation = Observation::from_live_scan(&workdir, state);
        Ok(Self {
            task,
            policy,
            trace,
            checkpoint,
            checkpoint_observation: observation.clone(),
            current: observation,
            durable: RuntimeEvidence::new(),
            unaccepted: None,
        })
    }

    pub fn task(&self) -> &Task {
        &self.task
    }

    pub fn trace(&self) -> &TraceLogger {
        &self.trace
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Digest of the last accepted workspace observation.
    pub fn version(&self) -> ContentHash {
        self.current.digest()
    }

    pub fn is_accepting(&self) -> bool {
        self.unaccepted.is_none()
    }

    /// Record semantic evidence. It can satisfy only `Soft` obligations.
    pub fn add_semantic(&mut self, fact: Semantic<RuntimeFact>) {
        self.durable.add_semantic(fact);
    }

    /// Ask an oracle how confident it is that `fact` holds, and record the
    /// answer as semantic evidence about exactly that fact.
    pub fn consult(&mut self, oracle: &dyn SemanticOracle, fact: RuntimeFact) {
        if let Some(confidence_bp) = oracle.confidence(&fact) {
            self.add_semantic(Semantic {
                value: fact,
                source: format!("oracle:{}", oracle.name()),
                confidence_bp,
            });
        }
    }

    /// Ingest a persisted receipt (e.g. from an imported trace) as
    /// proposed claims.
    pub fn ingest_record(&mut self, record: &ReceiptRecord, source: &str) {
        facts::ingest_record(record, source, &mut self.durable);
    }

    /// Derive and evaluate PRE obligations. Only `Allow` yields authority.
    pub async fn authorize(&mut self, proposal: ActionProposal) -> Result<Authorize> {
        let basis = self.version();
        let obligations = self.policy.obligations(&Context {
            manifest: self.policy.manifest(),
            workdir: &self.task.workdir,
            stage: Stage::Pre {
                proposal: &proposal,
                steps_used: self.task.steps_used,
                version: basis,
            },
        });
        let evidence = self.evidence(&obligations, Some(&self.current)).await;
        let decision = evaluate(Phase::Pre, &obligations, &evidence);
        self.log(&proposal.label(), &decision).await?;
        Ok(match decision.verdict {
            Verdict::Allow => Authorize::Allowed(Box::new(Authorization {
                task_id: self.task.id,
                policy: self.policy.digest(),
                basis,
                proposal,
                decision,
            })),
            Verdict::Deny => Authorize::Denied(decision),
            Verdict::Blocked => Authorize::Blocked(decision),
        })
    }

    /// Execute an authorization and judge the outcome.
    pub async fn execute(&mut self, auth: Authorization) -> Result<StepReport> {
        if auth.task_id != self.task.id || auth.policy != self.policy.digest() {
            return Err(GuardError::ForeignAuthorization);
        }
        let label = auth.proposal.label();

        // Freshness: the authorization only covers the version it saw.
        let pre = execution::observe_blocking(normalize_path(&self.task.workdir)).await?;
        let freshness = vec![Obligation {
            invariant: "authorization_basis_current".to_string(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: Subject::Workspace,
                value: Value::Digest { sha256: auth.basis },
                strength: Strength::Hard,
            },
        }];
        let mut evidence = RuntimeEvidence::new();
        facts::from_observation(&pre, [&Subject::Workspace], &mut evidence);
        let fresh = evaluate(Phase::Pre, &freshness, &evidence);
        if fresh.verdict != Verdict::Allow {
            self.log(&label, &fresh).await?;
            self.unaccepted
                .get_or_insert_with(|| "workspace changed outside authorized actions".to_string());
            return Ok(StepReport {
                executed: false,
                result: None,
                receipt: None,
                bundle: None,
                decision: fresh,
            });
        }

        let proposal = auth.proposal;
        match proposal.action.clone() {
            ProposedAction::Command(spec) => {
                let test_run = is_test_command(&self.policy.manifest().test_commands, &label);
                let (step, post) = execution::run_observed_from(
                    &mut self.task,
                    spec,
                    proposal.requested.clone(),
                    &self.trace,
                    &pre,
                )
                .await?;
                if test_run {
                    if let Some((fact, provenance)) =
                        facts::test_outcome(pre.digest(), step.receipt.outcome(), &label)
                    {
                        self.durable.add_verified(fact, provenance);
                    }
                }
                let touched = facts::receipt_names(&step.receipt);
                let obligations = self.policy.obligations(&Context {
                    manifest: self.policy.manifest(),
                    workdir: &self.task.workdir,
                    stage: Stage::PostCommand {
                        proposal: &proposal,
                        touched: &touched,
                    },
                });
                let mut evidence = self.evidence(&obligations, post.as_ref()).await;
                facts::from_receipt(&step.receipt, &mut evidence);
                let decision = evaluate(Phase::Post, &obligations, &evidence);
                self.log(&label, &decision).await?;
                match post {
                    Some(post) if decision.verdict == Verdict::Allow => self.current = post,
                    Some(_) => self.reject(&decision),
                    None => {
                        self.unaccepted = Some("post-state unobservable".to_string());
                    }
                }
                Ok(StepReport {
                    executed: true,
                    result: Some(step.result),
                    receipt: Some(step.receipt),
                    bundle: None,
                    decision,
                })
            }
            ProposedAction::Export => {
                let bundle = bundle::export_bundle_with_trace(&self.task, &self.trace);
                let obligations = self.policy.obligations(&Context {
                    manifest: self.policy.manifest(),
                    workdir: &self.task.workdir,
                    stage: Stage::PostExport,
                });
                let evidence = self.evidence(&obligations, None).await;
                let decision = evaluate(Phase::Post, &obligations, &evidence);
                self.log(&label, &decision).await?;
                let bundle = match (decision.verdict, bundle) {
                    (Verdict::Allow, Ok(bytes)) => Some(bytes),
                    (_, Err(err)) => return Err(bundle_error(err)),
                    _ => None,
                };
                Ok(StepReport {
                    executed: true,
                    result: None,
                    receipt: None,
                    bundle,
                    decision,
                })
            }
        }
    }

    /// Restore the checkpoint and judge the restoration (I3, I4). The
    /// receipts of earlier steps stay in the trace; the rollback gets its
    /// own.
    pub async fn rollback(&mut self) -> Result<StepReport> {
        let rolled = vfs::rollback_observed(self.task.id, self.checkpoint, &self.trace).await?;
        let checkpoint = self.checkpoint_observation.digest();
        let obligations = self.policy.obligations(&Context {
            manifest: self.policy.manifest(),
            workdir: &self.task.workdir,
            stage: Stage::PostRollback { checkpoint },
        });
        let mut evidence = self.evidence(&obligations, None).await;
        facts::from_receipt(&rolled.receipt, &mut evidence);
        let decision = evaluate(Phase::Post, &obligations, &evidence);
        self.log("rollback", &decision).await?;
        if decision.verdict == Verdict::Allow {
            // The verified post-state digest equals the checkpoint's, so the
            // checkpoint observation describes the workspace exactly.
            self.current = self.checkpoint_observation.clone();
            self.unaccepted = None;
        } else {
            self.reject(&decision);
        }
        Ok(StepReport {
            executed: true,
            result: None,
            receipt: Some(rolled.receipt),
            bundle: None,
            decision,
        })
    }

    fn reject(&mut self, decision: &RuntimeDecision) {
        let reasons: Vec<String> = decision
            .violated()
            .chain(decision.undetermined())
            .map(|f| f.obligation.invariant.clone())
            .collect();
        self.unaccepted = Some(format!(
            "post-execution verdict {:?}: {}",
            decision.verdict,
            reasons.join(", ")
        ));
    }

    /// Durable evidence plus fresh facts for every subject the obligations
    /// mention.
    async fn evidence(
        &self,
        obligations: &[Obligation<Subject, Value>],
        observation: Option<&Observation>,
    ) -> RuntimeEvidence {
        let subjects: Vec<&Subject> = obligations
            .iter()
            .filter_map(|o| match &o.requirement {
                Requirement::Fact { subject, .. } => Some(subject),
                _ => None,
            })
            .collect();
        let mut evidence = self.durable.clone();
        if let Some(observation) = observation {
            facts::from_observation(observation, subjects.iter().copied(), &mut evidence);
        }
        if subjects.contains(&&Subject::TrustedState) {
            if let Some((fact, provenance)) = facts::trusted_state(self.task.id, &self.trace).await
            {
                evidence.add_verified(fact, provenance);
            }
        }
        if subjects.contains(&&Subject::Transitions) {
            let (fact, provenance) = facts::transitions(match &self.unaccepted {
                None => Ok(()),
                Some(reason) => Err(reason.clone()),
            });
            evidence.add_verified(fact, provenance);
        }
        evidence
    }

    async fn log(&self, action: &str, decision: &RuntimeDecision) -> Result<()> {
        self.trace
            .log_event(TaskEvent::InvariantDecision {
                record: Box::new(decision.record(action)),
            })
            .await?;
        Ok(())
    }
}

/// Exact or argument-extended match against a configured test command
/// (`cargo test` matches `cargo test --release`, not `cargo testify`).
fn is_test_command(tests: &[String], command: &str) -> bool {
    tests.iter().map(|t| t.trim()).any(|test| {
        !test.is_empty()
            && (command == test
                || command
                    .strip_prefix(test)
                    .is_some_and(|rest| rest.starts_with(' ')))
    })
}

fn bundle_error(err: BundleError) -> GuardError {
    GuardError::Transaction(TransactionError::Safety(format!("export failed: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_matching_is_token_bounded() {
        let tests = vec!["cargo test".to_string()];
        assert!(is_test_command(&tests, "cargo test"));
        assert!(is_test_command(&tests, "cargo test --release"));
        assert!(!is_test_command(&tests, "cargo testify"));
        assert!(!is_test_command(&[String::new()], "anything"));
    }
}
