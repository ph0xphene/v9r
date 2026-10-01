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
//! | `I3.rollback_restores_checkpoint` | rollback ends in the checkpointed workspace | post (rollback) | fs `Workspace = Digest(checkpoint)` |
//! | shared | trusted state intact, well-formed proposal | pre (+post) | as in `crate::policy` |
//!
//! Existential invariants ("there is a release commit X such that …") are
//! discharged by *witnesses* in the proposal (`commit`, `manifest`), each
//! pinned by an obligation that only verified evidence can satisfy. The
//! kernel checks witnesses; it never searches for them. A wrong or
//! malicious witness is denied, a missing or malformed one is rejected by
//! `well_formed_proposal`.
//!
//! A release has no effect of its own; G1 and G3 are therefore judged
//! again after it (POST), on the state observed when it is released.
//!
//! The lifecycle is [`crate::runtime`]'s. This module supplies the git
//! domain to it: the observation is the workspace scan *plus* every
//! repo's refs; the authorization basis is the workspace digest plus each
//! repo's ref digest; commands and rollback reuse the workspace effect
//! machinery of `crate::guarded`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::effect::Observation;
use crate::execution::{CommandSpec, ExecutionError, StepOutput};
use crate::facts::{Subject, Value};
use crate::git::{GitObservation, GitObserver, GitRepo, GitSubject, GitValue, Oid};
use crate::guarded::{command_label, GuardError, Workspace, WorkspaceEffect, WorkspaceReceipt};
use crate::kernel::{
    Decision, Evidence, EvidenceBase, Fact, Invariant, Name, Obligation, Phase, Requirement,
    Strength,
};
use crate::manifest::{normalize_path, Manifest};
use crate::policy::workspace_name;
use crate::runtime::{
    self, Compensable, Concluded, DomainObligation, EffectDomain, Runtime, Stage,
};
use crate::state::ContentHash;
use crate::task::Task;
use crate::trace::TraceLogger;
use crate::trusted::StateRoot;

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

pub type CrossDecision = runtime::RtDecision<GitDomain>;
/// Kernel decision over the composed vocabulary alone.
pub type CrossKernelDecision = Decision<CrossSubject, CrossValue>;
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
    TrustedStateIntact,
    ReleaseDescendsFromBase,
    ProtectedRefsUnmoved,
    ArtifactMatchesVerifiedCommit,
    RollbackRestoresRefs,
    RollbackRestoresCheckpoint,
}

