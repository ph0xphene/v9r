//! Root of trust for delegation: issuer identity as verifiable keys.
//!
//! ```text
//!   Authority (runtime-owned)                       anyone holding the anchor
//!   ─────────────────────────                       ─────────────────────────
//!   root key R (private, never leaves)              anchor = R's public key (configuration)
//!   world keys (held FOR worlds; channel-bound)     LedgerCopy (untrusted bytes)
//!   append-only log ── hash chain ── head(seq, h)        │
//!   head signed by R ─────── publish() ─────────▶  verify(anchor, copy) ──▶ Verified facts
//!   witness = current (seq, h)  (trusted, fresh)          │  record, grantee, status, root,
//!   clock                                                 │  signer, head
//!        │                                                ▼
//!        └── obligations: delegation::compile (grant correctness)
//!                         + A1 signer pins (issuer authenticity)
//!                         + A2 head = witness (freshness) ─────▶ kernel::evaluate (unchanged)
//! ```
//!
//! Three separate questions, three separate pieces:
//!
//! * **Grant correctness**: `delegation::compile`, unchanged from X2.
//! * **Issuer authenticity**: every grant is signed by its `grantor` key;
//!   a root grant's grantor is the anchor; a revocation is signed by the
//!   grant's issuer or the anchor. [`verify`] checks the signatures from
//!   bytes alone. It trusts no ledger holder, only the anchor.
//! * **Freshness**: the log is a hash chain whose head the runtime signs.
//!   Signatures cannot prove that a newer head does not exist, so the
//!   runtime's own current head (the **witness**) is required as a fact
//!   (`A2.fresh_ledger`).
//!
//! World keys are held by the runtime. A world issues a grant by asking
//! on its own channel ([`Authority::delegate`]), and the runtime signs
//! with that world's key. A world never holds a private key: a key it
//! held could be copied, and the copy would issue as the world.

use std::collections::{BTreeMap, BTreeSet};

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::delegation::{
    compile_without, DEvidence, DObligation, DSubject, DValue, Grant, GrantId, Group, Parent, Use,
    MAX_CHAIN,
};
use crate::kernel::{
    self, Decision, Fact, Obligation, Phase, Provenance, Requirement, Strength, Verified,
};

/// `ed25519:<hex public key>`.
pub type KeyId = String;

const GRANT_DOMAIN: &[u8] = b"v9r-grant-v0\0";
const REVOKE_DOMAIN: &[u8] = b"v9r-revoke-v0\0";
const HEAD_DOMAIN: &[u8] = b"v9r-head-v0\0";
const GENESIS: &str = "v9r-ledger-v0";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn message(domain: &[u8], body: &str) -> Vec<u8> {
    [domain, body.as_bytes()].concat()
}

/// A signing identity. The private key exists only inside this value.
#[derive(Clone)]
pub struct Identity {
    pkcs8: Vec<u8>,
    id: KeyId,
}

impl Identity {
    pub fn generate() -> Self {
        let doc = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("rng");
        let pair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).expect("fresh key parses");
        Self {
            pkcs8: doc.as_ref().to_vec(),
            id: format!("ed25519:{}", hex(pair.public_key().as_ref())),
        }
    }

    pub fn id(&self) -> &KeyId {
        &self.id
    }

    fn sign(&self, msg: &[u8]) -> String {
        let pair = Ed25519KeyPair::from_pkcs8(&self.pkcs8).expect("own key parses");
        hex(pair.sign(msg).as_ref())
    }

    /// Sign a grant as its issuer (used by keys outside runtime custody:
    /// an attacker's own key, or a stolen copy of a world's).
    pub fn sign_grant(&self, grant: Grant) -> SignedGrant {
        let signature = self.sign(&message(GRANT_DOMAIN, &grant.id()));
        SignedGrant { grant, signature }
    }
}

