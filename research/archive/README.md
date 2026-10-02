# Archive: historical process records

**These documents are historical.** They record how v9r got to
`v9r-review-v1.1`: earlier experiments, design notes, audits,
checklists and release records. They are kept as written, so their
claims, counts and file names describe the revision they were written
against, not the current tree. Several describe architectures that no
longer exist in it. The current claim is in the [research root](../README.md).

Most code they measure was removed in Debloat Phase 1 and is at the
tag `v9r-archive-v0` (= `v9r-review-v0`, commit `da6d694`); see
[archive-boundary](../archive-boundary.md).

## Renamed documents

These documents refer to each other by their original names. Five of
them now live in the research root under new names:

| Original name | Now |
|---|---|
| `V9R_RESEARCH_THESIS_V0` | [thesis](../thesis.md) |
| `V9R_VERIFICATION_BOUNDARY_V0` | [verification-boundary](../verification-boundary.md) |
| `V9R_SELF_REFERENTIAL_OBSERVERS_V0` | [observer-boundary](../observer-boundary.md) |
| `STATE_VS_CAUSALITY_V0` | [causality](../causality.md) |
| `V9R_ARCHIVE_BOUNDARY_V0` | [archive-boundary](../archive-boundary.md) |

Every other name refers to a file in this directory.

## Experiments, in the order they were committed

Measured reports. Unless marked *in tree*, the code they measure is at
`v9r-archive-v0`.

| Document | Question |
|---|---|
| [EFFECT_RUNTIME_V0](EFFECT_RUNTIME_V0.md) | can v9r tell what a command returned from what the filesystem changed? |
| [INVARIANT_KERNEL_V0](INVARIANT_KERNEL_V0.md) | can verified facts feed a small, domain-free decision kernel? (the kernel is *in tree*) |
| [GIT_EVIDENCE_DOMAIN_V0](GIT_EVIDENCE_DOMAIN_V0.md) | can the kernel judge a second, non-filesystem domain? |
| [EFFECT_RUNTIME_V1_BASELINE](EFFECT_RUNTIME_V1_BASELINE.md) | note before the v1 refactor |
| [EFFECT_RUNTIME_V1](EFFECT_RUNTIME_V1.md) | one lifecycle, three domains |
| [COMPOSITION_BASELINE](COMPOSITION_BASELINE.md) | note before the evidence-graph experiment |
| [EVIDENCE_GRAPH_V0](EVIDENCE_GRAPH_V0.md) | can evidence domains compose without knowing each other? |
| [TEMPORAL_EVIDENCE_V0](TEMPORAL_EVIDENCE_V0.md) | can rules relate evidence before and after an effect? |
| [EVIDENCE_PROVENANCE_V0](EVIDENCE_PROVENANCE_V0.md) | can every fact carry why it should be trusted? |
| [VERIFIABLE_OBSERVERS_V0](VERIFIABLE_OBSERVERS_V0.md) | can observer conclusions be checked rather than trusted? |
| [CONTENT_ADDRESSED_STATE_V0](CONTENT_ADDRESSED_STATE_V0.md) | can observed state become a verifiable git object? (*in tree*: `content_addressed`) |
| [ATOMIC_CAPTURE_V0](ATOMIC_CAPTURE_V0.md) | can a snapshot be shown to represent a state that existed? (*in tree*: `atomic_capture`) |
| [CONTROLLED_DOMAIN_V0](CONTROLLED_DOMAIN_V0.md) | the minimum OS boundary for a runtime-owned execution domain (*in tree*: `controlled_domain`) |
| [CAPABILITY_INVENTORY_V0](CAPABILITY_INVENTORY_V0.md) | can an agent's complete authority be described before execution? |
| [CAPABILITY_MANIFEST_V0](CAPABILITY_MANIFEST_V0.md) | can a world built from a manifest be checked against it? |
| [CAPABILITY_OBJECT_IDENTITY_V0](CAPABILITY_OBJECT_IDENTITY_V0.md) | do grants bind to objects or to names? |
| [CAPABILITY_CONTENT_IDENTITY_V0](CAPABILITY_CONTENT_IDENTITY_V0.md) | can a read-only grant be bound to a snapshot? |
| [CAPABILITY_DELEGATION_V0](CAPABILITY_DELEGATION_V0.md) | can the unchanged kernel validate a delegation ledger? |
| [CAPABILITY_ROOT_OF_TRUST_V0](CAPABILITY_ROOT_OF_TRUST_V0.md) | can delegation authority be rooted in a verifiable identity? |
| [SNAPSHOT_CAPABILITY_BOUNDARY_V0](SNAPSHOT_CAPABILITY_BOUNDARY_V0.md) | can a snapshot view be isolated from the host's uid? |

