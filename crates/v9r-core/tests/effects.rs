//! Effect runtime v0: observed filesystem effects vs. command results.
//!
//! Every test runs real commands (coreutils, no shell) through the
//! existing `run_task_step` path, bracketed by observations.

use std::fs;
use std::path::PathBuf;

use uuid::Uuid;
use v9r_core::effect::{
    self, EffectReceipt, ExecutionOutcome, FsEffect, NoSemanticOracle, Observation, ReceiptRecord,
    SemanticAssessment, SemanticEvidence, SemanticOracle,
};
use v9r_core::execution::{run_observed_step, CommandSpec, ObservedStep};
use v9r_core::manifest::Manifest;
use v9r_core::state::{ContentHash, EntryKind};
use v9r_core::task::Task;
use v9r_core::trace::{TaskEvent, TraceLogger};
use v9r_core::trusted::StateRoot;
use v9r_core::vfs;

/// `.0` is the agent workspace, `.1` the trusted state root next to it.
struct Workdir(PathBuf, StateRoot, PathBuf);

impl Workdir {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("v9r-effects-{name}-{}", Uuid::new_v4()));
        let dir = base.join("work");
        fs::create_dir_all(&dir).unwrap();
        let root = StateRoot::open(&base.join("state")).unwrap();
        Self(dir, root, base)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

impl Drop for Workdir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.2);
    }
}

fn task_in(workdir: &Workdir) -> Task {
    let dir = workdir.0.as_path();
    let manifest = Manifest {
        allow_read: vec![dir.to_path_buf()],
        allow_write: vec![dir.to_path_buf()],
        allow_exec: [
            "touch", "cp", "rm", "mv", "mkdir", "true", "false", "cat", "ln", "chmod",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        token_limit: 1,
        max_steps: 16,
        timeout_ms: 30_000,
        mandatory_artifacts: Vec::new(),
        test_commands: Vec::new(),
    };
    let task = Task::new(manifest, dir.to_path_buf());
    vfs::register_task_with_state(task.id, dir.to_path_buf(), workdir.1.clone()).unwrap();
    task
}

async fn trace_for(dir: &Workdir, task: &Task) -> TraceLogger {
    dir.1.ensure_task_dir(task.id).unwrap();
    TraceLogger::for_task(&dir.1, task.id).await.unwrap()
}

fn spec(argv: &[&str]) -> CommandSpec {
    CommandSpec {
        program: argv[0].to_string(),
        args: argv[1..].iter().map(|s| s.to_string()).collect(),
        cwd: None,
        reads: Vec::new(),
        writes: Vec::new(),
    }
}

async fn observed(dir: &Workdir, argv: &[&str]) -> (ObservedStep, TraceLogger) {
    let mut task = task_in(dir);
    let trace = trace_for(dir, &task).await;
    let step = run_observed_step(&mut task, spec(argv), None, &trace)
        .await
        .unwrap();
    (step, trace)
}

/// `+path`, `~path`, `-path` for created / modified / deleted.
fn summary(receipt: &EffectReceipt) -> Vec<String> {
    receipt
        .verified()
        .iter()
        .map(|v| match v.get() {
            FsEffect::Created { path, .. } => format!("+{path}"),
            FsEffect::Modified { path, .. } => format!("~{path}"),
            FsEffect::Deleted { path, .. } => format!("-{path}"),
        })
        .collect()
}

fn exit_code(receipt: &EffectReceipt) -> i32 {
    match receipt.outcome() {
        ExecutionOutcome::Exited { code } => *code,
        other => panic!("unexpected outcome: {other:?}"),
    }
}

fn trace_receipts(trace: &TraceLogger) -> Vec<ReceiptRecord> {
    fs::read_to_string(trace.path())
        .unwrap()
        .lines()
        .filter_map(
            |line| match serde_json::from_str::<TaskEvent>(line).unwrap() {
                TaskEvent::EffectObserved { receipt } => Some(*receipt),
                _ => None,
            },
        )
        .collect()
}

// A. success + mutation
#[tokio::test]
async fn a_successful_command_with_verified_creation() {
    let dir = Workdir::new("a");
    let (step, _) = observed(&dir, &["touch", "created.txt"]).await;
    assert!(step.result.is_ok());
    assert!(step.receipt.outcome().is_success());
    assert_eq!(summary(&step.receipt), ["+created.txt"]);
    let FsEffect::Created { after, .. } = step.receipt.verified()[0].get() else {
        unreachable!()
    };
    assert_eq!(after.kind, EntryKind::File);
    assert_eq!(after.len, Some(0));
    assert_eq!(after.sha256, Some(ContentHash::of(b"")));
    assert!(step.receipt.unknown().is_empty());
}

// B. success, no observable change
#[tokio::test]
async fn b_successful_command_without_effect() {
    let dir = Workdir::new("b");
    dir.write("existing.txt", "content");
    let (step, _) = observed(&dir, &["cat", "existing.txt"]).await;
    assert_eq!(exit_code(&step.receipt), 0);
    assert!(step.receipt.verified().is_empty());
    assert!(step.receipt.unknown().is_empty());
    assert_eq!(Some(step.receipt.pre_state()), step.receipt.post_state());
}

// C. failure, no observable change
#[tokio::test]
async fn c_failed_command_without_effect() {
    let dir = Workdir::new("c");
    let (step, _) = observed(&dir, &["cp", "missing.txt", "out.txt"]).await;
    assert_ne!(exit_code(&step.receipt), 0);
    assert!(!step.receipt.outcome().is_success());
    assert!(step.receipt.verified().is_empty());
    assert!(!dir.0.join("out.txt").exists());
}

// D. failure *with* effect: the key case
#[tokio::test]
async fn d_failed_command_still_has_verified_effect() {
    let dir = Workdir::new("d");
    // touch creates the first operand, then fails on the second.
    let (step, trace) = observed(&dir, &["touch", "made.txt", "no-such-dir/x"]).await;
    assert_ne!(exit_code(&step.receipt), 0);
    assert!(!step.receipt.outcome().is_success());
    assert_eq!(summary(&step.receipt), ["+made.txt"]);
    // The persisted record keeps both facts side by side.
    let record = &trace_receipts(&trace)[0];
    assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 1 });
    assert_eq!(record.verified.len(), 1);
}