/// Does `signature` (hex) verify under key `key` for `msg`?
pub fn verifies(key: &KeyId, msg: &[u8], signature: &str) -> bool {
    let (Some(public), Some(sig)) = (
        key.strip_prefix("ed25519:").and_then(unhex),
        unhex(signature),
    ) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, public)
        .verify(msg, &sig)
        .is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedGrant {
    pub grant: Grant,
    /// By `grant.grantor`, over the grant's id.
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum Entry {
    Grant(SignedGrant),
    /// Valid if `by` is the grant's issuer or the anchor.
    Revoke {
        grant: GrantId,
        by: KeyId,
        signature: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHead {
    pub seq: u64,
    pub hash: String,
    /// By the anchor, over `<seq>:<hash>`.
    pub signature: String,
}

impl SignedHead {
    pub fn text(&self) -> String {
        format!("{}:{}", self.seq, self.hash)
    }
}

/// A ledger as anyone may hold it: plain data, freely copied and edited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerCopy {
    pub entries: Vec<Entry>,
    pub head: SignedHead,
}

fn chain_hash(entries: &[Entry]) -> String {
    let mut h = format!("{:x}", Sha256::digest(GENESIS));
    for e in entries {
        let body = serde_json::to_vec(e).expect("entry serializes");
        h = format!(
            "{:x}",
            Sha256::digest([h.as_bytes(), &Sha256::digest(&body)[..]].concat())
        );
    }
    h
}

// ---------------------------------------------------------------- authority

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    UnknownWorld(String),
    /// The record names another grantor than the world on whose channel
    /// the request arrived.
    NotTheRequester {
        requester: KeyId,
        grantor: KeyId,
    },
    BadSignature,
}

/// Runtime-owned root of trust: the root key, the world keys it holds
/// for worlds, the append-only log, and the clock.
#[derive(Clone)]
pub struct Authority {
    root: Identity,
    worlds: BTreeMap<String, Identity>,
    log: Vec<Entry>,
    clock: u64,
}

impl Default for Authority {
    fn default() -> Self {
        Self::new()
    }
}

impl Authority {
    pub fn new() -> Self {
        Self {
            root: Identity::generate(),
            worlds: BTreeMap::new(),
            log: Vec::new(),
            clock: 0,
        }
    }

    /// The anchor: the only thing a verifier must be configured with.
    pub fn anchor(&self) -> KeyId {
        self.root.id().clone()
    }

    /// Create the key the runtime holds for `world`.
    pub fn enroll(&mut self, world: &str) -> KeyId {
        let identity = Identity::generate();
        let id = identity.id().clone();
        self.worlds.insert(world.to_string(), identity);
        id
    }

    pub fn key(&self, world: &str) -> Option<KeyId> {
        self.worlds.get(world).map(|i| i.id().clone())
    }

    /// Issue a root grant: the runtime's own authority, signed by R.
    pub fn root_grant(&mut self, mut grant: Grant) -> GrantId {
        grant.parent = Parent::Root;
        grant.grantor = self.anchor();
        let signed = self.root.sign_grant(grant);
        let id = signed.grant.id();
        self.log.push(Entry::Grant(signed));
        id
    }

    /// A world's request to delegate, arriving on its own channel. The
    /// runtime signs with that world's key, and only if the record names
    /// the requester as grantor. Grant *correctness* is not checked here:
    /// it is checked at use.
    pub fn delegate(&mut self, requester: &str, grant: Grant) -> Result<GrantId, Refused> {
        let identity = self
            .worlds
            .get(requester)
            .ok_or_else(|| Refused::UnknownWorld(requester.into()))?;
        if grant.grantor != *identity.id() {
            return Err(Refused::NotTheRequester {
                requester: identity.id().clone(),
                grantor: grant.grantor,
            });
        }
        let signed = identity.sign_grant(grant);
        let id = signed.grant.id();
        self.log.push(Entry::Grant(signed));
        Ok(id)
    }

    /// Append an entry signed elsewhere (by a key outside runtime
    /// custody). Its signature must verify under the key it names; what
    /// that key may do is decided at use.
    pub fn submit(&mut self, entry: Entry) -> Result<(), Refused> {
        let ok = match &entry {
            Entry::Grant(g) => verifies(
                &g.grant.grantor,
                &message(GRANT_DOMAIN, &g.grant.id()),
                &g.signature,
            ),
            Entry::Revoke {
                grant,
                by,
                signature,
            } => verifies(by, &message(REVOKE_DOMAIN, grant), signature),
        };
        if !ok {
            return Err(Refused::BadSignature);
        }
        self.log.push(entry);
        Ok(())
    }

    /// A faulty appender: appends without checking anything.
    #[cfg(test)]
    pub(crate) fn append_unchecked(&mut self, entry: Entry) {
        self.log.push(entry);
    }

    /// Revoke as `world` (its key must be the grant's issuer to count).
    pub fn revoke(&mut self, world: &str, grant: &GrantId) -> Result<(), Refused> {
        let identity = self
            .worlds
            .get(world)
            .ok_or_else(|| Refused::UnknownWorld(world.into()))?;
        self.log.push(Entry::Revoke {
            grant: grant.clone(),
            by: identity.id().clone(),
            signature: identity.sign(&message(REVOKE_DOMAIN, grant)),
        });
        Ok(())
    }

    pub fn revoke_as_root(&mut self, grant: &GrantId) {
        self.log.push(Entry::Revoke {
            grant: grant.clone(),
            by: self.anchor(),
            signature: self.root.sign(&message(REVOKE_DOMAIN, grant)),
        });
    }

    /// The runtime's current head: the freshness witness.
    pub fn witness(&self) -> String {
        format!("{}:{}", self.log.len(), chain_hash(&self.log))
    }

    /// A copy of the ledger, with a head signed now.
    pub fn publish(&self) -> LedgerCopy {
        let head_text = self.witness();
        let hash = chain_hash(&self.log);
        LedgerCopy {
            entries: self.log.clone(),
            head: SignedHead {
                seq: self.log.len() as u64,
                hash,
                signature: self.root.sign(&message(HEAD_DOMAIN, &head_text)),
            },
        }
    }

    pub fn advance(&mut self, ticks: u64) {
        self.clock += ticks;
    }

    pub fn now(&self) -> u64 {
        self.clock
    }

    /// The runtime's own facts: its clock.
    pub fn evidence(&self) -> DEvidence {
        let mut ev = DEvidence::new();
        ev.add_verified(
            Verified::attest(Fact {
                subject: DSubject::Clock,
                value: DValue::Tick(self.clock),
            }),
            Provenance {
                observer: "runtime-clock".into(),
                basis: format!("tick {}", self.clock),
            },
        );
        ev
    }

    #[cfg(test)]
    pub(crate) fn world_identity(&self, world: &str) -> Identity {
        self.worlds[world].clone()
    }
}

// ---------------------------------------------------------------- verifier

/// What [`verify`] established and what it threw away.
#[derive(Debug, Default)]
pub struct Verification {
    pub evidence: DEvidence,
    /// Entries (by index) or the whole copy, with the reason.
    pub rejected: Vec<String>,
    /// Entries past the signed head, ignored.
    pub unsigned_tail: usize,
}

/// The verifier's checks, each of which the mutation tests switch off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Strict {
    pub head_signature: bool,
    pub grant_signatures: bool,
    pub revocation_entitlement: bool,
}

