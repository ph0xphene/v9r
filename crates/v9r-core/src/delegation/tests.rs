//! X2: can the unchanged kernel validate a delegation ledger?
//!
//! Every attack is a ledger state plus a claimed chain. The ledger stores
//! whatever an issuer wrote; the claimed chain is whatever a caller
//! supplies. Only `compile` (trusted) and `kernel::evaluate` (unchanged)
//! stand between them and an Allow.

use std::collections::BTreeSet;
use std::time::Instant;

use super::*;
use crate::kernel::{Status, Verdict};

fn grant(
    parent: Parent,
    grantor: &str,
    grantee: &str,
    objects: &[&str],
    operations: &[Op],
    not_after: u64,
    depth: u8,
) -> Grant {
    Grant {
        parent,
        grantor: grantor.into(),
        grantee: grantee.into(),
        objects: objects.iter().map(|s| s.to_string()).collect(),
        operations: operations.to_vec(),
        constraints: BTreeMap::new(),
        not_after,
        depth,
        identities: BTreeMap::new(),
    }
}

fn reads(world: &str, paths: &[&str]) -> Use {
    Use {
        world: world.into(),
        names: paths.iter().map(|p| cap_name(Op::Read, p)).collect(),
    }
}

/// runtime ─root─▶ A: read /data, until 100, depth 2
///                 A ─▶ B: read /data/reports, until 50, depth 1
struct Base {
    ledger: GrantLedger,
    a: GrantId,
    b: GrantId,
}

fn base() -> Base {
    let mut ledger = GrantLedger::new();
    let a = ledger.insert_root(grant(
        Parent::Root,
        "runtime",
        "A",
        &["/data"],
        &[Op::Read],
        100,
        2,
    ));
    let b = ledger.insert(grant(
        Parent::Grant(a.clone()),
        "A",
        "B",
        &["/data/reports"],
        &[Op::Read],
        50,
        1,
    ));
    Base { ledger, a, b }
}

/// One attack or honest case: everything `compile` and the kernel see.
struct Case {
    name: &'static str,
    ledger: GrantLedger,
    claimed: Vec<(GrantId, Grant)>,
    leaf: GrantId,
    use_: Use,
    context: DEvidence,
    /// The clock value the caller claims; `None` = the ledger's.
    claimed_now: Option<u64>,
    expect: Verdict,
    /// The invariant that must account for the verdict.
    by: &'static str,
}

impl Case {
    fn honest(
        name: &'static str,
        ledger: GrantLedger,
        leaf: GrantId,
        use_: Use,
        expect: Verdict,
        by: &'static str,
    ) -> Self {
        Case {
            name,
            claimed: ledger.chain(&leaf),
            ledger,
            leaf,
            use_,
            context: DEvidence::new(),
            claimed_now: None,
            expect,
            by,
        }
    }

    fn decide(&self, omit: &BTreeSet<Group>) -> Decision<DSubject, DValue> {
        let mut evidence = self.ledger.evidence();
        evidence.merge(&self.context);
        let now = self.claimed_now.unwrap_or(self.ledger.now());
        let obligations = compile_without(&self.claimed, &self.leaf, &self.use_, now, omit);
        kernel::evaluate(Phase::Pre, &obligations, &evidence)
    }
}

fn honest_cases() -> Vec<Case> {
    let mut out = Vec::new();

    let Base { ledger, b, .. } = base();
    out.push(Case::honest(
        "narrower read",
        ledger,
        b,
        reads("B", &["/data/reports/q3.csv"]),
        Verdict::Allow,
        "",
    ));

    let Base { mut ledger, b, .. } = base();
    let c = ledger.insert(grant(
        Parent::Grant(b),
        "B",
        "C",
        &["/data/reports/2026"],
        &[Op::Read],
        40,
        0,
    ));
    out.push(Case::honest(
        "three levels",
        ledger,
        c,
        reads("C", &["/data/reports/2026/jan.csv"]),
        Verdict::Allow,
        "",
    ));

    // Exact directory entries (`/.`) attenuate like any name.
    let mut ledger = GrantLedger::new();
    let r = ledger.insert_root(grant(
        Parent::Root,
        "runtime",
        "A",
        &["/nix/.", "/nix/store"],
        &[Op::Read],
        100,
        1,
    ));
    let x = ledger.insert(grant(
        Parent::Grant(r),
        "A",
        "B",
        &["/nix/.", "/nix/store/x/."],
        &[Op::Read],
        100,
        0,
    ));
    out.push(Case::honest(
        "exact entries within exact and prefix",
        ledger,
        x,
        reads("B", &["/nix/."]),
        Verdict::Allow,
        "",
    ));
    out
}

