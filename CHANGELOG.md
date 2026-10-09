# Changelog

## Unreleased — transactional-runtime research candidate

- Add Linux/macOS Rust CI definitions for format, tests, and strict clippy.
- Document support boundaries and claims separately from runtime ambitions.
- Fix strict-clippy findings in CLI and WASM runtime code.
- Repair the deterministic task-runtime example to use `cp` rather than forbidden shell interpreters and to inherit the task workdir.
- Document a model-free allow/reject/rollback demo.

This candidate is not the first release of the public v9r repository. The published `v9r-review-v1.1` is an observer-boundary research release on a different source lineage. These changes have only been validated locally; hosted CI results are not yet available for them.