// E. modification with before/after evidence
#[tokio::test]
async fn e_modification_carries_before_and_after_hashes() {
    let dir = Workdir::new("e");
    dir.write("target.txt", "old");
    dir.write("source.txt", "new content");
    let (step, _) = observed(&dir, &["cp", "source.txt", "target.txt"]).await;
    assert_eq!(summary(&step.receipt), ["~target.txt"]);
    let FsEffect::Modified { before, after, .. } = step.receipt.verified()[0].get() else {
        unreachable!()
    };
    assert_eq!(before.sha256, Some(ContentHash::of(b"old")));
    assert_eq!(before.len, Some(3));
    assert_eq!(after.sha256, Some(ContentHash::of(b"new content")));
    assert_eq!(after.len, Some(11));
}

// F. deletion
#[tokio::test]
async fn f_deletion_carries_before_evidence() {
    let dir = Workdir::new("f");
    dir.write("victim.txt", "bye");
    let (step, _) = observed(&dir, &["rm", "victim.txt"]).await;
    assert_eq!(summary(&step.receipt), ["-victim.txt"]);
    let FsEffect::Deleted { before, .. } = step.receipt.verified()[0].get() else {
        unreachable!()
    };
    assert_eq!(before.sha256, Some(ContentHash::of(b"bye")));
}

// G. one action, several effects of each kind
#[tokio::test]
async fn g_multiple_effects_in_one_receipt() {
    let dir = Workdir::new("g");
    dir.write("a.txt", "a");
    dir.write("c.txt", "c-new");
    dir.write("dest/c.txt", "c-old");
    let (step, trace) = observed(&dir, &["mv", "a.txt", "c.txt", "dest"]).await;
    assert_eq!(exit_code(&step.receipt), 0);
    assert_eq!(
        summary(&step.receipt),
        ["-a.txt", "-c.txt", "+dest/a.txt", "~dest/c.txt"]
    );
    assert_eq!(trace_receipts(&trace).len(), 1);
}

// H. rollback does not erase what was observed during execution
#[tokio::test]
async fn h_rollback_keeps_historical_effects_separate_from_final_state() {
    let dir = Workdir::new("h");
    dir.write("keep.txt", "original");
    let mut task = task_in(&dir);
    let trace = trace_for(&dir, &task).await;
    let checkpoint = vfs::checkpoint(task.id, &trace).await.unwrap();
    let before_task = Observation::capture(&dir.0).unwrap();

    let step = run_observed_step(
        &mut task,
        spec(&["touch", "new.txt"]),
        Some("create new.txt".to_string()),
        &trace,
    )
    .await
    .unwrap();
    fs::write(dir.0.join("keep.txt"), "changed").unwrap();

    let rolled = vfs::rollback_observed(task.id, checkpoint, &trace)
        .await
        .unwrap();
    rolled.result.unwrap();

    // What happened during execution is still a verified fact...
    assert_eq!(summary(&step.receipt), ["+new.txt"]);
    // ...the rollback is its own action with its own verified effects...
    assert_eq!(rolled.receipt.outcome(), &ExecutionOutcome::Completed);
    assert_eq!(summary(&rolled.receipt), ["~keep.txt", "-new.txt"]);
    // ...and the final state equals the pre-task state.
    let after_rollback = Observation::capture(&dir.0).unwrap();
    assert!(effect::diff(before_task.state(), after_rollback.state())
        .effects
        .is_empty());
    assert_eq!(rolled.receipt.post_state(), Some(step.receipt.pre_state()));

    // The trace survives rollback and holds both receipts.
    let records = trace_receipts(&trace);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].verified[0].path(), "new.txt");
    assert_eq!(records[0].requested.as_deref(), Some("create new.txt"));
    assert!(matches!(records[1].action, effect::Action::Rollback { .. }));
}

