# v9r Review Freeze Checklist v0

Goal: make the current branch a stable research artifact for external
review. No features, no experiments.

## Repository state

| Item | State |
|---|---|
| branch | `research/effect-runtime-v0` |
| last commit | `8a49c4b` (docs(research): atomic capture v0 report) |
| uncommitted work | everything since `8a49c4b`: capability, delegation, authority and object-identity modules; the snapshot and capability extensions; nine experiment tests; the demo; 18 research documents (list under *Commit structure*) |
| test status | **316 passed, 0 failed, 6 ignored** (`cargo test --offline --workspace --no-fail-fast`, run by the user on 2026-10-02). After removing `tests/agent_transition.rs` the expected count is **315 / 0 / 6**. **This must be re-run** |
| `kernel.rs` invariant | sha256 `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`, unchanged since `bfebebe`. Checked by `delegation::tests::kernel_is_unchanged_and_knows_no_delegation`; domain-freedom of the kernel, runtime and temporal layer checked by their guard tests |
| new dependency | `ring` 0.17 (already locked through rustls; builds `--offline`) |
| known warning | `capability.rs`: field `error` of `InsideNetwork::Error` is never read. Harmless, unchanged by the freeze |

## Decision: `tests/agent_transition.rs` → **remove** (option B)

**Criterion:** could a future reviewer mistake it for evidence that v9r
verifies transition causality? **Yes:**

- it passes;
- its only assertion reads "no case except the honest ones was
  accepted";
- its invariants are named `verification_executed` and
  `artifact_matches_output`;
- it was never shown to tell an authorized transition from an
  unauthorized one;
- its I4/I5 evidence executes agent-controlled code.

`#[ignore]` with a header would keep the code without keeping any
result: per-case verdicts were never recorded, and no results report
(`AGENT_TRANSITION_RUNTIME_V0.md`) exists. The approach and the reason
it was superseded remain in AGENT_TRANSITION_RUNTIME_DESIGN_V0 and
AGENT_TRANSITION_TRUST_MODEL_V0, both classified as historical.

The file is untracked, so removal is `rm crates/v9r-core/tests/agent_transition.rs`.

## Document audit

Checked: V9R_ARCHITECTURE_OVERVIEW_V0, V9R_EXTERNAL_REVIEW_PACKAGE_V0,
V9R_RESEARCH_MILESTONE_V0. Only clarity was changed; no result was
altered.

| Document | Finding | Type | Fixed |
|---|---|---|---|
| Overview | "accepted only if every rule is satisfied by verified evidence" lacked the observer-faithfulness assumption | guarantee missing trust assumption | added |
| Overview | content identity "after ingestion" did not say ingestion is trusted and non-atomic | condition missing | added |
| Overview | single-use / refused-on-change lacked "visible to the watched observations" | condition missing | added |
| Overview | "every decision is explained" did not say explanations can be consistent and wrong | condition missing | added |
| Overview, review package | "unchanged since its first milestone" (imprecise) | imprecise claim | now "since `bfebebe`, sha256 …" |
| Review package | "17 later experiments" (an uncounted number) | unverified figure | now "every later experiment" |
| Review package | "the kernel refused…" could be read as `kernel.rs` | ambiguity | now "the Linux kernel" |
| Milestone | controlled-domain minimum stated without "for the behaviours tested" | measured result without condition | added |
| all three | `agent_transition` described as "green" without the decision | superseded item not clearly marked | updated to the removal decision |
| Overview reading guide | four documents classified without being re-read | classification provenance | stated in the guide |

Not changed, recorded instead:

- **CAPABILITY_MANIFEST_V0** still says submounts are "not carried";
  Object Identity v0 showed inherited submounts make the bind *fail*.
- **CAPABILITY_MANIFEST_V0's** C2 identity `(dev, ino)` is weakened by
  inode reuse.
- **CAPABILITY_COMPOSITION_DESIGN_V0's** `(dev, ino, mnt_id)` was
  falsified.

These reports are history. Their corrections live in the later reports
and in the overview, and the reports themselves were not edited.

## Reviewer entry points

| Order | What | Why |
|---|---|---|
| 1 | `research/V9R_ARCHITECTURE_OVERVIEW_V0.md` | what v9r is and is not; guarantees with conditions; the reading guide |
| 2 | `cargo run --offline -p v9r-core --example v9r_demo` (needs `python3`, about 1 s, deterministic) | five verdicts on one screen |
| 3 | `research/STATE_VS_CAUSALITY_V0.md` | the boundary result |
| 4 | `research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md` §5 | the hard questions |
| key experiments | INVARIANT_KERNEL_V0, EFFECT_RUNTIME_V1, TEMPORAL_EVIDENCE_V0, EVIDENCE_PROVENANCE_V0, CAPABILITY_DELEGATION_V0, CAPABILITY_OBJECT_IDENTITY_V0, CAPABILITY_CONTENT_IDENTITY_V0 | measured foundations |

