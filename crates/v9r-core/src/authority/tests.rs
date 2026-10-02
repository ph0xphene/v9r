//! Root of trust v0: can delegation authority be rooted in a verifiable
//! identity?
//!
//! Each case names the layer that must catch it:
//! * `correctness`: `delegation::compile` (D0–D9), unchanged from X2;
//! * `authenticity`: signatures checked by `verify` against the anchor;
//! * `freshness`: the runtime's head witness (`A2`).

use std::collections::BTreeSet;
use std::time::Instant;

use super::*;
use crate::delegation::{cap_name, Op};
use crate::kernel::{Status, Verdict};

const DATA: &str = "inode:1:10";
const REPORTS: &str = "inode:1:20";

fn grant(
    parent: Parent,
    grantor: &str,
    grantee: &str,
    objects: &[(&str, &str)],
    depth: u8,
    not_after: u64,
) -> Grant {
    Grant {
        parent,
        grantor: grantor.into(),
        grantee: grantee.into(),
        objects: objects.iter().map(|(p, _)| p.to_string()).collect(),
        operations: vec![Op::Read],
        constraints: BTreeMap::new(),
        not_after,
        depth,
        identities: objects
            .iter()
            .map(|(p, i)| (p.to_string(), i.to_string()))
            .collect(),
    }
}

fn reads(world: &KeyId, paths: &[&str]) -> Use {
    Use {
        world: world.clone(),
        names: paths.iter().map(|p| cap_name(Op::Read, p)).collect(),
    }
}

/// The runtime's object resolver (trusted, synthetic here).
fn resolver(objects: &[(&str, &str)]) -> DEvidence {
    let mut ev = DEvidence::new();
    for (path, id) in objects {
        ev.add_verified(
            Verified::attest(Fact {
                subject: DSubject::Object(path.to_string()),
                value: DValue::Text(id.to_string()),
            }),
            Provenance {
                observer: "resolver".into(),
                basis: "synthetic".into(),
            },
        );
    }
    ev
}

fn objects_now() -> DEvidence {
    // Every object any case names resolves as its grant says, so that
    // object identity (D9) never masks the layer under test.
    resolver(&[
        ("/data", DATA),
        ("/data/reports", REPORTS),
        ("/etc", "inode:1:5"),
        ("/srv", "inode:1:7"),
    ])
}

/// runtime R ─root─▶ A: read /data (depth 2, until 100)
///                   A ─▶ B: read /data/reports (depth 1, until 50)
struct Fix {
    auth: Authority,
    a: KeyId,
    b: KeyId,
    root: GrantId,
    child: GrantId,
}

fn fix() -> Fix {
    let mut auth = Authority::new();
    let a = auth.enroll("A");
    let b = auth.enroll("B");
    let root = auth.root_grant(grant(Parent::Root, "", &a, &[("/data", DATA)], 2, 100));
    let child = auth
        .delegate(
            "A",
            grant(
                Parent::Grant(root.clone()),
                &a,
                &b,
                &[("/data/reports", REPORTS)],
                1,
                50,
            ),
        )
        .unwrap();
    Fix {
        auth,
        a,
        b,
        root,
        child,
    }
}

struct Case {
    name: &'static str,
    layer: &'static str,
    /// The runtime the decision runs in (anchor, witness, clock).
    runtime: Authority,
    copies: Vec<LedgerCopy>,
    claimed: Vec<(GrantId, Grant)>,
    leaf: GrantId,
    use_: Use,
    context: DEvidence,
    expect: Verdict,
    by: &'static str,
}

impl Case {
    fn new(
        name: &'static str,
        layer: &'static str,
        runtime: Authority,
        copy: LedgerCopy,
        leaf: GrantId,
        use_: Use,
        expect: Verdict,
        by: &'static str,
    ) -> Self {
        Case {
            name,
            layer,
            claimed: chain(&copy, &leaf),
            copies: vec![copy],
            runtime,
            leaf,
            use_,
            context: objects_now(),
            expect,
            by,
        }
    }

    fn decide(
        &self,
        omit_grant: &BTreeSet<Group>,
        omit_auth: &BTreeSet<Check>,
        strict: Strict,
    ) -> Decision<DSubject, DValue> {
        let request = Request {
            copies: &self.copies,
            claimed: &self.claimed,
            leaf: &self.leaf,
            use_: &self.use_,
            context: &self.context,
        };
        authorize_without(
            &self.runtime,
            &self.runtime.anchor(),
            &self.runtime.witness(),
            &request,
            omit_grant,
            omit_auth,
            strict,
        )
    }

