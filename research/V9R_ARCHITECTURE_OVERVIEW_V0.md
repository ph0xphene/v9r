# v9r: Architecture Overview v0

*Read this first. Every claim below is backed by a measured experiment;
the reports are named in each row and listed at the end.*

## What v9r is

v9r is a **state transition verifier for untrusted computation**. An
agent (typically an LLM) proposes a change. v9r authorizes it once,
against an approved and content-identified starting state. After the
agent acts, v9r collects evidence about the result **itself**, from
registered observers, and accepts the new state only if that evidence
satisfies declared rules. Every decision is Allow, Deny or Blocked, and
is recorded with an explanation down to which observer attested which
fact. v9r judges **states, not histories**: it does not know, and does
not claim to know, who or what produced a state.

## Problem

An LLM agent is probabilistic. The same task can yield a correct fix, a
plausible-looking wrong one, or a change that quietly makes the check
pass, for example by editing the test. The agent's own account ("all
tests pass") comes from the same process that might be wrong. A
conventional pipeline that trusts the agent's report, or only re-runs
the tests, cannot tell "the work is done" from "the definition of done
was changed".

What is needed is **deterministic acceptance**:

- a fixed rule;
- evidence that does not come from the agent;
- a verdict that is the same every time for the same state.

The agent may be arbitrarily creative. The acceptance step must not be.

## Core model

```text
 Proposal ──▶ Authorization ──▶ Execution ──▶ Evidence ──▶ Decision ──▶ State transition
 (agent;       (once, against     (the agent    (collected by   (Allow /      (accepted state
  no authority) approved state S0) acts)          v9r, never by   Deny /        becomes the new
                                                  the agent)      Blocked)      trusted state)
```

- **Proposal:** plain data. It carries no authority, and the agent's
  statements enter only as *claims*, which can never satisfy a rule.
- **Authorization:** single-use, bound to the observed state S0. It is
  re-checked on a fresh observation immediately before execution: if
  the world changed, execution is refused.
- **Evidence:** facts about the state before and after, from
  observers v9r registered.
- **Decision:** the invariant kernel evaluates rules against the
  evidence.
  - Unknown is never treated as true or false. Missing evidence gives
    **Blocked**, not Allow.
  - Contradicting evidence gives **Deny**.
- **State transition:** on Allow, the observed result S1 becomes the
  trusted state. Otherwise the runtime **holds**, and accepts no further
  work on top of an unaccepted result.

## Guarantees

Only what experiments support:

| Guarantee | Holds under | Report |
|---|---|---|
| A transition is accepted only if every declared rule is satisfied by verified evidence. Agent claims never satisfy a rule | the kernel and runtime are correct, and registered observers are faithful (a lying observer is believed: Provenance v0) | Invariant Kernel v0, Effect Runtime v1, Demo v0 |
| Changes outside a declared scope are denied, and named | observers see the whole scope | Temporal Evidence v0, Demo v0 |
| The starting and resulting states have exact content identities (git-compatible SHA-256 tree ids) | after ingestion. Ingestion itself is trusted and not atomic against concurrent writers (Atomic Capture v0) | Content-Addressed State v0 |
| An authorization is used at most once, and is refused if the state changed after it was granted | the change is visible to the watched observations | Effect Runtime v1, Demo v0 (run 5) |
| Missing or contradictory evidence blocks; it never allows | — | Evidence Graph v0, Temporal Evidence v0 |
| Every decision is explained down to the observer, request and snapshot of each fact | for verified provider facts. A consistent explanation can still be wrong (stale provider: Provenance v0) | Evidence Provenance v0 |
| Delegated authority attenuates, is authentic, live and unexpired, and is checked with the same kernel | ledger verifier, anchor and witness trusted | Capability Delegation v0, Root of Trust v0 |
| A read-only snapshot grant gives exactly the granted content, when realized from the snapshot | at verification time | Content Identity v0, Snapshot Boundary v0 |
| The kernel is domain-free (~550 lines, `std` + `serde` only) and unchanged since commit `bfebebe` (sha256 `85badb66…`) | — | guard tests; full suite 316 passed, 0 failed |

## Non-goals

- **No proof of agent intent.** v9r judges results against rules, never
  what the agent meant.
- **No proof of reasoning quality.** The agent's reasoning is never
  observed and never evidence.
- **No causal attribution without execution receipts.** If someone else
  produces the same correct state, v9r accepts it too (State vs
  Causality v0). Attribution would need evidence from an isolating
  executor, which v9r does not have.
