//! Effect runtime v0: derived claims about filesystem state transitions.
//!
//! Four facts about an action are kept apart:
//!
//! * **requested** — what someone asked for (free text, never checked here);
//! * **declared** — the write paths the command declared up front;
//! * **outcome** — what the process reported (exit code, denial, …);
//! * **observed** — what changed between two observations of the workdir.
//!
//! None of these implies another. A command can exit 0 and change nothing,
//! or exit non-zero after changing files; both are ordinary receipts.
//!
//! # Epistemic classes
//!
//! * [`Verified`] — established by comparing two live [`Observation`]s.
//!   It can only be constructed inside this module, and neither it nor
//!   `Observation` implements `Deserialize`, so it cannot be forged from
//!   data (a stored manifest, a bundle, an oracle's output).
//! * [`SemanticAssessment::Semantic`] — interpretation by a
//!   [`SemanticOracle`]. It lives in a separate field of a different type
//!   and has no conversion into `Verified`.
//! * Unknown — [`UnknownEffect`] for paths whose state could not be
//!   established, and [`SemanticAssessment::Unknown`] when no oracle
//!   answered.
//!
//! # What an observed effect means
//!
//! An effect is a claim about the *state transition* of one path between
//! the pre- and post-observation, not about events. It says nothing about
//! who caused the change (anything running during the window is
//! included), about intermediate states, or about how many times a path
//! was touched. "No effects" means the observable final state of every
//! path in scope equals its initial state.
//!
//! A persisted [`ReceiptRecord`] is a report of what a runtime claimed.
//! Reading one back does not re-establish anything.
//!
//! ```compile_fail
//! // `Verified` has no public constructor.
//! use v9r_core::effect::{FsEffect, Verified};
//! let forged: Verified<FsEffect> = Verified(todo!());
//! ```
//!
//! ```compile_fail
//! // …and cannot be deserialized into existence.
//! use v9r_core::effect::{FsEffect, Verified};
//! let forged: Verified<FsEffect> = serde_json::from_str("{}").unwrap();
//! ```
//!
//! ```compile_fail
//! // An observation cannot be deserialized either.
//! use v9r_core::effect::Observation;
//! let forged: Observation = serde_json::from_str("{}").unwrap();
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::normalize_path;
use crate::state::{self, ContentHash, Entry, FsState, ObserveError};
use crate::vfs::CheckpointId;

/// A claim established by deterministic before/after observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified<T>(T);

impl<T> Verified<T> {
    pub fn get(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

/// Epistemic label used in persisted records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Epistemic {
    Verified,
    Semantic,
    Unknown,
}

/// A live observation of the tree under `root`. Only [`Observation::capture`]
/// creates one.
#[derive(Clone, Debug)]
pub struct Observation {
    root: PathBuf,
    state: FsState,
}

impl Observation {
    pub fn capture(root: &Path) -> Result<Self, ObserveError> {
        let root = normalize_path(root);
        let state = state::observe(&root)?;
        Ok(Self { root, state })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state(&self) -> &FsState {
        &self.state
    }
}

/// Observable change of one path, with the before/after evidence that
/// justifies it. Paths are `/`-separated and relative to the scope root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum FsEffect {
    /// Absent before, present after.
    Created { path: String, after: Entry },
    /// Present before and after, observable state differs (content,
    /// symlink target, or entry kind).
    Modified {
        path: String,
        before: Entry,
        after: Entry,
    },
    /// Present before, absent after.
    Deleted { path: String, before: Entry },
}

impl FsEffect {
    pub fn path(&self) -> &str {
        match self {
            FsEffect::Created { path, .. }
            | FsEffect::Modified { path, .. }
            | FsEffect::Deleted { path, .. } => path,
        }
    }
}

/// A path (and everything below it) whose transition could not be
/// determined. `"."` denotes the whole scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnknownEffect {
    pub path: String,
    pub reason: String,
}

/// Plain state delta between two states. Carries no epistemic status on
/// its own; see [`EffectReceipt`] for the verified form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateDelta {
    pub effects: Vec<FsEffect>,
    pub unknown: Vec<UnknownEffect>,
}

