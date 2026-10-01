//! A tiny in-memory domain, used to falsify the claim that
//! [`crate::runtime`] is domain-neutral: no files, no processes, no
//! trace, no checkpoints, and no way to undo an effect.
//!
//! * **State**: one `u64` in a [`CounterCell`], shared so that a test can
//!   act as an out-of-band writer or a faulty actuator.
//! * **Effect**: `increment(by)`.
//! * **Invariants**:
//!   - `C1.counter_within_limit`: PRE on the predicted value
//!     (`before + by`), POST on the observed value;
//!   - `C2.observed_matches_proposal`: POST, the observed value is
//!     exactly `before + by`.
//! * **Not [`Compensable`](crate::runtime::Compensable)**: a rejected
//!   increment holds the runtime for good.
//!
//! ```compile_fail
//! // An irreversible domain offers no compensation.
//! use v9r_core::counter::{runtime, CounterCell};
//! let mut rt = runtime(CounterCell::new(0), 10).unwrap();
//! let _ = rt.compensate();
//! ```
//!
//! ```no_run
//! // Control: the same runtime authorizes.
//! use v9r_core::counter::{runtime, CounterCell, Increment};
//! let mut rt = runtime(CounterCell::new(0), 10).unwrap();
//! let _ = rt.authorize(Increment { by: 1, proposer: "x".into() });
//! ```

use std::convert::Infallible;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::kernel::{
    EvidenceBase, Fact, Obligation, Phase, Provenance, Requirement, Strength, Verified,
};
use crate::runtime::{
    Concluded, DomainEvidence, DomainObligation, EffectDomain, MemoryJournal, Runtime,
    RuntimeFault, Stage,
};

/// How the counter misbehaves (test fixture for faulty actuators).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fault {
    #[default]
    None,
    /// Every increment adds this much extra.
    Skew(u64),
    /// The counter becomes unreadable right after the next increment.
    BlindAfterIncrement,
}

#[derive(Debug, Default)]
struct Cell {
    value: u64,
    fault: Fault,
    blind: bool,
}

/// The reality the counter domain guards.
#[derive(Clone, Debug, Default)]
pub struct CounterCell(Arc<Mutex<Cell>>);

impl CounterCell {
    pub fn new(value: u64) -> Self {
        Self(Arc::new(Mutex::new(Cell {
            value,
            ..Cell::default()
        })))
    }

    fn cell(&self) -> std::sync::MutexGuard<'_, Cell> {
        self.0.lock().expect("counter lock")
    }

    pub fn get(&self) -> u64 {
        self.cell().value
    }

    /// An out-of-band write.
    pub fn set(&self, value: u64) {
        self.cell().value = value;
    }

    pub fn set_fault(&self, fault: Fault) {
        self.cell().fault = fault;
    }

    fn read(&self) -> Option<u64> {
        let cell = self.cell();
        (!cell.blind).then_some(cell.value)
    }

    fn increment(&self, by: u64) {
        let mut cell = self.cell();
        let extra = match cell.fault {
            Fault::Skew(extra) => extra,
            Fault::BlindAfterIncrement => {
                cell.blind = true;
                0
            }
            Fault::None => 0,
        };
        cell.value = cell.value.saturating_add(by).saturating_add(extra);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterSubject {
    Value,
}

/// Plain data from a proposer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Increment {
    pub by: u64,
    pub proposer: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CounterReceipt {
    pub before: u64,
    /// `None` if the result could not be read.
    pub after: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CounterError {
    Runtime(RuntimeFault),
    Unobservable,
}

impl fmt::Display for CounterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CounterError::Runtime(fault) => fault.fmt(f),
            CounterError::Unobservable => f.write_str("counter unobservable"),
        }
    }
}

impl From<RuntimeFault> for CounterError {
    fn from(fault: RuntimeFault) -> Self {
        CounterError::Runtime(fault)
    }
}

