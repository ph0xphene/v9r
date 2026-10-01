//! Guarded execution, filesystem domain: the narrow path where proposals
//! must pass the invariant kernel before they get execution authority.
//!
//! The lifecycle (authorize → freshness → execute → observe → judge →
//! accept / hold → rollback) is [`crate::runtime`]'s. This module only
//! supplies the filesystem domain to it:
//!
//! * **observe**: a workspace scan ([`Observation`]); its digest is the
//!   authorization basis;
//! * **effects**: command execution, export (a bundle, released only if
//!   accepted), and rollback to the checkpoint as compensation;
//! * **evidence**: observation facts, receipt facts, trusted-state seals,
//!   and test outcomes (durable, keyed by version);
//! * **invariants**: the [`Policy`] in `crate::policy`.
//!
//! [`GuardedTask`] is a facade over `Runtime<FsDomain, TraceLogger>` with
//! the pre-runtime API.
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

use crate::bundle::{self, BundleError};
use crate::effect::{
    Action, EffectReceipt, ExecutionOutcome, Observation, ReceiptRecord, SemanticOracle,
};
use crate::execution::{self, CommandSpec, ExecutionError, StepOutput};
use crate::facts::{self, RuntimeEvidence, RuntimeFact, Subject, Value};
use crate::kernel::{Fact, Name, Semantic};
use crate::manifest::{normalize_path, Manifest};
use crate::policy::{ActionProposal, Context, Policy, ProposedAction, Stage as PolicyStage};
use crate::runtime::{
    self, Compensable, Concluded, DomainObligation, EffectDomain, Runtime, RuntimeFault, Stage,
};
use crate::state::ContentHash;
use crate::task::Task;
use crate::trace::{TaskEvent, TraceError, TraceLogger};
use crate::trusted::{StateRoot, TrustError};
use crate::vfs::{self, CheckpointId, TransactionError};

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

impl From<RuntimeFault> for GuardError {
    fn from(fault: RuntimeFault) -> Self {
        match fault {
            RuntimeFault::ForeignAuthorization => GuardError::ForeignAuthorization,
        }
    }
}

pub type Result<T> = std::result::Result<T, GuardError>;

pub type RuntimeDecision = runtime::RtDecision<FsDomain>;
pub type Authorization = runtime::Authorization<FsDomain>;
pub type Authorize = runtime::Authorize<FsDomain>;

// ------------------------------------------------------------ workspace

/// The task workspace as effect machinery: the sealed trace, the
/// checkpoint, and how commands and rollbacks run in it. Shared by every
/// domain whose reality includes the workspace (filesystem, git).
pub(crate) struct Workspace {
    pub(crate) task: Task,
    pub(crate) trace: TraceLogger,
    checkpoint: CheckpointId,
    /// The checkpointed state (from the checkpoint walk; no extra scan).
    pub(crate) checkpoint_observation: Observation,
}

/// An effect in the workspace, before its result is observed.
pub enum WorkspaceEffect {
    Command {
        action: Action,
        requested: Option<String>,
        label: String,
        outcome: ExecutionOutcome,
        result: std::result::Result<StepOutput, ExecutionError>,
    },
    Export {
        bundle: std::result::Result<Vec<u8>, BundleError>,
    },
    Rollback {
        checkpoint: CheckpointId,
        result: std::result::Result<(), TransactionError>,
    },
}

/// The workspace's account of an effect.
#[derive(Debug)]
pub struct WorkspaceReceipt {
    /// The command's own report, for commands.
    pub result: Option<std::result::Result<StepOutput, ExecutionError>>,
    /// Observed effects, for commands and rollbacks.
    pub receipt: Option<EffectReceipt>,
}

impl Workspace {
    /// Register the task under `root`, open its trusted trace and take the
    /// checkpoint every later rollback returns to.
    pub(crate) async fn open(mut task: Task, manifest: Manifest, root: StateRoot) -> Result<Self> {
        task.manifest = manifest;
        vfs::register_task_with_state(task.id, task.workdir.clone(), root.clone())?;
        root.ensure_task_dir(task.id)?;
        let trace = TraceLogger::for_task(&root, task.id).await?;
        trace
            .log_event(TaskEvent::task_started(task.manifest.clone()))
            .await?;
        let (checkpoint, workdir, state) = vfs::checkpoint_with_state(task.id, &trace).await?;
        Ok(Self {
            task,
            trace,
            checkpoint,
            checkpoint_observation: Observation::from_live_scan(&workdir, state),
        })
    }

    pub(crate) async fn observe(&self) -> Result<Observation> {
        Ok(execution::observe_blocking(normalize_path(&self.task.workdir)).await?)
    }