/// Deterministic, normalized delta: effects sorted by path, one entry per
/// path. Paths covered by an unobserved record in either state are never
/// reported as effects; the unobserved records become unknowns instead.
/// A path absent from both states produces nothing.
pub fn diff(pre: &FsState, post: &FsState) -> StateDelta {
    let mut unknown: Vec<UnknownEffect> = pre
        .unobserved()
        .iter()
        .map(|u| UnknownEffect {
            path: u.path.clone(),
            reason: format!("pre-state: {}", u.reason),
        })
        .chain(post.unobserved().iter().map(|u| UnknownEffect {
            path: u.path.clone(),
            reason: format!("post-state: {}", u.reason),
        }))
        .collect();
    unknown.sort_by(|a, b| a.path.cmp(&b.path).then(a.reason.cmp(&b.reason)));

    let covered =
        |path: &str| pre.unobserved_cover(path).is_some() || post.unobserved_cover(path).is_some();
    let mut effects = Vec::new();
    for (path, before) in pre.entries() {
        if covered(path) {
            continue;
        }
        match post.get(path) {
            None => effects.push(FsEffect::Deleted {
                path: path.clone(),
                before: before.clone(),
            }),
            Some(after) if after != before => effects.push(FsEffect::Modified {
                path: path.clone(),
                before: before.clone(),
                after: after.clone(),
            }),
            Some(_) => {}
        }
    }
    for (path, after) in post.entries() {
        if pre.get(path).is_none() && !covered(path) {
            effects.push(FsEffect::Created {
                path: path.clone(),
                after: after.clone(),
            });
        }
    }
    effects.sort_by(|a, b| a.path().cmp(b.path()));
    StateDelta { effects, unknown }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Command {
        argv: Vec<String>,
        declared_writes: Vec<PathBuf>,
    },
    Rollback {
        checkpoint: CheckpointId,
    },
    Other {
        description: String,
    },
}

/// What the action itself reported. Independent of observed effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    /// Process exited with a code.
    Exited { code: i32 },
    /// Process ended without an exit code (e.g. killed by a signal).
    Terminated,
    /// In-runtime action (e.g. rollback) completed.
    Completed,
    /// In-runtime action failed.
    Failed { reason: String },
    /// Refused or failed to spawn before running.
    NotStarted { reason: String },
    /// The runtime could not determine what happened.
    Unknown { reason: String },
}

impl ExecutionOutcome {
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            ExecutionOutcome::Exited { code: 0 } | ExecutionOutcome::Completed
        )
    }
}

/// What a receipt's effects range over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationScope {
    pub root: PathBuf,
    /// Top-level names never observed (runtime-owned state).
    pub excluded: Vec<String>,
    /// Symlinks are recorded by target string and never followed.
    pub follows_symlinks: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticEvidence {
    pub oracle: String,
    pub statement: String,
    /// Oracle confidence in basis points (0..=10000).
    pub confidence_bp: u16,
}

/// Interpretation of whether observed effects satisfy the request. Has no
/// verified variant by design.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SemanticAssessment {
    Unknown { reason: String },
    Semantic(SemanticEvidence),
}

/// Future attachment point for semantic interpretation (e.g. "does this
/// delta satisfy the request?"). An oracle sees the receipt read-only and
/// can only return [`SemanticEvidence`]. Returning `None` means
/// unavailable; the runtime then proceeds with the assessment `Unknown`.
pub trait SemanticOracle {
    fn name(&self) -> &str;
    fn assess(&self, requested: &str, receipt: &EffectReceipt) -> Option<SemanticEvidence>;
}

/// The default: no semantic interpretation.
pub struct NoSemanticOracle;

impl SemanticOracle for NoSemanticOracle {
    fn name(&self) -> &str {
        "none"
    }

    fn assess(&self, _requested: &str, _receipt: &EffectReceipt) -> Option<SemanticEvidence> {
        None
    }
}

