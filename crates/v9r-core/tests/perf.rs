//! Coarse overhead measurements for the guarded path. Not a benchmark
//! suite; meant to catch pathological scaling. Run with:
//!
//!   cargo test --release -p v9r-core --test perf -- --ignored --nocapture --test-threads=1

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;
use v9r_core::effect::Observation;
use v9r_core::execution::{run_observed_step, run_task_step, CommandSpec};
use v9r_core::facts::{Subject, Value};
use v9r_core::guarded::{Authorize, GuardedTask};
use v9r_core::kernel::{evaluate, Obligation, Phase, Requirement, Strength};
use v9r_core::manifest::Manifest;
use v9r_core::policy::{ActionProposal, Policy, ProposedAction};
use v9r_core::task::Task;
use v9r_core::trace::TraceLogger;
use v9r_core::trusted::StateRoot;
use v9r_core::vfs;

struct Env {
    base: PathBuf,
    work: PathBuf,
    root: StateRoot,
}

impl Env {
    fn new(files: usize, bytes: usize) -> Self {
        let base =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("v9r-perf-{}", Uuid::new_v4()));
        let work = base.join("work");
        let payload = vec![b'x'; bytes];
        for i in 0..files {
            let dir = work.join(format!("d{:03}", i / 100));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("f{i}.bin")), &payload).unwrap();
        }
        fs::create_dir_all(&work).unwrap();
        // CARGO_TARGET_TMPDIR is inside the repository: opt in explicitly.
        fs::write(work.join(".v9r-workdir"), b"").unwrap();
        let root = StateRoot::open(&base.join("state")).unwrap();
        Self { base, work, root }
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            allow_read: vec![self.work.clone()],
            allow_write: vec![self.work.clone()],
            allow_exec: vec!["true".to_string()],
            token_limit: 1,
            max_steps: 10_000,
            timeout_ms: 600_000,
            mandatory_artifacts: Vec::new(),
            test_commands: Vec::new(),
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn spec() -> CommandSpec {
    CommandSpec {
        program: "true".to_string(),
        args: Vec::new(),
        cwd: None,
        reads: Vec::new(),
        writes: Vec::new(),
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1e3)
}

async fn measure(files: usize, bytes: usize) {
    const ROUNDS: usize = 7;
    let env = Env::new(files, bytes);

    let observe = median(
        (0..ROUNDS)
            .map(|_| {
                let t = Instant::now();
                Observation::capture(&env.work).unwrap();
                t.elapsed()
            })
            .collect(),
    );

    // Legacy: no observation at all.
    let mut task = Task::new(env.manifest(), env.work.clone());
    vfs::register_task_with_state(task.id, env.work.clone(), env.root.clone()).unwrap();
    env.root.ensure_task_dir(task.id).unwrap();
    let trace = TraceLogger::for_task(&env.root, task.id).await.unwrap();
    let mut legacy = Vec::new();
    let mut observed = Vec::new();
    for _ in 0..ROUNDS {
        let t = Instant::now();
        run_task_step(&mut task, spec(), &trace).await.unwrap();
        legacy.push(t.elapsed());
        let t = Instant::now();
        run_observed_step(&mut task, spec(), None, &trace)
            .await
            .unwrap();
        observed.push(t.elapsed());
    }

    // Guarded: start (checkpoint) + authorize + execute.
    let t = Instant::now();
    let manifest = env.manifest();
    let mut guard = GuardedTask::start(
        Task::new(manifest.clone(), env.work.clone()),
        Arc::new(Policy::standard(manifest)),
        env.root.clone(),
    )
    .await
    .unwrap();
    let start = t.elapsed();
    let (mut authorize, mut execute) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let proposal = ActionProposal {
            action: ProposedAction::Command(spec()),
            requested: None,
            expectations: Vec::new(),
            proposer: "perf".to_string(),
        };
        let t = Instant::now();
        let Authorize::Allowed(auth) = guard.authorize(proposal).await.unwrap() else {
            panic!("not allowed")
        };
        authorize.push(t.elapsed());
        let t = Instant::now();
        guard.execute(*auth).await.unwrap();
        execute.push(t.elapsed());
    }
    let t = Instant::now();
    guard.trace().verify_integrity().await.unwrap();
    let verify = t.elapsed();
    let trace_len = fs::metadata(guard.trace().path()).unwrap().len();
    let t = Instant::now();
    guard.rollback().await.unwrap();
    let rollback = t.elapsed();

    println!(
        "| {files} x {bytes} B | {} | {} | {} | {} | {} | {} | {} | {} ({} KiB) | {} |",
        ms(observe),
        ms(median(legacy)),
        ms(median(observed)),
        ms(start),
        ms(median(authorize.clone())),
        ms(median(execute.clone())),
        ms(median(
            authorize
                .iter()
                .zip(&execute)
                .map(|(a, e)| *a + *e)
                .collect()
        )),
        ms(verify),
        trace_len / 1024,
        ms(rollback),
    );
}

#[tokio::test]
#[ignore]
async fn overhead_table() {
    println!(
        "| workspace | observe | legacy step | observed step | guarded start | authorize | execute | guarded total | trace verify (size) | rollback (no-op) |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (files, bytes) in [
        (10, 1024),
        (1_000, 4 * 1024),
        (10_000, 1024),
        (100, 1024 * 1024),
    ] {
        measure(files, bytes).await;
    }
}

#[test]
#[ignore]
fn kernel_evaluation_cost() {
    let obligations: Vec<Obligation<Subject, Value>> = (0..10)
        .map(|i| Obligation {
            invariant: format!("i{i}"),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: Subject::Exists(format!("p{i}")),
                value: Value::Present,
                strength: Strength::Hard,
            },
        })
        .collect();
    let evidence = v9r_core::facts::RuntimeEvidence::new();
    const N: u32 = 100_000;
    let t = Instant::now();
    for _ in 0..N {
        std::hint::black_box(evaluate(Phase::Pre, &obligations, &evidence));
    }
    println!(
        "kernel evaluate, 10 obligations, empty evidence: {:.2} us/decision",
        t.elapsed().as_secs_f64() * 1e6 / f64::from(N)
    );
}