impl Strict {
    pub const ALL: Strict = Strict {
        head_signature: true,
        grant_signatures: true,
        revocation_entitlement: true,
    };
}

/// Verify a ledger copy from bytes alone, given only the anchor. Trusts
/// no holder of the copy: every fact comes from a signature that
/// verified under the anchor or under the key the record names.
pub fn verify(anchor: &KeyId, copy: &LedgerCopy) -> Verification {
    verify_with(anchor, copy, Strict::ALL)
}

pub(crate) fn verify_with(anchor: &KeyId, copy: &LedgerCopy, strict: Strict) -> Verification {
    let mut out = Verification::default();
    let head = &copy.head;
    if strict.head_signature
        && !verifies(anchor, &message(HEAD_DOMAIN, &head.text()), &head.signature)
    {
        out.rejected
            .push("head: signature does not verify under the anchor".into());
        return out;
    }
    let Some(signed) = copy.entries.get(..head.seq as usize) else {
        out.rejected.push(format!(
            "head: seq {} beyond {} entries",
            head.seq,
            copy.entries.len()
        ));
        return out;
    };
    if chain_hash(signed) != head.hash {
        out.rejected
            .push("head: entries do not hash to the signed head".into());
        return out;
    }
    out.unsigned_tail = copy.entries.len() - signed.len();

    let provenance = Provenance {
        observer: "ledger-verifier".into(),
        basis: format!("anchor {}…, head {}", &anchor[..16], head.text()),
    };
    let mut grants: BTreeMap<GrantId, Grant> = BTreeMap::new();
    let mut revoked: BTreeSet<GrantId> = BTreeSet::new();
    for (i, entry) in signed.iter().enumerate() {
        match entry {
            Entry::Grant(g) => {
                let id = g.grant.id();
                if !strict.grant_signatures
                    || verifies(&g.grant.grantor, &message(GRANT_DOMAIN, &id), &g.signature)
                {
                    grants.insert(id, g.grant.clone());
                } else {
                    out.rejected
                        .push(format!("entry {i}: grant signature does not verify"));
                }
            }
            Entry::Revoke {
                grant,
                by,
                signature,
            } => {
                let issuer = grants.get(grant).map(|g| &g.grantor);
                let entitled = !strict.revocation_entitlement || by == anchor || issuer == Some(by);
                if entitled && verifies(by, &message(REVOKE_DOMAIN, grant), signature) {
                    revoked.insert(grant.clone());
                } else {
                    out.rejected
                        .push(format!("entry {i}: revocation not by issuer or anchor"));
                }
            }
        }
    }

    let mut add = |subject, value| {
        out.evidence.add_verified(
            Verified::attest(Fact { subject, value }),
            provenance.clone(),
        );
    };
    add(DSubject::Head, DValue::Text(head.text()));
    for (id, g) in &grants {
        add(DSubject::Record(id.clone()), DValue::Digest(id.clone()));
        add(
            DSubject::Signer(id.clone()),
            DValue::World(g.grantor.clone()),
        );
        add(
            DSubject::Grantee(id.clone()),
            DValue::World(g.grantee.clone()),
        );
        let status = if revoked.contains(id) {
            DValue::Revoked
        } else {
            DValue::Live
        };
        add(DSubject::Status(id.clone()), status);
        if g.parent == Parent::Root && g.grantor == *anchor {
            add(DSubject::Root(id.clone()), DValue::Trusted);
        }
    }
    out
}

