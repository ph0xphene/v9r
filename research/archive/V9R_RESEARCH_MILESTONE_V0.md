# v9r Research Milestone v0

Branch `research/effect-runtime-v0`. Last committed baseline: `8a49c4b`
(Atomic Capture v0 report). Everything after it is uncommitted.

## 0. Release checks

Run by the user on 2026-10-02:

```
cargo test --offline --workspace --no-fail-fast
```

| Check | Result |
|---|---|
| full workspace suite | **316 passed, 0 failed, 6 ignored**, across 29 test binaries. That is the previous full run (313, after Content Identity v0) plus `snapshot_boundary` (1), `state_vs_causality` (1) and `agent_transition` (1) |
| capability/snapshot changes vs existing runtime guarantees | **no regression.** All pre-existing suites pass unchanged in count: effects 21, git 20, guarded 20, graph 12, temporal 14, provenance 7, verifiable 12, counter 8, content-addressed 10, capability manifest 1, object identity 3, content identity 2. The lib tests (125) include the runtime and temporal domain-neutrality guards |
| `kernel.rs` unchanged | **passes:** the guard test `kernel_is_unchanged_and_knows_no_delegation` (sha256 `85badb66…6177f`) is among the 125 lib tests |
| `tests/agent_transition.rs` | **compiled and passed on its first run.** Its one assertion held: no case other than the honest ones was accepted. Its per-case verdicts were not captured (no `--nocapture`). This does **not** validate the hypothesis: its I4/I5 checks execute agent-controlled code (§6) |

## 1. Core claim

v9r is a **state transition verifier** for untrusted computation. An
agent proposes. The runtime authorizes once, against an approved and
content-identified input state. After the agent acts, the runtime
collects evidence itself, from registered observers. It accepts the
resulting state only if that evidence shows the state satisfies
declared invariants (scope, input identity, required checks, freshness),
and it records an explanation of every Allow, Deny or Blocked down to
who attested each fact.

v9r is **not** a sandbox, a model of agent reasoning, or a provenance
system. It verifies **states, not histories**: if an unauthorized actor
produces the same correct state, v9r accepts it too (measured, State vs
Causality v0).

## 2. Architecture

```text
 proposal (plain data, no authority)
    │
 runtime lifecycle (runtime.rs, temporal.rs)
    authorize: PRE on the trusted observation ─▶ single-use Authorization
    execute:   re-check PRE on a fresh observation (stale ⇒ refuse; drift ⇒ hold)
               actor acts ─▶ observe after ─▶ POST ─▶ accept (trusted := after) or hold
    │                                 ▲
 invariant kernel (kernel.rs)         │ obligations: Fact / Within / AtMost
    three-valued: Allow / Deny / Blocked; unknown is never true or false
    │                                 │
 evidence (graph.rs registry, verify.rs verifiers)
    providers attest per request; verifiers derive (e.g. snapshot = git tree id);
    lineage: what the registry established vs what the provider claimed
    │
 snapshots and content identity (fs_raw.rs, snapshot.rs)
    tree ids identical to git (SHA-256); materialize verified trees
    │
 capability layer (capability.rs, delegation.rs, authority.rs, object_identity.rs)
    worlds built from a manifest; grants with attenuation; signed ledger;
    object identity (st_dev + handle); snapshot grants realized by construction
    │
 decision records
    kernel Decision (findings with reasons) ─▶ journal (DecisionRecord);
    provenance::explain resolves lineage
```

| Layer | What it owns | Trust |
|---|---|---|
| proposal | what the agent wants | untrusted; enters only as `Proposed` |
| runtime lifecycle | when to observe, authorize, accept, hold | trusted; domain-neutral (test-enforced) |
| invariant kernel | the verdict | trusted; ~550 lines, `std` + `serde` only, unchanged since `bfebebe` |
| evidence providers | observations | partially trusted: faithful, complete, present |
| snapshots/content identity | identity of states | ingestion trusted; after ingestion, checkable by anyone |
| capability layer | who may hold what, and the worlds that realize it | ledger state, keys and construction code trusted; ledger copies untrusted |
| decision records | explanation | complete for verified provider facts; contradictions carry no provenance |

