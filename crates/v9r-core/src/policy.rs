//! v9r's runtime invariants, expressed as kernel obligations.
//!
//! Each invariant makes an existing, implicit v9r property explicit:
//!
//! | Id | Invariant | Phase | Requirement |
//! |---|---|---|---|
//! | `I1.declared_writes_in_scope` | declared writes stay inside `allow_write` | pre | `Within` |
//! | `I1.observed_writes_declared` | observed changes stay inside the declared writes | post | `Within` (unknown effects are unknown subtrees) |
//! | `I3.rollback_restores_checkpoint` | rollback ends in the checkpointed version | post | hard `Workspace = Digest(checkpoint)` |
//! | `I4.trusted_state_intact` | trace and checkpoints unchanged by anyone but the runtime | pre + post | hard `TrustedState = Intact` |
//! | `workspace_accepted` | no action on top of an unaccepted transition | pre | hard `Transitions = Accepted` |
//! | `step_budget` | `max_steps` | pre | `AtMost` |
//! | `well_formed_proposal` | expectation paths are workspace names | pre | `AtMost` (0 malformed) |
//! | `export_gate` | export only a version whose configured tests passed, with mandatory artifacts present | pre | hard `TestsAt(version) = Passed`, `Exists(a) = Present` |
//! | `expectations` | the proposal's own postconditions | post | hard facts |
//!
//! I2 (verified provenance) and I5 (hard obligations need hard evidence)
//! are not policy entries: I2 is structural (only attested evidence can
//! be verified, records ingest as proposed, every verified entry carries
//! provenance) and I5 is the kernel's `Strength::Hard` semantics.
//!
//! A proposal can only *add* obligations (expectations). It cannot name,
//! remove or weaken an invariant; the policy is fixed when the guarded
//! task starts.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::execution::CommandSpec;
use crate::facts::{Subject, Value};
use crate::kernel::{Invariant, Name, Obligation, Phase, Requirement, Strength};
use crate::manifest::{normalize_path, Manifest};
use crate::state::ContentHash;

/// What a postcondition the proposer wants checked after execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "expect", rename_all = "snake_case")]
pub enum Expectation {
    Exists { path: String },
    Absent { path: String },
    FileContent { path: String, sha256: ContentHash },
}