/// Derived claim about the state transition across one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectReceipt {
    action: Action,
    requested: Option<String>,
    outcome: ExecutionOutcome,
    scope: ObservationScope,
    pre_state: ContentHash,
    post_state: Option<ContentHash>,
    verified: Vec<Verified<FsEffect>>,
    unknown: Vec<UnknownEffect>,
    semantic: SemanticAssessment,
}

impl EffectReceipt {
    /// Build a receipt from a pre-observation and the result of the
    /// post-observation. If the post-observation failed, or covers a
    /// different root, nothing is verified and the whole scope is unknown.
    pub fn from_observations(
        action: Action,
        requested: Option<String>,
        outcome: ExecutionOutcome,
        pre: &Observation,
        post: Result<&Observation, String>,
    ) -> Self {
        let scope = ObservationScope {
            root: pre.root.clone(),
            excluded: Vec::new(),
            follows_symlinks: false,
        };
        let (post_state, verified, unknown) = match post {
            Ok(post) if post.root == pre.root => {
                let delta = diff(&pre.state, &post.state);
                (
                    Some(post.state.digest()),
                    delta.effects.into_iter().map(Verified).collect(),
                    delta.unknown,
                )
            }
            Ok(post) => (
                None,
                Vec::new(),
                vec![UnknownEffect {
                    path: ".".to_string(),
                    reason: format!(
                        "post-observation covers a different root: {}",
                        post.root.display()
                    ),
                }],
            ),
            Err(err) => (
                None,
                Vec::new(),
                vec![UnknownEffect {
                    path: ".".to_string(),
                    reason: format!("post-state unobservable: {err}"),
                }],
            ),
        };
        Self {
            action,
            requested,
            outcome,
            scope,
            pre_state: pre.state.digest(),
            post_state,
            verified,
            unknown,
            semantic: SemanticAssessment::Unknown {
                reason: "no semantic oracle consulted".to_string(),
            },
        }
    }

    pub fn action(&self) -> &Action {
        &self.action
    }

    pub fn requested(&self) -> Option<&str> {
        self.requested.as_deref()
    }

    pub fn outcome(&self) -> &ExecutionOutcome {
        &self.outcome
    }

    pub fn scope(&self) -> &ObservationScope {
        &self.scope
    }

    pub fn pre_state(&self) -> ContentHash {
        self.pre_state
    }

    pub fn post_state(&self) -> Option<ContentHash> {
        self.post_state
    }

    pub fn verified(&self) -> &[Verified<FsEffect>] {
        &self.verified
    }

    pub fn unknown(&self) -> &[UnknownEffect] {
        &self.unknown
    }

    pub fn semantic(&self) -> &SemanticAssessment {
        &self.semantic
    }

    /// Verified effects on paths outside the command's declared writes.
    /// With no declared writes, every effect is undeclared.
    pub fn undeclared(&self) -> Vec<&FsEffect> {
        let Action::Command {
            declared_writes, ..
        } = &self.action
        else {
            return Vec::new();
        };
        let declared: Vec<String> = declared_writes
            .iter()
            .filter_map(|path| scope_relative(&self.scope.root, path))
            .collect();
        self.verified
            .iter()
            .map(Verified::get)
            .filter(|effect| !declared.iter().any(|d| path_within(effect.path(), d)))
            .collect()
    }

    /// Ask an oracle whether the observed transition satisfies the
    /// request. Only the semantic field can change; verified effects and
    /// unknowns are untouched whatever the oracle returns.
    pub fn assess_request(&mut self, oracle: &dyn SemanticOracle) {
        self.semantic = match self.requested.as_deref() {
            None => SemanticAssessment::Unknown {
                reason: "no requested effect stated".to_string(),
            },
            Some(requested) => match oracle.assess(requested, self) {
                Some(evidence) => SemanticAssessment::Semantic(evidence),
                None => SemanticAssessment::Unknown {
                    reason: format!("semantic oracle '{}' unavailable", oracle.name()),
                },
            },
        };
    }