// ---------------------------------------------------------------- obligations

/// Which authenticity obligations to emit (the mutation tests leave one
/// out).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Check {
    SignerPins,
    Freshness,
}

/// Issuer authenticity and freshness, over the same claimed chain that
/// `delegation::compile` judges for correctness.
pub(crate) fn authenticity(
    claimed: &[(GrantId, Grant)],
    leaf: &GrantId,
    witness: &str,
    omit: &BTreeSet<Check>,
) -> Vec<DObligation> {
    let pre = |invariant: &str, subject, value| Obligation {
        invariant: invariant.to_string(),
        phase: Phase::Pre,
        requirement: Requirement::Fact {
            subject,
            value,
            strength: Strength::Hard,
        },
    };
    let mut out = Vec::new();
    if !omit.contains(&Check::Freshness) {
        out.push(pre(
            "A2.fresh_ledger",
            DSubject::Head,
            DValue::Text(witness.to_string()),
        ));
    }
    if omit.contains(&Check::SignerPins) {
        return out;
    }
    let by_id: BTreeMap<&GrantId, &Grant> = claimed.iter().map(|(i, g)| (i, g)).collect();
    let mut next = Some(leaf.clone());
    let mut seen = BTreeSet::new();
    while let Some(id) = next.take() {
        let Some(grant) = by_id.get(&id) else { break };
        if !seen.insert(id.clone()) || seen.len() > MAX_CHAIN {
            break; // `compile` already fails the chain
        }
        out.push(pre(
            "A1.issuer_signed",
            DSubject::Signer(id.clone()),
            DValue::World(grant.grantor.clone()),
        ));
        if let Parent::Grant(p) = &grant.parent {
            next = Some(p.clone());
        }
    }
    out
}

