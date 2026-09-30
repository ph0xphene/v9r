//! Guarded git actions: git as a second evidence domain, composed with
//! the filesystem domain under a sum type and judged by the unchanged
//! kernel requirement forms.
//!
//! | Id | Invariant | Phase | Obligations |
//! |---|---|---|---|
//! | `G1.release_descends_from_base` | the release commit descends from the approved base | pre | `Ref(release) = X` (pins the witness), `Descends(base, X) = Yes` |
//! | `G2.protected_refs_unmoved` | only writable ref namespaces move | pre + post | `Within(declared refs)`, `Within(touched refs)` |
//! | `G3.artifact_matches_verified_commit` | the exported artifact has the content of the verified commit | pre | `Ref(release) = X`, `git ContentManifest(X) = D`, `fs ContentManifest(artifact) = D` |
//! | `G2.rollback_restores_refs` | rollback returns every repo's pointers to the checkpoint | post (rollback) | `Refs(repo) = Digest` |
//! | shared | trusted state intact, workspace accepted, well-formed proposal | pre (+post) | as in `crate::policy` |
//!
//! Existential invariants ("there is a release commit X such that …") are
//! discharged by *witnesses* in the proposal (`commit`, `manifest`), each
//! pinned by an obligation that only verified evidence can satisfy. The
//! kernel checks witnesses; it never searches for them. A wrong or
//! malicious witness is denied, a missing or malformed one is rejected by
//! `well_formed_proposal`.
//!
//! An authorization carries its obligations plus *basis* obligations
//! (workspace digest and every repo's ref digest at authorization time).
//! `execute` re-evaluates all of them against fresh evidence, so any change
//! between authorize and execute refuses the action.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::effect::Observation;
use crate::execution::{self, CommandSpec, ExecutionError, StepOutput};
use crate::facts::{self, RuntimeEvidence, Subject, Value};
use crate::git::{GitObservation, GitObserver, GitRepo, GitSubject, GitValue, Oid};
use crate::guarded::GuardError;
use crate::kernel::{
    evaluate, Decision, EvidenceBase, Invariant, Name, Obligation, Phase, Requirement, Strength,
    Verdict,
};
use crate::manifest::{normalize_path, Manifest};
use crate::policy::workspace_name;
use crate::state::ContentHash;
use crate::task::Task;
use crate::trace::{TaskEvent, TraceLogger};
use crate::trusted::StateRoot;
use crate::vfs::{self, CheckpointId};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "domain", content = "subject", rename_all = "snake_case")]
pub enum CrossSubject {
    Fs(Subject),
    Git(GitSubject),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "domain", content = "value", rename_all = "snake_case")]
pub enum CrossValue {
    Fs(Value),
    Git(GitValue),
}

pub type CrossDecision = Decision<CrossSubject, CrossValue>;
pub type CrossEvidence = EvidenceBase<CrossSubject, CrossValue>;
type CrossObligation = Obligation<CrossSubject, CrossValue>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum GitAction {
    Command(CommandSpec),
    /// Declare `artifact` (a workspace directory) the release of `commit`.
    /// `commit` and `manifest` are witnesses; neither is trusted.
    Release {
        commit: String,
        artifact: String,
        manifest: ContentHash,
    },
}