## Design notes

Not measured, or only partly.

| Document | Subject |
|---|---|
| [AGENT_TRANSITION_RUNTIME_DESIGN_V0](AGENT_TRANSITION_RUNTIME_DESIGN_V0.md) | is the architecture enough for one autonomous agent transition? |
| [AGENT_TRANSITION_TRUST_MODEL_V0](AGENT_TRANSITION_TRUST_MODEL_V0.md) | the minimum evidence for causality between a transition and its state |
| [CAPABILITY_COMPOSITION_DESIGN_V0](CAPABILITY_COMPOSITION_DESIGN_V0.md) | delegation and multi-agent composition (design only) |
| [V9R_DEMO_DESIGN_V0](V9R_DEMO_DESIGN_V0.md) | the demo, as designed (its labels were corrected in v1.1) |
| [V9R_DEMO_IMPLEMENTATION_V0](V9R_DEMO_IMPLEMENTATION_V0.md) | the demo, as first implemented |

## Models and claim revision (v1 → v1.1)

| Document | Subject |
|---|---|
| [V9R_ADVERSARIAL_REVIEW_V0](V9R_ADVERSARIAL_REVIEW_V0.md) | skeptical review of `v9r-review-v1`; source of X1–X5 and G′ |
| [V9R_CLAIM_REVISION_V0](V9R_CLAIM_REVISION_V0.md) | the claim narrowed after that review |
| [V9R_OBSERVER_INDEPENDENCE_MODEL_V0](V9R_OBSERVER_INDEPENDENCE_MODEL_V0.md) | the four observer properties; summarized in [observer-boundary](../observer-boundary.md) §10 |
| [V9R_OBSERVER_CAPABILITY_BOUNDARY_V0](V9R_OBSERVER_CAPABILITY_BOUNDARY_V0.md) | authority vs influence; summarized in [observer-boundary](../observer-boundary.md) §10.4 |

## Debloat Phase 1

| Document | Subject |
|---|---|
| [V9R_DEBLOAT_PLAN_V0](V9R_DEBLOAT_PLAN_V0.md) | plan |
| [V9R_DEBLOAT_CUT_MAP_V0](V9R_DEBLOAT_CUT_MAP_V0.md) | dependency analysis before the cut |
| [V9R_DEBLOAT_PHASE1_REPORT_V0](V9R_DEBLOAT_PHASE1_REPORT_V0.md) | what was removed |
| [V9R_PHASE1_CORE_AUDIT_V0](V9R_PHASE1_CORE_AUDIT_V0.md) | audit of what remained |
| [V9R_PUBLIC_SURFACE_AUDIT_V0](V9R_PUBLIC_SURFACE_AUDIT_V0.md) | every public item of `v9r-core`, classified |

## Review and release records

| Document | Release |
|---|---|
| [V9R_RESEARCH_MILESTONE_V0](V9R_RESEARCH_MILESTONE_V0.md) | before `v9r-review-v0` |
| [V9R_ARCHITECTURE_OVERVIEW_V0](V9R_ARCHITECTURE_OVERVIEW_V0.md) | `v9r-review-v0`; lists the older documents by role |
| [V9R_EXTERNAL_REVIEW_PACKAGE_V0](V9R_EXTERNAL_REVIEW_PACKAGE_V0.md) | `v9r-review-v0` |
| [V9R_REVIEW_FREEZE_CHECKLIST_V0](V9R_REVIEW_FREEZE_CHECKLIST_V0.md), [freeze.sh](freeze.sh) | `v9r-review-v0` freeze. The script is a record: it stops because the tag exists, and it is not meant to be run again |
| [V9R_PHASE1_EXTERNAL_REVIEW_V0](V9R_PHASE1_EXTERNAL_REVIEW_V0.md) | `v9r-review-v1`; guarantees G1–G12 with their tests |
| [V9R_REVIEW_CHECKLIST_V1](V9R_REVIEW_CHECKLIST_V1.md) | `v9r-review-v1`, `v9r-review-v1.1`: the reviewer's checklist |
| [V9R_RELEASE_FREEZE_V1](V9R_RELEASE_FREEZE_V1.md) | `v9r-review-v1`; §0 is the v1.1 release notes |
| [V9R_RELEASE_AUDIT_V0](V9R_RELEASE_AUDIT_V0.md) | audit of `v9r-review-v1` and post-freeze research |
| [V9R_RELEASE_AUDIT_V1_1](V9R_RELEASE_AUDIT_V1_1.md) | pre-tag audit of `v9r-review-v1.1` |
| [V9R_RELEASE_FREEZE_V1_1](V9R_RELEASE_FREEZE_V1_1.md) | `v9r-review-v1.1` freeze record; the basis of [release](../release.md) |