impl GitInvariant {
    pub const ALL: [GitInvariant; 7] = [
        GitInvariant::WellFormedProposal,
        GitInvariant::TrustedStateIntact,
        GitInvariant::ReleaseDescendsFromBase,
        GitInvariant::ProtectedRefsUnmoved,
        GitInvariant::ArtifactMatchesVerifiedCommit,
        GitInvariant::RollbackRestoresRefs,
        GitInvariant::RollbackRestoresCheckpoint,
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
    Post {
        proposal: &'a GitProposal,
        touched: &'a [Name],
    },
    PostRollback {
        checkpoint_refs: &'a BTreeMap<String, ContentHash>,
        checkpoint: ContentHash,
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
            GitInvariant::TrustedStateIntact => "I4.trusted_state_intact",
            GitInvariant::ReleaseDescendsFromBase => "G1.release_descends_from_base",
            GitInvariant::ProtectedRefsUnmoved => "G2.protected_refs_unmoved",
            GitInvariant::ArtifactMatchesVerifiedCommit => "G3.artifact_matches_verified_commit",
            GitInvariant::RollbackRestoresRefs => "G2.rollback_restores_refs",
            GitInvariant::RollbackRestoresCheckpoint => "I3.rollback_restores_checkpoint",
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
            (G::ReleaseDescendsFromBase, S::Pre { proposal } | S::Post { proposal, .. }) => {
                let phase = ctx.stage.phase();
                let one = |requirement| one(phase, requirement);
                let GitAction::Release { commit, .. } = &proposal.action else {
                    return Vec::new();
                };
                let Some(commit) = Oid::parse(commit) else {
                    return Vec::new(); // rejected by well_formed_proposal
                };
                [
                    one(pin(&commit)),
                    one(hard(
                            CrossSubject::Git(GitSubject::Descends {
                                repo: release.repo.clone(),
                                ancestor: release.approved_base.clone(),
                                descendant: commit,
                            }),
                            CrossValue::Git(GitValue::Yes),
                    )),
                ]
                .concat()
            }
            (G::ArtifactMatchesVerifiedCommit, S::Pre { proposal } | S::Post { proposal, .. }) => {
                let phase = ctx.stage.phase();
                let one = |requirement| one(phase, requirement);
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
                    one(pin(&commit)),
                    one(hard(
                        CrossSubject::Git(GitSubject::ContentManifest {
                            repo: release.repo.clone(),
                            commit,
                        }),
                        digest(|sha256| CrossValue::Git(GitValue::Digest { sha256 })),
                    )),
                    one(hard(
                        CrossSubject::Fs(Subject::ContentManifest(artifact)),
                        digest(|sha256| CrossValue::Fs(Value::Digest { sha256 })),
                    )),
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
            (G::ProtectedRefsUnmoved, S::Post { touched, .. }) => one(
                Phase::Post,
                Requirement::Within {
                    names: touched.to_vec(),
                    scopes: ctx.policy.writable_refs.clone(),
                },
            ),
            (G::RollbackRestoresRefs, S::PostRollback { checkpoint_refs, .. }) => ctx
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
            (G::RollbackRestoresCheckpoint, S::PostRollback { checkpoint, .. }) => one(
                Phase::Post,
                hard(
                    CrossSubject::Fs(Subject::Workspace),
                    CrossValue::Fs(Value::Digest {
                        sha256: *checkpoint,
                    }),
                ),
            ),
            _ => Vec::new(),
        }
    }
}

impl GitStage<'_> {
    fn phase(&self) -> Phase {
        match self {
            GitStage::Pre { .. } => Phase::Pre,
            _ => Phase::Post,
        }
    }
}

// ------------------------------------------------------------ domain

/// What the git domain sees: the workspace and every repo's refs.
#[derive(Clone)]
pub struct GitWorld {
    pub workspace: Observation,
    pub git: GitObservation,
}

pub enum GitEffect {
    Workspace(WorkspaceEffect),
    /// A release runs nothing; it is a judged declaration.
    Release { commit: String, manifest: ContentHash },
}

/// Git as a domain: the workspace effect machinery plus a git observer
/// and the git policy.
pub struct GitDomain {
    ws: Workspace,
    observer: GitObserver,
    policy: Arc<GitPolicy>,
    checkpoint_refs: BTreeMap<String, ContentHash>,
}

impl GitDomain {
    fn context<'a>(&'a self, stage: GitStage<'a>) -> GitContext<'a> {
        GitContext {
            policy: &self.policy,
            workdir: &self.ws.task.workdir,
            stage,
        }
    }
}

impl EffectDomain for GitDomain {
    type Subject = CrossSubject;
    type Value = CrossValue;
    type Proposal = GitProposal;
    type Observation = GitWorld;
    type Effect = GitEffect;
    type Receipt = WorkspaceReceipt;
    type Output = (Oid, ContentHash);
    type Error = GuardError;

    fn label(&self, proposal: &GitProposal) -> String {
        label(proposal)
    }

    fn policy_digest(&self) -> String {
        self.policy.digest().to_string()
    }

    async fn observe(&self) -> Result<GitWorld, GuardError> {
        Ok(GitWorld {
            workspace: self.ws.observe().await?,
            git: self.observer.observe(),
        })
    }