    pub(crate) async fn run(
        &mut self,
        spec: CommandSpec,
        requested: Option<String>,
    ) -> WorkspaceEffect {
        let label = command_label(&spec);
        let (action, outcome, result) =
            execution::run_command(&mut self.task, spec, &self.trace).await;
        WorkspaceEffect::Command {
            action,
            requested,
            label,
            outcome,
            result,
        }
    }

    pub(crate) async fn rollback(&self) -> WorkspaceEffect {
        WorkspaceEffect::Rollback {
            checkpoint: self.checkpoint,
            result: vfs::rollback(self.task.id, self.checkpoint, &self.trace).await,
        }
    }

    /// Receipt for a workspace effect, logged to the trace; plus the
    /// command's test outcome if it was a configured test command.
    pub(crate) async fn conclude(
        &self,
        effect: WorkspaceEffect,
        before: &Observation,
        after: std::result::Result<&Observation, String>,
    ) -> Result<(WorkspaceReceipt, Option<Vec<u8>>, RuntimeEvidence)> {
        let mut durable = RuntimeEvidence::new();
        match effect {
            WorkspaceEffect::Command {
                action,
                requested,
                label,
                outcome,
                result,
            } => {
                let receipt = execution::record_effect(
                    action,
                    requested,
                    outcome,
                    before,
                    after,
                    &self.trace,
                )
                .await?;
                if is_test_command(&self.task.manifest.test_commands, &label) {
                    if let Some((fact, provenance)) =
                        facts::test_outcome(before.digest(), receipt.outcome(), &label)
                    {
                        durable.add_verified(fact, provenance);
                    }
                }
                Ok((
                    WorkspaceReceipt {
                        result: Some(result),
                        receipt: Some(receipt),
                    },
                    None,
                    durable,
                ))
            }
            WorkspaceEffect::Export { bundle } => {
                let bundle = bundle.map_err(bundle_error)?;
                Ok((
                    WorkspaceReceipt {
                        result: None,
                        receipt: None,
                    },
                    Some(bundle),
                    durable,
                ))
            }
            WorkspaceEffect::Rollback { checkpoint, result } => {
                let receipt = execution::record_effect(
                    Action::Rollback { checkpoint },
                    None,
                    vfs::rollback_outcome(&result),
                    before,
                    after,
                    &self.trace,
                )
                .await?;
                Ok((
                    WorkspaceReceipt {
                        result: None,
                        receipt: Some(receipt),
                    },
                    None,
                    durable,
                ))
            }
        }
    }

    /// Verified workspace facts about `subjects`.
    pub(crate) async fn evidence<'a>(
        &self,
        subjects: impl IntoIterator<Item = &'a Subject> + Clone,
        observation: Option<&Observation>,
        receipt: Option<&WorkspaceReceipt>,
    ) -> RuntimeEvidence {
        let mut evidence = RuntimeEvidence::new();
        if let Some(observation) = observation {
            facts::from_observation(observation, subjects.clone(), &mut evidence);
        }
        if let Some(receipt) = receipt.and_then(|r| r.receipt.as_ref()) {
            facts::from_receipt(receipt, &mut evidence);
        }
        if subjects.into_iter().any(|s| *s == Subject::TrustedState) {
            if let Some((fact, provenance)) = facts::trusted_state(self.task.id, &self.trace).await
            {
                evidence.add_verified(fact, provenance);
            }
        }
        evidence
    }

    /// The workspace digest as the basis fact of `observation`.
    pub(crate) fn basis(observation: &Observation) -> Fact<Subject, Value> {
        Fact {
            subject: Subject::Workspace,
            value: Value::Digest {
                sha256: observation.digest(),
            },
        }
    }
}

// ------------------------------------------------------------ domain

/// The filesystem domain: a workspace plus the v9r runtime policy.
pub struct FsDomain {
    ws: Workspace,
    policy: Arc<Policy>,
}

impl EffectDomain for FsDomain {
    type Subject = Subject;
    type Value = Value;
    type Proposal = ActionProposal;
    type Observation = Observation;
    type Effect = WorkspaceEffect;
    type Receipt = WorkspaceReceipt;
    type Output = Vec<u8>;
    type Error = GuardError;

    fn label(&self, proposal: &ActionProposal) -> String {
        proposal.label()
    }

    fn policy_digest(&self) -> String {
        self.policy.digest().to_string()
    }

    async fn observe(&self) -> Result<Observation> {
        self.ws.observe().await
    }

    fn basis(&self, observation: &Observation) -> Vec<Fact<Subject, Value>> {
        vec![Workspace::basis(observation)]
    }

