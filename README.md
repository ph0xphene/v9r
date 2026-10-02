# v9r

**A state transition verifier for untrusted computation.**

An agent (typically an LLM) proposes a change. v9r authorizes it once,
against an approved, content-identified starting state. After the agent
acts, v9r collects evidence about the result itself, from registered
observers, and accepts the new state only if that evidence satisfies
declared rules. Every decision is **Allow**, **Deny** or **Blocked**, with
a reason per rule naming the observer behind each fact.

v9r verifies **states, not histories**. If an unauthorized writer produces
the same correct state, v9r accepts it too, with identical decisions
(`research/STATE_VS_CAUSALITY_V0.md`). It does not know, and does not
claim to know, who produced a state.

This is research code. Reviewers: start with `README_REVIEWER.md`.

## See it

```sh
cargo run --offline -p v9r-core --example v9r_demo     # needs python3
```

Five scripted agents against one small repository with a failing test:
ALLOW (honest fix), DENY (the agent edits the protected test), BLOCKED
(the agent only claims success), DENY (the same claim, with the tests run
by v9r), REFUSED (the code changed after approval).

## Check it

```sh
cargo test --offline --workspace
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
cargo test --offline -p v9r-core --test kernel_guard
```

## Layout

```text
crates/v9r-core    the verifier: kernel, runtime, temporal transitions,
                   evidence registry, filesystem observer, snapshots
research/          design notes and measured reports
```

The project's earlier "transactional shell" was removed in Debloat
Phase 1: the CLI, REPL, bundles, rollback, LLM providers, an in-memory
VFS and a Wasm agent host (`v9r-vfs`, `v9r-cap`, `v9r-runtime`,
`v9r-orchestrator`). It is not part of the verifier and remains at the
git tag `v9r-archive-v0` (`research/V9R_ARCHIVE_BOUNDARY_V0.md`).

## Status

Experimental. Measured on one Linux host. The guarantees and their
conditions are in `research/V9R_PHASE1_EXTERNAL_REVIEW_V0.md`.
