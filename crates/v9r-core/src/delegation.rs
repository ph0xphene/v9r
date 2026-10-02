//! Capability delegation v0: a grant ledger above the kernel.
//!
//! ```text
//!   GrantLedger (trusted state)          compile() (trusted, pure)
//!   records, revocations, clock,          claimed chain + use ──▶ obligations
//!   trusted roots ──▶ verified facts                         │
//!                         │                                  │
//!                         └──────────▶ kernel::evaluate ◀────┘   (unchanged)
//! ```
//!
//! The kernel does not know what a grant is. Two trusted pieces turn
//! delegation into ordinary obligations and evidence:
//!
//! * The **ledger** stores grant records, keyed by the sha256 of their
//!   canonical JSON, and attests only facts about its own state: which
//!   records it holds, whose they are, which are revoked, which roots
//!   are configured, and its clock. It does **not** validate what it
//!   stores and it never judges a chain: [`GrantLedger::insert`] accepts
//!   any record, so a careless or malicious issuer is modelled by
//!   inserting what it would have issued.
//! * The **compiler** ([`compile`]) turns a *claimed* chain (records the
//!   caller supplies; untrusted) and a requested use into obligations.
//!   Every value it reads from a claimed record is pinned to the ledger
//!   through `record(id) = sha256(claimed record)`, so the evidence-free
//!   forms (`Within`, `AtMost`) end up judging stored values.
//!
//! | Id | Rule | Requirement |
//! |---|---|---|
//! | `D0.canonical` | every object is a canonical absolute path | `Within(names, [])` on a non-canonical name (always violated) |
//! | `D1.chain_pinned` | each claimed record is the stored one; the top is a configured root | `Fact(record(id) = digest)`, `Fact(root(id) = trusted)` |
//! | `D2.issuer_holds_parent` | a grant's grantor is its parent's grantee | `Fact(grantee(parent) = grantor)` |
//! | `D3.attenuates` | a child's names lie within its parent's | `Within(child, parent)` |
//! | `D4.depth` | redelegation depth strictly decreases; the chain is bounded | `AtMost(child + 1 ≤ parent)`, `AtMost(length ≤ MAX_CHAIN)` |
//! | `D5.unexpired` | every grant on the chain is unexpired now | `Fact(clock = t)`, `AtMost(t ≤ not_after)` |
//! | `D6.live` | no grant on the chain is revoked | `Fact(status(id) = live)` |
//! | `D7.use_within_leaf` | the user is the leaf's grantee; the use lies within the leaf | `Fact(grantee(leaf) = world)`, `Within(use, leaf)` |
//! | `D8.constraints` | every constraint on the chain holds for this use | `Fact(context(k) = v)` |
//! | `D9.object_identity` | each path still names the object the grant was issued for | `Fact(object(path) = id)` |
//!
//! Constraints are conditions on *use*, checked for every grant on the
//! chain, so a child cannot drop an ancestor's constraint: attenuation of
//! constraints needs no edge check.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::kernel::{
    self, Decision, EvidenceBase, Fact, Name, Obligation, Phase, Provenance, Requirement, Strength,
    Verified,
};

/// Longest chain the compiler walks. Beyond it the chain is violated
/// (`D4.depth`), whatever the records say.
pub const MAX_CHAIN: usize = 32;

