//! Verifiable observers: observation, derivation and verification apart.
//!
//! ```text
//!   providers ── raw observations (bytes, listings, objects) ──┐
//!                                                              ▼
//!   verifier (trusted, deterministic) ── asks for inputs ── derives a fact
//!                                                              │
//!   registry ── Verified only if every input it used was ──────┘
//!               self-certified by the verifier, or vouched for by a
//!               trusted (Attesting) observer
//! ```
//!
//! A [`Verifier`] derives facts of some kinds. The registry drives it:
//! it asks for raw inputs ([`Step::Need`]), the registry collects them
//! from providers (or from other verifiers, for derived inputs) and hands
//! back every candidate value with who vouched for it, until the verifier
//! derives a value or gives up.
//!
//! The verifier states, for every input it used, how it relied on it
//! ([`Basis`]):
//!
//! * `SelfCertified`: the verifier checked the input itself (e.g. the
//!   bytes hash to the identifier they were requested by). Nobody needs
//!   to be trusted for it.
//! * `VouchedBy`: the input is an observation of mutable reality that only
//!   a trusted observer (or another verifier) vouched for.
//! * `Unvouched`: a claim nobody vouched for. A derivation that uses one
//!   is kept as a claim, never as verified evidence.
//!
//! Providers' answers for *derived* kinds are never verified: they are
//! demoted to claims, and a claim the verifier contradicts is logged.

use std::collections::BTreeMap;

use crate::graph::{Key, Term};

/// One candidate value for an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub value: Term,
    /// The provider (`provider:<id>`) or verifier (`verifier:<id>`) that
    /// vouched for it; `None` for a claim.
    pub vouched_by: Option<String>,
}

/// Every candidate per input key fetched so far. A key that was asked for
/// but not answered is present with no candidates.
pub type Inputs = BTreeMap<Key, Vec<Candidate>>;

/// How a derivation relied on one input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Basis {
    SelfCertified(Key),
    VouchedBy(Key, String),
    Unvouched(Key),
}

pub enum Step {
    /// Fetch these inputs, then ask again.
    Need(Vec<Key>),
    Derived {
        value: Term,
        basis: Vec<Basis>,
    },
    /// Cannot be derived from what is available (missing, contradictory,
    /// not representable, or failing its own checks).
    Incomplete(String),
}

/// A trusted, deterministic derivation. Must not observe anything itself.
pub trait Verifier: Send + Sync {
    fn id(&self) -> &str;
    fn derives(&self, key: &Key) -> bool;
    fn step(&self, key: &Key, inputs: &Inputs) -> Step;
}

/// The single value vouched for by someone, if all vouched candidates
/// agree; otherwise the reason it is not usable.
pub fn vouched(inputs: &Inputs, key: &Key) -> Result<(Term, String), String> {
    let candidates = inputs.get(key).map(Vec::as_slice).unwrap_or_default();
    let mut vouched = candidates
        .iter()
        .filter_map(|c| Some((&c.value, c.vouched_by.as_ref()?)));
    match vouched.next() {
        None if candidates.is_empty() => Err(format!("no observation of {key:?}")),
        None => Err(format!("{key:?} only claimed, vouched for by nobody")),
        Some((value, by)) => {
            if vouched.all(|(v, _)| v == value) {
                Ok((value.clone(), by.clone()))
            } else {
                Err(format!("contradictory observations of {key:?}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn verify_depends_on_no_domain_module() {
        let source = include_str!("verify.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for line in source
            .lines()
            .filter(|l| l.trim_start().starts_with("use "))
        {
            assert!(
                ["std::", "crate::graph::"].iter().any(|a| line.contains(a)),
                "{line}"
            );
        }
        let code = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        for word in ["path", "file", "commit", "git", "workspace", "artifact"] {
            assert!(
                !code
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|t| t == word),
                "verify code mentions `{word}`"
            );
        }
    }
}
