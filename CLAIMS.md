# Claims and evidence

This table keeps the public contract narrower than the research agenda.

| Claim | Evidence | Not proven | Failure boundary |
|---|---|---|---|
| Tasks can be bounded by a manifest | Manifest and runtime unit tests | Complete mediation of every host facility | An adapter or host path outside the runtime boundary |
| Required artifacts and tests gate success | Workspace tests and validation code | Correctness of arbitrary user-supplied tests | A misleading or incomplete validation command |
| Rollback handles supported filesystem failures | VFS/adapter regression tests | Recovery from external side effects | Effects outside the observed worktree |
| Bundles preserve task output and trace data | Bundle implementation and tests | Independent attestation of every claim in a bundle | Unobserved external state |
| Shell-like command smuggling is rejected by validation | Execution validator tests | Safety of every future command adapter | New adapter semantics not covered by tests |

`v9r` reports what its configured observers establish. It must not be described as arbitrary program verification, a universal sandbox, or proof that an agent's explanation is true.

When adding a capability, add a negative test and update this table if the support boundary changes.
