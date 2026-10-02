//! v9r-core: a state transition verifier for untrusted computation.
//!
//! v9r verifies states, not histories. Start at `kernel` (the verdict),
//! then `runtime` (the lifecycle), `temporal` (before/after), `graph`
//! (evidence), `snapshot` and `fs_raw` (state identity and observation).

pub mod content;
pub mod counter;
pub mod fs_raw;
pub mod fs_watch;
pub mod graph;
pub mod kernel;
pub mod provenance;
pub mod runtime;
pub mod snapshot;
pub mod temporal;
pub mod verifiers;
pub mod verify;