pub type GrantId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Read,
    Write,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Read => "read",
            Op::Write => "write",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Parent {
    /// The top of a chain. Valid only if the ledger is configured to
    /// trust this grant as a root.
    Root,
    Grant(GrantId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub parent: Parent,
    /// The world that issued it: must hold the parent.
    pub grantor: String,
    /// The child world.
    pub grantee: String,
    /// Canonical absolute paths. A final `/.` names the directory itself,
    /// exactly, not what lies below it (as in Capability Manifest v0).
    pub objects: Vec<String>,
    pub operations: Vec<Op>,
    /// Conditions on use: the use's context must have `key = value`.
    pub constraints: BTreeMap<String, String>,
    /// Last ledger tick at which it may authorize.
    pub not_after: u64,
    /// How many further levels may be delegated below it.
    pub depth: u8,
    /// Identity of the object behind each path (e.g. `inode:<dev>:<ino>`
    /// or `sha256:<tree>`), pinned at use against a resolver.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub identities: BTreeMap<String, String>,
}

impl Grant {
    /// Content address of the record: sha256 of its canonical JSON.
    pub fn id(&self) -> GrantId {
        let bytes = serde_json::to_vec(self).expect("grant serializes");
        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    /// Capability names, in the Capability Manifest v0 vocabulary.
    pub fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for op in &self.operations {
            for object in &self.objects {
                out.push(cap_name(*op, object));
            }
        }
        out
    }
}

pub fn cap_name(op: Op, path: &str) -> String {
    format!("fs/{}{}", op.name(), path)
}

/// Absolute, no empty, `.` or `..` components, not `/` itself; a single
/// final `.` is the exact-directory marker.
pub fn canonical(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let last = parts.len() - 1;
    !rest.is_empty()
        && parts
            .iter()
            .enumerate()
            .all(|(i, p)| !p.is_empty() && *p != ".." && (*p != "." || (i == last && i > 0)))
}

// ---------------------------------------------------------------- facts

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "subject", content = "of", rename_all = "snake_case")]
pub enum DSubject {
    /// Digest of the record the ledger holds under this id.
    Record(GrantId),
    Grantee(GrantId),
    Status(GrantId),
    /// The ledger is configured to trust this grant as a root.
    Root(GrantId),
    Clock,
    /// An attribute of the use being authorized.
    Context(String),
    /// The key whose signature over the record verified.
    Signer(GrantId),
    /// The ledger head the evidence was derived from (`<seq>:<hash>`).
    Head,
    /// The identity of the object at this path now.
    Object(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "value", content = "is", rename_all = "snake_case")]
pub enum DValue {
    Digest(String),
    World(String),
    Live,
    Revoked,
    Trusted,
    Tick(u64),
    Text(String),
}

pub type DEvidence = EvidenceBase<DSubject, DValue>;
pub type DObligation = Obligation<DSubject, DValue>;

// ---------------------------------------------------------------- ledger

/// Trusted grant state. Stores whatever it is given; attests only what
/// it holds.
#[derive(Clone, Debug, Default)]
pub struct GrantLedger {
    records: BTreeMap<GrantId, Grant>,
    revoked: BTreeSet<GrantId>,
    roots: BTreeSet<GrantId>,
    now: u64,
}

impl GrantLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a record unvalidated. Returns its content address.
    pub fn insert(&mut self, grant: Grant) -> GrantId {
        let id = grant.id();
        self.records.insert(id.clone(), grant);
        id
    }

    /// Store a record and configure it as a trusted root (trusted
    /// configuration, not an agent's request).
    pub fn insert_root(&mut self, grant: Grant) -> GrantId {
        let id = self.insert(grant);
        self.roots.insert(id.clone());
        id
    }

    pub fn revoke(&mut self, id: &GrantId) {
        self.revoked.insert(id.clone());
    }

    pub fn advance(&mut self, ticks: u64) {
        self.now += ticks;
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn get(&self, id: &GrantId) -> Option<&Grant> {
        self.records.get(id)
    }

    /// The stored chain from `leaf` upward, as an honest supplier would
    /// give it (bounded by `MAX_CHAIN + 1`).
    pub fn chain(&self, leaf: &GrantId) -> Vec<(GrantId, Grant)> {
        let mut out = Vec::new();
        let mut next = Some(leaf.clone());
        while let Some(id) = next.take() {
            if out.len() > MAX_CHAIN {
                break;
            }
            let Some(grant) = self.records.get(&id) else {
                break;
            };
            if let Parent::Grant(p) = &grant.parent {
                next = Some(p.clone());
            }
            out.push((id, grant.clone()));
        }
        out
    }

    fn provenance(&self) -> Provenance {
        Provenance {
            observer: "grant-ledger".into(),
            basis: format!("tick {}, {} records", self.now, self.records.len()),
        }
    }

    /// Everything the ledger can vouch for about its own state.
    pub fn evidence(&self) -> DEvidence {
        let mut ev = DEvidence::new();
        let p = self.provenance();
        let mut add = |subject, value| {
            ev.add_verified(Verified::attest(Fact { subject, value }), p.clone());
        };
        for (id, grant) in &self.records {
            add(DSubject::Record(id.clone()), DValue::Digest(grant.id()));
            add(
                DSubject::Grantee(id.clone()),
                DValue::World(grant.grantee.clone()),
            );
            let status = if self.revoked.contains(id) {
                DValue::Revoked
            } else {
                DValue::Live
            };
            add(DSubject::Status(id.clone()), status);
        }
        for id in &self.roots {
            add(DSubject::Root(id.clone()), DValue::Trusted);
        }
        add(DSubject::Clock, DValue::Tick(self.now));
        ev
    }
}

