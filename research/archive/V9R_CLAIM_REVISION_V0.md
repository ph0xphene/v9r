# v9r Claim Revision v0

*A corrected statement of what v9r claims, after
V9R_ADVERSARIAL_REVIEW_V0. It covers `v9r-review-v1` (code commit
`6a9c717`). No code and no released document was changed; this
document proposes the wording.*

**Evidence labels:**

- **measured**: a passing test at `v9r-review-v1` shows it;
- **predicted**: derived from code read at specific lines, not run. This
  covers X2, X3, X5 and G′ from the adversarial review, which a tool
  sandbox prevented from running;
- **measured (post-freeze)**: X1 and X4, measured on the
  `v9r-review-v1` code by `tests/self_referential_observers.rs`
  (V9R_SELF_REFERENTIAL_OBSERVERS_V0). The test is not part of the
  `v9r-review-v1` tag. X4 was measured with **the actor's leftover
  background process** as the writer, not with the observer's execution
  of S1 (G′), which remains predicted;
- **stated**: an assumption, not tested.

## 1. What v9r actually proves

### 1.1 The guarantees, narrowed

Old wording is from V9R_RELEASE_FREEZE_V1 §6 (G1–G12), which
README_REVIEWER and V9R_PHASE1_EXTERNAL_REVIEW_V0 repeat. The v0
package's wording is quoted where it differs.