## 3. Proven results (measured, by report)

| Area | Result | Report |
|---|---|---|
| Invariant kernel | three requirement forms suffice for every invariant since; the kernel stayed byte-identical through every later milestone | Invariant Kernel v0; every later report |
| Multi-domain runtime | fs, git and counter share one lifecycle; 0 lifecycle sites in adapters; 10 divergences (D1–D10) resolved, one a hole | Effect Runtime v1 |
| Evidence composition | git, fs and third-party CI compose without cross-domain code; a missing provider gives Blocked, never Deny | Evidence Graph v0 |
| Temporal evidence | transitions judged with before/after snapshots and derive-and-pin; replayed answers Blocked; drift held | Temporal Evidence v0 |
| Provenance | every verified provider fact has lineage. A stale or copying provider still gives a **false ALLOW** with a consistent explanation | Evidence Provenance v0 |
| Verifiable observers | derived facts re-derived by trusted verifiers; content-addressed data needs no trusted observer | Verifiable Observers v0, Content-Addressed State v0 |
| Atomic capture | no detection strategy certifies a snapshot against a same-uid writer; the cgroup freezer gives 200/200 true snapshots only if writers cannot leave | Atomic Capture v0 |
| Controlled domain | for the 11 behaviours tested, minimum containment = cgroup subtree + cgroupns + mountns + netns; delegated spawning was the largest escape class. Untested routes are listed in the report | Controlled Domain v0 |
| Capability inventory | ambient authority cannot be enumerated by observation; namespaces do not drop groups, fds, env or harness tools | Capability Inventory v0 (partly unmeasured: probe blocked) |
| Capability manifest | world built from a manifest; observed ⊆ declared (Allow); a smuggled socket Denied; 9/9 predictions held | Capability Manifest v0 |
| Capability delegation | the ledger compiles to existing kernel forms; 22/22 malicious records Denied or Blocked; a lying issuer was **allowed** | Capability Delegation v0 |
| Root of trust | Ed25519 anchor; lying issuer now Blocked; copies verify without the runtime; a stale ledger Denied. A **restored runtime** and a **stolen key** still Allow | Capability Root of Trust v0 |
| Object identity | names wrong 3/6; `(dev, ino)` wrong 1/6 (ino reused 500/500 on ext4); `(dev, handle)` right 6/6; host-sourced identity wrong | Capability Object Identity v0 |
| Content identity | snapshot grants exact only when **materialized** (9/9); a live view verified against the tree id missed 2 changes | Capability Content Identity v0 |
| Snapshot boundary | sealed in-world tmpfs: B sees only S; host and `/proc` writes fail. The **namespace owner** (same uid, with caps) changed it, and it was **detected** | Snapshot Capability Boundary v0 |
| State vs causality | authorized agent and unauthorized writer of identical bytes: **every measured field identical** | State vs Causality v0 |
| Demo | five scripted runs: ALLOW, DENY (tests pass, test file edited), BLOCKED (claim only), DENY (runner present), REFUSED (drift); deterministic across two runs | V9R Demo Implementation v0 |

## 4. Explicit non-goals

v9r does not claim:

- **to understand agent reasoning.** Proposals and statements are never
  evidence.
- **to prove agent intent.** It judges results against declared rules.
- **to prove causality without an execution domain.** Who produced a
  state is not observed. An isolating executor could supply it as
  evidence, but none exists in v9r.
- **to replace sandboxing.** Containment is a separate layer: Controlled
  Domain v0 measured what it takes, and Snapshot Boundary v0 measured
  what remains for the same uid.
