//! Invariant kernel on the guarded path: proposals → obligations →
//! Allow / Deny / Blocked, with filesystem effects as evidence.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use uuid::Uuid;
use v9r_core::effect::{EffectReceipt, ReceiptRecord, SemanticEvidence, SemanticOracle};
use v9r_core::execution::{run_observed_step, run_task_step, CommandSpec};
use v9r_core::facts::{self, RuntimeEvidence, RuntimeFact, Subject, Value};
use v9r_core::guarded::{Authorize, GuardedTask, StepReport};
use v9r_core::kernel::{evaluate, Fact, Obligation, Phase, Requirement, Status, Strength, Verdict};
use v9r_core::manifest::Manifest;
use v9r_core::policy::{ActionProposal, Expectation, Policy, PolicyError, ProposedAction};
use v9r_core::state::ContentHash;
use v9r_core::task::Task;
use v9r_core::trace::{TaskEvent, TraceLogger};
use v9r_core::trusted::StateRoot;
use v9r_core::vfs;

struct Env {
    base: PathBuf,
    work: PathBuf,
    root: StateRoot,
}

impl Env {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("v9r-guarded-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        fs::create_dir_all(&work).unwrap();
        let root = StateRoot::open(&base.join("state")).unwrap();
        Self { base, work, root }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.work.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            allow_read: vec![self.work.clone()],
            allow_write: vec![self.work.clone()],
            allow_exec: ["touch", "cp", "rm", "true", "false", "cat", "mkdir", "ln"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            token_limit: 1,
            max_steps: 16,
            timeout_ms: 30_000,
            mandatory_artifacts: Vec::new(),
            test_commands: vec!["cat tests.ok".to_string()],
        }
    }

    async fn start_with(&self, manifest: Manifest) -> GuardedTask {
        let task = Task::new(manifest.clone(), self.work.clone());
        GuardedTask::start(
            task,
            Arc::new(Policy::standard(manifest)),
            self.root.clone(),
        )
        .await
        .unwrap()
    }

    async fn start(&self) -> GuardedTask {
        self.start_with(self.manifest()).await
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn command(argv: &[&str], writes: &[&str]) -> ActionProposal {
    ActionProposal {
        action: ProposedAction::Command(CommandSpec {
            program: argv[0].to_string(),
            args: argv[1..].iter().map(|s| s.to_string()).collect(),
            cwd: None,
            reads: Vec::new(),
            writes: writes.iter().map(PathBuf::from).collect(),
        }),
        requested: None,
        expectations: Vec::new(),
        proposer: "test-agent".to_string(),
    }
}

fn export() -> ActionProposal {
    ActionProposal {
        action: ProposedAction::Export,
        requested: None,
        expectations: Vec::new(),
        proposer: "test-agent".to_string(),
    }
}

fn expect(mut proposal: ActionProposal, expectation: Expectation) -> ActionProposal {
    proposal.expectations.push(expectation);
    proposal
}

/// Authorize and, if allowed, execute. Returns the PRE verdict and the
/// step report when it ran.
async fn run(guard: &mut GuardedTask, proposal: ActionProposal) -> (Verdict, Option<StepReport>) {
    match guard.authorize(proposal).await.unwrap() {
        Authorize::Allowed(auth) => (Verdict::Allow, Some(guard.execute(*auth).await.unwrap())),
        other => (other.verdict(), None),
    }
}

fn finding<'a>(decision: &'a v9r_core::guarded::RuntimeDecision, invariant: &str) -> &'a Status {
    &decision
        .findings
        .iter()
        .find(|f| f.obligation.invariant == invariant)
        .unwrap_or_else(|| panic!("no finding for {invariant} in:\n{decision}"))
        .status
}

