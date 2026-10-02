//! The kernel freeze: `src/kernel.rs` is byte-identical to the reviewed
//! version (`bfebebe`) and knows nothing about the layers built above it.
//!
//! Moved here unchanged from `src/delegation/tests.rs` (Debloat Phase 1),
//! so that the guard does not depend on any module it guards against.
//! It lives outside `kernel.rs` on purpose: a guard inside the file it
//! hashes would change the hash.

use sha2::{Digest, Sha256};

#[test]
fn kernel_is_unchanged_and_knows_no_delegation() {
    let kernel = include_str!("../src/kernel.rs");
    let digest = format!("{:x}", Sha256::digest(kernel.as_bytes()));
    assert_eq!(
        digest,
        "85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f"
    );
    for word in ["delegat", "ledger", "revok", "grantee", "grantor"] {
        assert!(!kernel.to_lowercase().contains(word), "{word}");
    }
}
