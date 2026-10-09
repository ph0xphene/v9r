# Support policy

v9r is a research release, not a general-purpose sandbox or a production guarantee.

## Reference environment

- Rust stable
- Linux and macOS are exercised by the repository CI matrix
- A fresh checkout with Cargo network access

Run the supported baseline from the repository root:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The runtime's isolation and observation guarantees are platform- and adapter-dependent. Review `CLAIMS.md` and the source-level tests before relying on a behavior in a high-consequence workflow.

## Compatibility boundary

- Windows is not currently part of the supported CI contract.
- Provider integrations and host-level isolation may require additional local setup.
- A passing test suite does not establish containment against a compromised host or executor.
- Research branches may change semantics before the next tagged release.

Please include OS, Rust version, command, manifest, and complete output in compatibility reports.