    fn decide_strict(&self) -> Decision<DSubject, DValue> {
        self.decide(&BTreeSet::new(), &BTreeSet::new(), Strict::ALL)
    }
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    // ---------------------------------------------------------- 1. valid root
    let f = fix();
    out.push(Case::new(
        "1 root → A → B, B reads its grant",
        "honest",
        f.auth.clone(),
        f.auth.publish(),
        f.child,
        reads(&f.b, &["/data/reports/q3.csv"]),
        Verdict::Allow,
        "",
    ));
    let f = fix();
    out.push(Case::new(
        "1 A reads under the root grant",
        "honest",
        f.auth.clone(),
        f.auth.publish(),
        f.root,
        reads(&f.a, &["/data/x"]),
        Verdict::Allow,
        "",
    ));

    // ---------------------------------------------------------- 2. unknown issuer
    let mut f = fix();
    let k = Identity::generate();
    let g = grant(
        Parent::Root,
        k.id(),
        k.id(),
        &[("/etc", "inode:1:5")],
        9,
        100,
    );
    f.auth
        .submit(Entry::Grant(k.sign_grant(g.clone())))
        .unwrap();
    out.push(Case::new(
        "2a unknown key declares itself a root",
        "authenticity",
        f.auth.clone(),
        f.auth.publish(),
        g.id(),
        reads(k.id(), &["/etc/shadow"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    let mut f = fix();
    let k = Identity::generate();
    let g = grant(
        Parent::Root,
        &f.auth.anchor(),
        k.id(),
        &[("/etc", "inode:1:5")],
        9,
        100,
    );
    let forged = k.sign_grant(g.clone());
    assert_eq!(
        f.auth.submit(Entry::Grant(forged.clone())),
        Err(Refused::BadSignature)
    );
    f.auth.append_unchecked(Entry::Grant(forged));
    out.push(Case::new(
        "2b root grant naming the anchor, signed by another key (faulty appender)",
        "authenticity",
        f.auth.clone(),
        f.auth.publish(),
        g.id(),
        reads(k.id(), &["/etc/shadow"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    let mut f = fix();
    let k = Identity::generate();
    let g = grant(
        Parent::Grant(f.root.clone()),
        k.id(),
        k.id(),
        &[("/data", DATA)],
        1,
        100,
    );
    f.auth
        .submit(Entry::Grant(k.sign_grant(g.clone())))
        .unwrap();
    out.push(Case::new(
        "2c unknown key hangs a grant off A's grant, as itself",
        "correctness",
        f.auth.clone(),
        f.auth.publish(),
        g.id(),
        reads(k.id(), &["/data/x"]),
        Verdict::Deny,
        "D2.issuer_holds_parent",
    ));

    // X2's lying issuer: the record says A issued it. X2 allowed this.
    let mut f = fix();
    let k = Identity::generate();
    let g = grant(
        Parent::Grant(f.root.clone()),
        &f.a,
        k.id(),
        &[("/data", DATA)],
        1,
        100,
    );
    let lie = k.sign_grant(g.clone());
    assert_eq!(
        f.auth.submit(Entry::Grant(lie.clone())),
        Err(Refused::BadSignature)
    );
    f.auth.append_unchecked(Entry::Grant(lie));
    out.push(Case::new(
        "2d record claims grantor A, signed by another key (X2's lying issuer)",
        "authenticity",
        f.auth.clone(),
        f.auth.publish(),
        g.id(),
        reads(k.id(), &["/data/x"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    // ---------------------------------------------------------- 3. copied ledger
    let f = fix();
    out.push(Case::new(
        "3a verbatim copy of the current ledger",
        "honest",
        f.auth.clone(),
        f.auth.publish(),
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Allow,
        "",
    ));

    // A rival runtime with its own root key, the same world names and a
    // root grant over /: published as if it were this runtime's ledger.
    let f = fix();
    let mut rival = Authority::new();
    let m = rival.enroll("A");
    let g = rival.root_grant(grant(
        Parent::Root,
        "",
        &m,
        &[("/srv", "inode:1:7")],
        9,
        100,
    ));
    out.push(Case::new(
        "3b rival runtime's ledger (its own root key)",
        "authenticity",
        f.auth.clone(),
        rival.publish(),
        g,
        reads(&m, &["/srv/x"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    // Copy of the real ledger, plus a rival root grant, re-signed by the
    // rival key (an attacker who copied the ledger and holds some key).
    let f = fix();
    let r2 = Identity::generate();
    let mut copy = f.auth.publish();
    let g = grant(
        Parent::Root,
        r2.id(),
        r2.id(),
        &[("/srv", "inode:1:7")],
        9,
        100,
    );
    copy.entries.push(Entry::Grant(r2.sign_grant(g.clone())));
    copy.head.seq += 1;
    copy.head.hash = chain_hash(&copy.entries);
    copy.head.signature = r2.sign(&message(HEAD_DOMAIN, &copy.head.text()));
    out.push(Case::new(
        "3c copied ledger extended and re-signed by another key",
        "authenticity",
        f.auth.clone(),
        copy,
        g.id(),
        reads(r2.id(), &["/srv/x"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    // Entries appended past the signed head are not covered by it.
    let f = fix();
    let mut copy = f.auth.publish();
    let g = grant(
        Parent::Grant(f.root.clone()),
        &f.a,
        &f.a,
        &[("/data", DATA)],
        1,
        100,
    );
    copy.entries.push(Entry::Grant(SignedGrant {
        grant: g.clone(),
        signature: "00".into(),
    }));
    out.push(Case::new(
        "3d entry appended past the signed head",
        "authenticity",
        f.auth.clone(),
        copy,
        g.id(),
        reads(&f.a, &["/data/x"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    // A signed grant edited in place (its scope widened).
    let f = fix();
    let mut copy = f.auth.publish();
    let Entry::Grant(sg) = &mut copy.entries[1] else {
        panic!()
    };
    sg.grant.objects = vec!["/data".into()];
    sg.grant.identities = BTreeMap::from([("/data".to_string(), DATA.to_string())]);
    let widened = sg.grant.id();
    out.push(Case::new(
        "3e signed grant widened in a copy",
        "authenticity",
        f.auth.clone(),
        copy,
        widened,
        reads(&f.b, &["/data/payroll"]),
        Verdict::Blocked,
        "D1.chain_pinned",
    ));

    // ---------------------------------------------------------- 4. stale ledger
    let mut f = fix();
    let old = f.auth.publish();
    f.auth.revoke("A", &f.child).unwrap();
    out.push(Case::new(
        "4a ledger copy from before a revocation",
        "freshness",
        f.auth.clone(),
        old,
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Deny,
        "A2.fresh_ledger",
    ));

    // A copy that drops the revocation but keeps later entries, re-hashed;
    // the head signature cannot be redone without R.
    let mut f = fix();
    f.auth.revoke("A", &f.child).unwrap();
    let c = f.auth.enroll("C");
    let later = f
        .auth
        .delegate(
            "A",
            grant(
                Parent::Grant(f.root.clone()),
                &f.a,
                &c,
                &[("/data", DATA)],
                1,
                100,
            ),
        )
        .unwrap();
    let _ = later;
    let mut copy = f.auth.publish();
    copy.entries.remove(2);
    copy.head.seq -= 1;
    copy.head.hash = chain_hash(&copy.entries);
    out.push(Case::new(
        "4b revocation cut out of the middle, re-hashed",
        "authenticity",
        f.auth.clone(),
        copy,
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Blocked,
        "A2.fresh_ledger",
    ));

    // Revocation by someone who is neither issuer nor anchor is ignored.
    let mut f = fix();
    f.auth.revoke("B", &f.root).unwrap();
    out.push(Case::new(
        "4c revocation by a non-issuer (B revokes A's root grant)",
        "authenticity",
        f.auth.clone(),
        f.auth.publish(),
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Allow,
        "",
    ));

    // ---------------------------------------------------------- 5. disagreement
    let mut f = fix();
    let old = f.auth.publish();
    f.auth.revoke("A", &f.child).unwrap();
    let mut case = Case::new(
        "5a stale and current ledgers together",
        "freshness",
        f.auth.clone(),
        f.auth.publish(),
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Blocked,
        "A2.fresh_ledger",
    );
    case.copies.push(old);
    out.push(case);

    let mut f = fix();
    let mut fork = f.auth.clone();
    f.auth.revoke("A", &f.child).unwrap();
    let d = fork.enroll("D");
    fork.delegate(
        "A",
        grant(
            Parent::Grant(f.root.clone()),
            &f.a,
            &d,
            &[("/data", DATA)],
            1,
            100,
        ),
    )
    .unwrap();
    let mut case = Case::new(
        "5b forked runtime: two heads at one seq, both signed by R",
        "freshness",
        f.auth.clone(),
        f.auth.publish(),
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Blocked,
        "A2.fresh_ledger",
    );
    assert!(equivocates(
        &f.auth.anchor(),
        &case.copies[0],
        &fork.publish()
    ));
    case.copies.push(fork.publish());
    out.push(case);

    // ---------------------------------------------------------- authentic but wrong
    let mut f = fix();
    let wide = f
        .auth
        .delegate(
            "A",
            grant(
                Parent::Grant(f.root.clone()),
                &f.a,
                &f.b,
                &[("/srv", "inode:1:7")],
                1,
                50,
            ),
        )
        .unwrap();
    let mut case = Case::new(
        "G1 authentic grant wider than its parent",
        "correctness",
        f.auth.clone(),
        f.auth.publish(),
        wide,
        reads(&f.b, &["/srv/x"]),
        Verdict::Deny,
        "D3.attenuates",
    );
    case.context = resolver(&[("/data", DATA), ("/srv", "inode:1:7")]);
    out.push(case);

    let f = fix();
    let mut case = Case::new(
        "G2 object behind /data/reports replaced",
        "correctness",
        f.auth.clone(),
        f.auth.publish(),
        f.child,
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Deny,
        "D9.object_identity",
    );
    case.context = resolver(&[("/data", DATA), ("/data/reports", "inode:1:99")]);
    out.push(case);

    let mut f = fix();
    f.auth.advance(60);
    out.push(Case::new(
        "G3 authentic grant, expired",
        "correctness",
        f.auth.clone(),
        f.auth.publish(),
        f.child,
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Deny,
        "D5.unexpired",
    ));

    out
}

fn culprits(d: &Decision<DSubject, DValue>) -> BTreeSet<String> {
    d.findings
        .iter()
        .filter(|f| !matches!(f.status, Status::Satisfied(_)))
        .map(|f| f.obligation.invariant.clone())
        .collect()
}

#[test]
fn every_case_gets_its_verdict_from_its_layer() {
    for case in cases() {
        let d = case.decide_strict();
        println!(
            "{:<74} {:<12} {:?}  {:?}",
            case.name,
            case.layer,
            d.verdict,
            culprits(&d)
        );
        assert_eq!(d.verdict, case.expect, "{}\n{d}", case.name);
        if !case.by.is_empty() {
            assert!(
                culprits(&d).contains(case.by),
                "{}: no {}\n{d}",
                case.name,
                case.by
            );
        }
    }
}

#[test]
fn the_runtime_refuses_requests_naming_another_grantor() {
    let mut f = fix();
    let x = f.auth.enroll("X");
    let g = grant(
        Parent::Grant(f.root.clone()),
        &f.a,
        &x,
        &[("/data", DATA)],
        1,
        100,
    );
    assert!(matches!(
        f.auth.delegate("X", g),
        Err(Refused::NotTheRequester { .. })
    ));
    assert!(matches!(
        f.auth
            .delegate("nobody", grant(Parent::Root, "", "", &[], 0, 0)),
        Err(Refused::UnknownWorld(_))
    ));
}

/// Signatures authenticate keys, not principals. Whoever holds A's
/// private key issues as A.
#[test]
fn a_stolen_world_key_issues_as_that_world() {
    let mut f = fix();
    let stolen = f.auth.world_identity("A");
    let thief = Identity::generate();
    let g = grant(
        Parent::Grant(f.root.clone()),
        &f.a,
        thief.id(),
        &[("/data", DATA)],
        1,
        100,
    );
    f.auth
        .submit(Entry::Grant(stolen.sign_grant(g.clone())))
        .unwrap();
    let case = Case::new(
        "stolen key",
        "authenticity",
        f.auth.clone(),
        f.auth.publish(),
        g.id(),
        reads(thief.id(), &["/data/x"]),
        Verdict::Allow,
        "",
    );
    assert_eq!(case.decide_strict().verdict, Verdict::Allow);
}

/// Freshness is only as good as the witness. A runtime restored from a
/// backup holds an old witness and accepts the old ledger.
#[test]
fn a_restored_runtime_cannot_detect_staleness() {
    let mut f = fix();
    let backup = f.auth.clone();
    let old = backup.publish();
    f.auth.revoke("A", &f.child).unwrap();
    // The live runtime denies the old copy.
    let live = Case::new(
        "live",
        "freshness",
        f.auth.clone(),
        old.clone(),
        f.child.clone(),
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Deny,
        "A2.fresh_ledger",
    );
    assert_eq!(live.decide_strict().verdict, Verdict::Deny);
    // The restored runtime has the same key and an older witness.
    let restored = Case::new(
        "restored",
        "freshness",
        backup,
        old,
        f.child,
        reads(&f.b, &["/data/reports/q"]),
        Verdict::Allow,
        "",
    );
    assert_eq!(restored.decide_strict().verdict, Verdict::Allow);
}

/// Which checks carry which cases. A check whose removal changes no
/// verdict is not load-bearing.
#[test]
fn mutation_matrix() {
    let none_g = BTreeSet::new();
    let none_a = BTreeSet::new();
    let lax = |f: fn(&mut Strict)| {
        let mut s = Strict::ALL;
        f(&mut s);
        s
    };
    let runs: Vec<(&str, BTreeSet<Group>, BTreeSet<Check>, Strict)> = vec![
        (
            "no A1 signer pins",
            none_g.clone(),
            BTreeSet::from([Check::SignerPins]),
            Strict::ALL,
        ),
        (
            "no A2 freshness",
            none_g.clone(),
            BTreeSet::from([Check::Freshness]),
            Strict::ALL,
        ),
        (
            "verifier: no grant signatures",
            none_g.clone(),
            none_a.clone(),
            lax(|s| s.grant_signatures = false),
        ),
        (
            "verifier: no head signature",
            none_g.clone(),
            none_a.clone(),
            lax(|s| s.head_signature = false),
        ),
        (
            "verifier: no head signature, no A2",
            none_g.clone(),
            BTreeSet::from([Check::Freshness]),
            lax(|s| s.head_signature = false),
        ),
        (
            "verifier: no revocation entitlement",
            none_g.clone(),
            none_a.clone(),
            lax(|s| s.revocation_entitlement = false),
        ),
        (
            "no D2 issuer pins",
            BTreeSet::from([Group::IssuerPins]),
            none_a.clone(),
            Strict::ALL,
        ),
    ];
    let mut changed_by = BTreeMap::new();
    for (label, g, a, s) in &runs {
        let changed: Vec<String> = cases()
            .into_iter()
            .filter_map(|c| {
                let v = c.decide(g, a, *s).verdict;
                (v != c.expect).then(|| format!("{} → {v:?}", c.name))
            })
            .collect();
        println!(
            "{label}:\n  {}",
            if changed.is_empty() {
                "(no change)".to_string()
            } else {
                changed.join("\n  ")
            }
        );
        changed_by.insert(*label, changed);
    }
    let flipped = |label: &str, case: &str| changed_by[label].iter().any(|c| c.starts_with(case));
    assert!(flipped("no A2 freshness", "4a"));
    assert!(flipped("verifier: no grant signatures", "2d"));
    assert!(flipped("verifier: no grant signatures", "2b"));
    assert!(flipped("verifier: no head signature, no A2", "4b"));
    assert!(flipped("verifier: no revocation entitlement", "4c"));
    assert!(flipped("no D2 issuer pins", "2c"));
    // Measured, not assumed: A1 is implied by what the verifier emits.
    assert!(changed_by["no A1 signer pins"].is_empty());
}

#[test]
fn verifying_a_ten_thousand_entry_ledger() {
    let mut auth = Authority::new();
    let a = auth.enroll("A");
    let root = auth.root_grant(grant(
        Parent::Root,
        "",
        &a,
        &[("/d", "inode:1:1")],
        255,
        100,
    ));
    for i in 0..10_000u32 {
        let w = auth.enroll(&format!("w{i}"));
        auth.delegate(
            "A",
            grant(
                Parent::Grant(root.clone()),
                &a,
                &w,
                &[("/d", "inode:1:1")],
                0,
                100,
            ),
        )
        .unwrap();
    }
    let start = Instant::now();
    let copy = auth.publish();
    let published = start.elapsed();
    let start = Instant::now();
    let v = verify(&auth.anchor(), &copy);
    println!(
        "10,001 entries: publish {:?}, verify {:?}, {} facts, {} rejected",
        published,
        start.elapsed(),
        v.evidence.entries().len(),
        v.rejected.len()
    );
    assert!(v.rejected.is_empty());
}

#[test]
fn a_copy_verifies_without_the_runtime() {
    // Third-party verification: only the anchor string and the bytes.
    let f = fix();
    let bytes = serde_json::to_vec(&f.auth.publish()).unwrap();
    let anchor = f.auth.anchor();
    drop(f.auth);
    let copy: LedgerCopy = serde_json::from_slice(&bytes).unwrap();
    let v = verify(&anchor, &copy);
    assert!(v.rejected.is_empty(), "{:?}", v.rejected);
    assert_eq!(v.evidence.get(&DSubject::Root(f.root)).len(), 1);
    assert_eq!(v.evidence.get(&DSubject::Signer(f.child)).len(), 1);
}
