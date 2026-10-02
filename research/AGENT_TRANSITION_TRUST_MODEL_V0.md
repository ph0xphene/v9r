# Agent Transition Trust Model v0

Research question: **what is the minimum additional evidence needed to
establish causality between an authorized transition T and the
resulting state S1?**

> **Status: design review. No code was written or run for this
> document.** Results called *measured* come from earlier reports, named
> in each row. The Agent Transition experiment
> (`tests/agent_transition.rs`) has **never been compiled or run**.

## 1. Three kinds of validity

| Kind | Statement | Question it answers |
|---|---|---|
| **State validity** | S1 satisfies the invariants | *is the result acceptable?* |
| **Transition validity** | S1 was produced by the authorized computation, starting from S0, and by nothing else | *did the authorized thing make it?* |
| **Provenance validity** | each piece of evidence came from a trusted observer, about the state it names, now | *can the evidence be believed?* |

They are independent:

- A valid state can come from an unauthorized transition (someone else
  wrote it).
- An authorized transition can produce an invalid state.
- Either can be judged on evidence that was itself produced wrongly.

## 2. What previous experiments actually prove

| Kind | Support today | Measured in | Limit |
|---|---|---|---|
| State validity | **Yes**: content identity of S1 (git tree id); scope (`Within` over changed names); invariants in `Fact`/`Within`/`AtMost` | Content-Addressed State v0, Temporal Evidence v0, Content Identity v0 | only as good as the evidence (third row) |
| Transition validity | **Only the pairing**: `transition.ordered` (before < effect < after), the authorization's basis was current when it ran (drift ⇒ hold), and the authorization was single-use | Temporal Evidence v0, Effect Runtime v1 | **who produced the change is not observed** (Temporal FA6: an external write inside the window is attributed to the effect). Nothing links S1 to the agent's computation, only to the time window |
| Provenance validity | **Half**: who attested, in which request, round and snapshot, is *established*; how and from what state is *claimed* | Evidence Provenance v0; Root of Trust v0 for grants | stale or copying providers give a false ALLOW with a consistent explanation (measured) |

**What they do not prove:**

- that an accepted S1 was produced by the authorized agent rather than
  by any writer in the window;
- that a verification fact is independent of the state it verifies;
- anything about the agent runtime specifically, since that experiment
  was never run.

## 3. Current trusted computing base

A component is **trusted** if a fault in it can produce a false ALLOW
that nothing in v9r detects. It is **partially trusted** if it is
trusted for one narrow property, and redundancy or a check turns its
fault into Blocked or Deny. It is **untrusted** if everything it
produces is either checked or treated as a claim.

| Class | Component | Why it is in this class |
|---|---|---|
| **Trusted** | `kernel.rs` | it defines Allow, Deny and Blocked. A wrong evaluation is not detectable from inside |
| | runtime lifecycle (`runtime.rs`, `temporal.rs`) | it decides which obligations are evaluated, against which observation, and whether a result becomes the trusted state. Dropping an obligation is invisible to the kernel (Delegation v0 mutation matrix: every group was load-bearing) |
| | evidence verification (registry binding, verifiers, ledger signature checks) | it alone turns observations into `Verified` facts. A verifier bug is a forged fact |
| | observers, *for completeness and presence* | measured: one observer that drops a file, or reports a cached answer, gives a false ALLOW with a consistent explanation (Content-Addressed State v0, Temporal v0). Only redundancy helps, and it gives Blocked, not truth |
| | artifact ingestion (snapshot creation) | a snapshot is many calls over time; against a concurrent writer no detection strategy certified it (Atomic Capture v0). After ingestion, the snapshot is checkable by anyone |
| | external builders and checkers whose results become evidence | their result is taken as the fact. If they are wrong, or if what they compute depends on the state being judged, the fact is wrong |
| **Partially trusted** | providers registered `Attesting` | trusted to report faithfully and in the present; request binding, declared definitions and observers, and redundancy limit what a faulty one can do (Graph v0, Provenance v0) |
| | domain adapters (fs, git, temporal domain, invariants) | they define `basis`, `conclude` and the obligations. They are outside the kernel, reviewed as policy, and their output is still evaluated by the kernel |
| | execution environment (Linux, namespaces, the interpreter) | it contains the agent only as far as measured (Controlled Domain v0, Manifest v0), and not against the runtime's own uid (Snapshot Boundary v0) |
| **Untrusted** | agent proposal | plain data; carries no authority; it enters only as `Proposed` |
| | agent reasoning | never observed and never evidence |
| | generated code | it is part of the state being judged |
| | workspace modifications | they *are* the transition. They are observed, never reported |

