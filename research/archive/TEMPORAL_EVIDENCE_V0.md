# Temporal Evidence v0: can v9r judge state transitions?

Research question: **can invariants be stated as relations between
evidence observed before (t0) and after (t1) an effect, using the
evidence model and independent providers that already exist?**

Short answer:

- **Yes, with one new value form and one idiom.**
  - `kernel.rs` and `runtime.rs` are unchanged.
  - Providers still answer only "now" and know nothing of snapshots.
  - A domain-neutral temporal layer owns snapshot identity, pairing,
    consistency and freshness.
  - "Release operation is safe" composes git, filesystem and a
    third-party CI provider as transition invariants written outside
    v9r-core.
- **The experiment located the boundary precisely.**
  - The layer can bind attestations to *requests*, but not *values* to
    *the present*. A provider that keeps hidden state produces a false
    ALLOW, and nothing in the model can tell.
  - Two domains that share a substrate (git lives in the filesystem)
    cannot have independent transition invariants: the fs invariant must
    know where git writes.
  - A transition is judged by **what** changed, never by **who**
    changed it.

## Baseline

| | |
|---|---|
| commit | `2d41f0c` (evidence graph v0 report), clean tree |
| tests | **224 passing, 2 ignored** |
| `kernel.rs` | sha256 `85badb66…6177f` (unchanged since `bfebebe`) |

## Architecture

```text
                 trusted snapshot s_k ─────────── basis: watched keys' values (key@current)
                         │
   authorize ─▶ PRE ─────┤
                         ▼
   execute ──▶ snapshot s0 ── Actor.act() ── clock tick (done) ── snapshot s1
                 (2 reads)                                         (2 reads)
                         │                                              │
                         └──────────── TransitionReceipt ───────────────┘
                                              │
   POST:  transition.ordered        transition(s0, done, s1)@now = true     ◀─ attested by the temporal clock
          invariant obligations     key@s0 = v0, key@s1 = v1 (pins)          ◀─ snapshot evidence, relabeled
                                    Within(changed(v0, v1), scopes)          ◀─ derived from pinned values
                                    key'@now = …                             ◀─ live, via the registry
                                              │
                                    kernel::evaluate (unchanged)
                                              │
                                    runtime: accept (trusted := s1) or hold

   Registry ──▶ git provider:  ref, refs (Map), descends, tree_content     (answers "now")
            ──▶ fs provider:   entries (Map), dir_content                   (answers "now")
            ──▶ FakeCi (test crate):  tests                                (answers "now")
```

### The abstraction

| Concept | Owner | Form |
|---|---|---|
| temporal subject | temporal layer | `TKey { at: Now \| Current \| Snapshot(id), key }` over the graph's `Key` |
| snapshot | temporal layer | id from one monotonic clock; evidence for the watched keys; no public constructor, no `Deserialize` |
| consistency | temporal layer | every snapshot reads every provider twice; a key whose readings differ keeps no value |
| pairing | temporal layer | `transition.ordered`: `before < done < after` on the clock, attested by the layer |
| freshness | temporal layer → runtime | basis = every watched key's verified value in the trusted snapshot |
| effect | `Actor` | knows nothing of snapshots or evidence |
| observation | providers | unchanged contract: answer keys about now |
| relation between t0 and t1 | invariant | **derive-and-pin** (below) |

### Derive-and-pin: relations with the existing kernel forms

The kernel compares a subject with a *given* value. It cannot compare
two unknown values. A transition invariant therefore:

1. reads the verified values of the two snapshots (`refs@s0 = M0`,
   `refs@s1 = M1`);
2. derives ordinary obligations from them:
   - `Within(changed(M0, M1), scopes)`;
   - `ref@s1 = expected`;
   - `descends(old, new)@now`;
   - `tests(resulting commit)@now`;
3. **pins** every value it read: `refs@s0 = M0` and `refs@s1 = M1` are
   hard obligations, so the kernel re-checks that the derivation inputs
   really were the snapshots' verified values.