| # | Old wording | Why too broad | New wording | Support |
|---|---|---|---|---|
| G1 | "Accepted only if every declared rule is satisfied by **verified** evidence; an actor's claims never satisfy a rule." (v0: "an accepted state satisfied every declared rule on verified evidence") | "Verified" reads as "true". In the code it means *attested by a registered observer in reply to v9r's own request*. A lone liar's or a caching observer's attestations are "verified" | **S1 is accepted only if every declared rule is satisfied by attestations that registered observers gave in reply to v9r's own requests. Statements entered as claims never satisfy a rule. An attestation is as true as its observer.** | measured: kernel unit tests; `claims_instead_of_evidence_block`. The "as true as its observer" part: `a_trusted_liar_is_believed_alone_…`, `stale_raw_content_attested_freshly_is_believed` |
| G2 | "Missing or contradictory evidence → Blocked, never Allow." | Accurate, but silent on the converse: a contradiction also turns a true violation into Blocked | **Missing or contradictory attestations for a subject a rule names give Blocked, never Allow. A contradicting observer can therefore turn a Deny into Blocked, never into Allow.** | measured: `removing_any_observer_…`, `disagreeing_observers_block`, `observer_disappearing_…`, `observer_unavailable_after_effect_…`, `observers_reading_different_versions_block` |
| G3 | "Changes outside a declared scope are **denied and named**: files, symlinks, directories (empty ones too), **file modes**." (v0: "changes outside a declared scope are denied and named") | (a) "File modes" means the **owner-execute bit** only (`fs_raw.rs:73`). Group/other permissions, setuid/setgid/sticky, ownership, xattrs, ACLs and timestamps are invisible (X3, predicted). (b) Only changes present **when the after-snapshot is taken** are judged (X4 measured: a write landing after the after-snapshot, during the decision, was accepted 40/40; X2 predicted). (c) Only inside the observed root | **Within the observed root, a change outside the declared scope is denied and named if it is visible in names, file content, symlink targets, empty directories or a file's owner-execute bit, and is present when the after-snapshot is taken. Other metadata, and changes made after that moment, are not judged.** | measured: `scope_entries` (18 cases). Limit measured: X4 (`self_referential_observers`). Limits predicted: X2, X3 |
| G4 | "S0 and S1 have **exact identities**: the SHA-256 git tree id; `git` computes the same id; a claimed id is recomputed." (v0: "input and output states have exact content identities") | "Exact identity of a state" suggests the whole filesystem state. It is git's notion: names, content, symlink targets, owner-execute bit. It is also only as exact as the capture, which is trusted | **S0 and S1 are identified by the SHA-256 git tree id of the captured directory: names, content, symlink targets and owner-execute bits, the same id `git` computes. A claimed id is recomputed, not believed. The capture is assumed not to be torn by a concurrent writer.** | measured: `a_snapshot_is_the_git_tree_of_the_same_content`, `a_claimed_snapshot_id_is_recomputed_not_believed` |
| G5 | "An omitted subdirectory, an unreadable file, or a lost or altered stored object → identity incomplete (Blocked)." | The subdirectory check depends on the filesystem keeping directory link counts, and is skipped when `nlink < 2` (`snapshot.rs:349`) | **An unreadable file, or a lost or altered stored object, makes the identity incomplete (Blocked). On filesystems that keep directory link counts, so does an omitted subdirectory. An omitted file goes unnoticed unless a second observer lists it.** | measured: the four `content_addressed` tests named in the freeze, incl. `hidden_file_…_only_with_an_independent_observer` |
| G6 | "Authorization is single-use and **refused** if the state changed after it was granted; drift **holds** the runtime." | (a) Refusal covers changes visible in the watched keys between the authorization's snapshot and the fresh observation at `execute`. `authorize` itself does not observe afresh (`runtime.rs:416`). (b) "Holds" is per process: a restart that re-derives S0 from the current state accepts a rejected state (X5, predicted) | **An authorization can be used at most once, by the runtime that granted it. Execution is refused if any watched key differs, at the start of execution, from the snapshot the authorization was granted on. After a refusal for drift, or a rejected transition, that runtime authorizes nothing further. The hold lives in that process's memory only.** | measured: `artifact_changed_after_authorization_is_refused`, `external_modification_between_authorization_and_execution_refuses`; demo run 5. Restart limit predicted: X5 |
| G7 | "Replayed, forwarded, volunteered or anachronistic attestations are dropped." | Accurate, but "replay-proof" invites "fresh". Binding is to the **request**, not to the **present** | **An attestation counts only for the provider and request it was minted in, and only for a key that provider was asked. This does not make its value current: an observer that re-attests a cached value is believed.** | measured: the five G7 tests; the limit: `stale_state_attested_freshly_…` |
| G8 | "Anything written inside the window is attributed to the effect." | "The window" is not defined in the guarantee. It closes when the actor's `act` returns (`runtime.rs:485`), not when its processes end. Observers that run afterwards (POST evidence, `runtime.rs:510`) act on the live state after S1 is fixed | **The window runs from the fresh observation at the start of execution to the after-snapshot taken when the actor returns. Every change present in the after-snapshot is attributed to the effect, whoever wrote it. Changes after that moment are outside the transition, including changes by the actor's own background processes and by observers that execute code during the decision.** | measured: `external_modification_during_the_effect_is_attributed_to_it`. Limit measured: X4 (`self_referential_observers`: write during POST evidence, accepted S1 stale 40/40). Limit predicted: X2 |
| G9 | "Facts bind to the state they name." | A fact about tree T is *about* T, but if its observer executes T, then **T controls the fact** (X1, measured) | **A fact keyed by a tree id is evidence about that tree only. A result recorded for another tree does not count. Whether the observer producing it is independent of that tree is a deployment property, not checked.** | measured: `test_success_for_another_tree_does_not_count`. Limit measured: X1 (`self_referential_observers`: Allow with `greet` absent) |
| G10 | "Each rule's finding names its observer…" (v0: "every verdict is explained down to the observer and request of each fact") | Explanations explain; they do not verify. A stale provider gets a clean explanation (Provenance v0) | **Each finding names the observer (and, for provider facts, the request, round and snapshot) its value came from. An explanation records provenance; it does not make the fact true.** | measured: `release_is_explained_…`, `snapshot_evidence_carries_its_snapshot_in_lineage`, `claimed_state_is_shown_as_a_claim_…` |
| G11 | "Identical S1 from an authorized actor or an unauthorized writer → identical complete decisions." | Not too broad: this is the boundary, and it is measured | **unchanged.** Add: *identical up to counter values (snapshot ids, clock readings, lineage ids)* | measured: `state_vs_causality`, `identical_s1_gets_identical_decisions_whoever_wrote_it` |
| G12 | "Kernel unchanged since review." | Accurate | **unchanged** | measured: `kernel_guard` |