fn trace_events(trace: &TraceLogger) -> Vec<TaskEvent> {
    fs::read_to_string(trace.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

// 5. An in-scope, declared filesystem action is allowed before and after.
#[tokio::test]
async fn allowed_filesystem_action_is_allowed_pre_and_post() {
    let env = Env::new("allow");
    let mut guard = env.start().await;
    let (pre, report) = run(&mut guard, command(&["mkdir", "out"], &["out"])).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Allow,
        "{}",
        report.decision
    );
    assert!(guard.is_accepting());
    assert!(env.work.join("out").is_dir());
}

// 6a. A declared write outside the writable scope is denied before execution.
#[tokio::test]
async fn declared_write_outside_scope_is_denied_before_execution() {
    let env = Env::new("scope-pre");
    let mut manifest = env.manifest();
    manifest.allow_write = vec![env.work.join("out")];
    let mut guard = env.start_with(manifest).await;
    let result = guard
        .authorize(command(&["touch", "elsewhere.txt"], &["elsewhere.txt"]))
        .await
        .unwrap();
    let Authorize::Denied(decision) = result else {
        panic!("expected Deny")
    };
    assert!(matches!(
        finding(&decision, "I1.declared_writes_in_scope"),
        Status::Violated(_)
    ));
    assert!(!env.work.join("elsewhere.txt").exists());
}

// 6b. An undeclared observed write is denied after execution, the
// workspace is held until rollback, and rollback restores it.
#[tokio::test]
async fn undeclared_observed_write_is_denied_and_held_until_rollback() {
    let env = Env::new("scope-post");
    let mut guard = env.start().await;
    let (_, report) = run(
        &mut guard,
        command(&["touch", "out.txt", "stray.txt"], &["out.txt"]),
    )
    .await;
    let report = report.unwrap();
    assert_eq!(report.decision.verdict, Verdict::Deny);
    let Status::Violated(because) = finding(&report.decision, "I1.observed_writes_declared") else {
        panic!("{}", report.decision)
    };
    assert!(because.contains("stray.txt"), "{because}");
    assert!(!guard.is_accepting());

    // Nothing else is authorized on top of an unaccepted transition.
    let result = guard.authorize(command(&["true"], &[])).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(matches!(
        finding(result.decision(), "workspace_accepted"),
        Status::Violated(_)
    ));

    let rolled = guard.rollback().await.unwrap();
    assert_eq!(
        rolled.decision.verdict,
        Verdict::Allow,
        "{}",
        rolled.decision
    );
    assert!(guard.is_accepting());
    assert!(!env.work.join("stray.txt").exists());
    assert_eq!(
        guard
            .authorize(command(&["true"], &[]))
            .await
            .unwrap()
            .verdict(),
        Verdict::Allow
    );
}

// 7a. Writing runtime-looking paths in the workspace touches only the
// workspace; trusted state stays intact.
#[tokio::test]
async fn agent_writes_cannot_reach_trusted_state() {
    let env = Env::new("trusted-writes");
    let mut guard = env.start().await;
    let (_, report) = run(
        &mut guard,
        command(&["mkdir", "-p", ".v9r/backups"], &[".v9r"]),
    )
    .await;
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Allow,
        "{}",
        report.decision
    );
    assert!(matches!(
        finding(&report.decision, "I4.trusted_state_intact"),
        Status::Satisfied(_)
    ));
    assert!(!guard.trace().path().starts_with(&env.work));
}

// 7b. Out-of-band tampering with the trace (a same-UID process) is caught.
#[tokio::test]
async fn out_of_band_trace_tampering_is_denied() {
    let env = Env::new("trusted-trace");
    let mut guard = env.start().await;
    let mut bytes = fs::read(guard.trace().path()).unwrap();
    bytes.extend_from_slice(b"{\"TaskFinished\":{\"status\":\"Success\"}}\n");
    fs::write(guard.trace().path(), bytes).unwrap();
    let result = guard.authorize(command(&["true"], &[])).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(matches!(
        finding(result.decision(), "I4.trusted_state_intact"),
        Status::Violated(_)
    ));
}

