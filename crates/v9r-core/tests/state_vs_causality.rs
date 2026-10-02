//! State vs Causality v0: two executions, one observable result.
//!
//! World A: the authorized agent (the `Actor`) writes the change.
//! World B: the `Actor` changes nothing; an outsider (a thread that is
//! not the agent and holds no authorization) writes byte-identical
//! content during the same window.
//!
//! Both run through the unchanged `temporal::runtime` with the same
//! registry kinds and invariants. The invariants read state only; no
//! code from the workspace is executed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use v9r_core::fs_raw::RawFsObserver;
use v9r_core::graph::{Key, Registry, Term, Trust};
use v9r_core::kernel::{Obligation, Phase, Requirement, Status, Strength, Verdict};
use v9r_core::runtime::Authorize;
use v9r_core::snapshot::FsSnapshot;
use v9r_core::snapshot::ObjectStore;
use v9r_core::temporal::{
    self, changed, pin, require, Actor, Snapshot, TKey, TObligation, Transition,
    TransitionInvariant,
};

const S0_GREET: &[u8] = b"def greet(n):\n    return \"hello \" + n\n";
const S1_GREET: &[u8] = b"def greet(n):\n    return \"Hello, \" + n + \"!\"\n";
const TARGET: &str = "ws/src/greet.py";

#[derive(Clone, Debug)]
struct Task;

fn key(kind: &str, arg: &str) -> Key {
    Key::new(kind, [arg])
}

// ------------------------------------------------------------ invariants

struct InputApproved {
    s0: String,
}

impl TransitionInvariant<Task> for InputApproved {
    fn id(&self) -> &str {
        "I1.input_is_approved"
    }
    fn watches(&self) -> Vec<Key> {
        vec![key("snapshot", "ws")]
    }
    // PRE is re-checked at execution on a fresh snapshot, so it names the
    // state under check (`@current`), not the snapshot it was derived on.
    fn pre(&self, _: &Task, _before: &Snapshot) -> Vec<TObligation> {
        vec![Obligation {
            invariant: self.id().into(),
            phase: Phase::Pre,
            requirement: Requirement::Fact {
                subject: TKey {
                    at: temporal::At::Current,
                    key: key("snapshot", "ws"),
                },
                value: Term::Id(self.s0.clone()),
                strength: Strength::Hard,
            },
        }]
    }
    fn post(&self, t: &Transition<'_, Task>) -> Vec<TObligation> {
        vec![require(
            self.id(),
            TKey::at(t.before, key("snapshot", "ws")),
            Term::Id(self.s0.clone()),
        )]
    }
}

struct WithinScope;

impl TransitionInvariant<Task> for WithinScope {
    fn id(&self) -> &str {
        "I2.changes_within_scope"
    }
    fn watches(&self) -> Vec<Key> {
        vec![key("snapshot_entries", "ws")]
    }
    fn post(&self, t: &Transition<'_, Task>) -> Vec<TObligation> {
        let k = key("snapshot_entries", "ws");
        let (p0, e0) = pin(self.id(), t.before, &k);
        let mut out = vec![p0];
        let e1 = t.after.and_then(|a| {
            let (p1, e1) = pin(self.id(), a, &k);
            out.push(p1);
            e1
        });
        out.push(Obligation {
            invariant: self.id().into(),
            phase: Phase::Post,
            requirement: Requirement::Within {
                names: changed("", e0, e1),
                scopes: vec!["ws/src".into()],
            },
        });
        out
    }
}

/// I3: the output meets the specification, judged on the bytes alone.
struct OutputMeetsSpec;

impl TransitionInvariant<Task> for OutputMeetsSpec {
    fn id(&self) -> &str {
        "I3.output_meets_spec"
    }
    fn watches(&self) -> Vec<Key> {
        vec![key("fs_file", TARGET)]
    }
    fn post(&self, t: &Transition<'_, Task>) -> Vec<TObligation> {
        match t.after {
            Some(after) => vec![require(
                self.id(),
                TKey::at(after, key("fs_file", TARGET)),
                Term::Bytes(S1_GREET.to_vec()),
            )],
            None => vec![require(
                self.id(),
                TKey::now(key("fs_file", "unobserved")),
                Term::Bytes(S1_GREET.to_vec()),
            )],
        }
    }
}