impl Expectation {
    fn path(&self) -> &str {
        match self {
            Expectation::Exists { path }
            | Expectation::Absent { path }
            | Expectation::FileContent { path, .. } => path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProposedAction {
    Command(CommandSpec),
    /// Commit the current workspace version as a bundle.
    Export,
}

/// A candidate action from a planner or agent. Plain data: it carries no
/// authority and can be freely (de)serialized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionProposal {
    pub action: ProposedAction,
    #[serde(default)]
    pub requested: Option<String>,
    #[serde(default)]
    pub expectations: Vec<Expectation>,
    pub proposer: String,
}

impl ActionProposal {
    pub fn label(&self) -> String {
        match &self.action {
            ProposedAction::Command(spec) => std::iter::once(spec.program.as_str())
                .chain(spec.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            ProposedAction::Export => "export".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeInvariant {
    DeclaredWritesInScope,
    ObservedWritesDeclared,
    RollbackRestoresCheckpoint,
    TrustedStateIntact,
    WorkspaceAccepted,
    StepBudget,
    WellFormedProposal,
    ExportGate,
    Expectations,
}

impl RuntimeInvariant {
    pub const ALL: [RuntimeInvariant; 9] = [
        RuntimeInvariant::WellFormedProposal,
        RuntimeInvariant::WorkspaceAccepted,
        RuntimeInvariant::TrustedStateIntact,
        RuntimeInvariant::StepBudget,
        RuntimeInvariant::DeclaredWritesInScope,
        RuntimeInvariant::ExportGate,
        RuntimeInvariant::ObservedWritesDeclared,
        RuntimeInvariant::Expectations,
        RuntimeInvariant::RollbackRestoresCheckpoint,
    ];
}

/// Where in the action lifecycle obligations are being derived.
pub enum Stage<'a> {
    Pre {
        proposal: &'a ActionProposal,
        steps_used: usize,
        version: ContentHash,
    },
    PostCommand {
        proposal: &'a ActionProposal,
        touched: &'a [Name],
    },
    PostExport,
    PostRollback {
        checkpoint: ContentHash,
    },
}

pub struct Context<'a> {
    pub manifest: &'a Manifest,
    pub workdir: &'a Path,
    pub stage: Stage<'a>,
}

fn obligation(
    invariant: RuntimeInvariant,
    phase: Phase,
    requirement: Requirement<Subject, Value>,
) -> Obligation<Subject, Value> {
    Obligation {
        invariant: invariant.id().to_string(),
        phase,
        requirement,
    }
}

fn hard(subject: Subject, value: Value) -> Requirement<Subject, Value> {
    Requirement::Fact {
        subject,
        value,
        strength: Strength::Hard,
    }
}

impl Invariant<Context<'_>, Subject, Value> for RuntimeInvariant {
    fn id(&self) -> &str {
        match self {
            RuntimeInvariant::DeclaredWritesInScope => "I1.declared_writes_in_scope",
            RuntimeInvariant::ObservedWritesDeclared => "I1.observed_writes_declared",
            RuntimeInvariant::RollbackRestoresCheckpoint => "I3.rollback_restores_checkpoint",
            RuntimeInvariant::TrustedStateIntact => "I4.trusted_state_intact",
            RuntimeInvariant::WorkspaceAccepted => "workspace_accepted",
            RuntimeInvariant::StepBudget => "step_budget",
            RuntimeInvariant::WellFormedProposal => "well_formed_proposal",
            RuntimeInvariant::ExportGate => "export_gate",
            RuntimeInvariant::Expectations => "expectations",
        }
    }

    fn obligations(&self, ctx: &Context<'_>) -> Vec<Obligation<Subject, Value>> {
        use RuntimeInvariant as I;
        use Stage as S;
        let pre = |req| vec![obligation(*self, Phase::Pre, req)];
        let post = |req| vec![obligation(*self, Phase::Post, req)];
        match (self, &ctx.stage) {
            (I::WellFormedProposal, S::Pre { proposal, .. }) => {
                let malformed = proposal
                    .expectations
                    .iter()
                    .filter(|e| workspace_name(ctx.workdir, Path::new(e.path())).is_none())
                    .count();
                pre(Requirement::AtMost {
                    quantity: "malformed expectation paths".to_string(),
                    value: malformed as u64,
                    limit: 0,
                })
            }
            (I::WorkspaceAccepted, S::Pre { .. }) => {
                pre(hard(Subject::Transitions, Value::Accepted))
            }
            (I::TrustedStateIntact, S::Pre { .. }) => {
                pre(hard(Subject::TrustedState, Value::Intact))
            }
            (I::TrustedStateIntact, _) => post(hard(Subject::TrustedState, Value::Intact)),
            (
                I::StepBudget,
                S::Pre {
                    proposal,
                    steps_used,
                    ..
                },
            ) => match proposal.action {
                ProposedAction::Command(_) => pre(Requirement::AtMost {
                    quantity: "steps".to_string(),
                    value: *steps_used as u64 + 1,
                    limit: ctx.manifest.max_steps as u64,
                }),
                ProposedAction::Export => Vec::new(),
            },
            (I::DeclaredWritesInScope, S::Pre { proposal, .. }) => match &proposal.action {
                ProposedAction::Command(spec) => pre(Requirement::Within {
                    names: spec
                        .writes
                        .iter()
                        .map(|w| Name::Known(name_or_outside(ctx.workdir, w)))
                        .collect(),
                    scopes: scopes(ctx.workdir, &ctx.manifest.allow_write),
                }),
                ProposedAction::Export => Vec::new(),
            },
            (
                I::ExportGate,
                S::Pre {
                    proposal, version, ..
                },
            ) => {
                if proposal.action != ProposedAction::Export {
                    return Vec::new();
                }
                let mut out = Vec::new();
                if !ctx.manifest.test_commands.is_empty() {
                    out.extend(pre(hard(Subject::TestsAt(*version), Value::Passed)));
                }
                for artifact in &ctx.manifest.mandatory_artifacts {
                    let name = name_or_outside(ctx.workdir, artifact);
                    out.extend(pre(hard(Subject::Exists(name), Value::Present)));
                }
                out
            }
            (I::ObservedWritesDeclared, S::PostCommand { proposal, touched }) => {
                let ProposedAction::Command(spec) = &proposal.action else {
                    return Vec::new();
                };
                post(Requirement::Within {
                    names: touched.to_vec(),
                    scopes: scopes(ctx.workdir, &spec.writes),
                })
            }
            (I::Expectations, S::PostCommand { proposal, .. }) => proposal
                .expectations
                .iter()
                .filter_map(|e| {
                    let name = workspace_name(ctx.workdir, Path::new(e.path()))?;
                    let requirement = match e {
                        Expectation::Exists { .. } => hard(Subject::Exists(name), Value::Present),
                        Expectation::Absent { .. } => hard(Subject::Exists(name), Value::Absent),
                        Expectation::FileContent { sha256, .. } => {
                            hard(Subject::Path(name), Value::File { sha256: *sha256 })
                        }
                    };
                    Some(obligation(*self, Phase::Post, requirement))
                })
                .collect(),
            (I::RollbackRestoresCheckpoint, S::PostRollback { checkpoint }) => post(hard(
                Subject::Workspace,
                Value::Digest {
                    sha256: *checkpoint,
                },
            )),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error(
        "policy source {0} is inside the agent workspace; the agent could rewrite its own policy"
    )]
    SourceInWorkspace(std::path::PathBuf),
    #[error("policy source io at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

/// The trusted invariant set of a guarded task. Constructed by privileged
/// code, immutable afterwards: there are no mutators, and the guarded
/// task holds it behind a shared reference.
#[derive(Debug)]
pub struct Policy {
    manifest: Manifest,
    invariants: Vec<RuntimeInvariant>,
    digest: ContentHash,
}

impl Policy {
    pub fn new(manifest: Manifest, invariants: Vec<RuntimeInvariant>) -> Self {
        let digest = ContentHash::of(
            &serde_json::to_vec(&(&manifest, &invariants))
                .expect("policy serialization is infallible"),
        );
        Self {
            manifest,
            invariants,
            digest,
        }
    }

    /// Every runtime invariant.
    pub fn standard(manifest: Manifest) -> Self {
        Self::new(manifest, RuntimeInvariant::ALL.to_vec())
    }

    /// Like [`Policy::standard`], for a manifest parsed from `source`.
    /// Refuses a source inside the workspace: the agent can write there,
    /// so it could weaken the policy of any later run.
    pub fn from_trusted_source(
        manifest: Manifest,
        source: &Path,
        workdir: &Path,
    ) -> Result<Self, PolicyError> {
        let canonical = |p: &Path| {
            std::fs::canonicalize(p).map_err(|source| PolicyError::Io {
                path: p.to_path_buf(),
                source,
            })
        };
        let source = canonical(source)?;
        if source.starts_with(canonical(workdir)?) {
            return Err(PolicyError::SourceInWorkspace(source));
        }
        Ok(Self::standard(manifest))
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn invariants(&self) -> &[RuntimeInvariant] {
        &self.invariants
    }

    pub fn digest(&self) -> ContentHash {
        self.digest
    }

    pub fn obligations(&self, ctx: &Context<'_>) -> Vec<Obligation<Subject, Value>> {
        self.invariants
            .iter()
            .flat_map(|invariant| invariant.obligations(ctx))
            .collect()
    }
}

/// `path` as a `/`-separated name relative to the workspace, or `None`
/// if it is not lexically inside it (absolute elsewhere, or any `..`).
pub fn workspace_name(workdir: &Path, path: &Path) -> Option<String> {
    let relative = if path.is_absolute() {
        normalize_path(path)
            .strip_prefix(normalize_path(workdir))
            .ok()?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// A name that is never inside any workspace scope.
fn name_or_outside(workdir: &Path, path: &Path) -> String {
    workspace_name(workdir, path)
        .unwrap_or_else(|| format!("<outside workspace>{}", path.display()))
}

fn scopes(workdir: &Path, paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|p| workspace_name(workdir, p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_names_are_lexically_fenced() {
        let w = Path::new("/w");
        assert_eq!(
            workspace_name(w, Path::new("a/./b")).as_deref(),
            Some("a/b")
        );
        assert_eq!(workspace_name(w, Path::new("/w/a")).as_deref(), Some("a"));
        assert_eq!(workspace_name(w, Path::new(".")).as_deref(), Some(""));
        assert_eq!(workspace_name(w, Path::new("/w")).as_deref(), Some(""));
        assert_eq!(workspace_name(w, Path::new("a/../b")), None);
        assert_eq!(workspace_name(w, Path::new("/elsewhere/a")), None);
        assert_eq!(workspace_name(w, Path::new("/w/../w2")), None);
    }

    #[test]
    fn policy_digest_changes_with_any_weakening() {
        let manifest = Manifest {
            allow_read: Vec::new(),
            allow_write: vec!["out".into()],
            allow_exec: Vec::new(),
            token_limit: 1,
            max_steps: 4,
            timeout_ms: 1,
            mandatory_artifacts: Vec::new(),
            test_commands: Vec::new(),
        };
        let full = Policy::standard(manifest.clone());
        let mut fewer = RuntimeInvariant::ALL.to_vec();
        fewer.retain(|i| *i != RuntimeInvariant::TrustedStateIntact);
        assert_ne!(full.digest(), Policy::new(manifest.clone(), fewer).digest());
        let mut wider = manifest.clone();
        wider.allow_write.push(".".into());
        assert_ne!(full.digest(), Policy::standard(wider).digest());
        assert_eq!(full.digest(), Policy::standard(manifest).digest());
    }
}