// 7c. Tampering with a checkpoint manifest is caught before authority is
// granted, and rollback refuses the forged checkpoint.
#[tokio::test]
async fn checkpoint_tampering_is_denied_and_not_restorable() {
    let env = Env::new("trusted-checkpoint");
    env.write("a.txt", "orig");
    let mut guard = env.start().await;
    let checkpoints = env.root.checkpoints_dir(guard.task().id);
    let manifest = fs::read_dir(&checkpoints)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("manifest.json");
    let forged = fs::read_to_string(&manifest)
        .unwrap()
        .replace("a.txt", "b.txt");
    fs::write(&manifest, forged).unwrap();

    let result = guard.authorize(command(&["true"], &[])).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny, "{}", result.decision());

    // The restore itself is refused (seal mismatch) and reported as a
    // failed outcome; the kernel denies on I4 even though the workspace
    // happens to still equal the checkpoint (I3 holds).
    let rolled = guard.rollback().await.unwrap();
    assert!(!rolled.receipt.as_ref().unwrap().outcome().is_success());
    assert_eq!(rolled.decision.verdict, Verdict::Deny);
    assert!(matches!(
        finding(&rolled.decision, "I3.rollback_restores_checkpoint"),
        Status::Satisfied(_)
    ));
    assert!(matches!(
        finding(&rolled.decision, "I4.trusted_state_intact"),
        Status::Violated(_)
    ));
    assert!(!guard.is_accepting());
}

// 8. An observed effect satisfies a postcondition.
#[tokio::test]
async fn observed_effect_satisfies_postcondition() {
    let env = Env::new("post-ok");
    let mut guard = env.start().await;
    let proposal = expect(
        command(&["touch", "report.txt"], &["report.txt"]),
        Expectation::Exists {
            path: "report.txt".into(),
        },
    );
    let (_, report) = run(&mut guard, proposal).await;
    let report = report.unwrap();
    assert_eq!(report.decision.verdict, Verdict::Allow);
    assert!(matches!(
        finding(&report.decision, "expectations"),
        Status::Satisfied(_)
    ));
}

// 9. Command success without the required postcondition is not success.
#[tokio::test]
async fn command_success_without_postcondition_is_denied() {
    let env = Env::new("post-missing");
    let mut guard = env.start().await;
    let proposal = expect(
        command(&["true"], &[]),
        Expectation::Exists {
            path: "report.txt".into(),
        },
    );
    let (_, report) = run(&mut guard, proposal).await;
    let report = report.unwrap();
    assert!(report.receipt.as_ref().unwrap().outcome().is_success());
    assert_eq!(report.decision.verdict, Verdict::Deny);
    let Status::Violated(because) = finding(&report.decision, "expectations") else {
        panic!("{}", report.decision)
    };
    assert!(because.contains("Absent"), "{because}");
}

// 10. A failed command's verified effect still counts.
#[tokio::test]
async fn failed_command_still_contributes_verified_effect() {
    let env = Env::new("post-failed");
    let mut guard = env.start().await;
    let proposal = expect(
        command(&["touch", "made.txt", "no-such-dir/x"], &["."]),
        Expectation::Exists {
            path: "made.txt".into(),
        },
    );
    let (_, report) = run(&mut guard, proposal).await;
    let report = report.unwrap();
    assert!(!report.receipt.as_ref().unwrap().outcome().is_success());
    assert!(matches!(
        finding(&report.decision, "expectations"),
        Status::Satisfied(_)
    ));
    assert_eq!(report.decision.verdict, Verdict::Allow);
}