// ------------------------------------------------------------ actors

/// World A: the authorized agent makes the change itself.
struct Agent {
    ws: PathBuf,
}

impl Actor<Task> for Agent {
    async fn act(&mut self, _: &Task) -> Result<String, String> {
        fs::write(self.ws.join("src/greet.py"), S1_GREET).map_err(|e| e.to_string())?;
        Ok(String::new())
    }
}

/// World B: the agent changes nothing. While it runs, an outsider that is
/// not the agent writes the same bytes; the agent only waits for it so
/// the write falls inside the transition window.
struct Bystander {
    start: mpsc::Sender<()>,
    done: mpsc::Receiver<()>,
}

impl Actor<Task> for Bystander {
    async fn act(&mut self, _: &Task) -> Result<String, String> {
        self.start.send(()).map_err(|e| e.to_string())?;
        self.done.recv().map_err(|e| e.to_string())?;
        Ok(String::new())
    }
}

// ------------------------------------------------------------ lab

struct Lab {
    base: PathBuf,
    store: ObjectStore,
}

impl Lab {
    fn new(tag: &str) -> Lab {
        let base = std::env::temp_dir().join(format!("v9r-svc-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(base.join("ws/src")).unwrap();
        fs::write(base.join("ws/src/greet.py"), S0_GREET).unwrap();
        fs::write(base.join("ws/README"), "read me\n").unwrap();
        Lab {
            base: fs::canonicalize(base).unwrap(),
            store: ObjectStore::new(),
        }
    }

    fn registry(&self) -> Registry {
        let r = Registry::new();
        r.register(RawFsObserver::new("raw", &self.base), Trust::Attesting);
        for kind in ["fs_file", "fs_link"] {
            r.restrict(kind, &["raw"]);
        }
        r.add_verifier(FsSnapshot::new(self.store.clone()));
        r
    }

    fn s0(&self) -> String {
        match self.registry().query(&key("snapshot", "ws")) {
            Some(Term::Id(id)) => id,
            other => panic!("S0: {other:?}"),
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn invariants(s0: &str) -> Vec<Box<dyn TransitionInvariant<Task>>> {
    vec![
        Box::new(InputApproved { s0: s0.into() }),
        Box::new(WithinScope),
        Box::new(OutputMeetsSpec),
    ]
}

// ------------------------------------------------------------ measurement

#[derive(Debug, PartialEq)]
struct Measured {
    s0: String,
    pre: Verdict,
    post: Verdict,
    accepted: bool,
    accepting_after: bool,
    s1: Option<Term>,
    /// (invariant, status kind) of every POST finding, in order.
    findings: Vec<(String, &'static str)>,
    /// Which observers the POST reasons name.
    observers: Vec<String>,
    agent_result: Result<String, String>,
    ordered: bool,
    journal: Vec<(String, Verdict)>,
}

fn kind(s: &Status) -> &'static str {
    match s {
        Status::Satisfied(_) => "satisfied",
        Status::Violated(_) => "violated",
        Status::Undetermined(_) => "undetermined",
    }
}

async fn run<A: Actor<Task>>(lab: &Lab, actor: A) -> (Measured, String) {
    let s0 = lab.s0();
    let mut rt = temporal::runtime(lab.registry(), invariants(&s0), actor);
    let auth = rt.authorize(Task).await.unwrap();
    let pre = auth.verdict();
    let auth = match auth {
        Authorize::Allowed(auth) => auth,
        other => panic!("PRE {pre:?}: {}", other.decision()),
    };
    let report = rt.execute(*auth).await.unwrap();
    let text = report.decision.to_string();
    assert!(report.executed, "execution refused:\n{text}");
    let receipt = report.receipt.as_ref().expect("executed");
    let s1 = receipt
        .after
        .as_ref()
        .and_then(|a| a.verified(&key("snapshot", "ws")).cloned());
    let ordered = report
        .decision
        .findings
        .iter()
        .any(|f| f.obligation.invariant == "transition.ordered" && kind(&f.status) == "satisfied");
    let mut observers: Vec<String> = ["provider:raw", "provider:fs", "verifier:fs-snapshot", "temporal-clock"]
        .iter()
        .filter(|o| text.contains(*o))
        .map(|o| o.to_string())
        .collect();
    observers.sort();
    let measured = Measured {
        s0,
        pre,
        post: report.decision.verdict,
        accepted: report.accepted,
        accepting_after: rt.is_accepting(),
        s1,
        findings: report
            .decision
            .findings
            .iter()
            .map(|f| (f.obligation.invariant.clone(), kind(&f.status)))
            .collect(),
        observers,
        agent_result: receipt.result.clone(),
        ordered,
        journal: rt
            .journal()
            .records()
            .iter()
            .map(|r| (r.action.clone(), r.verdict))
            .collect(),
    };
    (measured, text)
}

fn tree_of(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[tokio::test]
async fn state_vs_causality() {
    // World A: the authorized agent.
    let lab_a = Lab::new("a");
    let (a, text_a) = run(&lab_a, Agent { ws: lab_a.base.join("ws") }).await;

    // World B: a bystander agent; an outsider writes the same bytes.
    let lab_b = Lab::new("b");
    let (start_tx, start_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let target = lab_b.base.join("ws/src/greet.py");
    let outsider = std::thread::spawn(move || {
        start_rx.recv().unwrap();
        fs::write(&target, S1_GREET).unwrap();
        done_tx.send(()).unwrap();
    });
    let (b, text_b) = run(
        &lab_b,
        Bystander {
            start: start_tx,
            done: done_rx,
        },
    )
    .await;
    outsider.join().unwrap();

    println!("== World A (authorized agent)\n{text_a}\n");
    println!("== World B (outsider wrote the change)\n{text_b}\n");
    println!("World A: {a:#?}");
    println!("World B: {b:#?}");

    let same_bytes = tree_of(&lab_a.base.join("ws")) == tree_of(&lab_b.base.join("ws"));
    println!("\nfinal workspace bytes identical: {same_bytes}");
    println!("S0 identical: {}", a.s0 == b.s0);
    println!("S1 identical: {}", a.s1 == b.s1);
    println!("measurements identical: {}", a == b);
    for (field, same) in [
        ("pre", a.pre == b.pre),
        ("post", a.post == b.post),
        ("accepted", a.accepted == b.accepted),
        ("accepting_after", a.accepting_after == b.accepting_after),
        ("s1", a.s1 == b.s1),
        ("findings", a.findings == b.findings),
        ("observers", a.observers == b.observers),
        ("agent_result", a.agent_result == b.agent_result),
        ("ordered", a.ordered == b.ordered),
        ("journal", a.journal == b.journal),
    ] {
        println!("  {field:<16} {}", if same { "same" } else { "DIFFERENT" });
    }

    // Preconditions of the experiment, not its result.
    assert!(same_bytes, "the two worlds must end byte-identical");
    assert_eq!(a.s0, b.s0);
    assert_eq!(a.post, Verdict::Allow, "World A must satisfy every invariant");
}

// ------------------------------------------------------------ regression: whole decisions

/// The serialized decisions of one world: PRE (the authorization), POST
/// and the journal, with every value that is only a counter replaced by
/// its role. What remains is everything the verifier decided and why.
#[derive(Debug, PartialEq)]
struct Decided {
    pre: String,
    post: String,
    journal: String,
    post_verdict: Verdict,
}

/// Replace the run's counters by roles: the two snapshot ids and the
/// effect's clock reading (as `At::Snapshot` subjects, `@s<n>` in reasons
/// and the `transition` key), and lineage ids (`lineage:L<n>`).
fn normalize(json: &str, before: u64, done: u64, after: u64) -> String {
    let mut s = json.to_string();
    for (n, role) in [(before, "BEFORE"), (after, "AFTER")] {
        s = s.replace(&format!("{{\"snapshot\":{n}}}"), &format!("{{\"snapshot\":\"{role}\"}}"));
        s = s.replace(&format!("@s{n} "), &format!("@s{role} "));
    }
    s = s.replace(
        &format!("\"args\":[\"{before}\",\"{done}\",\"{after}\"]"),
        "\"args\":[\"BEFORE\",\"DONE\",\"AFTER\"]",
    );
    s = s.replace(
        &format!("transition({before}, {done}, {after})"),
        "transition(BEFORE, DONE, AFTER)",
    );
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("lineage:L") {
        out.push_str(&rest[..i + "lineage:L".len()]);
        rest = rest[i + "lineage:L".len()..].trim_start_matches(|c: char| c.is_ascii_digit());
        out.push('#');
    }
    out.push_str(rest);
    // Nothing that names a moment may survive normalization.
    assert!(!out.contains("{\"snapshot\":") || !out
        .split("{\"snapshot\":")
        .skip(1)
        .any(|t| t.starts_with(|c: char| c.is_ascii_digit())), "{out}");
    out
}

/// The normalized decisions, and the raw POST decision for comparison.
async fn decide<A: Actor<Task>>(lab: &Lab, actor: A) -> (Decided, String) {
    let s0 = lab.s0();
    let mut rt = temporal::runtime(lab.registry(), invariants(&s0), actor);
    let Authorize::Allowed(auth) = rt.authorize(Task).await.unwrap() else {
        panic!("PRE")
    };
    let pre = auth.decision().clone();
    let report = rt.execute(*auth).await.unwrap();
    assert!(report.executed);
    let receipt = report.receipt.as_ref().unwrap();
    let (before, done, after) = (
        receipt.before.id(),
        receipt.done,
        receipt.after.as_ref().expect("observed after").id(),
    );
    let json = |v: &dyn erased::Json| normalize(&v.json(), before, done, after);
    let decided = Decided {
        pre: json(&pre),
        post: json(&report.decision),
        journal: json(&rt.journal().records()),
        post_verdict: report.decision.verdict,
    };
    (decided, erased::Json::json(&report.decision))
}

/// Serialize any decision-like value (keeps `decide` free of the long
/// runtime type names).
mod erased {
    pub trait Json {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap()
        }
    }
}

/// World B's actor, wired to an outsider that writes `bytes`.
fn outsider(lab: &Lab, bytes: &'static [u8]) -> (Bystander, std::thread::JoinHandle<()>) {
    let (start_tx, start_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let target = lab.base.join("ws/src/greet.py");
    let handle = std::thread::spawn(move || {
        start_rx.recv().unwrap();
        fs::write(&target, bytes).unwrap();
        done_tx.send(()).unwrap();
    });
    (
        Bystander {
            start: start_tx,
            done: done_rx,
        },
        handle,
    )
}

/// Regression for the State vs Causality boundary: from the same S0, an
/// authorized transition and an unauthorized writer of byte-identical S1
/// get identical decisions: the whole PRE and POST decisions and the
/// journal, every finding and every reason, not a projection of them.
/// A third world, where the outsider writes different bytes, shows the
/// comparison would see a difference.
#[tokio::test]
async fn identical_s1_gets_identical_decisions_whoever_wrote_it() {
    let lab_a = Lab::new("reg-a");
    let (a, raw_a) = decide(&lab_a, Agent {
        ws: lab_a.base.join("ws"),
    })
    .await;

    let lab_b = Lab::new("reg-b");
    let (bystander, handle) = outsider(&lab_b, S1_GREET);
    let (b, raw_b) = decide(&lab_b, bystander).await;
    handle.join().unwrap();

    assert_eq!(tree_of(&lab_a.base.join("ws")), tree_of(&lab_b.base.join("ws")));
    assert_eq!(a.post_verdict, Verdict::Allow, "{}", a.post);
    assert_eq!(a.pre, b.pre, "PRE decisions differ");
    assert_eq!(a.post, b.post, "POST decisions differ");
    assert_eq!(a.journal, b.journal, "journals differ");
    assert_eq!(a, b);
    // The raw decisions do differ, in counters only: normalization is
    // what makes them equal, and it replaced nothing but counters.
    assert_ne!(raw_a, raw_b);

    // Control: an outsider writing *other* bytes is not normalized away.
    let lab_c = Lab::new("reg-c");
    let (bystander, handle) = outsider(&lab_c, b"def greet(n):\n    return n\n");
    let (c, _) = decide(&lab_c, bystander).await;
    handle.join().unwrap();
    assert_eq!(a.pre, c.pre, "same S0, same PRE");
    assert_eq!(c.post_verdict, Verdict::Deny, "{}", c.post);
    assert_ne!(a.post, c.post);
}