**The point that matters for this review.** "Generated code" is
untrusted, but in the Agent Transition design, the checks that produce
I4/I5 evidence *run* parts of the state being judged. Whatever a
computation over agent-controlled input reports belongs on the
untrusted side unless that computation's result cannot be steered by
the input it processes. The current TCB table has no class for "trusted
executor, untrusted program". That is the gap the models below
address.

## 4. Candidate trust models

### 4.1 Transaction-only model

**Claim:** the runtime does not care how the agent reached S1. Only S1
and evidence about S1 matter.

- **What it proves:** state validity of S1 against the invariants;
  scope relative to S0; freshness; content identity of the accepted
  result. All four are already measured.
- **What it intentionally does not prove:**
  - who produced S1;
  - whether the agent did the work, or only arranged for a state that
    passes;
  - anything about intermediate states.
- **Relation to databases:** like a commit-time constraint check, but
  weaker. A database *owns* its state and serializes writers, so "who
  wrote it" is implied by isolation. v9r observes a state it does not
  own, so without isolation even "this transaction wrote it" is not
  given.
- **Relation to git:** git records *what* the tree is and who signed
  the commit, never how the tree was produced. This model is git plus
  rules on the change.
- **Its exact requirement:** the invariants must be **sufficient
  specifications**. Every evidence fact must be computed by something
  whose result the state cannot steer. A check that depends on the state
  it judges is a claim, not evidence.

### 4.2 Deterministic replay

**Claim:** the transition is valid because an independent execution of
the same computation from S0 reproduces S1.

- **Required assumptions:**
  - the computation is fully specified by recorded inputs;
  - every source of nondeterminism is recorded or removed: time,
    randomness, scheduling, network, environment;
  - the replayer is trusted and independent.
- **The nondeterminism problem:** an agent's reasoning is generally not
  replayable. Model sampling, external services and timing make its
  output a function of more than its recorded inputs. Replay therefore
  applies to **mechanical steps** (builds, formatters, code generators
  with pinned inputs), not to the agent's decision process.
- **Where it belongs:** outside the kernel. It is an evidence provider
  ("replay of step X from S0 yields tree id R"), and the comparison
  `R = S1` is an ordinary `Fact`. It fits the current architecture
  without kernel change. It establishes transition validity only for
  replayable steps.

### 4.3 Proof-carrying execution

**Claim:** the agent supplies evidence of its computation that a
verifier can check independently.

- **Required evidence format:** objects whose checking is cheaper than
  producing them and does not trust the producer. Examples:
  content-addressed artifacts with their derivation recipe, signed
  receipts from a trusted executor, or certificates a pure verifier
  checks.
- **Is the current model sufficient?** For the *checking*, yes:
  - the Verifiable Observers path already demotes provider-derived
    claims and re-derives them with trusted verifiers;
  - signatures and content addressing already make claims checkable
    (Root of Trust v0, Content-Addressed State v0).

  What is missing is a **certificate format for computation**. Today
  v9r can check derivations over data (tree ids, git objects), not
  "this program ran on that input".
- **Where trust moves:** from the agent to the verifier and to whatever
  issued the certificate. If the certificate comes from a trusted
  executor, this collapses into 4.2 or 4.4. If it comes from the agent
  alone, it is only as strong as the verifier's independent check.

### 4.4 Build provenance

**Claim:** the artifact is trusted because a trusted builder produced
it from S1.

- **Relation to reproducible builds:** the same idea, with one builder.
  Reproducible builds remove trust in any single builder by agreement
  between independent ones. v9r would use one builder as a provider,
  and could add a second for redundancy, which turns disagreement into
  Blocked.