## Known boundaries

**What v9r proves** (measured, with conditions in the overview):

- accepted states satisfy declared rules on verified evidence;
- out-of-scope changes are denied;
- content identity of input and output;
- single-use, fresh authorization;
- missing or contradictory evidence never allows;
- delegated grants are attenuating and authentic;
- explained decisions.

**What v9r deliberately does not prove:**

- agent intent or reasoning quality;
- who produced a state (State vs Causality v0);
- containment of the agent;
- honesty of registered observers;
- independence of checks that execute the judged state.

## Open risks

| Risk | Status | Where |
|---|---|---|
| **observer completeness** | one observer that drops a file gives a false Allow; redundancy gives Blocked | Content-Addressed State v0 |
| **atomic capture** | no detection strategy certifies capture against a same-uid writer; the freezer only with a contained writer set; two observers in one decision read at different instants (wrong verdict not constructed) | Atomic Capture v0, Temporal v0 FA7 |
| **causality** | not provided; an isolating executor's receipt would be needed | State vs Causality v0 |
| **persistence / freshness** | a runtime restored from backup accepts a stale ledger; no unprivileged rollback-protected counter | Root of Trust v0 |
| **execution isolation** | the namespace owner (runtime uid) changed a sealed view, which was detected; the demo agent is unconfined | Snapshot Boundary v0, Controlled Domain v0 |

## Commit structure (proposed; not committed)

Ordered so that **every commit builds and its tests pass**. Files that
grew across several experiments (`capability.rs`, `snapshot.rs`,
`world_probe.py`, `lib.rs`) are committed whole, once. Splitting their
history would need hunk staging and would produce intermediate states
that never existed.

| # | Commit message | Files |
|---|---|---|
| 0 | *(no commit)* | `rm crates/v9r-core/tests/agent_transition.rs`; re-run the suite and expect 315 / 0 / 6 |
| 1 | `feat(capability): manifest worlds, delegation ledger, signed authority, object and snapshot identity` | `crates/v9r-core/src/{capability.rs, world_probe.py, delegation.rs, delegation/tests.rs, authority.rs, authority/tests.rs, object_identity.rs, snapshot.rs, lib.rs}`, `Cargo.toml`, `Cargo.lock`, `crates/v9r-core/Cargo.toml` |
| 2 | `test(capability): controlled domain, manifest, identity and boundary experiments` | `crates/v9r-core/tests/{controlled_domain.rs, capability_probe.py, capability_manifest.rs, object_identity.rs, content_identity.rs, snapshot_boundary.rs}` |
| 3 | `test: state vs causality; example: v9r demo` | `crates/v9r-core/tests/state_vs_causality.rs`, `crates/v9r-core/examples/v9r_demo.rs` |
| 4 | `docs(research): containment and capability reports` | `research/{CONTROLLED_DOMAIN_V0, CAPABILITY_INVENTORY_V0, CAPABILITY_MANIFEST_V0, CAPABILITY_COMPOSITION_DESIGN_V0, CAPABILITY_DELEGATION_V0, CAPABILITY_ROOT_OF_TRUST_V0, CAPABILITY_OBJECT_IDENTITY_V0, CAPABILITY_CONTENT_IDENTITY_V0, SNAPSHOT_CAPABILITY_BOUNDARY_V0}.md` |
| 5 | `docs(research): agent transition (historical), trust model, state vs causality, demo` | `research/{AGENT_TRANSITION_RUNTIME_DESIGN_V0, AGENT_TRANSITION_TRUST_MODEL_V0, STATE_VS_CAUSALITY_V0, V9R_DEMO_DESIGN_V0, V9R_DEMO_IMPLEMENTATION_V0}.md` |
| 6 | `docs(research): milestone, architecture overview, review package, freeze checklist` | `research/{V9R_RESEARCH_MILESTONE_V0, V9R_ARCHITECTURE_OVERVIEW_V0, V9R_EXTERNAL_REVIEW_PACKAGE_V0, V9R_REVIEW_FREEZE_CHECKLIST_V0}.md` |

After commit 6, tag the revision (e.g. `v9r-review-v0`) and write its
hash into the review package's opening line.

## Remaining blockers before external review

1. **Remove** `tests/agent_transition.rs` (step 0).
2. **Re-run the full suite** and record the result here; 315 / 0 / 6
   expected.
3. **Run `git status`** to confirm the file list above is complete. This
   checklist was written from the session's records. A file not listed
   here must be classified before committing.
4. **Commit** in the sequence above and **tag** the revision.
5. **Update the revision** in V9R_EXTERNAL_REVIEW_PACKAGE_V0 and in the
   overview's branch-state note, replacing "uncommitted since
   `8a49c4b`".