// I. runtime state lives outside the workspace and never shows up as an effect
#[tokio::test]
async fn i_runtime_state_is_not_in_the_workspace() {
    let dir = Workdir::new("i");
    let mut task = task_in(&dir);
    let trace = trace_for(&dir, &task).await;
    vfs::checkpoint(task.id, &trace).await.unwrap();
    let step = run_observed_step(&mut task, spec(&["true"]), None, &trace)
        .await
        .unwrap();
    assert!(step.receipt.verified().is_empty());
    assert!(step.receipt.scope().excluded.is_empty());
    assert!(!step.receipt.scope().follows_symlinks);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    assert!(trace.path().starts_with(dir.1.path()));
}

// I'. …so the former blind spot is gone: writing `.v9r` in the workspace
// is an ordinary, observed effect that cannot reach runtime state.
#[tokio::test]
async fn i_writes_to_workspace_dot_v9r_are_observed() {
    let dir = Workdir::new("i-blind");
    fs::create_dir_all(dir.0.join(".v9r")).unwrap();
    let (step, _) = observed(&dir, &["touch", ".v9r/planted"]).await;
    assert_eq!(exit_code(&step.receipt), 0);
    assert_eq!(
        summary(&step.receipt),
        [".v9r/planted"].map(|p| format!("+{p}"))
    );
    assert!(fs::read_dir(dir.1.path().join("tasks"))
        .unwrap()
        .all(|task| !task.unwrap().path().join("planted").exists()));
}

// J. determinism
#[tokio::test]
async fn j_same_transition_yields_same_normalized_delta() {
    let mut records = Vec::new();
    for name in ["j1", "j2"] {
        let dir = Workdir::new(name);
        dir.write("z.txt", "z");
        dir.write("m/y.txt", "y");
        dir.write("dest/b.txt", "old");
        let (step, _) = observed(&dir, &["mv", "z.txt", "m/y.txt", "dest"]).await;
        let pre = Observation::capture(&dir.0).unwrap();
        // Diffing the same pair twice is stable too.
        assert_eq!(
            effect::diff(pre.state(), pre.state()),
            effect::diff(pre.state(), pre.state())
        );
        let mut record = step.receipt.to_record();
        record.scope.root = PathBuf::new();
        records.push(record);
    }
    assert_eq!(records[0], records[1]);
    assert_eq!(
        serde_json::to_string(&records[0]).unwrap(),
        serde_json::to_string(&records[1]).unwrap()
    );
}

// Adversarial / boundary cases.

#[tokio::test]
async fn identical_rewrite_is_not_an_effect() {
    let dir = Workdir::new("same-bytes");
    dir.write("a.txt", "same");
    dir.write("b.txt", "same");
    let (step, _) = observed(&dir, &["cp", "a.txt", "b.txt"]).await;
    assert_eq!(exit_code(&step.receipt), 0);
    // b.txt was written, but its observable state is unchanged. The
    // receipt makes no claim that it was untouched.
    assert!(step.receipt.verified().is_empty());
}

#[tokio::test]
async fn absent_before_and_after_is_not_a_deletion() {
    let dir = Workdir::new("ghost");
    let (step, _) = observed(&dir, &["rm", "-f", "ghost.txt"]).await;
    assert_eq!(exit_code(&step.receipt), 0);
    assert!(step.receipt.verified().is_empty());
}

#[tokio::test]
async fn rename_is_delete_plus_create_not_a_rename_claim() {
    let dir = Workdir::new("rename");
    dir.write("old.txt", "payload");
    let (step, _) = observed(&dir, &["mv", "old.txt", "new.txt"]).await;
    assert_eq!(summary(&step.receipt), ["+new.txt", "-old.txt"]);
    let hashes: Vec<_> = step
        .receipt
        .verified()
        .iter()
        .map(|v| match v.get() {
            FsEffect::Created { after, .. } => after.sha256,
            FsEffect::Deleted { before, .. } => before.sha256,
            FsEffect::Modified { .. } => unreachable!(),
        })
        .collect();
    // Same content moved: a fact about state, not evidence of a rename event.
    assert_eq!(hashes[0], hashes[1]);
}

#[tokio::test]
async fn nested_directories_are_individual_effects() {
    let dir = Workdir::new("nested");
    let (step, _) = observed(&dir, &["mkdir", "-p", "a/b/c"]).await;
    assert_eq!(summary(&step.receipt), ["+a", "+a/b", "+a/b/c"]);
}

