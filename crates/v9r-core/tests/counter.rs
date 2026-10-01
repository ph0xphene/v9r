//! Third domain: an in-memory counter under the same runtime as the
//! filesystem and git domains. Nothing here touches a file, a process or
//! a checkpoint.

use v9r_core::counter::{runtime, CounterCell, CounterRuntime, Fault, Increment};
use v9r_core::kernel::{Decision, Status, Verdict};
use v9r_core::runtime::{Authorize, Report, ACCEPTED, BASIS};

fn inc(by: u64) -> Increment {
    Increment {
        by,
        proposer: "test-agent".to_string(),
    }
}

fn status<'a, S, V>(decision: &'a Decision<S, V>, invariant: &str) -> Vec<&'a Status> {
    decision
        .findings
        .iter()
        .filter(|f| f.obligation.invariant == invariant)
        .map(|f| &f.status)
        .collect()
}

fn violated<S, V>(decision: &Decision<S, V>, invariant: &str) -> bool {
    status(decision, invariant)
        .iter()
        .any(|s| matches!(s, Status::Violated(_)))
}

async fn run(
    rt: &mut CounterRuntime,
    proposal: Increment,
) -> (Verdict, Option<Report<v9r_core::counter::CounterDomain>>) {
    match rt.authorize(proposal).await.unwrap() {
        Authorize::Allowed(auth) => (Verdict::Allow, Some(rt.execute(*auth).await.unwrap())),
        other => (other.verdict(), None),
    }
}

// ALLOW: counter 0, increment 5, limit 10.
#[tokio::test]
async fn increment_within_limit_is_allowed_pre_and_post() {
    let cell = CounterCell::new(0);
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let (pre, report) = run(&mut rt, inc(5)).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(report.executed && report.accepted, "{}", report.decision);
    assert_eq!(report.decision.verdict, Verdict::Allow);
    let receipt = report.receipt.unwrap();
    assert_eq!((receipt.before, receipt.after), (0, Some(5)));
    assert_eq!(cell.get(), 5);
    assert_eq!(*rt.trusted(), 5);
    assert!(rt.is_accepting());
}

// DENY: counter 8, increment 5, limit 10.
#[tokio::test]
async fn increment_past_limit_is_denied_before_execution() {
    let cell = CounterCell::new(8);
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let result = rt.authorize(inc(5)).await.unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(violated(result.decision(), "C1.counter_within_limit"));
    assert_eq!(cell.get(), 8, "nothing ran");
    // A refused proposal is not a rejected transition.
    assert!(rt.is_accepting());
    let (pre, _) = run(&mut rt, inc(2)).await;
    assert_eq!(pre, Verdict::Allow);
    assert_eq!(cell.get(), 10);
}

// Stale authorization: granted on a state an accepted step has since
// superseded.
#[tokio::test]
async fn authorization_superseded_by_an_accepted_step_is_stale_but_not_drift() {
    let cell = CounterCell::new(0);
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let Authorize::Allowed(first) = rt.authorize(inc(2)).await.unwrap() else {
        panic!()
    };
    let Authorize::Allowed(second) = rt.authorize(inc(3)).await.unwrap() else {
        panic!()
    };
    assert!(rt.execute(*first).await.unwrap().accepted);
    let report = rt.execute(*second).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(violated(&report.decision, BASIS));
    assert_eq!(cell.get(), 2);
    // Reality equals the trusted state: nothing to hold.
    assert!(rt.is_accepting());
    let (pre, report) = run(&mut rt, inc(3)).await;
    assert_eq!(pre, Verdict::Allow);
    assert!(report.unwrap().accepted);
    assert_eq!(cell.get(), 5);
}

// State changed between authorization and execution.
#[tokio::test]
async fn out_of_band_change_after_authorization_refuses_and_holds() {
    let cell = CounterCell::new(0);
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let Authorize::Allowed(auth) = rt.authorize(inc(2)).await.unwrap() else {
        panic!()
    };
    cell.set(7);
    let report = rt.execute(*auth).await.unwrap();
    assert!(!report.executed);
    assert!(violated(&report.decision, BASIS));
    assert_eq!(cell.get(), 7, "nothing ran");
    assert!(!rt.is_accepting());
    let held = rt.authorize(inc(1)).await.unwrap();
    assert_eq!(held.verdict(), Verdict::Deny);
    assert!(violated(held.decision(), ACCEPTED));
}

// Observed result contradicts the proposal.
#[tokio::test]
async fn observed_result_contradicting_proposal_is_denied_and_held_for_good() {
    let cell = CounterCell::new(0);
    cell.set_fault(Fault::Skew(1));
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let (pre, report) = run(&mut rt, inc(2)).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(report.executed && !report.accepted);
    assert_eq!(
        report.decision.verdict,
        Verdict::Deny,
        "{}",
        report.decision
    );
    assert!(violated(&report.decision, "C2.observed_matches_proposal"));
    // Within the limit, so only the contradiction is reported.
    assert!(!violated(&report.decision, "C1.counter_within_limit"));
    assert_eq!(*rt.trusted(), 0, "rejected result not trusted");
    // No compensation exists for this domain (see the compile_fail
    // doctest in `counter`): the runtime stays held.
    cell.set_fault(Fault::None);
    let held = rt.authorize(inc(1)).await.unwrap();
    assert_eq!(held.verdict(), Verdict::Deny);
    assert!(violated(held.decision(), ACCEPTED));
}

#[tokio::test]
async fn faulty_actuator_overshooting_the_limit_violates_both_invariants() {
    let cell = CounterCell::new(8);
    cell.set_fault(Fault::Skew(1));
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let (pre, report) = run(&mut rt, inc(2)).await;
    assert_eq!(pre, Verdict::Allow, "predicted 10 <= 10");
    let decision = report.unwrap().decision;
    assert!(violated(&decision, "C1.counter_within_limit"));
    assert!(violated(&decision, "C2.observed_matches_proposal"));
}

#[tokio::test]
async fn unobservable_result_blocks_and_holds() {
    let cell = CounterCell::new(0);
    cell.set_fault(Fault::BlindAfterIncrement);
    let mut rt = runtime(cell.clone(), 10).unwrap();
    let (pre, report) = run(&mut rt, inc(2)).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(report.executed && !report.accepted);
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    assert_eq!(report.receipt.unwrap().after, None);
    assert!(!rt.is_accepting());
}

#[tokio::test]
async fn journal_records_every_decision_in_domain_free_form() {
    let mut rt = runtime(CounterCell::new(8), 10).unwrap();
    run(&mut rt, inc(1)).await;
    run(&mut rt, inc(5)).await;
    let records = rt.journal().records();
    let summary: Vec<_> = records
        .iter()
        .map(|r| (r.action.as_str(), r.phase, r.verdict))
        .collect();
    use v9r_core::kernel::Phase::{Post, Pre};
    assert_eq!(
        summary,
        [
            ("increment 1", Pre, Verdict::Allow),
            ("increment 1", Post, Verdict::Allow),
            ("increment 5", Pre, Verdict::Deny),
        ]
    );
    let invariants: Vec<_> = records[0]
        .findings
        .iter()
        .map(|f| f.invariant.as_str())
        .collect();
    assert_eq!(invariants, ["C1.counter_within_limit", ACCEPTED, BASIS]);
}