/// Plain data from the agent. Carries no authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitProposal {
    pub action: GitAction,
    /// Refs the action intends to move, as `<repo>/<refname>`.
    #[serde(default)]
    pub declared_refs: Vec<String>,
    pub proposer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReleasePolicy {
    pub repo: String,
    pub release_ref: String,
    pub approved_base: Oid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitInvariant {
    WellFormedProposal,
    WorkspaceAccepted,
    TrustedStateIntact,
    ReleaseDescendsFromBase,
    ProtectedRefsUnmoved,
    ArtifactMatchesVerifiedCommit,
    RollbackRestoresRefs,
}

impl GitInvariant {
    pub const ALL: [GitInvariant; 7] = [
        GitInvariant::WellFormedProposal,
        GitInvariant::WorkspaceAccepted,
        GitInvariant::TrustedStateIntact,
        GitInvariant::ReleaseDescendsFromBase,
        GitInvariant::ProtectedRefsUnmoved,
        GitInvariant::ArtifactMatchesVerifiedCommit,
        GitInvariant::RollbackRestoresRefs,
    ];
}

/// Trusted, immutable git policy.
#[derive(Debug, Serialize)]
pub struct GitPolicy {
    manifest: Manifest,
    repos: Vec<GitRepo>,
    release: ReleasePolicy,
    writable_refs: Vec<String>,
    invariants: Vec<GitInvariant>,
    #[serde(skip)]
    digest: Option<ContentHash>,
}

impl GitPolicy {
    pub fn new(
        manifest: Manifest,
        repos: Vec<GitRepo>,
        release: ReleasePolicy,
        writable_refs: Vec<String>,
        invariants: Vec<GitInvariant>,
    ) -> Self {
        let mut policy = Self {
            manifest,
            repos,
            release,
            writable_refs,
            invariants,
            digest: None,
        };
        policy.digest = Some(ContentHash::of(
            &serde_json::to_vec(&policy).expect("policy serialization is infallible"),
        ));
        policy
    }

    pub fn standard(
        manifest: Manifest,
        repos: Vec<GitRepo>,
        release: ReleasePolicy,
        writable_refs: Vec<String>,
    ) -> Self {
        Self::new(
            manifest,
            repos,
            release,
            writable_refs,
            GitInvariant::ALL.to_vec(),
        )
    }

    pub fn digest(&self) -> ContentHash {
        self.digest.expect("set at construction")
    }

    pub fn obligations(&self, ctx: &GitContext<'_>) -> Vec<CrossObligation> {
        self.invariants
            .iter()
            .flat_map(|i| i.obligations(ctx))
            .collect()
    }
}

pub enum GitStage<'a> {
    Pre {
        proposal: &'a GitProposal,
    },
    PostCommand {
        touched: &'a [Name],
    },
    PostRollback {
        checkpoint_refs: &'a BTreeMap<String, ContentHash>,
    },
}

pub struct GitContext<'a> {
    pub policy: &'a GitPolicy,
    pub workdir: &'a Path,
    pub stage: GitStage<'a>,
}

fn hard(subject: CrossSubject, value: CrossValue) -> Requirement<CrossSubject, CrossValue> {
    Requirement::Fact {
        subject,
        value,
        strength: Strength::Hard,
    }
}

fn ref_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