- **Does it solve causality?** **Partly.** It establishes:

  > the artifact = build(S1)

  It does **not** establish:

  > S1 = the authorized agent's work

  It moves the causality question from the artifact to the source. It
  also requires that the build result is not steerable by S1 beyond the
  intended function. Compiling is processing S1's content; *executing*
  S1's code during the build lets S1 influence the builder's reported
  result.

### Summary

| Model | State validity | Transition validity | Fits current architecture | Kernel change |
|---|---|---|---|---|
| Transaction-only | yes | no, by design | yes (measured parts) | none |
| Deterministic replay | yes | for replayable steps only | yes, as a provider | none expected |
| Proof-carrying | yes | as strong as the certificate | checking yes; certificate format missing | none expected |
| Build provenance | yes, for artifact = build(S1) | no (moves the question to S1) | yes, as a provider | none expected |

None of the four establishes that the *agent's decisions* produced S1.
Only isolation can establish "nothing else produced S1": an execution
domain whose writer set is exactly the agent, with that domain
enforced. That property is outside evidence evaluation (Atomic Capture
v0, Controlled Domain v0).

## 5. Smallest falsification experiment

Distinguish:

- **H1:** v9r only needs state validity;
- **H2:** v9r needs transition causality.

**Experiment: two transitions, one result.** Using the existing
temporal runtime and invariants from the Agent Transition design:

1. **Run T_auth.** The authorized agent produces S1 from S0 inside the
   declared scope.
2. **Run T_other** from the same S0. The agent does nothing, and a
   different writer (an out-of-band process in the window, or a copy of
   an earlier accepted result) puts the **byte-identical** S1 in place.
3. **Record for both:** the verdict, the accepted tree id, the receipts,
   and the full decision explanation.

**Expected (prediction, not measured):** both are **ALLOW** with the
same S1, and the decision records differ only in the agent's own
result string, which is not evidence.

**Reading the result:**

- **Identical verdicts and S1:** v9r cannot distinguish them. That is
  the expected outcome. The question then becomes normative:
  - **under H1, it intentionally accepts:** the resulting state is
    exactly as valid, and nothing in the guarantee was violated;
  - **under H2, it should reject**, and that requires evidence the
    current model does not collect: an isolated execution domain whose
    writer set is the agent, and a receipt that binds S1 to that
    domain.
- **Different verdicts:** some existing evidence already carries
  causality. That would be a surprise, and the explanation would show
  which fact did it.

**A second, independent check:** repeat T_auth with I4/I5 evidence
produced by (a) a computation that executes S1's code and (b) a
computation that only reads S1's content. If (a) can be made to pass by
a state that fails the task while (b) cannot, then the evidence
discipline in 4.1 is required for H1 itself to hold.

## 6. Final decision

**What v9r claims:**

- an accepted state satisfies declared invariants on evidence from
  registered observers;
- it changed only within declared scope, starting from an approved,
  content-identified input;
- the authorization was fresh, single-use and authentically delegated;
- every verdict is explained down to who attested each fact.

**What v9r does not claim:**

- who or what produced the accepted state;
- that the agent's reasoning was correct or followed a process;
- that evidence produced by running the judged state is independent of
  it;
- containment of the agent.

**The strongest honest one-sentence description:**

> v9r is a state transition verifier: it accepts a new state only when
> independent evidence shows that it satisfies declared invariants
> relative to an approved prior state, and it explains every decision.

**Where causality belongs:** **outside v9r's kernel and evaluation
model**, as an input to it. Causality is a property of execution
(isolation of the writer set), which v9r can *record as evidence* when
an isolating executor provides it, but cannot establish by evaluation.

- The architecture already accepts such evidence: a provider or
  receipt, checked by `Fact`.
- Whether v9r should *require* it is a policy choice per deployment,
  which is H2.
- The experiment in §5 is what tells whether H1 alone is defensible for
  a given use.

On this analysis the answer to the A/B/C question is **A**, a state
transition verifier, with B and C reachable only by adding trusted
executors outside the kernel.