// ---------------------------------------------------------------- use

/// A requested use of a grant: which world, which capability names, and
/// the context attributes constraints are checked against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Use {
    pub world: String,
    pub names: Vec<String>,
}

/// Verified context of a use (e.g. which commit is being released).
/// Crate-private: in v9r it would come from providers.
#[cfg(test)]
pub(crate) fn context_evidence(attrs: &[(&str, &str)]) -> DEvidence {
    let mut ev = DEvidence::new();
    for (k, v) in attrs {
        ev.add_verified(
            Verified::attest(Fact {
                subject: DSubject::Context(k.to_string()),
                value: DValue::Text(v.to_string()),
            }),
            Provenance {
                observer: "test-context".into(),
                basis: "synthetic".into(),
            },
        );
    }
    ev
}

// ---------------------------------------------------------------- compiler

/// Obligation groups the compiler can be told to leave out. Used only by
/// the mutation tests, to show which group each attack depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Group {
    Canonical,
    RecordPins,
    IssuerPins,
    Attenuation,
    Depth,
    Expiry,
    ClockPin,
    Liveness,
    ObjectPins,
}

/// Turn a claimed chain and a use into obligations. Pure; trusts nothing
/// in `claimed` and nothing in `now` (both are pinned).
pub fn compile(
    claimed: &[(GrantId, Grant)],
    leaf: &GrantId,
    use_: &Use,
    now: u64,
) -> Vec<DObligation> {
    compile_without(claimed, leaf, use_, now, &BTreeSet::new())
}

