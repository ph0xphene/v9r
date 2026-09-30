//! v9r's fact vocabulary and the evidence sources that feed the kernel.
//!
//! The kernel sees only `(Subject, Value)` pairs. Everything that knows
//! how a fact is established (observations, receipts, exit statuses,
//! trusted-state seals) lives here, and this is the only module besides
//! `effect` that attests `Verified` runtime facts.
//!
//! | Subject | Verified by | Basis |
//! |---|---|---|
//! | `Path(p)`, `Exists(p)` | live workspace observation | observation digest |
//! | `Workspace` | live observation, or a live receipt's post-state digest | that digest |
//! | `TestsAt(v)` | the runtime's own wait status of a configured test command started on workspace version `v` | `v` |
//! | `TrustedState` | trace seal + checkpoint seals | seal check |
//! | `Transitions` | the runtime's own record of post-execution verdicts | task |
//!
//! Anything read back from disk (a `ReceiptRecord` in a trace or bundle)
//! enters only as `Proposed`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::effect::{EffectReceipt, ExecutionOutcome, FsEffect, Observation, ReceiptRecord};
use crate::kernel::{EvidenceBase, Fact, Name, Proposed, Provenance, Verified};
use crate::state::{ContentHash, Entry, EntryKind};
use crate::trace::{TraceIntegrity, TraceLogger};
use crate::vfs;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "subject", content = "of", rename_all = "snake_case")]
pub enum Subject {
    /// Exact observable state of a workspace path.
    Path(String),
    /// Whether anything exists at a workspace path.
    Exists(String),
    /// Digest of the whole workspace observation ("version").
    Workspace,
    /// Outcome of a configured test command started on this version.
    TestsAt(ContentHash),
    /// Integrity of the task's trusted runtime state.
    TrustedState,
    /// Whether every workspace transition so far was accepted.
    Transitions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "value", rename_all = "snake_case")]
pub enum Value {
    Absent,
    Present,
    File { sha256: ContentHash },
    Dir,
    Symlink { target: String },
    Other,
    Digest { sha256: ContentHash },
    Passed,
    Failed { code: Option<i32> },
    Intact,
    Tampered { reason: String },
    Accepted,
    Unaccepted { reason: String },
}

pub type RuntimeFact = Fact<Subject, Value>;
pub type RuntimeEvidence = EvidenceBase<Subject, Value>;

fn verified(subject: Subject, value: Value) -> Verified<RuntimeFact> {
    Verified::attest(Fact { subject, value })
}

fn entry_value(entry: &Entry) -> Value {
    match entry.kind {
        EntryKind::File => match entry.sha256 {
            Some(sha256) => Value::File { sha256 },
            None => Value::Other,
        },
        EntryKind::Dir => Value::Dir,
        EntryKind::Symlink => Value::Symlink {
            target: entry.link_target.clone().unwrap_or_default(),
        },
        EntryKind::Other => Value::Other,
    }
}

/// Verified facts about `subjects`, read from a live observation. Paths
/// the observation could not see yield no fact (unknown), never `Absent`.
pub(crate) fn from_observation<'a>(
    observation: &Observation,
    subjects: impl IntoIterator<Item = &'a Subject>,
    evidence: &mut RuntimeEvidence,
) {
    let state = observation.state();
    let digest = observation.digest();
    let provenance = Provenance {
        observer: "workspace-observation".to_string(),
        basis: digest.to_string(),
    };
    for subject in subjects {
        let value = match subject {
            Subject::Workspace => Value::Digest { sha256: digest },
            Subject::Path(path) | Subject::Exists(path) => {
                if state.unobserved_cover(path).is_some() {
                    continue;
                }
                match (subject, state.get(path)) {
                    (Subject::Exists(_), Some(_)) => Value::Present,
                    (_, None) => Value::Absent,
                    (_, Some(entry)) => entry_value(entry),
                }
            }
            _ => continue,
        };
        evidence.add_verified(verified(subject.clone(), value), provenance.clone());
    }
}