    fn basis(&self, world: &GitWorld) -> Vec<Fact<CrossSubject, CrossValue>> {
        let workspace = Workspace::basis(&world.workspace);
        let mut out = vec![Fact {
            subject: CrossSubject::Fs(workspace.subject),
            value: CrossValue::Fs(workspace.value),
        }];
        for repo in &self.policy.repos {
            if let Some(sha256) = world.git.refs_digest(&repo.path) {
                out.push(Fact {
                    subject: CrossSubject::Git(GitSubject::Refs {
                        repo: repo.path.clone(),
                    }),
                    value: CrossValue::Git(GitValue::Digest { sha256 }),
                });
            }
        }
        out
    }

    fn obligations(&self, stage: Stage<'_, Self>) -> Vec<DomainObligation<Self>> {
        let touched: Vec<Name>;
        let stage = match stage {
            Stage::Pre { proposal, .. } => GitStage::Pre { proposal },
            Stage::Post {
                proposal,
                before,
                after,
                ..
            } => {
                touched = match after {
                    Some(after) => GitObservation::touched_refs(&before.git, &after.git),
                    // Nothing observed: any ref may have moved.
                    None => vec![Name::UnknownBelow(String::new())],
                };
                GitStage::Post {
                    proposal,
                    touched: &touched,
                }
            }
            Stage::Compensated { .. } => GitStage::PostRollback {
                checkpoint_refs: &self.checkpoint_refs,
                checkpoint: self.ws.checkpoint_observation.digest(),
            },
        };
        self.policy.obligations(&self.context(stage))
    }

    async fn evidence(
        &self,
        subjects: &[&CrossSubject],
        world: Option<&GitWorld>,
        receipt: Option<&WorkspaceReceipt>,
    ) -> CrossEvidence {
        let mut fs_subjects = Vec::new();
        let mut git_subjects = Vec::new();
        for subject in subjects {
            match subject {
                CrossSubject::Fs(s) => fs_subjects.push(s),
                CrossSubject::Git(s) => git_subjects.push(s),
            }
        }
        let workspace = world.map(|w| &w.workspace);
        let fs = self
            .ws
            .evidence(fs_subjects.iter().copied(), workspace, receipt)
            .await;
        let mut evidence = fs.map(
            |s| CrossSubject::Fs(s.clone()),
            |v| CrossValue::Fs(v.clone()),
        );
        if let Some(world) = world {
            evidence.merge(
                &self
                    .observer
                    .evidence(&world.git, workspace, git_subjects)
                    .map(|s| CrossSubject::Git(s.clone()), |v| CrossValue::Git(v.clone())),
            );
        }
        evidence
    }

    async fn execute(&mut self, proposal: &GitProposal, _before: &GitWorld) -> Result<GitEffect, GuardError> {
        Ok(match &proposal.action {
            GitAction::Command(spec) => GitEffect::Workspace(self.ws.run(spec.clone(), None).await),
            GitAction::Release {
                commit, manifest, ..
            } => GitEffect::Release {
                commit: commit.clone(),
                manifest: *manifest,
            },
        })
    }

    async fn conclude(
        &mut self,
        effect: GitEffect,
        before: &GitWorld,
        after: Result<&GitWorld, String>,
    ) -> Result<Concluded<Self>, GuardError> {
        match effect {
            GitEffect::Workspace(effect) => {
                let (receipt, _, durable) = self
                    .ws
                    .conclude(effect, &before.workspace, after.map(|w| &w.workspace))
                    .await?;
                Ok(Concluded {
                    receipt,
                    output: None,
                    durable: durable.map(
                        |s| CrossSubject::Fs(s.clone()),
                        |v| CrossValue::Fs(v.clone()),
                    ),
                })
            }
            GitEffect::Release { commit, manifest } => Ok(Concluded {
                receipt: WorkspaceReceipt {
                    result: None,
                    receipt: None,
                },
                output: Oid::parse(&commit).map(|oid| (oid, manifest)),
                durable: EvidenceBase::new(),
            }),
        }
    }
}

impl Compensable for GitDomain {
    const LABEL: &'static str = "rollback";