pub(crate) fn compile_without(
    claimed: &[(GrantId, Grant)],
    leaf: &GrantId,
    use_: &Use,
    now: u64,
    omit: &BTreeSet<Group>,
) -> Vec<DObligation> {
    let on = |g: Group| !omit.contains(&g);
    let pre = |invariant: &str, requirement| Obligation {
        invariant: invariant.to_string(),
        phase: Phase::Pre,
        requirement,
    };
    let hard = |subject, value| Requirement::Fact {
        subject,
        value,
        strength: Strength::Hard,
    };
    let by_id: BTreeMap<&GrantId, &Grant> = claimed.iter().map(|(i, g)| (i, g)).collect();
    let mut out = Vec::new();

    if on(Group::ClockPin) {
        out.push(pre(
            "D5.unexpired",
            hard(DSubject::Clock, DValue::Tick(now)),
        ));
    }

    // The use, against the leaf.
    let Some(leaf_grant) = by_id.get(leaf) else {
        out.push(gap("D1.chain_pinned", leaf));
        return out;
    };
    out.push(pre(
        "D7.use_within_leaf",
        hard(
            DSubject::Grantee(leaf.clone()),
            DValue::World(use_.world.clone()),
        ),
    ));
    out.push(pre(
        "D7.use_within_leaf",
        Requirement::Within {
            names: use_.names.iter().cloned().map(Name::Known).collect(),
            scopes: leaf_grant.names(),
        },
    ));

    // Walk up. Every step pins the record it read.
    let mut id = leaf.clone();
    let mut grant = *leaf_grant;
    let mut seen: BTreeSet<GrantId> = BTreeSet::new();
    loop {
        let cycle = !seen.insert(id.clone());
        if cycle || seen.len() > MAX_CHAIN {
            // A cycle is an unbounded chain: it is never walked to its end.
            if on(Group::Depth) {
                out.push(pre(
                    "D4.depth",
                    Requirement::AtMost {
                        quantity: format!(
                            "chain length ({} at {id})",
                            if cycle { "cycle" } else { "stopped" }
                        ),
                        value: MAX_CHAIN as u64 + 1,
                        limit: MAX_CHAIN as u64,
                    },
                ));
            }
            return out;
        }
        if on(Group::Canonical) {
            let bad: Vec<Name> = grant
                .objects
                .iter()
                .filter(|o| !canonical(o))
                .map(|o| Name::Known(format!("noncanonical:{o}")))
                .collect();
            if !bad.is_empty() {
                out.push(pre(
                    "D0.canonical",
                    Requirement::Within {
                        names: bad,
                        scopes: vec![],
                    },
                ));
            }
        }
        if on(Group::RecordPins) {
            out.push(pre(
                "D1.chain_pinned",
                hard(DSubject::Record(id.clone()), DValue::Digest(grant.id())),
            ));
        }
        if on(Group::ObjectPins) {
            for (path, object) in &grant.identities {
                out.push(pre(
                    "D9.object_identity",
                    hard(DSubject::Object(path.clone()), DValue::Text(object.clone())),
                ));
            }
        }
        if on(Group::Liveness) {
            out.push(pre(
                "D6.live",
                hard(DSubject::Status(id.clone()), DValue::Live),
            ));
        }
        if on(Group::Expiry) {
            out.push(pre(
                "D5.unexpired",
                Requirement::AtMost {
                    quantity: format!("tick vs not_after({id})"),
                    value: now,
                    limit: grant.not_after,
                },
            ));
        }
        for (k, v) in &grant.constraints {
            out.push(pre(
                "D8.constraints",
                hard(DSubject::Context(k.clone()), DValue::Text(v.clone())),
            ));
        }

        let parent_id = match &grant.parent {
            Parent::Root => {
                if on(Group::RecordPins) {
                    out.push(pre(
                        "D1.chain_pinned",
                        hard(DSubject::Root(id.clone()), DValue::Trusted),
                    ));
                }
                return out;
            }
            Parent::Grant(p) => p.clone(),
        };
        let Some(parent) = by_id.get(&parent_id) else {
            out.push(gap("D1.chain_pinned", &parent_id));
            return out;
        };
        if on(Group::IssuerPins) {
            out.push(pre(
                "D2.issuer_holds_parent",
                hard(
                    DSubject::Grantee(parent_id.clone()),
                    DValue::World(grant.grantor.clone()),
                ),
            ));
        }
        if on(Group::Attenuation) {
            out.push(pre(
                "D3.attenuates",
                Requirement::Within {
                    names: grant.names().into_iter().map(Name::Known).collect(),
                    scopes: parent.names(),
                },
            ));
        }
        if on(Group::Depth) {
            out.push(pre(
                "D4.depth",
                Requirement::AtMost {
                    quantity: format!("depth({id}) + 1 vs depth({parent_id})"),
                    value: grant.depth as u64 + 1,
                    limit: parent.depth as u64,
                },
            ));
        }
        id = parent_id;
        grant = parent;
    }
}

/// A record the chain needs but nobody supplied: unknown, never absent.
fn gap(invariant: &str, id: &GrantId) -> DObligation {
    Obligation {
        invariant: invariant.to_string(),
        phase: Phase::Pre,
        requirement: Requirement::Within {
            names: vec![Name::UnknownBelow(format!("grant/{id}"))],
            scopes: vec![],
        },
    }
}

/// Authorize `use_` by the claimed chain ending at `leaf`, against the
/// ledger's attested state and the use's verified context.
pub fn authorize(
    ledger: &GrantLedger,
    claimed: &[(GrantId, Grant)],
    leaf: &GrantId,
    use_: &Use,
    context: &DEvidence,
) -> Decision<DSubject, DValue> {
    let mut evidence = ledger.evidence();
    evidence.merge(context);
    kernel::evaluate(
        Phase::Pre,
        &compile(claimed, leaf, use_, ledger.now()),
        &evidence,
    )
}

#[cfg(test)]
mod tests;
