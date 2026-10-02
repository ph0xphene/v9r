# v9r Release Audit v0

*An audit of the frozen `v9r-review-v1` and all post-freeze research,
as of 2026-10-02. It changes no code and no other document. It proposes
no further phase and designs nothing. Every finding names the file and
line it refers to.*

## 0. What was audited

**The frozen release:**

- Tag `v9r-review-v1` → commit `560d8da`, which is `HEAD` of
  `research/effect-runtime-v0`. Code commit `6a9c717`.
- `git diff 6a9c717 HEAD -- crates Cargo.toml Cargo.lock`: **empty**.
- `kernel.rs` sha256 `85badb669f50…6177f`, unchanged.
- No commit and no tag were made after the freeze.

**The working tree on top of it (uncommitted):**

| State | Files |
|---|---|
| modified | `README_REVIEWER.md`, `research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md`, `research/V9R_RELEASE_FREEZE_V1.md`: post-freeze wording corrections |
| new test | `crates/v9r-core/tests/self_referential_observers.rs` |
| new documents | `V9R_ADVERSARIAL_REVIEW_V0`, `V9R_CLAIM_REVISION_V0`, `V9R_SELF_REFERENTIAL_OBSERVERS_V0`, `V9R_OBSERVER_INDEPENDENCE_MODEL_V0`, `V9R_OBSERVER_CAPABILITY_BOUNDARY_V0`, `V9R_VERIFICATION_BOUNDARY_V0`, `V9R_RESEARCH_THESIS_V0` |

**Re-measured for this audit:**

```
cargo test --offline --workspace        (working tree, rustc 1.98.1)
→ 88 passed, 0 failed, 4 ignored
```

That is the tag's **86 passed, 4 ignored**, plus the 2 tests of
`self_referential_observers`. No `src/` file differs from the tag.

---

## 1. Is the final claim consistent with all measured falsifications?

**The final claim** is the one in V9R_VERIFICATION_BOUNDARY_V0 §1.2–§1.3,
repeated in V9R_RESEARCH_THESIS_V0 §3. Each measured falsification in
the record was checked against it:

| Measured falsification | Where | Contradicts the final claim? |
|---|---|---|
| a lone liar is believed | `a_trusted_liar_is_believed_alone_…` | **no.** The claim is relative to attestations. Faithfulness is a stated validity condition |
| a caching observer is believed | `stale_raw_content_attested_freshly_is_believed`, `stale_state_…` | **no.** Currency is a stated condition |
| an omitted file is invisible alone | `hidden_file_…_only_with_an_independent_observer` | **no.** Completeness is a stated condition. Clause 7 claims only unreadable entries and lost objects |
| a clean explanation for a stale provider | EVIDENCE_PROVENANCE_V0 | **no.** The claim does not say explanations verify |
| identical S1, different writers → identical decisions | `state_vs_causality` | **no.** This is clause 5 |
| X1: `sys.exit(0)` on import → Allow, `greet` absent | `x1_…` | **no.** Independence and adequacy are stated conditions |
| X4: write after capture, during the decision → stale S1 accepted, 40/40 | `x4_…` | **no.** The claim says "as captured at the after-snapshot" |
| detection strategies accept a torn snapshot | ATOMIC_CAPTURE_V0 | **no.** Untorn capture is in tier 2 |
| restored runtime accepts stale state | CAPABILITY_ROOT_OF_TRUST_V0 (archived) | **no.** Clause 6 is "that runtime process" |
| `chmod +x` outside scope allowed by the old source | V9R_DEBLOAT_PHASE1_REPORT_V0 §3.1 | **no.** Fixed before the freeze; `scope_entries` covers it |
| same-uid namespace owner changes a sealed view | SNAPSHOT_CAPABILITY_BOUNDARY_V0 (archived) | **no.** Not claimed |
| PRE facts cannot name the snapshot they were derived on | STATE_VS_CAUSALITY_V0 FA1 | **no.** Clause 1 is "on a fresh observation" |

**Answer: yes.** No measured falsification contradicts the final claim.
That is by construction rather than by luck. The final claim was
written after the falsifications and is stated relative to
attestations, with each falsified property moved into an explicit
validity condition.

**Two qualifications:**

1. **Clause 5 ("whoever wrote S1") rests on one setup.**
   `state_vs_causality` used observers that execute nothing from the
   workspace. With a self-referential observer, writer independence is
   not measured. V9R_OBSERVER_INDEPENDENCE_MODEL_V0 §6.3 cells 3–4
   define the test, and it was not run. Clause 5 is therefore measured
   for byte-comparing rules only.