// 11. Rollback restores the checkpoint (I3) and keeps historical evidence.
#[tokio::test]
async fn rollback_restores_state_and_preserves_history() {
    let env = Env::new("rollback");
    env.write("keep.txt", "original");
    let mut guard = env.start().await;
    let (_, report) = run(
        &mut guard,
        command(&["cp", "keep.txt", "copy.txt"], &["copy.txt"]),
    )
    .await;
    assert_eq!(report.unwrap().decision.verdict, Verdict::Allow);

    let rolled = guard.rollback().await.unwrap();
    assert!(matches!(
        finding(&rolled.decision, "I3.rollback_restores_checkpoint"),
        Status::Satisfied(_)
    ));
    assert!(matches!(
        finding(&rolled.decision, "I4.trusted_state_intact"),
        Status::Satisfied(_)
    ));
    assert!(!env.work.join("copy.txt").exists());

    let events = trace_events(guard.trace());
    let receipts: Vec<&ReceiptRecord> = events
        .iter()
        .filter_map(|e| match e {
            TaskEvent::EffectObserved { receipt } => Some(receipt.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].verified[0].path(), "copy.txt");
    assert!(events.iter().any(
        |e| matches!(e, TaskEvent::InvariantDecision { record } if record.action == "rollback")
    ));
}

// 12. Identical state and evidence give identical decisions.
#[tokio::test]
async fn decisions_are_deterministic_across_runs() {
    let mut records = Vec::new();
    for name in ["det-1", "det-2"] {
        let env = Env::new(name);
        env.write("src.txt", "same");
        let mut guard = env.start().await;
        let proposal = expect(
            command(&["cp", "src.txt", "dst.txt", "missing"], &["dst.txt"]),
            Expectation::Absent {
                path: "dst.txt".into(),
            },
        );
        let pre = guard.authorize(proposal).await.unwrap();
        let pre_record = pre.decision().record("x");
        let Authorize::Allowed(auth) = pre else {
            panic!()
        };
        let post = guard.execute(*auth).await.unwrap().decision.record("x");
        records.push((pre_record, post));
    }
    assert_eq!(records[0], records[1]);
}

// 1–4 at runtime level, plus stale evidence: export needs verified tests
// on the *current* version.
#[tokio::test]
async fn export_requires_verified_tests_on_current_version() {
    let env = Env::new("export");
    let mut guard = env.start().await;

    // Unknown: no test run yet.
    let result = guard.authorize(export()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());

    // Verified failure: the test command exits non-zero.
    run(&mut guard, command(&["cat", "tests.ok"], &[])).await;
    assert_eq!(
        guard.authorize(export()).await.unwrap().verdict(),
        Verdict::Deny
    );

    // Make the tests pass. That changes the version, so the old failure is
    // about a different version: unknown again.
    run(&mut guard, command(&["touch", "tests.ok"], &["tests.ok"])).await;
    assert_eq!(
        guard.authorize(export()).await.unwrap().verdict(),
        Verdict::Blocked
    );

    // Verified pass on this version.
    run(&mut guard, command(&["cat", "tests.ok"], &[])).await;
    let Authorize::Allowed(auth) = guard.authorize(export()).await.unwrap() else {
        panic!("export should be allowed")
    };
    let report = guard.execute(*auth).await.unwrap();
    assert_eq!(report.decision.verdict, Verdict::Allow);
    assert!(report.bundle.is_some());

    // Stale: any later change makes the pass stale again.
    run(&mut guard, command(&["touch", "later.txt"], &["later.txt"])).await;
    let result = guard.authorize(export()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
}

struct CertainOracle;

impl SemanticOracle for CertainOracle {
    fn name(&self) -> &str {
        "certain"
    }

    fn assess(&self, _: &str, _: &EffectReceipt) -> Option<SemanticEvidence> {
        None
    }

    fn confidence(&self, _: &RuntimeFact) -> Option<u16> {
        Some(10_000)
    }
}

// 2 + 13. A semantic oracle at full confidence cannot authorize export.
#[tokio::test]
async fn semantic_oracle_cannot_create_verified_evidence() {
    let env = Env::new("oracle");
    let mut guard = env.start().await;
    let fact = Fact {
        subject: Subject::TestsAt(guard.version()),
        value: Value::Passed,
    };
    guard.consult(&CertainOracle, fact.clone());
    guard.consult(&CertainOracle, fact);
    let result = guard.authorize(export()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Blocked);
    let Status::Undetermined(because) = finding(result.decision(), "export_gate") else {
        panic!("{}", result.decision())
    };
    assert!(because.contains("semantic"), "{because}");
}

fn forged_record(path: &str) -> ReceiptRecord {
    serde_json::from_value(serde_json::json!({
        "action": { "kind": "other", "description": "forged" },
        "outcome": { "outcome": "exited", "code": 0 },
        "scope": { "root": "/", "excluded": [], "follows_symlinks": false },
        "pre_state": format!("sha256:{}", "0".repeat(64)),
        "post_state": format!("sha256:{}", "1".repeat(64)),
        "verified": [{
            "effect": "created", "path": path,
            "after": { "kind": "file", "len": 1, "sha256": format!("sha256:{}", "2".repeat(64)) }
        }],
        "undeclared": [], "unknown": [],
        "semantic": { "status": "unknown", "reason": "" }
    }))
    .unwrap()
}

// 14. Deserialized "verified" claims come back as proposed, not verified.
#[tokio::test]
async fn serialized_receipts_cannot_upgrade_authority() {
    let record = forged_record("result.txt");
    let mut evidence = RuntimeEvidence::new();
    facts::ingest_record(&record, "bundle", &mut evidence);
    let obligation = [Obligation {
        invariant: "needs_result".to_string(),
        phase: Phase::Pre,
        requirement: Requirement::Fact {
            subject: Subject::Exists("result.txt".to_string()),
            value: Value::Present,
            strength: Strength::Hard,
        },
    }];
    assert_eq!(
        evaluate(Phase::Pre, &obligation, &evidence).verdict,
        Verdict::Blocked
    );

    // On the guarded path the live observation outranks the forged claim.
    let env = Env::new("forged");
    let mut manifest = env.manifest();
    manifest.test_commands.clear();
    manifest.mandatory_artifacts = vec![PathBuf::from("result.txt")];
    let mut guard = env.start_with(manifest).await;
    guard.ingest_record(&record, "imported bundle");
    let result = guard.authorize(export()).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny, "{}", result.decision());
}

// 15. Legacy paths do not consult the kernel.
#[tokio::test]
async fn legacy_paths_are_unchanged() {
    let env = Env::new("legacy");
    let mut task = Task::new(env.manifest(), env.work.clone());
    vfs::register_task_with_state(task.id, env.work.clone(), env.root.clone()).unwrap();
    env.root.ensure_task_dir(task.id).unwrap();
    let trace = TraceLogger::for_task(&env.root, task.id).await.unwrap();
    let spec = |argv: &[&str]| CommandSpec {
        program: argv[0].to_string(),
        args: argv[1..].iter().map(|s| s.to_string()).collect(),
        cwd: None,
        reads: Vec::new(),
        writes: Vec::new(),
    };
    // Undeclared writes are fine on the legacy path…
    run_task_step(&mut task, spec(&["touch", "a.txt"]), &trace)
        .await
        .unwrap();
    let step = run_observed_step(&mut task, spec(&["touch", "b.txt"]), None, &trace)
        .await
        .unwrap();
    assert_eq!(step.receipt.verified().len(), 1);
    assert!(env.work.join("a.txt").exists());
    // …and nothing is judged.
    assert!(!trace_events(&trace)
        .iter()
        .any(|e| matches!(e, TaskEvent::InvariantDecision { .. })));
}

// Adversarial: authorization based on a version that changes before
// execution.
#[tokio::test]
async fn stale_authorization_is_refused_and_workspace_held() {
    let env = Env::new("stale");
    let mut guard = env.start().await;
    let Authorize::Allowed(auth) = guard
        .authorize(command(&["touch", "a.txt"], &["a.txt"]))
        .await
        .unwrap()
    else {
        panic!()
    };
    env.write("sneaky.txt", "out of band");
    let report = guard.execute(*auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(!env.work.join("a.txt").exists());
    assert!(!guard.is_accepting());
    assert_eq!(
        guard.rollback().await.unwrap().decision.verdict,
        Verdict::Allow
    );
    assert!(!env.work.join("sneaky.txt").exists());
}

// Adversarial: a symlink into trusted state. Present at start, the
// checkpoint refuses it; planted mid-task, it is an unauthorized change.
#[cfg(unix)]
#[tokio::test]
async fn symlink_into_trusted_state_is_refused() {
    let env = Env::new("symlink-start");
    std::os::unix::fs::symlink(env.root.path(), env.work.join("state")).unwrap();
    let task = Task::new(env.manifest(), env.work.clone());
    let started = GuardedTask::start(
        task,
        Arc::new(Policy::standard(env.manifest())),
        env.root.clone(),
    )
    .await;
    assert!(started.is_err());

    let env = Env::new("symlink-mid");
    let mut guard = env.start().await;
    let task_dir = env.root.task_dir(guard.task().id);
    std::os::unix::fs::symlink(&task_dir, env.work.join("link")).unwrap();
    let Authorize::Allowed(auth) = guard
        .authorize(command(&["touch", "link/trace.jsonl"], &["link"]))
        .await
        .unwrap()
    else {
        panic!()
    };
    let report = guard.execute(*auth).await.unwrap();
    assert!(
        !report.executed,
        "the planted link made the authorization stale"
    );
    assert_eq!(
        guard.rollback().await.unwrap().decision.verdict,
        Verdict::Allow
    );
    assert!(fs::symlink_metadata(env.work.join("link")).is_err());
}

// Adversarial: the policy cannot come from the agent's workspace.
#[test]
fn policy_source_inside_workspace_is_refused() {
    let env = Env::new("policy-source");
    env.write("task.toml", "");
    let err = Policy::from_trusted_source(env.manifest(), &env.work.join("task.toml"), &env.work)
        .unwrap_err();
    assert!(matches!(err, PolicyError::SourceInWorkspace(_)));
    fs::write(env.base.join("task.toml"), "").unwrap();
    assert!(
        Policy::from_trusted_source(env.manifest(), &env.base.join("task.toml"), &env.work).is_ok()
    );
}

// Adversarial: expectations can only add obligations; a proposal cannot
// widen its scope beyond the manifest by declaring more.
#[tokio::test]
async fn proposals_cannot_weaken_the_policy() {
    let env = Env::new("weaken");
    let mut manifest = env.manifest();
    manifest.allow_write = vec![env.work.join("out")];
    let mut guard = env.start_with(manifest).await;
    let digest = guard.policy().digest();
    let result = guard
        .authorize(command(&["touch", "x"], &["."]))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    let result = guard
        .authorize(expect(
            command(&["true"], &[]),
            Expectation::Exists {
                path: "../escape".into(),
            },
        ))
        .await
        .unwrap();
    assert!(matches!(
        finding(result.decision(), "well_formed_proposal"),
        Status::Violated(_)
    ));
    assert_eq!(guard.policy().digest(), digest);
}

// Every verified entry the runtime relies on carries provenance (I2).
#[tokio::test]
async fn verified_evidence_carries_provenance() {
    let env = Env::new("provenance");
    let mut guard = env.start().await;
    run(&mut guard, command(&["cat", "tests.ok"], &[])).await;
    let result = guard.authorize(export()).await.unwrap();
    for finding in &result.decision().findings {
        if let Status::Satisfied(because) | Status::Violated(because) = &finding.status {
            if matches!(finding.obligation.requirement, Requirement::Fact { .. }) {
                assert!(because.contains("verified by"), "{because}");
            }
        }
    }
    let _ = ContentHash::of(b"");
}