/// Two copies whose heads both verify under the anchor at the same
/// sequence number with different hashes: the anchor signed a fork.
pub fn equivocates(anchor: &KeyId, a: &LedgerCopy, b: &LedgerCopy) -> bool {
    let signed = |c: &LedgerCopy| {
        verifies(
            anchor,
            &message(HEAD_DOMAIN, &c.head.text()),
            &c.head.signature,
        )
    };
    a.head.seq == b.head.seq && a.head.hash != b.head.hash && signed(a) && signed(b)
}

/// The supplier-side view of a chain: the records a copy holds from
/// `leaf` upward (untrusted, as in X2).
pub fn chain(copy: &LedgerCopy, leaf: &GrantId) -> Vec<(GrantId, Grant)> {
    let by_id: BTreeMap<GrantId, &Grant> = copy
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Grant(g) => Some((g.grant.id(), &g.grant)),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    let mut next = Some(leaf.clone());
    while let Some(id) = next.take() {
        let Some(g) = by_id.get(&id) else { break };
        if out.len() > MAX_CHAIN {
            break;
        }
        if let Parent::Grant(p) = &g.parent {
            next = Some(p.clone());
        }
        out.push((id, (*g).clone()));
    }
    out
}

/// Everything a decision sees, kept apart by role.
pub struct Request<'a> {
    /// Untrusted: the copies offered as evidence.
    pub copies: &'a [LedgerCopy],
    /// Untrusted: the chain the caller claims.
    pub claimed: &'a [(GrantId, Grant)],
    pub leaf: &'a GrantId,
    /// Trusted: from the runtime's channel binding and observation.
    pub use_: &'a Use,
    /// Trusted: verified context and object resolutions.
    pub context: &'a DEvidence,
}

/// Authorize a use. `anchor`, `witness` and the clock come from the
/// runtime; everything in `request.copies` and `request.claimed` is
/// checked.
pub fn authorize(
    runtime: &Authority,
    anchor: &KeyId,
    witness: &str,
    request: &Request,
) -> Decision<DSubject, DValue> {
    authorize_without(
        runtime,
        anchor,
        witness,
        request,
        &BTreeSet::new(),
        &BTreeSet::new(),
        Strict::ALL,
    )
}

pub(crate) fn authorize_without(
    runtime: &Authority,
    anchor: &KeyId,
    witness: &str,
    request: &Request,
    omit_grant: &BTreeSet<Group>,
    omit_auth: &BTreeSet<Check>,
    strict: Strict,
) -> Decision<DSubject, DValue> {
    let mut evidence = runtime.evidence();
    evidence.merge(request.context);
    for copy in request.copies {
        evidence.merge(&verify_with(anchor, copy, strict).evidence);
    }
    let mut obligations = compile_without(
        request.claimed,
        request.leaf,
        request.use_,
        runtime.now(),
        omit_grant,
    );
    obligations.extend(authenticity(
        request.claimed,
        request.leaf,
        witness,
        omit_auth,
    ));
    kernel::evaluate(Phase::Pre, &obligations, &evidence)
}

#[cfg(test)]
mod tests;