impl Invariant<GitContext<'_>, CrossSubject, CrossValue> for GitInvariant {
    fn id(&self) -> &str {
        match self {
            GitInvariant::WellFormedProposal => "well_formed_proposal",
            GitInvariant::WorkspaceAccepted => "workspace_accepted",
            GitInvariant::TrustedStateIntact => "I4.trusted_state_intact",
            GitInvariant::ReleaseDescendsFromBase => "G1.release_descends_from_base",
            GitInvariant::ProtectedRefsUnmoved => "G2.protected_refs_unmoved",
            GitInvariant::ArtifactMatchesVerifiedCommit => "G3.artifact_matches_verified_commit",
            GitInvariant::RollbackRestoresRefs => "G2.rollback_restores_refs",
        }
    }

    fn obligations(&self, ctx: &GitContext<'_>) -> Vec<CrossObligation> {
        use GitInvariant as G;
        use GitStage as S;
        let one = |phase, requirement| {
            vec![Obligation {
                invariant: self.id().to_string(),
                phase,
                requirement,
            }]
        };
        let release = &ctx.policy.release;
        let pin = |commit: &Oid| {
            hard(
                CrossSubject::Git(GitSubject::Ref {
                    repo: release.repo.clone(),
                    name: release.release_ref.clone(),
                }),
                CrossValue::Git(GitValue::Points {
                    oid: commit.clone(),
                }),
            )
        };
        match (self, &ctx.stage) {
            (G::WellFormedProposal, S::Pre { proposal }) => {
                let mut malformed = proposal
                    .declared_refs
                    .iter()
                    .filter(|r| !ref_name_ok(r))
                    .count();
                if let GitAction::Release {
                    commit, artifact, ..
                } = &proposal.action
                {
                    malformed += usize::from(Oid::parse(commit).is_none());
                    malformed += usize::from(
                        workspace_name(ctx.workdir, Path::new(artifact))
                            .is_none_or(|name| name.is_empty()),
                    );
                }
                one(
                    Phase::Pre,
                    Requirement::AtMost {
                        quantity: "malformed witnesses".to_string(),
                        value: malformed as u64,
                        limit: 0,
                    },
                )
            }
            (G::WorkspaceAccepted, S::Pre { .. }) => one(
                Phase::Pre,
                hard(
                    CrossSubject::Fs(Subject::Transitions),
                    CrossValue::Fs(Value::Accepted),
                ),
            ),
            (G::TrustedStateIntact, stage) => one(
                if matches!(stage, S::Pre { .. }) {
                    Phase::Pre
                } else {
                    Phase::Post
                },
                hard(
                    CrossSubject::Fs(Subject::TrustedState),
                    CrossValue::Fs(Value::Intact),
                ),
            ),
            (G::ReleaseDescendsFromBase, S::Pre { proposal }) => {
                let GitAction::Release { commit, .. } = &proposal.action else {
                    return Vec::new();
                };
                let Some(commit) = Oid::parse(commit) else {
                    return Vec::new(); // rejected by well_formed_proposal
                };
                [
                    one(Phase::Pre, pin(&commit)),
                    one(
                        Phase::Pre,
                        hard(
                            CrossSubject::Git(GitSubject::Descends {
                                repo: release.repo.clone(),
                                ancestor: release.approved_base.clone(),
                                descendant: commit,
                            }),
                            CrossValue::Git(GitValue::Yes),
                        ),
                    ),
                ]
                .concat()
            }
            (G::ArtifactMatchesVerifiedCommit, S::Pre { proposal }) => {
                let GitAction::Release {
                    commit,
                    artifact,
                    manifest,
                } = &proposal.action
                else {
                    return Vec::new();
                };
                let (Some(commit), Some(artifact)) = (
                    Oid::parse(commit),
                    workspace_name(ctx.workdir, Path::new(artifact)),
                ) else {
                    return Vec::new();
                };
                let digest = |wrap: fn(ContentHash) -> CrossValue| wrap(*manifest);
                [
                    one(Phase::Pre, pin(&commit)),
                    one(
                        Phase::Pre,
                        hard(
                            CrossSubject::Git(GitSubject::ContentManifest {
                                repo: release.repo.clone(),
                                commit,
                            }),
                            digest(|sha256| CrossValue::Git(GitValue::Digest { sha256 })),
                        ),
                    ),
                    one(
                        Phase::Pre,
                        hard(
                            CrossSubject::Fs(Subject::ContentManifest(artifact)),
                            digest(|sha256| CrossValue::Fs(Value::Digest { sha256 })),
                        ),
                    ),
                ]
                .concat()
            }
            (G::ProtectedRefsUnmoved, S::Pre { proposal }) => one(
                Phase::Pre,
                Requirement::Within {
                    names: proposal
                        .declared_refs
                        .iter()
                        .cloned()
                        .map(Name::Known)
                        .collect(),
                    scopes: ctx.policy.writable_refs.clone(),
                },
            ),
            (G::ProtectedRefsUnmoved, S::PostCommand { touched }) => one(
                Phase::Post,
                Requirement::Within {
                    names: touched.to_vec(),
                    scopes: ctx.policy.writable_refs.clone(),
                },
            ),
            (G::RollbackRestoresRefs, S::PostRollback { checkpoint_refs }) => ctx
                .policy
                .repos
                .iter()
                .flat_map(|repo| {
                    let subject = CrossSubject::Git(GitSubject::Refs {
                        repo: repo.path.clone(),
                    });
                    match checkpoint_refs.get(&repo.path) {
                        Some(sha256) => one(
                            Phase::Post,
                            hard(
                                subject,
                                CrossValue::Git(GitValue::Digest { sha256: *sha256 }),
                            ),
                        ),
                        // Never observed at the checkpoint: cannot be shown restored.
                        None => one(
                            Phase::Post,
                            hard(subject, CrossValue::Git(GitValue::Absent)),
                        ),
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Single-use permission for one git proposal.
#[derive(Debug)]
pub struct GitAuthorization {
    task_id: Uuid,
    policy: ContentHash,
    proposal: GitProposal,
    obligations: Vec<CrossObligation>,
    decision: CrossDecision,
}

impl GitAuthorization {
    pub fn decision(&self) -> &CrossDecision {
        &self.decision
    }
}

#[derive(Debug)]
pub enum GitAuthorize {
    Allowed(Box<GitAuthorization>),
    Denied(CrossDecision),
    Blocked(CrossDecision),
}

impl GitAuthorize {
    pub fn verdict(&self) -> Verdict {
        match self {
            GitAuthorize::Allowed(_) => Verdict::Allow,
            GitAuthorize::Denied(_) => Verdict::Deny,
            GitAuthorize::Blocked(_) => Verdict::Blocked,
        }
    }

    pub fn decision(&self) -> &CrossDecision {
        match self {
            GitAuthorize::Allowed(auth) => &auth.decision,
            GitAuthorize::Denied(d) | GitAuthorize::Blocked(d) => d,
        }
    }
}

#[derive(Debug)]
pub struct GitReport {
    pub executed: bool,
    pub result: Option<std::result::Result<StepOutput, ExecutionError>>,
    /// POST decision if a command ran; otherwise the execution-time
    /// re-evaluation of the PRE and basis obligations.
    pub decision: CrossDecision,
    /// For an accepted release: the verified commit and content manifest.
    pub released: Option<(Oid, ContentHash)>,
}

const BASIS: &str = "authorization_basis_current";

pub struct GitGuard {
    task: Task,
    policy: Arc<GitPolicy>,
    observer: GitObserver,
    trace: TraceLogger,
    checkpoint: CheckpointId,
    checkpoint_refs: BTreeMap<String, ContentHash>,
    unaccepted: Option<String>,
}

impl GitGuard {
    pub async fn start(
        mut task: Task,
        policy: Arc<GitPolicy>,
        root: StateRoot,
    ) -> Result<Self, GuardError> {
        task.manifest = policy.manifest.clone();
        vfs::register_task_with_state(task.id, task.workdir.clone(), root.clone())?;
        root.ensure_task_dir(task.id)?;
        let trace = TraceLogger::for_task(&root, task.id).await?;
        trace
            .log_event(TaskEvent::task_started(task.manifest.clone()))
            .await?;
        let (checkpoint, _, _) = vfs::checkpoint_with_state(task.id, &trace).await?;
        let observer = GitObserver::new(&normalize_path(&task.workdir), policy.repos.clone());
        let observation = observer.observe();
        let checkpoint_refs = policy
            .repos
            .iter()
            .filter_map(|r| Some((r.path.clone(), observation.refs_digest(&r.path)?)))
            .collect();
        Ok(Self {
            task,
            policy,
            observer,
            trace,
            checkpoint,
            checkpoint_refs,
            unaccepted: None,
        })
    }

    pub fn task(&self) -> &Task {
        &self.task
    }

    pub fn trace(&self) -> &TraceLogger {
        &self.trace
    }

    pub fn is_accepting(&self) -> bool {
        self.unaccepted.is_none()
    }

    /// The verified value of a git subject right now, for planners that
    /// need witnesses. Read-only; the answer is data, not authority.
    pub async fn query(&self, subject: &GitSubject) -> Option<GitValue> {
        let workspace = self.capture().await.ok();
        let observation = self.observer.observe();
        let evidence = self
            .observer
            .evidence(&observation, workspace.as_ref(), [subject]);
        evidence.get(subject).iter().find_map(|e| match e {
            crate::kernel::Evidence::Verified { value, .. } => Some(value.clone()),
            _ => None,
        })
    }

    pub async fn authorize(&mut self, proposal: GitProposal) -> Result<GitAuthorize, GuardError> {
        let workspace = self.capture().await?;
        let git = self.observer.observe();
        let mut obligations = self.policy.obligations(&GitContext {
            policy: &self.policy,
            workdir: &self.task.workdir,
            stage: GitStage::Pre {
                proposal: &proposal,
            },
        });
        obligations.extend(basis(&workspace, &git, &self.policy.repos));
        let evidence = self.evidence(&obligations, Some(&workspace), &git).await;
        let decision = evaluate(Phase::Pre, &obligations, &evidence);
        self.log(&label(&proposal), &decision).await?;
        Ok(match decision.verdict {
            Verdict::Allow => GitAuthorize::Allowed(Box::new(GitAuthorization {
                task_id: self.task.id,
                policy: self.policy.digest(),
                proposal,
                obligations,
                decision,
            })),
            Verdict::Deny => GitAuthorize::Denied(decision),
            Verdict::Blocked => GitAuthorize::Blocked(decision),
        })
    }

    pub async fn execute(&mut self, auth: GitAuthorization) -> Result<GitReport, GuardError> {
        if auth.task_id != self.task.id || auth.policy != self.policy.digest() {
            return Err(GuardError::ForeignAuthorization);
        }
        let label = label(&auth.proposal);
        // Re-check everything the authorization relied on, on fresh evidence.
        let workspace = self.capture().await?;
        let pre_git = self.observer.observe();
        let evidence = self
            .evidence(&auth.obligations, Some(&workspace), &pre_git)
            .await;
        let fresh = evaluate(Phase::Pre, &auth.obligations, &evidence);
        if fresh.verdict != Verdict::Allow {
            self.log(&label, &fresh).await?;
            if fresh.violated().any(|f| f.obligation.invariant == BASIS) {
                self.unaccepted
                    .get_or_insert_with(|| "state changed outside authorized actions".to_string());
            }
            return Ok(GitReport {
                executed: false,
                result: None,
                decision: fresh,
                released: None,
            });
        }

        match auth.proposal.action {
            GitAction::Release {
                commit, manifest, ..
            } => {
                self.log(&label, &fresh).await?;
                Ok(GitReport {
                    executed: true,
                    result: None,
                    decision: fresh,
                    released: Oid::parse(&commit).map(|oid| (oid, manifest)),
                })
            }
            GitAction::Command(spec) => {
                let (step, post_workspace) = execution::run_observed_from(
                    &mut self.task,
                    spec,
                    None,
                    &self.trace,
                    &workspace,
                )
                .await?;
                let post_git = self.observer.observe();
                let touched = GitObservation::touched_refs(&pre_git, &post_git);
                let obligations = self.policy.obligations(&GitContext {
                    policy: &self.policy,
                    workdir: &self.task.workdir,
                    stage: GitStage::PostCommand { touched: &touched },
                });
                let evidence = self
                    .evidence(&obligations, post_workspace.as_ref(), &post_git)
                    .await;
                let decision = evaluate(Phase::Post, &obligations, &evidence);
                self.log(&label, &decision).await?;
                if decision.verdict != Verdict::Allow {
                    self.unaccepted =
                        Some(format!("post-execution verdict {:?}", decision.verdict));
                }
                Ok(GitReport {
                    executed: true,
                    result: Some(step.result),
                    decision,
                    released: None,
                })
            }
        }
    }

    /// Restore the checkpoint (files, and therefore every repository) and
    /// require every repo's pointers to equal the checkpoint's.
    pub async fn rollback(&mut self) -> Result<GitReport, GuardError> {
        let rolled = vfs::rollback_observed(self.task.id, self.checkpoint, &self.trace).await?;
        let git = self.observer.observe();
        let obligations = self.policy.obligations(&GitContext {
            policy: &self.policy,
            workdir: &self.task.workdir,
            stage: GitStage::PostRollback {
                checkpoint_refs: &self.checkpoint_refs,
            },
        });
        let evidence = self.evidence(&obligations, None, &git).await;
        let decision = evaluate(Phase::Post, &obligations, &evidence);
        self.log("rollback", &decision).await?;
        self.unaccepted = match (decision.verdict, rolled.result) {
            (Verdict::Allow, Ok(())) => None,
            (verdict, _) => Some(format!("rollback verdict {verdict:?}")),
        };
        Ok(GitReport {
            executed: true,
            result: None,
            decision,
            released: None,
        })
    }

    async fn capture(&self) -> Result<Observation, GuardError> {
        Ok(execution::observe_blocking(normalize_path(&self.task.workdir)).await?)
    }

    async fn evidence(
        &self,
        obligations: &[CrossObligation],
        workspace: Option<&Observation>,
        git: &GitObservation,
    ) -> CrossEvidence {
        let mut fs_subjects = Vec::new();
        let mut git_subjects = Vec::new();
        for obligation in obligations {
            if let Requirement::Fact { subject, .. } = &obligation.requirement {
                match subject {
                    CrossSubject::Fs(s) => fs_subjects.push(s),
                    CrossSubject::Git(s) => git_subjects.push(s),
                }
            }
        }
        let mut fs = RuntimeEvidence::new();
        if let Some(workspace) = workspace {
            facts::from_observation(workspace, fs_subjects.iter().copied(), &mut fs);
        }
        if fs_subjects.contains(&&Subject::TrustedState) {
            if let Some((fact, provenance)) = facts::trusted_state(self.task.id, &self.trace).await
            {
                fs.add_verified(fact, provenance);
            }
        }
        if fs_subjects.contains(&&Subject::Transitions) {
            let (fact, provenance) = facts::transitions(match &self.unaccepted {
                None => Ok(()),
                Some(reason) => Err(reason.clone()),
            });
            fs.add_verified(fact, provenance);
        }
        let mut evidence = fs.map(
            |s| CrossSubject::Fs(s.clone()),
            |v| CrossValue::Fs(v.clone()),
        );
        evidence.merge(&self.observer.evidence(git, workspace, git_subjects).map(
            |s| CrossSubject::Git(s.clone()),
            |v| CrossValue::Git(v.clone()),
        ));
        evidence
    }

    async fn log(&self, action: &str, decision: &CrossDecision) -> Result<(), GuardError> {
        self.trace
            .log_event(TaskEvent::InvariantDecision {
                record: Box::new(decision.record(action)),
            })
            .await?;
        Ok(())
    }
}

/// Obligations pinning the state an authorization was granted on.
fn basis(workspace: &Observation, git: &GitObservation, repos: &[GitRepo]) -> Vec<CrossObligation> {
    let mut out = vec![Obligation {
        invariant: BASIS.to_string(),
        phase: Phase::Pre,
        requirement: hard(
            CrossSubject::Fs(Subject::Workspace),
            CrossValue::Fs(Value::Digest {
                sha256: workspace.digest(),
            }),
        ),
    }];
    for repo in repos {
        if let Some(sha256) = git.refs_digest(&repo.path) {
            out.push(Obligation {
                invariant: BASIS.to_string(),
                phase: Phase::Pre,
                requirement: hard(
                    CrossSubject::Git(GitSubject::Refs {
                        repo: repo.path.clone(),
                    }),
                    CrossValue::Git(GitValue::Digest { sha256 }),
                ),
            });
        }
    }
    out
}

fn label(proposal: &GitProposal) -> String {
    match &proposal.action {
        GitAction::Command(spec) => std::iter::once(spec.program.as_str())
            .chain(spec.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        GitAction::Release {
            commit, artifact, ..
        } => format!("release {commit} as {artifact}"),
    }
}