#[tokio::test]
async fn deleting_a_tree_reports_every_entry() {
    let dir = Workdir::new("rm-tree");
    dir.write("t/u/v.txt", "v");
    let (step, _) = observed(&dir, &["rm", "-r", "t"]).await;
    assert_eq!(summary(&step.receipt), ["-t", "-t/u", "-t/u/v.txt"]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_creation_is_recorded_without_following() {
    let dir = Workdir::new("symlink");
    dir.write("target.txt", "secret");
    let (step, _) = observed(&dir, &["ln", "-s", "target.txt", "link"]).await;
    assert_eq!(summary(&step.receipt), ["+link"]);
    let FsEffect::Created { after, .. } = step.receipt.verified()[0].get() else {
        unreachable!()
    };
    assert_eq!(after.kind, EntryKind::Symlink);
    assert_eq!(after.link_target.as_deref(), Some("target.txt"));
    assert_eq!(after.sha256, None);
}

#[cfg(unix)]
#[tokio::test]
async fn file_becoming_unreadable_is_unknown_not_modified() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Workdir::new("unreadable");
    dir.write("f.txt", "x");
    let (step, _) = observed(&dir, &["chmod", "000", "f.txt"]).await;
    let readable = fs::read(dir.0.join("f.txt")).is_ok();
    fs::set_permissions(dir.0.join("f.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    if readable {
        return; // running as root; the file never became unobservable
    }
    assert_eq!(exit_code(&step.receipt), 0);
    assert!(step.receipt.verified().is_empty());
    assert_eq!(step.receipt.unknown().len(), 1);
    assert_eq!(step.receipt.unknown()[0].path, "f.txt");
    assert!(step.receipt.unknown()[0].reason.starts_with("post-state:"));
}

#[tokio::test]
async fn denied_command_is_not_started_and_has_no_effect() {
    let dir = Workdir::new("denied");
    let (step, _) = observed(&dir, &["touch", "../escape.txt"]).await;
    assert!(step.result.is_err());
    assert!(matches!(
        step.receipt.outcome(),
        ExecutionOutcome::NotStarted { .. }
    ));
    assert!(step.receipt.verified().is_empty());
}

#[tokio::test]
async fn effects_outside_declared_writes_are_flagged() {
    let dir = Workdir::new("declared");
    let mut task = task_in(&dir);
    let trace = trace_for(&dir, &task).await;
    let mut command = spec(&["touch", "out/a.txt", "stray.txt"]);
    command.writes = vec![PathBuf::from("out")];
    fs::create_dir_all(dir.0.join("out")).unwrap();
    let step = run_observed_step(&mut task, command, None, &trace)
        .await
        .unwrap();
    assert_eq!(summary(&step.receipt), ["+out/a.txt", "+stray.txt"]);
    let undeclared: Vec<_> = step.receipt.undeclared().iter().map(|e| e.path()).collect();
    assert_eq!(undeclared, ["stray.txt"]);
    assert_eq!(step.receipt.to_record().undeclared, ["stray.txt"]);
}

// Semantic boundary.

struct MockOracle;

impl SemanticOracle for MockOracle {
    fn name(&self) -> &str {
        "mock"
    }

    fn assess(&self, requested: &str, receipt: &EffectReceipt) -> Option<SemanticEvidence> {
        Some(SemanticEvidence {
            oracle: "mock".to_string(),
            statement: format!(
                "'{requested}' looks satisfied by {} effects",
                receipt.verified().len()
            ),
            confidence_bp: 9_000,
        })
    }
}

#[tokio::test]
async fn semantic_assessment_is_unknown_without_oracle_and_never_verified() {
    let dir = Workdir::new("semantic");
    let mut task = task_in(&dir);
    let trace = trace_for(&dir, &task).await;
    let mut step = run_observed_step(
        &mut task,
        spec(&["touch", "report.txt"]),
        Some("write the report".to_string()),
        &trace,
    )
    .await
    .unwrap();
    assert!(matches!(
        step.receipt.semantic(),
        SemanticAssessment::Unknown { .. }
    ));

    step.receipt.assess_request(&NoSemanticOracle);
    assert!(matches!(
        step.receipt.semantic(),
        SemanticAssessment::Unknown { reason } if reason.contains("unavailable")
    ));

    let verified_before = step.receipt.verified().to_vec();
    step.receipt.assess_request(&MockOracle);
    assert!(matches!(
        step.receipt.semantic(),
        SemanticAssessment::Semantic(SemanticEvidence {
            confidence_bp: 9_000,
            ..
        })
    ));
    // The oracle's opinion lives only in the semantic field.
    assert_eq!(step.receipt.verified(), verified_before.as_slice());
    assert!(step.receipt.unknown().is_empty());
}