If a value is missing, two things happen:

- its pin is undetermined;
- the derived names widen to `UnknownBelow`.

The decision is therefore Blocked, never a false Allow.

Helpers: `pin`, `changed`, `require`. All three are domain-neutral and
public.

## Results

**240 passing, 2 ignored**, which is 224 plus:

- 2 unit tests in `temporal.rs`;
- 13 integration tests in `tests/temporal.rs`;
- 1 compile-fail doctest.

### "Release operation is safe"

Three invariants, 107 lines in the test crate, using only the public
API:

| Invariant | Requires |
|---|---|
| `release.refs_transition` (git) | `changed(refs@s0, refs@s1) ⊆ {repo/refs/heads/release}`; `ref(release)@s1 = proposed commit`; `descends(ref(release)@s0, proposed)@now` |
| `release.artifact_placement` (fs, + git tree) | `changed(entries(.)@s0, entries(.)@s1) ⊆ {dist, repo/.git}`; `dir_content(dist)@now = D = tree_content(commit)@now` |
| `release.ci_for_result` (CI) | `tests(ref(release)@s1)@now = passed`: CI on the commit *actually* released, not the one proposed |

| Scenario | Verdict |
|---|---|
| honest release | **ALLOW**; all invariants and `transition.ordered` satisfied |
| effect also moves `main` | **DENY** (names `repo/refs/heads/main`); held |
| effect releases a different commit | **DENY**; CI undetermined for the commit actually released |
| effect writes `stray.txt` at the root | **DENY** (names `stray.txt`) |
| CI has no result for the resulting commit | **BLOCKED**; held: the effect happened and is not accepted |

### Adversarial

| Case | Verdict | Mechanism | Without it |
|---|---|---|---|
| **before replayed as after**, provider level: git replays its t0 `refs` attestation at t1 while the effect moved `main` | **BLOCKED**, held | attestations bound to their request (graph) | **false ALLOW** (verified by mutation) |
| before replayed as after, snapshot level: pairing `(s0, s0)`, `(s1, s0)`, or after taken before the effect ended | **DENY** | `transition.ordered` (unit test; no public API accepts a snapshot) | — |
| nothing observable after the effect | **BLOCKED** | pairing undetermined, not false | — |
| **providers observe different versions**: a concurrent writer moves `main` between the git and fs readings of s1 | **BLOCKED**; `refs` listed as inconsistent in s1 | two readings per snapshot | **DENY** (mutation). This is defensible here: the kept (second) reading saw the move in every provider. The test shows the incoherence is *detected*, not that one reading would have judged wrongly |
| **one provider unavailable after the effect** (fs removed during it) | **BLOCKED**, held; the release ref *did* move | missing evidence is unknown | — |
| **external modification between authorization and execution** | refused, held (drift); nothing ran | basis = watched values of the trusted snapshot | the effect runs on a state nobody authorized (mutation) |
| external modification *during* the effect (moves `main`) | **DENY**, held | attributed to the transition | — |
| external benign write inside allowed scope (`repo/.git/description`) | **ALLOW** | indistinguishable by design | — |
| **provider keeps hidden state**: attests its cached t0 `refs` *freshly* at t1 while `main` moved | **false ALLOW** alone; **BLOCKED** with an independent second git provider | none, except redundancy | — |

Each mutation listed in the table was applied and confirmed:

- disabling the double reading;
- disabling request binding;
- emptying the basis.

Each fails exactly the corresponding test.

## Evaluation

### 1. Can temporal invariants use the same evidence model?

**Yes.**

- **Unchanged:** the same kernel requirement forms (`Fact`, `Within`;
  no `AtMost` needed), the same evidence classes, the same registry,
  the same providers. `kernel.rs` and `runtime.rs` are unchanged.