    fn obligations(&self, stage: Stage<'_, Self>) -> Vec<DomainObligation<Self>> {
        let touched: Vec<Name>;
        let stage = match stage {
            Stage::Pre { proposal, before } => PolicyStage::Pre {
                proposal,
                steps_used: self.ws.task.steps_used,
                version: before.digest(),
            },
            Stage::Post {
                proposal, receipt, ..
            } => match &proposal.action {
                ProposedAction::Command(_) => {
                    touched = receipt
                        .receipt
                        .as_ref()
                        .map(facts::receipt_names)
                        .unwrap_or_else(|| vec![Name::UnknownBelow(String::new())]);
                    PolicyStage::PostCommand {
                        proposal,
                        touched: &touched,
                    }
                }
                ProposedAction::Export => PolicyStage::PostExport,
            },
            Stage::Compensated { .. } => PolicyStage::PostRollback {
                checkpoint: self.ws.checkpoint_observation.digest(),
            },
        };
        self.policy.obligations(&Context {
            manifest: self.policy.manifest(),
            workdir: &self.ws.task.workdir,
            stage,
        })
    }

    async fn evidence(
        &self,
        subjects: &[&Subject],
        observation: Option<&Observation>,
        receipt: Option<&WorkspaceReceipt>,
    ) -> RuntimeEvidence {
        self.ws
            .evidence(subjects.iter().copied(), observation, receipt)
            .await
    }

    async fn execute(
        &mut self,
        proposal: &ActionProposal,
        _before: &Observation,
    ) -> Result<WorkspaceEffect> {
        Ok(match &proposal.action {
            ProposedAction::Command(spec) => {
                self.ws.run(spec.clone(), proposal.requested.clone()).await
            }
            ProposedAction::Export => WorkspaceEffect::Export {
                bundle: bundle::export_bundle_with_trace(&self.ws.task, &self.ws.trace),
            },
        })
    }

    async fn conclude(
        &mut self,
        effect: WorkspaceEffect,
        before: &Observation,
        after: std::result::Result<&Observation, String>,
    ) -> Result<Concluded<Self>> {
        let (receipt, output, durable) = self.ws.conclude(effect, before, after).await?;
        Ok(Concluded {
            receipt,
            output,
            durable,
        })
    }
}

impl Compensable for FsDomain {
    const LABEL: &'static str = "rollback";

    async fn compensate(&mut self, _before: &Observation) -> Result<WorkspaceEffect> {
        Ok(self.ws.rollback().await)
    }
}

// ------------------------------------------------------------ facade

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

impl From<runtime::Report<FsDomain>> for StepReport {
    fn from(report: runtime::Report<FsDomain>) -> Self {
        let (result, receipt) = match report.receipt {
            Some(r) => (r.result, r.receipt),
            None => (None, None),
        };
        Self {
            executed: report.executed,
            result,
            receipt,
            bundle: report.output,
            decision: report.decision,
        }
    }
}

/// The filesystem domain under the runtime, with the pre-runtime API.
pub struct GuardedTask {
    rt: Runtime<FsDomain, TraceLogger>,
}

impl GuardedTask {
    pub async fn start(task: Task, policy: Arc<Policy>, root: StateRoot) -> Result<Self> {
        let ws = Workspace::open(task, policy.manifest().clone(), root).await?;
        let journal = ws.trace.clone();
        let baseline = ws.checkpoint_observation.clone();
        Ok(Self {
            rt: Runtime::new(FsDomain { ws, policy }, journal, baseline),
        })
    }

    pub fn task(&self) -> &Task {
        &self.rt.domain().ws.task
    }

    pub fn trace(&self) -> &TraceLogger {
        self.rt.journal()
    }

    pub fn policy(&self) -> &Policy {
        &self.rt.domain().policy
    }

    /// Digest of the last accepted workspace observation.
    pub fn version(&self) -> ContentHash {
        self.rt.trusted().digest()
    }

    pub fn is_accepting(&self) -> bool {
        self.rt.is_accepting()
    }

    /// Record semantic evidence. It can satisfy only `Soft` obligations.
    pub fn add_semantic(&mut self, fact: Semantic<RuntimeFact>) {
        self.rt.add_semantic(fact);
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
        let mut claims = RuntimeEvidence::new();
        facts::ingest_record(record, source, &mut claims);
        self.rt.absorb(&claims);
    }

    pub async fn authorize(&mut self, proposal: ActionProposal) -> Result<Authorize> {
        self.rt.authorize(proposal).await
    }

    pub async fn execute(&mut self, auth: Authorization) -> Result<StepReport> {
        Ok(self.rt.execute(auth).await?.into())
    }

    /// Restore the checkpoint and judge the restoration (I3, I4).
    pub async fn rollback(&mut self) -> Result<StepReport> {
        Ok(self.rt.compensate().await?.into())
    }
}

// ------------------------------------------------------------ helpers

pub(crate) fn command_label(spec: &CommandSpec) -> String {
    std::iter::once(spec.program.as_str())
        .chain(spec.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
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