- **that its observers are honest.** A registered provider is believed.
  Redundancy turns a disagreement into Blocked, never into the truth.
- **independence of checks that execute the judged state.** A test that
  imports the code under test is part of the specification, not an
  independent observer.

## 5. Remaining open questions (architectural only)

1. **Observation binding within one decision.** Different providers
   observe the same state at different instants. Is a wrong verdict
   constructible from two readings of one snapshot? (Temporal FA7: not
   constructed.)
2. **Evidence independence.** How should v9r type facts whose producer
   executes the state being judged? Today they are `Verified` like any
   other provider fact.
3. **Compensation for transitions.** The temporal domain has no rollback.
   Where does "restore S0" belong?
4. **Freshness across restarts.** The ledger witness is in memory. A
   restored runtime accepts stale state, and no rollback-protected
   counter is available unprivileged.
5. **Structured provenance in decisions.** Explanations parse lineage
   tokens from text, and contradictions lose provenance. Fixing this
   would touch `kernel.rs`.

## 6. Documents since the baseline: review

| Document | Status | Notes from review |
|---|---|---|
| CONTROLLED_DOMAIN_V0 | measured | — |
| CAPABILITY_INVENTORY_V0 | **partly measured** | the probe was blocked mid-session; unmeasured rows are marked |
| CAPABILITY_MANIFEST_V0 | measured | two later corrections are not back-ported: submounts are **refused**, not trimmed (Object Identity v0); C2's `(dev, ino)` is weakened by inode reuse |
| CAPABILITY_COMPOSITION_DESIGN_V0 | design | R4's `(dev, ino, mnt_id)` was falsified by Object Identity v0 |
| CAPABILITY_DELEGATION_V0 | measured | one unexplained intermittent failure in `atomic_capture`, recorded |
| CAPABILITY_ROOT_OF_TRUST_V0 | measured | adds `ring` (already in `Cargo.lock`) |
| CAPABILITY_OBJECT_IDENTITY_V0 | measured | — |
| CAPABILITY_CONTENT_IDENTITY_V0 | measured | — |
| SNAPSHOT_CAPABILITY_BOUNDARY_V0 | measured (two user runs) | A5/A6 are printed but not asserted |
| AGENT_TRANSITION_RUNTIME_DESIGN_V0 | **design, superseded** | its I4/I5 evidence executes agent-controlled code; its PRE pin named the wrong snapshot (fixed in the test file). `tests/agent_transition.rs` now compiles and passes its single soundness assertion (§0); its per-case results are unrecorded |
| AGENT_TRANSITION_TRUST_MODEL_V0 | analysis | its §5 prediction was then measured by State vs Causality v0 |
| STATE_VS_CAUSALITY_V0 | measured | — |
| V9R_DEMO_DESIGN_V0 / IMPLEMENTATION_V0 | measured (two runs) | — |

**Decided (review freeze): remove `tests/agent_transition.rs`.**

- Its passing assertion ("no case except the honest ones was accepted")
  could be read as evidence about transitions or causality. State vs
  Causality v0 measured that v9r makes no such guarantee.
- It never recorded per-case results, so there is no historical
  measurement to keep.
- The design document stays, classified as historical.

No `AGENT_TRANSITION_RUNTIME_V0.md` (results report) exists.

## 7. Minimal next milestone

**External review.**

- **Not a release artifact first.** The tree is uncommitted, and the
  agent-transition question above is open.
- **Not a public demo first.** The demo exists and is deterministic.
  Publishing it before anyone outside has read the claims risks
  overstating them.
- **Not executor integration.** That is expansion, explicitly out of
  scope (§4).

Concretely:

1. Release checks (§0): **green.**
2. Commit the branch in reviewable commits, after the
   `agent_transition` decision above.
3. Give one external reviewer three things: this document, the demo
   command, and STATE_VS_CAUSALITY_V0.
4. Ask them one question: *does §1 match what the code and measurements
   show?*