- **Two additions:**
  - `Term::Map`: set-valued facts. "Nothing else changed" needs the
    *whole* table at both moments; point queries cannot express it.
    This is a value-vocabulary addition in `graph.rs` (+10 lines), not
    a kernel form.
  - **Derive-and-pin**, an idiom. It makes obligations depend on
    evidence, which the kernel's model says invariants should not do.
    The pins keep it sound: the kernel still decides whether the
    derivation inputs were verified. What the kernel does *not* check
    is the derivation itself (`changed` is trusted helper code). The
    `Within(touched)` of fs I1 already had this property; it is now a
    named, general pattern.

### 2. Does any provider need lifecycle knowledge?

**No.**

- Providers answer "now", and the layer tags their answers. Both
  in-crate providers only gained new *kinds* (`refs`, `entries`). The
  import guard from the graph experiment still passes.
- **But the contract has two obligations the layer cannot enforce:**
  - **Answer about the present.** Request binding catches a replayed
    *attestation*, but not a stale *value* attested freshly (the
    caching test is a false ALLOW). "Providers must not store hidden
    mutable state" is therefore a **trust assumption on registered
    providers**, not a property of the architecture. The only
    architectural defence is redundancy, which converts a stale answer
    into a contradiction (Blocked), never into the truth.
  - **Be complete for set-valued kinds.** `entries` and `refs` must
    either cover everything or not answer. The fs provider refuses if
    any part of the tree was unobservable. A provider that silently
    omits entries would make `changed` miss them, and the pins cannot
    detect omission.

### 3. Does the abstraction remain domain-neutral?

**Yes.**

- `temporal.rs` imports only `std`, `serde`, the kernel, the runtime
  and the graph. It names no domain concept; a test enforces both.
- Snapshot identity, the clock, double reading, pairing and the basis
  know only keys and terms.

There is one runtime-contract strain. `EffectDomain::observe()` takes
no proposal, so the watched keys are **static per invariant set**.

- Everything a transition compares must be watchable without knowing
  the proposal.
- The CI fact depends on the *resulting* commit, so it cannot be
  watched. It fits only because `tests(commit)` is timeless and can be
  asked live (`@now`).
- A transition invariant over a *mutable* fact whose key depends on the
  proposal would not fit without changing the runtime.

### 4. Does composition still work?

**Yes, but transition invariants of overlapping domains are not
independent.**

- **What works.** The three invariants never name a provider, and the
  CI provider is third-party.
- **The overlap.** `release.artifact_placement` must allow `repo/.git`,
  because the *git* effect writes there. Git state lives in the
  filesystem, so:
  - "only `dist/` changed" is false for every release;
  - the fs invariant has to know git's storage layout.
- **What it means.** The coupling is not in providers or the layer. It
  is in the **policy**, and it is forced by the physical overlap of the
  domains: the same grain conflict the git report found for versions
  (`.git/index` stat data). For current-state facts, overlapping
  domains composed freely. For transitions, every domain's footprint on
  a shared substrate must be accounted for in every other domain's
  "nothing else changed".

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | A temporal model needs timestamped evidence (`Fact(key, value, t)`) in the kernel | **False.** Moments as part of the subject (`key@sN`) suffice; the kernel is unchanged |
| 2 | Relations between t0 and t1 need a new requirement form (`Same(a, b)`) | **False here.** Derive-and-pin over the existing forms worked. The cost: derivations are trusted helper code, and invariants become evidence-dependent |
| 3 | Statelessness of providers can be enforced | **False.** Only attestation replay can be caught. A provider that remembers and re-attests is believed: a false ALLOW (caching test) |
| 4 | Snapshot-level replay is a realistic attack | **Mostly false.** No API accepts a snapshot, so pairing is correct by construction (and checked by `transition.ordered`). The real replay surface is a *provider* replaying its own earlier answers |
| 5 | Domains that compose for state facts compose for transitions | **False for overlapping domains.** "Only X changed" must name every other domain's footprint on the shared substrate (`repo/.git`) |
| 6 | A transition invariant can attribute changes to the effect | **False.** Snapshots show what changed, not who changed it. A concurrent external write is denied as the agent's (held), and a benign one in an allowed scope is accepted. Attribution needs exclusive access (locking), which is outside this model |
| 7 | One reading per snapshot is a snapshot | **False in principle.** Providers read at different instants, so one reading can combine values that never coexisted. The double reading *detects* a change visible across its two passes and blocks. It cannot detect a change that reverts in between (ABA), and it doubles the observation cost. The test shows detection only: with one reading, that case was judged DENY on a coherent later reading, which is not a wrong verdict. A case where one reading gives a *wrong* verdict was not constructed |
| 8 | Watches can depend on the proposal | **False with the current runtime contract** (`observe()` has no proposal). They are static; proposal-dependent facts must be timeless and asked live |