2. **Clause 5's X4 support is by inspection, not assertion.**
   V9R_VERIFICATION_BOUNDARY_V0 §1.2 cites "X4's S1 = the honest
   control's S1, same decision".
   - The tree ids are equal and printed by the test.
   - That the **decisions** are equal up to counters was read from the
     printed reasons. The test asserts the verdict and acceptance, not a
     field-by-field comparison.
   - V9R_SELF_REFERENTIAL_OBSERVERS_V0 §5 says the same thing ("the
     decision is the honest control's decision, up to counter values").
     Accurate as observed, but weaker than the `state_vs_causality`
     comparison it resembles.

## 2. Are any statements stronger than the evidence?

**Yes.** Findings are grouped by severity:

- **High:** seen first by a reviewer, or contradicted by a measured
  test.
- **Medium:** contradicted by a code fact.
- **Low:** wording or bookkeeping.

### 2.1 High

| # | Where | Statement | Evidence against | Note |
|---|---|---|---|---|
| H1 | `crates/v9r-core/examples/v9r_demo.rs:185` | I3 label: `"tests pass on the result, run by v9r"` | X1 (measured) | **code**. A docs-only release cannot change it. README_REVIEWER now warns about it, the demo still prints it |
| H2 | `v9r_demo.rs:399` | `"{t} (run by v9r on S1)"` | X1 | code, as H1 |
| H3 | `v9r_demo.rs:417` | run 1 title `"honest fix"` | G11 (measured): v9r cannot judge the agent | code, as H1 |
| H4 | `v9r_demo.rs:503` | `"Every decision came from what v9r observed."` | X1: I3 came from a process S1 controlled | code, as H1 |
| H5 | `v9r_demo.rs:34` | policy line `required: tests pass on the result` | X1 | code, as H1 |
| H6 | `README.md:26` | "ALLOW (honest fix)"; "with the tests run by v9r" | G11, X1 | **not corrected**. README.md was not in the post-freeze correction scope |
| H7 | `README.md:54–55` | "The guarantees and their conditions are in V9R_PHASE1_EXTERNAL_REVIEW_V0" | that document's G1, G3, G4, G8 are uncorrected (H8) | points the first-time reader to the uncorrected wording |
| H8 | `V9R_PHASE1_EXTERNAL_REVIEW_V0.md:116–124` | G1 "verified evidence", condition "observers faithful" only; G8 "attributed to the effect" with no window; step 6 "S1 becomes the trusted state" (l. 29) | X1 (independence missing from G1's condition); X4 (window ends at the after-snapshot) | **README_REVIEWER lists this document first under "Where to start reading"** (l. 160), and it was not corrected |
| H9 | `V9R_REVIEW_CHECKLIST_V1.md:17` | "accepted only on verified evidence that satisfies the rules", shown by "run 1 (ALLOW)" | X1: run 1's mechanism also accepts a broken `greet` | |
| H10 | `V9R_REVIEW_CHECKLIST_V1.md:9–10` | "The demo and the State vs Causality test are enough to understand the central claim" | X1 and X4 are not visible in either. The demo's labels overstate I3 (H1–H4) | the checklist's own rule (l. 33–34) says to report this as a finding |

### 2.2 Medium (contradicted by a code fact, not a measurement)

| # | Where | Statement | Code fact |
|---|---|---|---|
| M1 | `V9R_RELEASE_FREEZE_V1.md` G3; `V9R_PHASE1_EXTERNAL_REVIEW_V0.md:119` | scope covers "file modes" | `fs_raw.rs:73`: only `100755` / `100644`, i.e. the owner-execute bit. Measured only for that bit (`scope_entries`). X3 (setgid, group/other bits) is predicted, but the narrowing does not need X3: the code fixes it |
| M2 | `V9R_RELEASE_FREEZE_V1.md` G4; `V9R_PHASE1_EXTERNAL_REVIEW_V0.md:120`; `V9R_EXTERNAL_REVIEW_PACKAGE_V0.md` §1 | "exact identities" / "exact content identities" | exact in git's sense: names, content, symlink targets, owner-execute bit (`fs_raw.rs:70–75`) |
| M3 | `V9R_RELEASE_FREEZE_V1.md` G6; `V9R_PHASE1_EXTERNAL_REVIEW_V0.md:122`; `README_REVIEWER.md` "After a rejection, v9r holds" | "holds the runtime" | per process: `temporal.rs:526` takes the current state as the baseline of a new runtime. The restart consequence (X5) is predicted only |
| M4 | `V9R_CLAIM_REVISION_V0.md:273` (final claim §7.1) | validity "only for observers that are faithful, current and independent" | omits **complete**, which `hidden_file_…` measured. The later model documents list all four |

M1–M3 were proposed in V9R_CLAIM_REVISION_V0 §1.1. They were left out
of the post-freeze corrections, because that pass was limited to
**measured** results (X1, X4). They are code facts, not predictions,
and can be corrected on the same basis as a measurement.

### 2.3 Low

| # | Where | Issue |
|---|---|---|
| L1 | `V9R_RESEARCH_THESIS_V0.md` §6, row "success on stale evidence" | says "solved", citing tree-keyed facts and G7. Accurate for evidence about **another version**. Stale **observers** (caching) are not solved, as §7 and §5.3 of the same document say. The row title is broader than its evidence |
| L2 | `V9R_ADVERSARIAL_REVIEW_V0.md` | still labels X1 and X4 "predicted, not run" throughout, with no status note. Correct as a record of predictions, but a reader reaching it first does not learn they were measured |
| L3 | `V9R_SELF_REFERENTIAL_OBSERVERS_V0.md`, header | measured with rustc 1.98.1; V9R_RELEASE_FREEZE_V1 says 1.98.0. Not a contradiction (different runs), but the freeze's reproduction section names one version |
| L4 | `README_REVIEWER.md`, header table | "86 passed" is the tag's count. A tree including the new test gives 88. The table row labels it correctly as the tag's; a reviewer running the working tree will see 88 |
| L5 | `V9R_DEMO_DESIGN_V0.md:63, 79` | "the test was run **by v9r** … and passed". A design document, historical, but it is the source of H1–H4 and is not marked superseded on this point |
| L6 | `V9R_VERIFICATION_BOUNDARY_V0.md` §7.1 | "would address" rows. They are labelled as following from archived measurements, and as "not measured in a v9r transition" / "never assembled for an observer". Correctly hedged, but they are the most forward-looking statements in the record. They should stay classified as future ideas (§3) |

### 2.4 Checked and found accurate

- README_REVIEWER "What v9r is" item 4, "What v9r is not", demo note,
  TCB row and limitations, as corrected post-freeze.
- V9R_RELEASE_FREEZE_V1: G1, G8, G9, §7, §8 rows and §9 Q2, as
  corrected.
- V9R_EXTERNAL_REVIEW_PACKAGE_V0 §1, §4, §5.3, §6, as corrected.
- V9R_CLAIM_REVISION_V0: every X1/X4 label, after replacement.
- V9R_OBSERVER_INDEPENDENCE_MODEL_V0 §5.1. The kernel ignores
  provenance in `check_fact` (`kernel.rs:491–521`) and de-duplicates by
  value: verified in code.
- V9R_ARCHITECTURE_OVERVIEW_V0 and V9R_EXTERNAL_REVIEW_PACKAGE_V0 carry
  "describes `v9r-review-v0`" banners. The overview's G1 row states its
  faithful-observer condition (l. 110).

## 3. Which documents define what

Classified by the role each document plays **for the current result**.
A document can carry more than one role; its primary role is given,
and secondary sections are named.

### 3.1 Implementation facts (what the code at `v9r-review-v1` is and does)

| Document | Scope |
|---|---|
| `README.md` | entry; subject to H6, H7 |
| `README_REVIEWER.md` | reviewer entry: identity, TCB, limitations; corrected post-freeze |
| `V9R_RELEASE_FREEZE_V1.md` | release identity, commit graph, removed/remaining modules, reproduction, guarantees G1–G12; subject to M1–M3 |
| `V9R_PHASE1_EXTERNAL_REVIEW_V0.md` | claim, guarantees with conditions, TCB; **uncorrected** (H8, M1–M3) |
| `V9R_REVIEW_CHECKLIST_V1.md` | how to check the claim; subject to H9, H10 |
| `V9R_PHASE1_CORE_AUDIT_V0.md`, `V9R_PUBLIC_SURFACE_AUDIT_V0.md` | module and public-item inventory of the v1 tree |
| `V9R_DEBLOAT_PHASE1_REPORT_V0.md`, `V9R_ARCHIVE_BOUNDARY_V0.md` | what was removed, how removal was made safe, what stays |
| `V9R_DEMO_IMPLEMENTATION_V0.md` | the demo as built |

### 3.2 Measured limits (passing tests or reproducible reports, on v1 code unless marked)

| Document | Limit |
|---|---|
| `V9R_SELF_REFERENTIAL_OBSERVERS_V0.md` | X1, X4 (test not in the tag) |
| `STATE_VS_CAUSALITY_V0.md` | G11, the writer is not an input; re-measured after debloat |
| `CONTENT_ADDRESSED_STATE_V0.md` | omitted file needs a second observer; identity completeness |
| `TEMPORAL_EVIDENCE_V0.md`, `EVIDENCE_PROVENANCE_V0.md` | caching observer believed; clean explanation of stale evidence |
| `ATOMIC_CAPTURE_V0.md` | capture is trusted without a freezer (its tests remain in v1, partly ignored) |
| `V9R_RELEASE_FREEZE_V1.md` §8 | the table of known falsifications, as corrected |
| `CONTROLLED_DOMAIN_V0.md` | archived-era, test still in the tree (ignored) |

### 3.3 Model documents (derived from measurements; no new measurement)

| Document | Role |
|---|---|
| `V9R_ADVERSARIAL_REVIEW_V0.md` | attack constructions; X2, X3, X5, G′ still **predictions** |
| `V9R_CLAIM_REVISION_V0.md` | proposed wording; partly applied |
| `V9R_OBSERVER_INDEPENDENCE_MODEL_V0.md` | the four observer properties; Fact/Within analysis |
| `V9R_OBSERVER_CAPABILITY_BOUNDARY_V0.md` | authority vs influence |
| `V9R_VERIFICATION_BOUNDARY_V0.md` | formal claim, layers, TCB tiers, assumptions |
| `V9R_RESEARCH_THESIS_V0.md` | the final thesis |

### 3.4 Historical experiments (measured on earlier architectures; code at `v9r-archive-v0`)

- **Runtime and evidence:** `INVARIANT_KERNEL_V0`, `EFFECT_RUNTIME_V0`,
  `EFFECT_RUNTIME_V1_BASELINE`, `EFFECT_RUNTIME_V1`,
  `GIT_EVIDENCE_DOMAIN_V0`, `EVIDENCE_GRAPH_V0`, `COMPOSITION_BASELINE`,
  `VERIFIABLE_OBSERVERS_V0`.
- **Capability work:** `CAPABILITY_INVENTORY_V0`,
  `CAPABILITY_MANIFEST_V0`, `CAPABILITY_DELEGATION_V0`,
  `CAPABILITY_ROOT_OF_TRUST_V0`, `CAPABILITY_OBJECT_IDENTITY_V0`,
  `CAPABILITY_CONTENT_IDENTITY_V0`, `SNAPSHOT_CAPABILITY_BOUNDARY_V0`.
- **The previous release and its process:** `V9R_RESEARCH_MILESTONE_V0`,
  `V9R_ARCHITECTURE_OVERVIEW_V0`, `V9R_EXTERNAL_REVIEW_PACKAGE_V0`
  (bannered, corrected), `V9R_REVIEW_FREEZE_CHECKLIST_V0`,
  `research/freeze.sh`, `V9R_DEBLOAT_PLAN_V0`, `V9R_DEBLOAT_CUT_MAP_V0`.

### 3.5 Future ideas (not built, not measured)

**Whole documents:**

- `AGENT_TRANSITION_RUNTIME_DESIGN_V0` (superseded);
- `AGENT_TRANSITION_TRUST_MODEL_V0` (design review);
- `CAPABILITY_COMPOSITION_DESIGN_V0`;
- `V9R_DEMO_DESIGN_V0` (design, implemented; see L5).

**Sections:**

- V9R_SELF_REFERENTIAL_OBSERVERS_V0 §9;
- V9R_VERIFICATION_BOUNDARY_V0 §7 (L6);
- V9R_OBSERVER_INDEPENDENCE_MODEL_V0 §6.3 (a defined experiment, not
  run);
- V9R_RELEASE_FREEZE_V1 §9 (open questions);
- every report's "Next falsification experiment".

## 4. The minimal reproducible artifact for an external reviewer

**Contents:** one repository revision, containing:

| Part | Why it is needed |
|---|---|
| `crates/v9r-core` at code commit `6a9c717` (unchanged library and kernel) | the thing under review |
| `crates/v9r-core/tests/self_referential_observers.rs` | the only executable form of X1 and X4. Without it, the corrected wording cites tests the reviewer cannot run |
| `README_REVIEWER.md` | entry |
| `V9R_VERIFICATION_BOUNDARY_V0.md` | the claim, its conditions, TCB, non-goals |
| `V9R_SELF_REFERENTIAL_OBSERVERS_V0.md`, `STATE_VS_CAUSALITY_V0.md` | the two boundary experiments on v1 code |
| `V9R_RELEASE_FREEZE_V1.md` | identity and reproduction commands |

Everything else is supporting or historical. `v9r-archive-v0` is needed
only to re-run archived results.

**Commands:**

```sh
cargo test --offline --workspace
#  → 88 passed, 0 failed, 4 ignored     (86 at the v1 tag, without the new test)
cargo test --offline -p v9r-core --test kernel_guard
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
cargo test --offline -p v9r-core --test self_referential_observers -- --nocapture --test-threads=1
cargo run  --offline -p v9r-core --example v9r_demo
```

**Requirements:** Linux, Rust 1.98, `python3` (demo, X1/X4), `git` (one
identity test). Run time: about one minute for the suite, under a second
for each named test.

**What the reviewer can then confirm without trusting any document:**

- the claim's clauses (tests per clause, Boundary §1.2);
- the five kept false Allows: liar, caching, omitted file, X1, X4;
- that the decision is identical for two writers of the same bytes;
- that the kernel is unchanged.

## 5. Should `v9r-review-v1.1` be the final research release?

**Yes, as the final research release of Phase 1, provided it is cut
with the conditions below. It does not exist yet.**

**For:**

- The code under review is unchanged and complete for the claim. No
  measured falsification contradicts the final claim (§1).
- The boundary is measured where it matters, and the remaining
  predictions (X2, X3, X5, G′) are labelled as such.
- The model documents end at a thesis that claims nothing beyond the
  measurements.
- Further work would be new research, not a correction of this result.

**Conditions:**

1. **Fix the High and Medium wording before tagging.**
   - **Docs:** H6–H10 and M1–M4. These are wording changes on the same
     basis as the post-freeze corrections: measurement (X1, X4, G11) or
     code fact (`fs_raw.rs:73`, `temporal.rs:526`).
   - **H8 must be corrected or replaced** as the first reading
     document: it is what README_REVIEWER sends the reviewer to.
2. **Decide the demo text (H1–H5) explicitly.** It is code, in an
   example, not `src/`. Two consistent options:
   - **(a)** v1.1 is strictly docs-plus-test. The demo keeps its
     labels, and every reviewer-facing document says so and points to
     X1, as README_REVIEWER now does.
   - **(b)** v1.1 changes the demo's printed strings only. The release
     notes then cannot say "no code changed". They must say "library
     and kernel unchanged; example strings and one test added".

   Either is consistent. Leaving it implicit is not.
3. **State the release delta exactly.** V9R_CLAIM_REVISION_V0 §7.2
   suggested that the message say the code is `6a9c717`, unchanged.
   With the new test, `git diff 6a9c717 v9r-review-v1.1 -- crates` is
   **not empty**. The accurate statement is:
   - `git diff 6a9c717 v9r-review-v1.1 -- crates/v9r-core/src Cargo.toml Cargo.lock` is empty;
   - one test file is added (plus example strings under option b);
   - the test count is 88 / 0 / 4.
4. **Add status notes to `V9R_ADVERSARIAL_REVIEW_V0`** (L2), and fix L1,
   so that no post-freeze document reads stronger than its successor.
5. **Keep `v9r-review-v1` as it is.** It is the release the corrections
   refer to. The tag and its documents must stay unchanged, so that the
   difference between v1 and v1.1 is the audit trail.

**Not conditions** (accepted as open, and stated as such):

- clause 5 with a self-referential observer (§1, qualification 1);
- G′, X2, X3, X5 unmeasured;
- the four-cell observer experiment unrun.

These are limits of the result, not defects in the release.

**If the conditions are not met:** v1.1 would ship a first-reading
document (H8), a checklist (H9, H10) and a top-level README (H6, H7)
that are each contradicted by a test in the same release. Then it
should not be called final.