impl From<Infallible> for CounterError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

pub struct CounterDomain {
    cell: CounterCell,
    limit: u64,
}

pub type CounterRuntime = Runtime<CounterDomain, MemoryJournal>;

/// A runtime guarding `cell` under `counter <= limit`.
pub fn runtime(cell: CounterCell, limit: u64) -> Result<CounterRuntime, CounterError> {
    let baseline = cell.read().ok_or(CounterError::Unobservable)?;
    Ok(Runtime::new(
        CounterDomain { cell, limit },
        MemoryJournal::default(),
        baseline,
    ))
}

const C1: &str = "C1.counter_within_limit";
const C2: &str = "C2.observed_matches_proposal";

impl CounterDomain {
    fn at_most(&self, phase: Phase, quantity: &str, value: u64) -> DomainObligation<Self> {
        Obligation {
            invariant: C1.to_string(),
            phase,
            requirement: Requirement::AtMost {
                quantity: quantity.to_string(),
                value,
                limit: self.limit,
            },
        }
    }
}

impl EffectDomain for CounterDomain {
    type Subject = CounterSubject;
    type Value = u64;
    type Proposal = Increment;
    type Observation = u64;
    type Effect = ();
    type Receipt = CounterReceipt;
    type Output = ();
    type Error = CounterError;

    fn label(&self, proposal: &Increment) -> String {
        format!("increment {}", proposal.by)
    }

    fn policy_digest(&self) -> String {
        format!("counter <= {}", self.limit)
    }

    async fn observe(&self) -> Result<u64, CounterError> {
        self.cell.read().ok_or(CounterError::Unobservable)
    }

    fn basis(&self, observation: &u64) -> Vec<Fact<CounterSubject, u64>> {
        vec![Fact {
            subject: CounterSubject::Value,
            value: *observation,
        }]
    }

    fn obligations(&self, stage: Stage<'_, Self>) -> Vec<DomainObligation<Self>> {
        match stage {
            Stage::Pre { proposal, before } => vec![self.at_most(
                Phase::Pre,
                "counter after increment",
                before.checked_add(proposal.by).unwrap_or(u64::MAX),
            )],
            Stage::Post {
                proposal,
                before,
                after,
                ..
            } => {
                let mut out = vec![Obligation {
                    invariant: C2.to_string(),
                    phase: Phase::Post,
                    requirement: Requirement::Fact {
                        subject: CounterSubject::Value,
                        value: before.checked_add(proposal.by).unwrap_or(u64::MAX),
                        strength: Strength::Hard,
                    },
                }];
                if let Some(after) = after {
                    out.push(self.at_most(Phase::Post, "observed counter", *after));
                }
                out
            }
            Stage::Compensated { .. } => Vec::new(),
        }
    }

    async fn evidence(
        &self,
        subjects: &[&CounterSubject],
        observation: Option<&u64>,
        _receipt: Option<&CounterReceipt>,
    ) -> DomainEvidence<Self> {
        let mut evidence = EvidenceBase::new();
        if let Some(value) = observation {
            for subject in subjects {
                evidence.add_verified(
                    Verified::attest(Fact {
                        subject: **subject,
                        value: *value,
                    }),
                    Provenance {
                        observer: "counter-read".to_string(),
                        basis: value.to_string(),
                    },
                );
            }
        }
        evidence
    }

    async fn execute(&mut self, proposal: &Increment, _before: &u64) -> Result<(), CounterError> {
        self.cell.increment(proposal.by);
        Ok(())
    }

    async fn conclude(
        &mut self,
        _effect: (),
        before: &u64,
        after: Result<&u64, String>,
    ) -> Result<Concluded<Self>, CounterError> {
        Ok(Concluded {
            receipt: CounterReceipt {
                before: *before,
                after: after.ok().copied(),
            },
            output: None,
            durable: EvidenceBase::new(),
        })
    }
}