- **No sandbox replacement.** Containing the agent is a separate layer.
  v9r measured what that layer takes (Controlled Domain v0) but does not
  provide it.

## Architecture map

| Component | Code | What it does | Trust |
|---|---|---|---|
| **Kernel** | `kernel.rs` | evaluates obligations (`Fact`, `Within`, `AtMost`) against evidence. `Verified` evidence can be created only inside the crate | trusted, small, unchanged |
| **Runtime** | `runtime.rs`, `temporal.rs` | the lifecycle: authorize, re-check, execute, observe, decide, accept or hold. Before/after snapshots, read twice for consistency. Domain-neutral (test-enforced) | trusted |
| **Evidence providers** | `graph.rs`, `fs_provider.rs`, `fs_raw.rs`, git provider | observers answer typed keys; the registry binds every answer to its provider and request, and discards replayed or unasked answers | partially trusted: assumed faithful, complete and current |
| **Snapshots** | `snapshot.rs`, `verifiers.rs` | turn a directory into git objects and a tree id; anyone can verify them afterwards; materialize a verified tree | ingestion trusted, then checkable |
| **Capabilities** | `capability.rs`, `delegation.rs`, `authority.rs`, `object_identity.rs` | build an agent's world from a manifest; grants that attenuate, signed by a runtime anchor; object identity (`st_dev` + file handle); snapshot grants realized by construction | construction code, keys and witness trusted; ledger copies untrusted |
| **Provenance** | `graph.rs` lineage, `provenance.rs` | separates what the registry *established* (who, which request, which snapshot) from what a provider *claimed* (method, observed state) | explains; does not make a claim true |
| **Decision records** | kernel `Decision`, runtime journal | each verdict with a reason per rule, recorded | complete for verified facts; contradictions carry no provenance |

## Key experiments

| Experiment | Question | Result | Consequence |
|---|---|---|---|
| Invariant Kernel v0 | can a tiny, domain-free kernel decide every rule? | yes. Three requirement forms have sufficed for every later experiment | rules stay small and auditable |
| Effect Runtime v1 | is there one lifecycle for different domains? | fs, git and an in-memory counter share it. The counter found a defect shared by both real guards | the lifecycle is generic; domains supply only observation and rules |
| Evidence Graph v0 | do independent observers compose? | yes, without cross-domain code; a missing observer gives Blocked | new evidence sources plug in |
| Temporal Evidence v0 | can transitions (before/after) be judged? | yes. Replayed answers are blocked and drift is held. A concurrent outside write is attributed to the effect | first sign of the boundary: what changed, not who |
| Evidence Provenance v0 | does provenance make evidence trustworthy? | it explains, but a stale provider still gives a **false Allow** with a consistent explanation | observers stay in the trusted base |
| Content-Addressed State v0 / Atomic Capture v0 | can observed state become verifiable state? | yes, as git trees. Capturing a changing directory atomically needs an OS mechanism (freezer: 200/200) | identity is solid; capture is trusted |
| Capability Manifest v0 | can an agent's world be built and then checked against its declaration? | yes: observed ⊆ declared; a smuggled socket was denied | authority by construction, observation as the check |
| Capability Delegation v0 / Root of Trust v0 | can delegated authority be checked by the same kernel? | yes, 22/22 malicious records refused, and forged issuers blocked by signatures. A **stolen key** and a **restored runtime** still pass | delegation sits above the kernel; key custody and freshness are trusted |
| Object Identity v0 / Content Identity v0 | do grants bind to names, objects or content? | names were wrong in 3/6 cases; inode numbers are reused (500/500); file handles were right 6/6. Snapshot grants are exact only when built from the snapshot (9/9) | object grants and snapshot grants are two different things |
| Snapshot Boundary v0 | can a snapshot view be isolated from the host? | yes from the agent and from host writes. The namespace owner (same uid) could change it, and that was **detected**, not prevented | v9r detects; it does not replace OS isolation |
| **State vs Causality v0** (the boundary result) | does v9r distinguish an authorized agent from an unauthorized writer of identical bytes? | **no: every measured field was identical** (verdict, findings, observers, receipts, journal, both tree ids) | **v9r verifies states, not histories.** Attribution needs evidence from an execution layer, which v9r could check, but does not produce |
| Demo v0 | is the concept understandable in five minutes? | five deterministic runs: ALLOW, DENY (tests pass, test file edited), BLOCKED (claim only), DENY, REFUSED (state changed after approval) | `cargo run --example v9r_demo` |