## Complexity

Code lines, counted without blanks, comments or tests:

| Piece | Lines |
|---|---|
| `temporal.rs` | 395 (vocabulary, snapshot, consistency, invariant trait, helpers, domain) |
| `graph.rs` | +10 (`Term::Map`) |
| providers | +97 inserted lines including docs, across `fs_provider.rs`, `git_provider.rs` and `git.rs` (`refs`, `entries`) |
| release invariants (test crate) | 107: `RefsTransition` 41, `ArtifactPlacement` 39, `CiForResult` 27 |

- **Snapshots.** Each snapshot reads every provider twice. The watched
  keys include `entries(.)`, a full tree map, so a snapshot is two full
  scans plus git spawns.
- **Per release.** Three such snapshots (baseline or trusted, t0, t1),
  plus live keys at POST, plus a full-map basis obligation.
- **Not measured or optimized**, by instruction. The obvious costs are
  linear in tree size and doubled by the consistency reading.
- **Trust surface.** Unchanged from the graph experiment (v9r-core plus
  every `Attesting` provider), plus one new assumption: providers
  report the present.

## Decision

**Temporal invariants fit the evidence model.**

- Providers stay lifecycle-free.
- The layer stays domain-neutral.
- Composition works.
- Kernel and runtime unchanged.

The real abstraction boundary is not in the code structure. It is in
three **semantic assumptions** the model cannot discharge by itself:

1. **Present-tense honesty of providers.** Statelessness is trusted, not
   enforced. The model can bind answers to requests, not to time.
2. **Disjoint footprints.** Transition invariants compose independently
   only for domains that do not share a substrate. Where they overlap,
   policy must encode one domain's footprint in another's "nothing else
   changed".
3. **Exclusive access during the transition.** Without it, changes are
   judged but not attributed.

None of these is fixable by another abstraction layer. They are
properties of what is observed. (1) needs provider attestation of
freshness, e.g. a challenge the provider must incorporate into a
present-tense observation. (2) needs a model of footprints. (3) needs
OS or transactional support.

## Next falsification experiment

**Footprints as first-class evidence.** Attack assumption 2, the one
that touches the architecture rather than the OS.

1. Let each provider declare, per kind, the substrate region its state
   occupies, as evidence. For example, the git provider answers
   `footprint(repo)` = `{repo/.git}` from its own observation.
2. Rewrite `release.artifact_placement` as "changed paths ⊆ allowed ∪
   footprints of the domains the operation is allowed to change", with
   no hard-coded `repo/.git`.

- **Supports the claim that domains compose for transitions:**
  - the fs invariant names no git layout;
  - adding a second git repo or a different VCS needs no change to the
    fs invariant;
  - every existing temporal test still distinguishes ALLOW, DENY and
    BLOCKED.
- **Falsifies it (the boundary is wrong):**
  - footprints cannot be stated as provider evidence without a provider
    knowing another domain's semantics; or
  - a footprint must be computed from the *effect* (lifecycle knowledge
    in a provider).

Still deferred until this is answered: CI history, external
integrations, freshness attestation (challenge-bound observations).

## Commits

| Commit | Content |
|---|---|
| `8eb0746` | `Term::Map`; git `refs`, fs `entries` |
| `716c772` | `temporal.rs`: snapshots, clock, consistency, pairing, basis, derive-and-pin helpers |
| `91eb6e0` | release-operation invariants and the adversarial suite |
