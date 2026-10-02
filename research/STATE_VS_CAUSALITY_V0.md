# State vs Causality v0: does v9r verify states or histories?

Research question: **if an unauthorized actor produces exactly the state
an authorized agent would have produced, does v9r tell them apart?**

Short answer: **No, and this is a property of the model, not a defect.**

- Both executions were accepted with identical verdicts, findings,
  observers, receipts, journals, input identity and output identity.
- The only differences were counter values: snapshot ids and lineage
  ids. They identify *when* and *which reading*, not *who*.
- v9r verifies states. It records no evidence about who caused a change.

**Decision: A, v9r intentionally verifies states, not histories.**

`kernel.rs`, `runtime.rs` and `temporal.rs` were not modified.

## Setup

`crates/v9r-core/tests/state_vs_causality.rs`, one test. Both worlds use
the unchanged `temporal::runtime`, the same registry kinds and the same
invariants.

| | |
|---|---|
| S0 | `ws/src/greet.py` (old body), `ws/README` |
| evidence | `RawFsObserver` (`fs_file`, raw transcription), `FilesystemEvidenceProvider` (`entries`), `FsSnapshot` (`snapshot`, git tree id), the temporal clock |
| I1 | PRE: `snapshot(ws)@current = S0`; POST: `snapshot(ws)@s_before = S0` |
| I2 | changed names between `entries(ws)` before and after ⊆ `ws/src` |
| I3 | `fs_file(ws/src/greet.py)@s_after` = the specified bytes. A byte comparison: **no code from the workspace is executed** |

The two worlds:

| World | Actor (the authorized party) | Who changed the file |
|---|---|---|
| A | writes the new `greet.py` | the agent |
| B | changes nothing; waits while an outsider runs | a separate thread, holding no authorization, writing the same bytes inside the transition window |

## Results

Measured on 2026-10-02; run by the user.

```
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
test state_vs_causality ... ok
```

| Measurement | World A | World B | Same? |
|---|---|---|---|
| final workspace bytes | — | — | **identical** |
| S0 (input tree id) | `b69da1dd…1497` | `b69da1dd…1497` | **same** |
| S1 (output tree id) | `8348b34c…fd5e` | `8348b34c…fd5e` | **same** |
| PRE verdict | Allow | Allow | **same** |
| POST verdict | Allow | Allow | **same** |
| transition accepted | true | true | **same** |
| runtime still accepting | true | true | **same** |
| POST findings (invariant, status) | 6 × satisfied: `transition.ordered`, I1, I2 ×3, I3 | identical | **same** |
| observers named in the reasons | `provider:fs`, `provider:raw`, `temporal-clock`, `verifier:fs-snapshot` | identical | **same** |
| `entries(ws)` before / after | `{3 entries, #4922c1b7…}` / `{3 entries, #9d4b3894…}` | identical | **same** |
| `greet.py` after | `[45 bytes, #5672b0da…]` | identical | **same** |
| transition receipt: agent result | `Ok("")` | `Ok("")` | **same** |
| `transition.ordered` | `transition(2, 3, 4)` = true | `transition(6, 7, 8)` = true | **same** (only the clock values differ) |
| decision journal | `[(Task, Allow), (Task, Allow)]` | identical | **same** |
| lineage ids | L43, L64, L65 | L117, L138, L139 | differ only as counters |

### What differed, and why it carries no causality

| Field | A | B | Meaning |
|---|---|---|---|
| snapshot ids | s2, s4 | s6, s8 | positions on the runtime's monotonic clock |
| lineage ids | L43… | L117… | registry counters for attestations |

Both are process-global counters. They establish ordering and binding:

- which reading;
- that before < effect < after;
- that an attestation answers this request.

Neither records which process wrote a byte. Within each world, the
pairing `transition(before, done, after)` was satisfied. It proves the
change happened **inside the window**, not **by the actor**.

## Answers

1. **Does v9r produce identical verdicts?** Yes, with identical
   findings, observers, receipts and journals.
2. **Documented as a fundamental property.** v9r judges a transition by
   the observed states at its two ends and by the time window between
   them. Earlier reports already noted this (Temporal Evidence v0 FA6:
   "a transition is judged by what changed, never by who changed it").
   This experiment measures it directly, with a byte-identical result.
3. **Which evidence would carry causal information?** None of the
   evidence v9r collects today. To distinguish A from B, a fact would
   have to bind the change to its writer, for example:
   - an execution domain whose writer set is provably only the actor
     (Controlled Domain v0: cgroup + namespaces, with freezing);
   - a receipt from that domain that the bytes were written by processes
     inside it.

   That evidence would come from an execution layer, and v9r could check
   it as an ordinary `Fact`. It does not exist in v9r now.

## Decision

**A: v9r intentionally verifies states, not histories.**

- **Not B.** B would mean v9r *requires* a causal execution layer.
  Nothing in its guarantees asks for one. Every guarantee it makes (scope,
  approved input, output identity, freshness, explanation) held for both
  worlds and was *true* for both: World B's resulting state really is in
  scope, really descends from S0 and really meets the specification.
  Causality is a separate property a deployment may want. If it does, it
  adds an execution layer whose receipts v9r evaluates.
- **Not C.** The architecture *can* express the distinction, as evidence
  supplied by an isolating executor and checked by the existing `Fact`
  form. What it lacks is the producer of that evidence, not a way to
  express it.

**What v9r claims:** an accepted state satisfies declared invariants
relative to an approved prior state, on evidence from registered
observers, within an ordered time window, under a fresh single-use
authorization.

**What v9r does not claim:** who or what produced the accepted state.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | A PRE obligation can name the snapshot it was derived on (`key@s_before`) | **False.** The first run stopped: World A's authorization was refused at execution. The runtime re-checks PRE on a *fresh* snapshot with a new id, so a pin to the old id had no evidence. PRE facts must name `@current`, as Temporal v0's basis does. The same mistake was in the unrun `tests/agent_transition.rs` and was corrected there too. The refusal itself was correct runtime behaviour |
| 2 | Lineage or snapshot ids might distinguish the worlds | **False.** They differ, but only as counters. They bind facts to readings, not to writers |

## What this does not show

- **Containment.** World B's outsider wrote the workspace freely. That is
  the intended setup, not a finding.
- **A causal layer.** No isolating executor was built. That B could be
  distinguished *with* one is an expectation based on Controlled Domain
  v0, not a measurement.
- **Verification that executes the judged state.** I3 compares bytes on
  purpose. Evidence produced by running workspace code would add the
  independence problem described in the trust model review.

## Reproducing

```
cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
```

This needs nothing beyond the crate. It takes under a second.