## Current limitations

- **Observers are trusted** to be faithful, complete and current. One
  wrong observer can cause a false Allow; a second one turns that into
  Blocked, not into the truth.
- **Snapshot capture is trusted** while anything else can write the
  directory.
- **Checks that run the code being judged are not independent of it.**
  A test that imports the code under test is part of the specification;
  v9r currently records its result like any other verified fact.
- **Same-uid authority is not contained.** The runtime's own uid can
  change a sealed view (detected, not prevented).
- **Freshness across restarts:** a runtime restored from a backup
  accepts a stale ledger.
- **Transitions cannot be rolled back** in the generic temporal domain;
  a rejected transition holds the runtime.
- **Explanations** come from text in kernel findings; a contradiction's
  explanation names no provenance.
- **Everything is research code**, measured on one Linux host.

## Minimal future directions

1. **Public demo:** the existing deterministic demo, with this document.
2. **External review:** one reviewer checks whether "What v9r is"
   matches the code and the measurements.
3. **Optional execution receipts:** evidence from an isolating executor
   that binds a result to the computation that produced it, checked
   like any other fact. Only where a deployment needs attribution.

---

## Reading guide: research documents by role

**Foundational** (directly support the claim above):

| Document | Supports |
|---|---|
| INVARIANT_KERNEL_V0 | the kernel |
| EFFECT_RUNTIME_V1 | the generic lifecycle |
| EVIDENCE_GRAPH_V0 | composable observers |
| TEMPORAL_EVIDENCE_V0 | transition rules, freshness |
| EVIDENCE_PROVENANCE_V0 | explanations, and their limit |
| CONTENT_ADDRESSED_STATE_V0 | state identity |
| STATE_VS_CAUSALITY_V0 | the boundary of the claim |
| V9R_RESEARCH_MILESTONE_V0 | consolidated results and release checks |

**Implementation** (experiments and prototypes):

| Document | Content |
|---|---|
| VERIFIABLE_OBSERVERS_V0 | re-derivation of derived facts |
| GIT_EVIDENCE_DOMAIN_V0 | git as an evidence domain |
| ATOMIC_CAPTURE_V0 | snapshot capture against concurrent writers |
| CONTROLLED_DOMAIN_V0 | minimum OS containment |
| CAPABILITY_INVENTORY_V0 | ambient authority on one host (partly unmeasured) |
| CAPABILITY_MANIFEST_V0 | worlds built from manifests |
| CAPABILITY_DELEGATION_V0 | delegation above the kernel |
| CAPABILITY_ROOT_OF_TRUST_V0 | signed grants, untrusted ledgers |
| CAPABILITY_OBJECT_IDENTITY_V0 | object identity across worlds |
| CAPABILITY_CONTENT_IDENTITY_V0 | snapshot grants |
| SNAPSHOT_CAPABILITY_BOUNDARY_V0 | sealing a snapshot view |
| V9R_DEMO_DESIGN_V0, V9R_DEMO_IMPLEMENTATION_V0 | the demo |

**Historical** (superseded approaches or falsified assumptions):

| Document | Why historical |
|---|---|
| EFFECT_RUNTIME_V0 | superseded by Effect Runtime v1 |
| EFFECT_RUNTIME_V1_BASELINE, COMPOSITION_BASELINE | pre-change baselines |
| CAPABILITY_COMPOSITION_DESIGN_V0 | design; its `(dev, ino, mnt_id)` object identity was falsified by Object Identity v0 |
| AGENT_TRANSITION_RUNTIME_DESIGN_V0 | superseded: its verification evidence executes agent-controlled code |
| AGENT_TRANSITION_TRUST_MODEL_V0 | analysis whose prediction became State vs Causality v0 |

**Revision:**

- **Tag:** `v9r-review-v0` on `research/effect-runtime-v0`. Run
  `git rev-parse v9r-review-v0` for the hash.
- **Full suite at that revision:** 315 passed, 0 failed, 6 ignored.
- **`kernel.rs` hash guard:** passed (sha256 85badb66…6177f).
- **`tests/agent_transition.rs`:** removed at the freeze. It was
  superseded by State vs Causality v0, and keeping it suggested that v9r
  verifies transition causality. Its design is kept above as historical.

The classification of EFFECT_RUNTIME_V0, GIT_EVIDENCE_DOMAIN_V0 and the
two baselines comes from how later reports cite them; those four were
not re-read for this guide.