### 1.2 The narrowest positive claim

> **For one runtime process, from a host-approved S0:**
>
> - v9r refuses to start a transition whose watched state no longer
>   matches the authorization;
> - it accepts the state captured when the actor returns only if every
>   declared rule is satisfied by attestations its registered observers
>   gave in reply to its own requests;
> - otherwise it denies or blocks and accepts nothing further.
>
> The decision depends only on S0, the captured S1, the rules, the
> observers' attestations and the ordering of the window. **It does not
> depend on who wrote S1.**

## 2. What v9r explicitly does not prove

| Not proved | What this means concretely | Evidence |
|---|---|---|
| **independent execution** | v9r does not run anything independently of the judged state. An observer that executes S1 (the demo's test runner) is controlled by S1 | X1 (measured); G′, the runner's execution of S1 writing the live state (predicted); the demo's runner runs `python3 tests/test_greet.py` on a checkout of S1 and reports its exit status |
| **correctness of tests** | a rule "the test command exits 0 on S1" proves exactly that. Not that the test is adequate, not that it ran to completion, not that the code is correct | X1 (measured): `sys.exit(0)` on import passes the demo's test; the same module without the exit is denied |
| **agent containment** | the actor holds whatever authority its process has: it can write outside the scope and the observed root, spawn processes that outlive the window, or reach the verifier as the same uid | non-goal; X4 (measured: a process left by the actor rewrote the protected test during the decision, 40/40 accepted); X2 (predicted); Controlled Domain v0 measured what containment would take |
| **causal attribution** | Allow does not mean the authorized actor produced S1 | measured: G11 |
| **persistence across restart** | the approved S0, the trusted snapshot and the hold are in memory. A new runtime takes the current state as its baseline (`temporal.rs:526`) | X5 (predicted); stated limitation |
| **atomic filesystem observation** | a snapshot is a walk, read twice. A tree torn by a concurrent writer and read identically twice is accepted | stated; Atomic Capture v0 measured that only the cgroup freezer gave true snapshots |
| **protection against same-uid interference** | any process with the verifier's uid can write the workspace between or after observations, and could interfere with the verifier itself | stated; Snapshot Boundary v0 (archived): the same-uid namespace owner changed a sealed view, **detected, not prevented** |
| **metadata integrity** | ownership, group/other permission bits, setuid/setgid/sticky, xattrs, ACLs and timestamps are not part of the state | X3 (predicted); `fs_raw.rs:73` |
| **truth of observations** | a registered observer is believed | measured: liar and caching tests |

## 3. Demo claim review

The demo, as presented, reads: *"agent changes code, v9r checks tests and
accepts/rejects."* Its rule I3 is labelled **"tests pass on the result,
run by v9r"**, its run 1 is labelled **"honest fix → ALLOW"**, and it
ends with *"Every decision came from what v9r observed."*

**Which reading should the demo support?**

- **A. "v9r verifies that the resulting state satisfies declared
  invariants."** **Supported, with the observer caveat.** In every run,
  the verdict follows from I1 (the input was S0), I2 (only `src/`
  changed) and I3 (the test command's exit status on S1, as reported by
  a registered observer). That is all the decisions encode.
- **B. "v9r verifies that an agent successfully completed a task."**
  **Unsupported.** It needs three things v9r does not establish:
  1. **That the agent did it.** G11: an unauthorized writer of the same
     S1 gets the same decisions (measured).
  2. **That the task was completed.** "Completed" means *greet is
     correct*. The evidence is *the test process exited 0*. With a test
     that executes S1's code, S1 can produce that evidence without being
     correct (X1, measured).
  3. **That success is the state the agent leaves behind.** The decision
     covers the state captured when `act` returned. Later changes by the
     agent's processes, or by the test run itself, are not judged (X2,
     X4 measured; X2 predicted).

**So the demo must say A.** Concretely:

- I3's label becomes **"the protected test command exits 0 on a
  checkout of S1 (S1's code runs inside it, so this is not independent
  evidence)"**;
- run 1 is labelled by what the agent *did*, not by what v9r *proved*:
  "agent edits `src/greet.py`" instead of "honest fix";
- the closing line becomes **"Every decision came from what v9r's
  registered observers reported to v9r's own requests. None of it came
  from the agent's statements."**

The demo's DENY and BLOCKED runs and run 5 (REFUSED) remain accurate
as they are.

## 4. Observer model

### 4.1 Three categories

**Trusted observer.** Registered as `Trust::Attesting`; its attestations
are evidence. Assumptions it must meet:

- **faithful:** it reports what it observed;
- **complete:** it does not omit, as far as the rule needs;
- **current:** it observes now and does not re-attest cached values;
- **independent of the judged state:** nothing in S0 or S1 can change
  what it reports, except by being the thing observed.

v9r **enforces none of these**. It only binds each answer to the
request that asked for it (G7). A second independent observer turns a
violation of the first three into Blocked.

**Untrusted observer.** Registered as `Trust::ClaimsOnly`, or any source
outside the registry, such as the agent. Its answers enter as claims:
they are recorded and shown, and never satisfy a rule. Where a
verifier derives a fact from its raw data and can check that data
itself (content-addressed objects), the derived fact can still be
verified. Otherwise the rule is Blocked. **This category is enforced
by v9r** (measured: `claims_instead_of_evidence_block`; `content_addressed`
with the store registered claims-only).

**Self-referential observer.** An observer whose observation process
the observed state controls, typically by executing it: a test runner,
a linter that imports plugins, a build. Two sub-cases:

- **controls the report** (X1, measured): S1 decides what is reported about S1;
- **controls the world** (G′, predicted; the measured X4 used a
  different writer, the actor's leftover process): S1's code runs as the observer, with the
  observer's authority, during the decision. It can change the live
  state, other files, or the verifier.

In v9r today it is registered as a trusted observer, so its reports are
fully believed. Nothing in the evidence records that the state
controlled its own observation.

### 4.2 First-class concept or deployment assumption?

**Decision:**

- **now:** first-class in the claim and its documents;
- **later:** first-class in the code, but **after external review**,
  not in Phase 1 or before the reviewer's answer.

**For first-class in the claim, now:**

- The adversarial review's weakest assumption is exactly this
  distinction (§5 there). A claim that says "registered observers" while
  the flagship demo uses a self-referential one is misleading by
  omission.
- Naming the category costs nothing and changes no verdict. Every
  guarantee then says which observer kinds it holds for: G1 and G9 do
  not hold against a self-referential observer.

**Why not first-class in the code yet:**

- Typing it means a new evidence class or a provenance attribute that
  rules can require ("independent of the judged tree"). That changes
  what `Verified` means: a kernel change, against a freeze the review
  relies on.
- It also needs a way to *establish* independence (a runner outside the
  judged state's authority), which does not exist. A label without a
  mechanism would be a claim the code cannot back.
- Whether v9r should own this distinction at all, or leave it to the
  deployment, is the reviewer's first question (adversarial review §7.1).
  The model should not be changed before that answer.

**Until then it is a deployment assumption, stated as one:**

> *Every registered observer must be independent of the state it
> observes. The demo's test runner is not. Its evidence about S1 is
> controlled by S1.*

## 5. Language replacements for the review package

The phrases are taken from README_REVIEWER, V9R_RELEASE_FREEZE_V1,
V9R_PHASE1_EXTERNAL_REVIEW_V0, V9R_EXTERNAL_REVIEW_PACKAGE_V0 and the
demo's output, as noted.

| Current phrase | Where | Problem | Replacement phrase |
|---|---|---|---|
| "tests pass on the result, run by v9r" | demo rule I3; freeze G-table via demo | "pass" implies correctness; "run by v9r" implies independence. The test executes S1's code (X1) | "the protected test command exits 0 on a checkout of S1. S1's code runs inside it, so this is not independent evidence" |
| "tests passed" / "tests: passed (run by v9r on S1)" | demo output | same | "test command exit status on S1: 0" |
| "honest fix → ALLOW" | demo, README_REVIEWER run table | "honest" judges the agent; v9r cannot (G11) | "agent edits `src/greet.py` → ALLOW" |
| "verified evidence" / "verified artifact" | G1; v0 package "on verified evidence"; earlier experiments ("artifact matches verified commit", archived) | "verified" reads as "shown true". It means "attested by a registered observer to v9r's own request" | "attested evidence (from a registered observer, bound to v9r's request)". For artifacts: "the artifact's tree id equals the approved one" |
| "secure transition" (and "safe", "trusted state") | not used verbatim in the v1 documents. "Trusted state" is: README_REVIEWER, runtime docs | implies protection v9r does not give: containment, metadata, after-window changes, same-uid interference | "an accepted transition: the captured S1 satisfied the declared rules on attested evidence". For the runtime's state: "the last accepted snapshot" |
| "agent execution" / "the agent acts" / "after the actor runs" | README_REVIEWER §What v9r is; v0 package §1 | implies v9r observes the execution. It observes the state before and after, and the actor's processes may outlive the window | "the actor's `act` call". For the observation: "the after-snapshot, taken when `act` returns" |
| "evidence" (unqualified) | throughout | conflates attested evidence, claims, and observer self-reports controlled by the judged state | "attested evidence" (trusted observer), "claims" (untrusted), "self-referential evidence" (observer controlled by the judged state). Never "evidence" alone in a guarantee |
| "changes outside a declared scope are denied and named" | v0 package; freeze G3 | metadata and after-window changes are invisible (X3, X2) | G3's new wording (§1.1) |
| "exact content identities" | v0 package; freeze G4 | git's notion, not the filesystem's | "git tree id: names, content, symlink targets, owner-execute bits" |
| "After a rejection, v9r holds" | README_REVIEWER | per process; a restart can launder (X5) | "after a rejection, that runtime process authorizes nothing further. Nothing persists the hold" |
| "Every decision came from what v9r observed" | demo closing line | v9r observes through registered observers, some self-referential | "Every decision came from what v9r's registered observers reported to v9r's own requests, never from the agent's statements" |
| "explained down to the observer and request of each fact" | v0 package | an explanation does not verify | "each finding names where its value came from; that does not make it true" |

## 6. Next experiment

**Choice: A. Measure X1 and X4 and confirm the demo limitation.**

- **Not B** (an independent test-runner observer). B is a fix. It would
  change the deployment model before the limitation is even measured,
  and before the reviewer says whether independence belongs to v9r
  (§4.2). It also needs a containment mechanism (a separate uid,
  namespaces, or a freezer) that Phase 1 removed from the tree on
  purpose.
- **Not C** (execution-domain receipts). C addresses causality (G11),
  which is a stated boundary, not a defect found by the review. It is
  the largest of the three and the least connected to the findings.
- **For A:**
  1. Every wording change in §5 rests on X1–X5, which are **predictions**.
     A measures the two that matter most: X1, the demo's I3 is satisfied
     by S1's own code; X4, the observer changes the live state after S1
     is fixed. That turns the revision's central correction into a
     measured result.
  2. It needs **no change to the verifier**: two scripted agents in the
     existing demo setup, kept as tests that **assert the false ALLOW**,
     like the liar and caching limits. The kernel freeze is untouched.
  3. Its failure would be as informative as its success: if v9r does
     not Allow X1, the adversarial review misread the code.

**Result of A (measured 2026-10-02, V9R_SELF_REFERENTIAL_OBSERVERS_V0):**

- **X1: Allow**, `greet` absent (independent check: incorrect).
  Controls: the honest fix Allow, the same module without `sys.exit(0)`
  Deny. The prediction held.
- **X4, as constructed** (the actor's leftover process rewrites the
  live protected test during POST evidence): **Allow and accepted
  40/40**; the write landed before `execute` returned 40/40; the live
  tree differed from the accepted S1 40/40. The prediction held for this
  writer. **G′** (the runner's execution of S1 performs the write) was
  not constructed and remains predicted.

**Scope of A, as originally planned:**

- X1 and X4 as tests asserting Allow, plus a check that `greet` is
  broken (X1) and that the live protected test changed (X4);
- X2, X3 and X5 run the same way, as one suite, so the whole revision is
  measured rather than predicted.

## 7. Conclusions

### 7.1 Final claim

> v9r decides whether to accept a state transition. A single-use
> authorization, granted on a host-approved state S0, is refused if the
> watched state has changed when execution starts. After the actor's
> call returns, v9r captures the resulting state S1, identified by its
> git tree id (names, content, symlink targets, owner-execute bits). It
> accepts S1 only if every declared rule is satisfied by attestations its
> registered observers gave in reply to its own requests; the actor's
> statements never count. Otherwise it denies, or blocks when evidence
> is missing or contradictory, and that runtime then accepts nothing
> further. The decision depends only on S0, S1, the rules, the
> attestations and the ordering of the window. It does not depend on who
> wrote S1, so **v9r verifies states, not histories**. Every guarantee
> holds only for observers that are faithful, complete, current and independent of
> the state they observe. v9r does not establish that independence, does
> not contain the actor, and does not persist its decisions across a
> restart.

### 7.2 Is Phase 1 still publishable?

**Yes for external review, after a documentation correction. No for
anything presented as a product.**

- **The code at `v9r-review-v1` stands:** no attack contradicts §1.2.
- **The released wording does not stand.** G1, G3, G4, G6, G8 and G9,
  the demo's I3 label and run-1 label, and "verified evidence" all
  overclaim (§1, §5).
- **What to publish:** `v9r-review-v1` stays as frozen. The corrected
  wording goes in a documentation-only commit on top, tagged as a review
  revision (e.g. `v9r-review-v1.1`). Its message states that the code is
  `6a9c717`, unchanged, and why the wording changed. Ideally Experiment
  A lands first, so the corrected wording cites measurements rather
  than predictions. *(Experiment A has run: X1 and X4 are measured;
  X2, X3, X5 and G′ remain predicted.)*

### 7.3 What must change in README_REVIEWER.md before external review

1. **"What v9r is":**
   - item 4: "It accepts S1 only if that evidence satisfies declared
     rules" → G1's new wording ("attestations its registered observers
     gave in reply to its own requests; an attestation is as true as its
     observer");
   - "After a rejection, v9r holds" → "that runtime process authorizes
     nothing further; the hold is not persisted";
   - "the actor runs" → "the actor's call returns", for the
     after-snapshot.
2. **"What v9r is not":** add four bullets, each with its X number:
   - not independent execution or test correctness (X1, X4);
   - not persistence across restart (X5);
   - not metadata integrity (X3);
   - not observation of changes after the actor returns (X2).
3. **Demo table:** run 1 "honest fix" → "agent edits `src/greet.py`".
   Add one line: *"The ALLOW means the captured S1 satisfied I1–I3. I3
   is the test command's exit status on S1; S1's code runs inside it, so
   I3 is self-referential evidence."*
4. **Trusted computing base:** split "every registered observer" into
   the three observer categories (§4.1). State that **the demo's test
   runner is self-referential**, and that it runs S1's code as the
   verifier's uid, unsandboxed.
5. **Known limitations:**
   - replace "Checks that run the judged code are not independent of
     it" with the self-referential observer paragraph (X1, measured),
     and the X4 limit (measured: the live state can change after the
     after-snapshot, during the decision, and the earlier S1 is
     accepted);
   - add "scope and identity cover names, content, symlink targets and
     the owner-execute bit only".
6. **Where to start reading:** add V9R_ADVERSARIAL_REVIEW_V0 and this
   document right after the review package. X1 and X4 are measured
   (V9R_SELF_REFERENTIAL_OBSERVERS_V0); mark X2, X3, X5 and G′ as
   predicted.
7. **The release table:** add the revision tag and its doc-only nature,
   once created.