fn attack_cases() -> Vec<Case> {
    let mut out = Vec::new();
    let child = |base: &mut Base, objects: &[&str], ops: &[Op]| {
        base.ledger.insert(grant(
            Parent::Grant(base.a.clone()),
            "A",
            "B",
            objects,
            ops,
            50,
            1,
        ))
    };

    // ---- R1: scope expansion
    let mut b = base();
    let leaf = child(&mut b, &["/srv"], &[Op::Read]);
    out.push(Case::honest(
        "R1 sibling object",
        b.ledger,
        leaf,
        reads("B", &["/srv/x"]),
        Verdict::Deny,
        "D3.attenuates",
    ));

    let mut b = base();
    let leaf = child(&mut b, &["/database"], &[Op::Read]);
    out.push(Case::honest(
        "R1 prefix-spelled sibling /database",
        b.ledger,
        leaf,
        reads("B", &["/database/x"]),
        Verdict::Deny,
        "D3.attenuates",
    ));

    let mut b = base();
    let leaf = child(&mut b, &["/data/reports"], &[Op::Read, Op::Write]);
    let use_ = Use {
        world: "B".into(),
        names: vec![cap_name(Op::Write, "/data/reports/q3.csv")],
    };
    out.push(Case::honest(
        "R1 added operation",
        b.ledger,
        leaf,
        use_,
        Verdict::Deny,
        "D3.attenuates",
    ));

    let mut b = base();
    let leaf = child(&mut b, &["/data/reports/../../etc"], &[Op::Read]);
    out.push(Case::honest(
        "R1 dot-dot path",
        b.ledger,
        leaf,
        reads("B", &["/data/reports/../../etc/shadow"]),
        Verdict::Deny,
        "D0.canonical",
    ));

    let mut ledger = GrantLedger::new();
    let r = ledger.insert_root(grant(
        Parent::Root,
        "runtime",
        "A",
        &["/nix/."],
        &[Op::Read],
        100,
        1,
    ));
    let x = ledger.insert(grant(
        Parent::Grant(r),
        "A",
        "B",
        &["/nix"],
        &[Op::Read],
        100,
        0,
    ));
    out.push(Case::honest(
        "R1 prefix below an exact entry",
        ledger,
        x,
        reads("B", &["/nix/store/secret"]),
        Verdict::Deny,
        "D3.attenuates",
    ));

    // ---- R2: chain validity
    // The stored child names /secrets under A; the caller presents a
    // forged A (wider) under A's real id.
    let mut b = base();
    let leaf = child(&mut b, &["/secrets"], &[Op::Read]);
    let mut forged_a = b.ledger.get(&b.a).unwrap().clone();
    forged_a.objects.push("/secrets".into());
    let claimed = vec![
        (leaf.clone(), b.ledger.get(&leaf).unwrap().clone()),
        (b.a.clone(), forged_a),
    ];
    out.push(Case {
        name: "R2 forged parent record under its real id",
        ledger: b.ledger,
        claimed,
        leaf,
        use_: reads("B", &["/secrets/k"]),
        context: DEvidence::new(),
        claimed_now: None,
        expect: Verdict::Deny,
        by: "D1.chain_pinned",
    });

    let mut b = base();
    let dangling =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
    let leaf = b.ledger.insert(grant(
        Parent::Grant(dangling),
        "A",
        "B",
        &["/data/reports"],
        &[Op::Read],
        50,
        1,
    ));
    out.push(Case::honest(
        "R2 dangling parent link",
        b.ledger,
        leaf,
        reads("B", &["/data/reports/q"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    let mut b = base();
    let leaf = b.ledger.insert(grant(
        Parent::Grant(b.a.clone()),
        "X",
        "X",
        &["/data/reports"],
        &[Op::Read],
        50,
        1,
    ));
    out.push(Case::honest(
        "R2 issuer does not hold the parent",
        b.ledger,
        leaf,
        reads("X", &["/data/reports/q"]),
        Verdict::Deny,
        "D2.issuer_holds_parent",
    ));

    let mut b = base();
    let leaf = b.ledger.insert(grant(
        Parent::Root,
        "M",
        "M",
        &["/etc"],
        &[Op::Read],
        100,
        5,
    ));
    out.push(Case::honest(
        "R2 self-declared root",
        b.ledger,
        leaf,
        reads("M", &["/etc/shadow"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    // The caller's records are honest, but the ledger never stored B:
    // the pin has no evidence.
    let b = base();
    let mut partial = GrantLedger::new();
    partial.insert_root(b.ledger.get(&b.a).unwrap().clone());
    out.push(Case {
        name: "R2 record the ledger does not hold (pin without evidence)",
        claimed: b.ledger.chain(&b.b),
        ledger: partial,
        leaf: b.b,
        use_: reads("B", &["/data/reports/q"]),
        context: DEvidence::new(),
        claimed_now: None,
        expect: Verdict::Blocked,
        by: "D1.chain_pinned",
    });

    // ---- R3: depth
    let mut b = base();
    let leaf = b.ledger.insert(grant(
        Parent::Grant(b.a.clone()),
        "A",
        "B",
        &["/data/reports"],
        &[Op::Read],
        50,
        2,
    ));
    out.push(Case::honest(
        "R3 depth not decremented",
        b.ledger,
        leaf,
        reads("B", &["/data/reports/q"]),
        Verdict::Deny,
        "D4.depth",
    ));

    let mut b = base();
    let c = b.ledger.insert(grant(
        Parent::Grant(b.b.clone()),
        "B",
        "C",
        &["/data/reports"],
        &[Op::Read],
        50,
        0,
    ));
    let d = b.ledger.insert(grant(
        Parent::Grant(c),
        "C",
        "D",
        &["/data/reports"],
        &[Op::Read],
        50,
        0,
    ));
    out.push(Case::honest(
        "R3 redelegation past depth 0",
        b.ledger,
        d,
        reads("D", &["/data/reports/q"]),
        Verdict::Deny,
        "D4.depth",
    ));

    let mut ledger = GrantLedger::new();
    let mut id = ledger.insert_root(grant(
        Parent::Root,
        "runtime",
        "w0",
        &["/data"],
        &[Op::Read],
        100,
        255,
    ));
    for i in 1..40u8 {
        id = ledger.insert(grant(
            Parent::Grant(id),
            &format!("w{}", i - 1),
            &format!("w{i}"),
            &["/data"],
            &[Op::Read],
            100,
            255 - i,
        ));
    }
    out.push(Case::honest(
        "R3 chain longer than MAX_CHAIN, depths valid",
        ledger,
        id,
        reads("w39", &["/data/x"]),
        Verdict::Deny,
        "D4.depth",
    ));

    // A cycle is only claimable: content addressing makes one impossible
    // to store. The caller labels two records with ids that point at
    // each other.
    let b = base();
    let x = "sha256:x".to_string();
    let y = "sha256:y".to_string();
    let gx = grant(
        Parent::Grant(y.clone()),
        "B",
        "B",
        &["/data"],
        &[Op::Read],
        100,
        3,
    );
    let gy = grant(
        Parent::Grant(x.clone()),
        "B",
        "B",
        &["/data"],
        &[Op::Read],
        100,
        3,
    );
    out.push(Case {
        name: "R3 claimed delegation cycle",
        ledger: b.ledger,
        claimed: vec![(x.clone(), gx), (y, gy)],
        leaf: x,
        use_: reads("B", &["/data/x"]),
        context: DEvidence::new(),
        claimed_now: None,
        expect: Verdict::Deny,
        by: "D4.depth",
    });

    // ---- R4: expiry
    let mut b = base();
    b.ledger.advance(60);
    out.push(Case::honest(
        "R4 expired leaf",
        b.ledger,
        b.b,
        reads("B", &["/data/reports/q"]),
        Verdict::Deny,
        "D5.unexpired",
    ));

    let mut b = base();
    let leaf = b.ledger.insert(grant(
        Parent::Grant(b.a.clone()),
        "A",
        "B",
        &["/data/reports"],
        &[Op::Read],
        500,
        1,
    ));
    b.ledger.advance(150);
    out.push(Case::honest(
        "R4 expired ancestor, unexpired leaf",
        b.ledger,
        leaf,
        reads("B", &["/data/reports/q"]),
        Verdict::Deny,
        "D5.unexpired",
    ));

    let mut b = base();
    b.ledger.advance(60);
    out.push(Case {
        name: "R4 caller claims a stale clock",
        claimed: b.ledger.chain(&b.b),
        ledger: b.ledger,
        leaf: b.b,
        use_: reads("B", &["/data/reports/q"]),
        context: DEvidence::new(),
        claimed_now: Some(10),
        expect: Verdict::Deny,
        by: "D5.unexpired",
    });

    // ---- R5: revocation
    let mut b = base();
    b.ledger.revoke(&b.a);
    out.push(Case::honest(
        "R5 revoked ancestor",
        b.ledger,
        b.b,
        reads("B", &["/data/reports/q"]),
        Verdict::Deny,
        "D6.live",
    ));

    let mut b = base();
    b.ledger.revoke(&b.b);
    out.push(Case::honest(
        "R5 revoked leaf",
        b.ledger,
        b.b,
        reads("B", &["/data/reports/q"]),
        Verdict::Deny,
        "D6.live",
    ));

    // ---- use
    let b = base();
    out.push(Case::honest(
        "use by another world",
        b.ledger,
        b.b,
        reads("C", &["/data/reports/q"]),
        Verdict::Deny,
        "D7.use_within_leaf",
    ));

    let b = base();
    out.push(Case::honest(
        "use outside the leaf",
        b.ledger,
        b.b,
        reads("B", &["/data/payroll"]),
        Verdict::Deny,
        "D7.use_within_leaf",
    ));

    // ---- constraints: the child drops its parent's constraint
    let mut ledger = GrantLedger::new();
    let mut root = grant(
        Parent::Root,
        "runtime",
        "A",
        &["/release"],
        &[Op::Write],
        100,
        1,
    );
    root.constraints.insert("commit".into(), "c0ffee".into());
    let r = ledger.insert_root(root);
    let leaf = ledger.insert(grant(
        Parent::Grant(r),
        "A",
        "B",
        &["/release/v1"],
        &[Op::Write],
        100,
        0,
    ));
    let mut case = Case::honest(
        "constraint dropped by the child, violated by the use",
        ledger,
        leaf,
        Use {
            world: "B".into(),
            names: vec![cap_name(Op::Write, "/release/v1/bin")],
        },
        Verdict::Deny,
        "D8.constraints",
    );
    case.context = context_evidence(&[("commit", "deadbeef")]);
    out.push(case);

    out
}

fn culprits(decision: &Decision<DSubject, DValue>) -> BTreeSet<String> {
    decision
        .findings
        .iter()
        .filter(|f| !matches!(f.status, Status::Satisfied(_)))
        .map(|f| f.obligation.invariant.clone())
        .collect()
}

#[test]
fn honest_ledger_output_is_allowed() {
    for case in honest_cases() {
        let decision = case.decide(&BTreeSet::new());
        assert_eq!(
            decision.verdict,
            Verdict::Allow,
            "{}: {decision}",
            case.name
        );
    }
}

#[test]
fn malicious_ledger_output_is_blocked_or_denied_by_the_right_rule() {
    for case in attack_cases() {
        let decision = case.decide(&BTreeSet::new());
        println!(
            "{:<60} {:?}  {:?}",
            case.name,
            decision.verdict,
            culprits(&decision)
        );
        assert_eq!(decision.verdict, case.expect, "{}: {decision}", case.name);
        assert!(
            culprits(&decision).contains(case.by),
            "{}: expected {} among the culprits\n{decision}",
            case.name,
            case.by
        );
    }
}

#[test]
fn constraint_holds_and_unknown_context_blocks() {
    let mut cases = attack_cases();
    let case = cases
        .iter_mut()
        .find(|c| c.name.starts_with("constraint"))
        .unwrap();
    case.context = context_evidence(&[("commit", "c0ffee")]);
    assert_eq!(case.decide(&BTreeSet::new()).verdict, Verdict::Allow);
    case.context = DEvidence::new();
    assert_eq!(case.decide(&BTreeSet::new()).verdict, Verdict::Blocked);
}

/// Ledger correctness, not kernel correctness: each obligation group is
/// load-bearing. With the group left out, some attack is allowed, and
/// the kernel cannot notice that an obligation is missing.
#[test]
fn every_obligation_group_is_necessary() {
    let groups = [
        Group::Canonical,
        Group::RecordPins,
        Group::IssuerPins,
        Group::Attenuation,
        Group::Depth,
        Group::Expiry,
        Group::ClockPin,
        Group::Liveness,
    ];
    for group in groups {
        let omit = BTreeSet::from([group]);
        let allowed: Vec<&str> = attack_cases()
            .into_iter()
            .filter(|c| c.decide(&omit).verdict == Verdict::Allow)
            .map(|c| c.name)
            .collect();
        println!("without {group:?}: ALLOW for {allowed:?}");
        assert!(!allowed.is_empty(), "{group:?} is not needed by any attack");
    }
    // Leaving nothing out: no attack is allowed.
    assert!(attack_cases()
        .iter()
        .all(|c| c.decide(&BTreeSet::new()).verdict != Verdict::Allow));
}

/// D2 checks that the record's `grantor` field names the parent's
/// grantee. It cannot check who wrote the record. X, holding nothing,
/// writes a record that says A issued it to X: every obligation is
/// satisfied. Authenticating the issuer is an issue-time property of the
/// ledger, outside anything the kernel sees.
#[test]
fn a_record_that_lies_about_its_grantor_is_allowed() {
    let mut b = base();
    let leaf = b.ledger.insert(grant(
        Parent::Grant(b.a.clone()),
        "A", // written by X, not by A
        "X",
        &["/data/reports"],
        &[Op::Read],
        50,
        1,
    ));
    let decision = authorize(
        &b.ledger,
        &b.ledger.chain(&leaf),
        &leaf,
        &reads("X", &["/data/reports/q"]),
        &DEvidence::new(),
    );
    assert_eq!(decision.verdict, Verdict::Allow, "{decision}");
}

/// The kernel believes the ledger. A replica that missed a revocation
/// authorizes; two replicas that disagree block.
#[test]
fn a_stale_ledger_is_believed_and_disagreeing_ledgers_block() {
    let mut b = base();
    let replica = b.ledger.clone();
    b.ledger.revoke(&b.a);
    let claimed = b.ledger.chain(&b.b);
    let use_ = reads("B", &["/data/reports/q"]);
    let obligations = compile(&claimed, &b.b, &use_, b.ledger.now());

    let alone = kernel::evaluate(Phase::Pre, &obligations, &replica.evidence());
    assert_eq!(alone.verdict, Verdict::Allow, "{alone}");

    let mut both = b.ledger.evidence();
    both.merge(&replica.evidence());
    let together = kernel::evaluate(Phase::Pre, &obligations, &both);
    assert_eq!(together.verdict, Verdict::Blocked, "{together}");
    assert!(together.to_string().contains("contradictory"), "{together}");
}

/// The compiler's walk is bounded whatever the ledger holds.
#[test]
fn compiling_a_ten_thousand_grant_chain_is_bounded() {
    let mut ledger = GrantLedger::new();
    let mut id = ledger.insert_root(grant(
        Parent::Root,
        "runtime",
        "w0",
        &["/d"],
        &[Op::Read],
        100,
        255,
    ));
    for i in 1..10_000u32 {
        id = ledger.insert(grant(
            Parent::Grant(id),
            &format!("w{}", i - 1),
            &format!("w{i}"),
            &["/d"],
            &[Op::Read],
            100,
            255,
        ));
    }
    let claimed = ledger.chain(&id);
    assert_eq!(claimed.len(), MAX_CHAIN + 1);
    let start = Instant::now();
    let obligations = compile(&claimed, &id, &reads("w9999", &["/d/x"]), ledger.now());
    let decision = kernel::evaluate(Phase::Pre, &obligations, &ledger.evidence());
    println!(
        "10,000 stored grants: {} obligations, {:?} to compile and decide (evidence of {} records)",
        obligations.len(),
        start.elapsed(),
        10_000
    );
    assert_eq!(decision.verdict, Verdict::Deny);
    assert!(obligations.len() < 10 * (MAX_CHAIN + 2));
}

#[test]
fn decisions_are_deterministic() {
    for (a, b) in attack_cases().into_iter().zip(attack_cases()) {
        let none = BTreeSet::new();
        assert_eq!(a.decide(&none), b.decide(&none), "{}", a.name);
    }
}

#[test]
fn canonical_paths() {
    for ok in ["/a", "/a/b", "/nix/.", "/a/b/."] {
        assert!(canonical(ok), "{ok}");
    }
    for bad in [
        "", "/", "a", "/a/", "/a//b", "/a/../b", "/./a", "/.", "/a/./b", "/..",
    ] {
        assert!(!canonical(bad), "{bad}");
    }
}

#[test]
fn kernel_is_unchanged_and_knows_no_delegation() {
    let kernel = include_str!("../kernel.rs");
    let digest = format!("{:x}", Sha256::digest(kernel.as_bytes()));
    assert_eq!(
        digest,
        "85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f"
    );
    for word in ["delegat", "ledger", "revok", "grantee", "grantor"] {
        assert!(!kernel.to_lowercase().contains(word), "{word}");
    }
}