    pub fn to_record(&self) -> ReceiptRecord {
        ReceiptRecord {
            action: self.action.clone(),
            requested: self.requested.clone(),
            outcome: self.outcome.clone(),
            scope: self.scope.clone(),
            pre_state: self.pre_state,
            post_state: self.post_state,
            verified: self.verified.iter().map(|v| v.get().clone()).collect(),
            undeclared: self
                .undeclared()
                .into_iter()
                .map(|e| e.path().to_string())
                .collect(),
            unknown: self.unknown.clone(),
            semantic: self.semantic.clone(),
        }
    }
}

/// Serializable report of an [`EffectReceipt`], as stored in the trace.
/// There is intentionally no conversion back into `EffectReceipt`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRecord {
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    pub outcome: ExecutionOutcome,
    pub scope: ObservationScope,
    pub pre_state: ContentHash,
    pub post_state: Option<ContentHash>,
    /// Effects the producing runtime classified as verified.
    pub verified: Vec<FsEffect>,
    /// Paths of verified effects outside the declared writes.
    pub undeclared: Vec<String>,
    pub unknown: Vec<UnknownEffect>,
    pub semantic: SemanticAssessment,
}

fn scope_relative(root: &Path, path: &Path) -> Option<String> {
    let path = if path.is_absolute() {
        normalize_path(path).strip_prefix(root).ok()?.to_path_buf()
    } else {
        normalize_path(path)
    };
    let parts: Option<Vec<&str>> = path.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

fn path_within(path: &str, declared: &str) -> bool {
    declared.is_empty()
        || path == declared
        || (path.len() > declared.len()
            && path.starts_with(declared)
            && path.as_bytes()[declared.len()] == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EntryKind;

    fn file(content: &[u8]) -> Entry {
        Entry {
            kind: EntryKind::File,
            len: Some(content.len() as u64),
            sha256: Some(ContentHash::of(content)),
            link_target: None,
        }
    }

    fn state(entries: &[(&str, Entry)]) -> FsState {
        let mut state = FsState::default();
        for (path, entry) in entries {
            state.entries.insert(path.to_string(), entry.clone());
        }
        state
    }

    #[test]
    fn diff_classifies_and_sorts() {
        let pre = state(&[("b", file(b"1")), ("c", file(b"gone"))]);
        let post = state(&[("a", file(b"new")), ("b", file(b"2"))]);
        let delta = diff(&pre, &post);
        let kinds: Vec<_> = delta
            .effects
            .iter()
            .map(|e| match e {
                FsEffect::Created { path, .. } => format!("+{path}"),
                FsEffect::Modified { path, .. } => format!("~{path}"),
                FsEffect::Deleted { path, .. } => format!("-{path}"),
            })
            .collect();
        assert_eq!(kinds, ["+a", "~b", "-c"]);
        assert!(delta.unknown.is_empty());
    }

    #[test]
    fn diff_of_identical_states_is_empty() {
        let s = state(&[("a", file(b"x"))]);
        assert_eq!(diff(&s, &s.clone()), StateDelta::default());
    }

    #[test]
    fn unobserved_subtree_yields_unknown_not_deleted() {
        let pre = state(&[
            (
                "d",
                Entry {
                    kind: EntryKind::Dir,
                    len: None,
                    sha256: None,
                    link_target: None,
                },
            ),
            ("d/x", file(b"x")),
            ("dx", file(b"sibling")),
        ]);
        let mut post = state(&[("dx", file(b"sibling"))]);
        post.unobserved.push(state::Unobserved {
            path: "d".to_string(),
            reason: "directory not listable".to_string(),
        });
        let delta = diff(&pre, &post);
        assert!(delta.effects.is_empty(), "{:?}", delta.effects);
        assert_eq!(delta.unknown.len(), 1);
        assert_eq!(delta.unknown[0].path, "d");
    }

    #[test]
    fn declared_path_matching_is_component_bounded() {
        assert!(path_within("out/a.txt", "out"));
        assert!(path_within("out", "out"));
        assert!(!path_within("output.txt", "out"));
        assert!(path_within("anything", ""));
    }
}