    /// Restore the checkpoint: files, and therefore every repository.
    async fn compensate(&mut self, _before: &GitWorld) -> Result<GitEffect, GuardError> {
        Ok(GitEffect::Workspace(self.ws.rollback().await))
    }
}

// ------------------------------------------------------------ facade

pub type GitAuthorization = runtime::Authorization<GitDomain>;
pub type GitAuthorize = runtime::Authorize<GitDomain>;

#[derive(Debug)]
pub struct GitReport {
    pub executed: bool,
    pub result: Option<std::result::Result<StepOutput, ExecutionError>>,
    /// POST decision if the action ran; otherwise the execution-time
    /// re-evaluation of the PRE and basis obligations.
    pub decision: CrossDecision,
    /// For an accepted release: the verified commit and content manifest.
    pub released: Option<(Oid, ContentHash)>,
}

impl From<runtime::Report<GitDomain>> for GitReport {
    fn from(report: runtime::Report<GitDomain>) -> Self {
        Self {
            executed: report.executed,
            result: report.receipt.and_then(|r| r.result),
            decision: report.decision,
            released: report.output,
        }
    }
}

/// The git domain under the runtime, with the pre-runtime API.
pub struct GitGuard {
    rt: Runtime<GitDomain, TraceLogger>,
}

impl GitGuard {
    pub async fn start(
        task: Task,
        policy: Arc<GitPolicy>,
        root: StateRoot,
    ) -> Result<Self, GuardError> {
        let ws = Workspace::open(task, policy.manifest.clone(), root).await?;
        let observer = GitObserver::new(&normalize_path(&ws.task.workdir), policy.repos.clone());
        let git = observer.observe();
        let checkpoint_refs = policy
            .repos
            .iter()
            .filter_map(|r| Some((r.path.clone(), git.refs_digest(&r.path)?)))
            .collect();
        let baseline = GitWorld {
            workspace: ws.checkpoint_observation.clone(),
            git,
        };
        let journal = ws.trace.clone();
        let domain = GitDomain {
            ws,
            observer,
            policy,
            checkpoint_refs,
        };
        Ok(Self {
            rt: Runtime::new(domain, journal, baseline),
        })
    }

    pub fn task(&self) -> &Task {
        &self.rt.domain().ws.task
    }

    pub fn trace(&self) -> &TraceLogger {
        self.rt.journal()
    }

    pub fn is_accepting(&self) -> bool {
        self.rt.is_accepting()
    }

    /// The verified value of a git subject right now, for planners that
    /// need witnesses. Read-only; the answer is data, not authority.
    pub async fn query(&self, subject: &GitSubject) -> Option<GitValue> {
        let domain = self.rt.domain();
        let workspace = domain.ws.observe().await.ok();
        let observation = domain.observer.observe();
        let evidence = domain
            .observer
            .evidence(&observation, workspace.as_ref(), [subject]);
        evidence.get(subject).iter().find_map(|e| match e {
            Evidence::Verified { value, .. } => Some(value.clone()),
            _ => None,
        })
    }

    pub async fn authorize(&mut self, proposal: GitProposal) -> Result<GitAuthorize, GuardError> {
        self.rt.authorize(proposal).await
    }

    pub async fn execute(&mut self, auth: GitAuthorization) -> Result<GitReport, GuardError> {
        Ok(self.rt.execute(auth).await?.into())
    }

    /// Restore the checkpoint (files, and therefore every repository) and
    /// require the workspace and every repo's pointers to equal the
    /// checkpoint's.
    pub async fn rollback(&mut self) -> Result<GitReport, GuardError> {
        Ok(self.rt.compensate().await?.into())
    }
}

fn label(proposal: &GitProposal) -> String {
    match &proposal.action {
        GitAction::Command(spec) => command_label(spec),
        GitAction::Release {
            commit, artifact, ..
        } => format!("release {commit} as {artifact}"),
    }
}