/// The workspace version a live receipt ends in, if it was observable.
pub(crate) fn from_receipt(receipt: &EffectReceipt, evidence: &mut RuntimeEvidence) {
    if let Some(sha256) = receipt.post_state() {
        evidence.add_verified(
            verified(Subject::Workspace, Value::Digest { sha256 }),
            Provenance {
                observer: "effect-receipt".to_string(),
                basis: sha256.to_string(),
            },
        );
    }
}

/// Names a live receipt touched, for containment checks: verified effects
/// are known names, unknown effects are unknown subtrees.
pub fn receipt_names(receipt: &EffectReceipt) -> Vec<Name> {
    receipt
        .verified()
        .iter()
        .map(|effect| Name::Known(effect.get().path().to_string()))
        .chain(receipt.unknown().iter().map(|unknown| {
            let path = if unknown.path == "." {
                String::new()
            } else {
                unknown.path.clone()
            };
            Name::UnknownBelow(path)
        }))
        .collect()
}

/// A test command's result as observed by the runtime itself. Only a
/// real exit status counts; denial, signals and unknown outcomes yield
/// no fact.
pub(crate) fn test_outcome(
    version: ContentHash,
    outcome: &ExecutionOutcome,
    argv: &str,
) -> Option<(Verified<RuntimeFact>, Provenance)> {
    let ExecutionOutcome::Exited { code } = outcome else {
        return None;
    };
    let value = if *code == 0 {
        Value::Passed
    } else {
        Value::Failed { code: Some(*code) }
    };
    Some((
        verified(Subject::TestsAt(version), value),
        Provenance {
            observer: "runtime-wait-status".to_string(),
            basis: format!("{argv} on {version}"),
        },
    ))
}

/// Check the task's trace and checkpoint seals. `None` if the check itself
/// could not be completed (unknown).
pub(crate) async fn trusted_state(
    task_id: Uuid,
    trace: &TraceLogger,
) -> Option<(Verified<RuntimeFact>, Provenance)> {
    let trace_status = trace.verify_integrity().await.ok()?;
    let tampered = vfs::tampered_checkpoints(task_id).ok()?;
    let value = match (trace_status, tampered.as_slice()) {
        (TraceIntegrity::Intact, []) => Value::Intact,
        (TraceIntegrity::Tampered { reason }, _) => Value::Tampered { reason },
        (TraceIntegrity::Intact, ids) => Value::Tampered {
            reason: format!(
                "checkpoint manifest(s) changed: {:?}",
                ids.iter().map(|id| id.0).collect::<Vec<_>>()
            ),
        },
    };
    Some((
        verified(Subject::TrustedState, value),
        Provenance {
            observer: "trusted-state-seals".to_string(),
            basis: "trace seal and checkpoint seals".to_string(),
        },
    ))
}

pub(crate) fn transitions(accepted: Result<(), String>) -> (Verified<RuntimeFact>, Provenance) {
    let value = match accepted {
        Ok(()) => Value::Accepted,
        Err(reason) => Value::Unaccepted { reason },
    };
    (
        verified(Subject::Transitions, value),
        Provenance {
            observer: "runtime-verdicts".to_string(),
            basis: "post-execution decisions of this task".to_string(),
        },
    )
}

/// Ingest a persisted receipt. Whatever it calls verified becomes a
/// `Proposed` claim attributed to the record: reading it back does not
/// re-establish it.
pub fn ingest_record(record: &ReceiptRecord, source: &str, evidence: &mut RuntimeEvidence) {
    let claim = |subject: Subject, value: Value| Proposed {
        value: Fact { subject, value },
        source: format!("record from {source}"),
    };
    for effect in &record.verified {
        let (path, value) = match effect {
            FsEffect::Created { path, after } | FsEffect::Modified { path, after, .. } => {
                (path, entry_value(after))
            }
            FsEffect::Deleted { path, .. } => (path, Value::Absent),
        };
        let exists = if value == Value::Absent {
            Value::Absent
        } else {
            Value::Present
        };
        evidence.add_proposed(claim(Subject::Path(path.clone()), value));
        evidence.add_proposed(claim(Subject::Exists(path.clone()), exists));
    }
    if let Some(sha256) = record.post_state {
        evidence.add_proposed(claim(Subject::Workspace, Value::Digest { sha256 }));
    }
}
