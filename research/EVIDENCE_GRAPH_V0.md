# Evidence Graph v0: can evidence domains compose without knowing each other?

Research question: **is v9r a compositional invariant runtime, or a
generic lifecycle wrapped around hand-written domain integrations?**

Method:

- Put an evidence layer between invariants and observers. Invariants
  state the facts they need; the layer asks whichever registered
  providers can answer.
- Build a release invariant over three independent providers (git,
  filesystem, CI).
- Attack it.

Short answer:

- **For facts about state, yes.**
  - The release invariant composes git, filesystem and a third-party CI
    provider.
  - No provider knows any other provider, the lifecycle or the
    invariants.
  - `kernel.rs` and `runtime.rs` are unchanged.
  - The invariants and the CI provider are written outside `v9r-core`.
  - Removing a provider yields Blocked, never Deny.
- **The cross-domain knowledge did not vanish, it moved.**
  - It now lives in a shared, unchecked vocabulary: kind names,
    argument conventions and value normal forms.
  - `VERIFIED` now means "attested by a provider the host registered",
    which is weaker than before.
- **Effectful transitions were not composed.** The fs and git guards
  still compose by hand.

Verdict: **A, scoped to evidence about state.** Transitions are the
open question (see *Decision*).

## Baseline

Recorded in `COMPOSITION_BASELINE.md`:

- Commit `631ff46`: **211 passing, 2 ignored**; `kernel.rs` sha256
  `85badb66…6177f`.
- **Fifteen places (K1–K15) where git code knows about the filesystem.**
  - Twelve are in `git_guard.rs`: a hand-written sum vocabulary,
    hand-written evidence routing, a composite observation and basis,
    shared effects, fs facts named inside git invariants.
  - Three are below the adapter: the git observer reads fs
    observations, and the shared content normal form is typed by an fs
    structure.
- **110 of 635** code lines in `git_guard.rs` name fs or composite types
  directly.

## Architecture

```text
   invariants (outside v9r-core, over Key/Term only)
   ┌──────────────────┬───────────────────────────┬───────────────────┐
   │ commit_verified  │ artifact_matches          │ tests_passed      │
   │ ref(r, rel) = X  │ tree_content(r, X) = D    │ tests(r, X) =     │
   │ descends(r,B,X)  │ dir_content(A)     = D    │        "passed"   │
   │       = true     │ (D: witness in proposal)  │                   │
   └────────┬─────────┴─────────────┬─────────────┴─────────┬─────────┘
            ▼   obligations over keys (kernel forms, unchanged)
   ┌────────────────────────────── runtime.rs (unchanged) ──────────────────────────────┐
   │ authorize → re-evaluate on fresh evidence → "execute" → judge again → release output │
   └────────────────────────────────────────┬───────────────────────────────────────────┘
                                            │ GraphDomain (declaration domain: nothing runs;
                                            │ the accepted declaration is the output)
   ┌────────────────────────── graph.rs: Registry ──────────────────────────────────────┐
   │ for each provider: asked = keys it `answers`;  Attestor(provider, request)          │
   │ keep: attestations bound to this provider+request about asked keys                  │
   │ downgrade: ClaimsOnly providers → proposed;  discard: forwarded/replayed/unasked    │
   │ merge into one EvidenceBase<Key, Term> → kernel decides (contradiction → blocked)   │
   └──────────┬───────────────────────────┬───────────────────────────┬─────────────────┘
              ▼                           ▼                           ▼
   GitEvidenceProvider          FilesystemEvidenceProvider      FakeCi (tests/graph.rs,
   ref, descends, tree_content  dir_content                     outside v9r-core)
   (git.rs plumbing observer)   (fresh tree observation)        tests
              └──────────── content.rs normal form ───┘
                    (the one shared value contract)
```

### What each piece knows

| Piece | Knows | Does not know |
|---|---|---|
| invariant | kind names, argument order, value conventions, the witness | which provider answers; how |
| registry | which providers answer which keys; request binding | the lifecycle, invariants, any domain |
| provider | its own kinds and how to observe them | other providers, invariants, authorization, the lifecycle |
| runtime | the lifecycle | anything about keys or providers |
| kernel | evaluation | everything else |

Guards:

- `graph_depends_on_no_domain_module`: `graph.rs` uses only `std`,
  `serde`, the kernel and the runtime, and names no domain.
- `providers_know_nothing_of_each_other_the_lifecycle_or_invariants`:
  the providers import neither each other nor
  `runtime`/`kernel`/`policy`/`facts`/the guards/the graph's domain or
  registry.

## Before / after (release path)

| | Before: `GitGuard` release (G1 + G3) | After: graph release |
|---|---|---|
| composition vocabulary | `CrossSubject`/`CrossValue` sum, hand-written | open `Key`/`Term`; no per-pair type |
| evidence routing | hand-written in `GitDomain::evidence` (split by tag, map, merge) | registry: every provider that `answers` |
| cross-domain lines in providers/observers | git observer reads fs observations (K13) | **0** in both providers |
| cross-domain statement | G3 arms inside the git adapter, naming fs subjects | `ArtifactMatches`: 20 declarative lines in the invariant |
| where invariants live | inside v9r-core (closed enum) | anywhere: written in the test crate |
| third-party evidence | impossible (`Verified::attest` crate-private) | `FakeCi` lives outside v9r-core |
| CI requirement | not expressible | `tests(repo, commit) = "passed"` |
| missing observer | n/a (observers hard-wired) | Blocked, with a `Plan` naming unanswerable kinds |
| `kernel.rs` / `runtime.rs` | — | **unchanged** (sha256 `85badb66…`; empty diff since `631ff46`) |

### Code size

Counted without blanks, comments or tests:

| File | Lines |
|---|---|
| `graph.rs` | 367 (vocabulary, provider trait, attestor, registry, declaration domain) |
| `fs_provider.rs` | 45 |
| `git_provider.rs` | 68 |
| release invariants (test crate) | 77, of which `ArtifactMatches` (the only cross-domain statement) is 20 |
| `FakeCi` (test crate) | 46 |
| `git.rs` | refactored, not grown: `evidence` now loops over a new `answer` |
| shared normal form `content.rs` | 63, unchanged; both providers depend on it |

`GitGuard` is still there and still hand-composed; the graph path runs
alongside it. The comparison above is for the release path only.

## Results

### Phase 4: composite invariant

| Scenario | Verdict | Notes |
|---|---|---|
| all three providers, honest | **ALLOW** (PRE and POST) | each fact's provenance names the provider that answered it |
| git provider removed | **BLOCKED** | `commit_verified` and `artifact_matches` undetermined; plan: `ref`, `descends`, `tree_content` unanswerable; nothing violated |
| fs provider removed | **BLOCKED** | `artifact_matches` undetermined; plan: `dir_content` |
| CI provider removed | **BLOCKED** | `tests_passed` undetermined; plan: `tests` |
| CI ran and failed on this commit | **DENY** | verified negative evidence |
| commit does not descend from base | **DENY** | |
| artifact tampered | **DENY** | `dir_content ≠ D` |

### Phase 5: adversarial

| # | Attack | Verdict | Mechanism |
|---|---|---|---|
| 1 | git returns claims instead of attestations | **BLOCKED** | proposed evidence never satisfies a hard obligation (kernel) |
| 1 | git registered `ClaimsOnly` | **BLOCKED** | registry downgrades its attestations |
| 1b | a provider volunteers an attestation it was not asked for (`tests = passed`) | **BLOCKED** | discarded: not asked of this provider |
| 1c | an *attesting* provider lies (`descends = true` for a rogue commit) | **ALLOW** alone; **BLOCKED** once an independent git provider is registered | see failed assumption 3 |
| 2 | artifact changed after authorization | refused; **DENY** | execute re-collects; `dir_content ≠ D` |
| 3 | CI attests a pass for another commit | **BLOCKED** | discarded: the attested key was not asked |
| 3 | CI only has a result for another commit | **BLOCKED** | no answer |
| 4 | two CI providers disagree | **BLOCKED** | contradictory verified evidence (kernel) |
| 5 | release ref moves after authorization | refused; **DENY** | `ref = X` pin violated on fresh evidence |
| 5 | CI replays an attestation from an earlier request | refused; **BLOCKED** | discarded: attestation from another request |
| 6 | provider removed between authorization and execution | refused; **BLOCKED** | not held: nothing ran |

Mutation checks:

- Disabling request binding fails `stale_evidence_is_refused`.
- Disabling the asked-key check fails both
  `volunteered_attestations_are_discarded` and
  `ci_success_for_another_commit_does_not_count`.
- Disabling the claims-only downgrade fails
  `git_claims_instead_of_evidence_block`.

Tests: **224 passing, 2 ignored** (211 + the graph guard + 12 graph
tests).

## Evaluation

### Does `kernel.rs` remain unchanged?

**Yes**, byte for byte, and so does `runtime.rs`. No new requirement
forms were needed.

The graph uses one crate-private kernel affordance,
`Verified::attest`, and only inside `Attestor::attest`. That
affordance is what makes outside providers possible.

### How many lines of cross-domain code exist?

- **In code: zero** on the graph path.
  - Neither provider references another domain (checked by grep and by
    a source guard).
  - The registry and the runtime are domain-free.
- **In declarations: 20 lines.** `ArtifactMatches` states the only
  cross-domain relation, as two obligations sharing a witness.
- **In contracts: the vocabulary.** This is where the glue went:
  - kind names (`ref`, `descends`, `tree_content`, `dir_content`,
    `tests`);
  - argument order;
  - the text `"passed"`;
  - above all the **value normal form** of `Digest`. `tree_content`
    and `dir_content` are comparable only because both providers emit
    `content.rs` manifests rendered as `sha256:<hex>`.

  No type enforces any of this. A misspelled kind fails closed
  (Blocked, visible in the `Plan`). A provider that emits a different
  normal form for "the same content" makes every release Deny.

### Can a third-party provider be added outside v9r-core?

**Yes.** `FakeCi` and five adversarial providers live in the test crate
and use only the public API.

The price: **registration is the trust decision.**

- A registered `Attesting` provider produces `VERIFIED` evidence, and
  nothing checks that what it attests is true (test 1c).
- The registry *does* enforce:
  - attestations are bound to their provider and request (no forwarding
    or replay);
  - attestations cover only keys that provider was asked (no scope
    creep).

### Can an invariant be written without knowing implementation details?

**Yes, for providers. No, for vocabulary.**

- The release invariants never name a provider, an observer, a file
  path type or git plumbing.
- They must know the shared vocabulary: kind names, argument order,
  value conventions.
- They must also know that "same content" is decided by comparing two
  digests against a **witness** `D` from the proposal. The graph does
  not derive `artifact_matches(commit, artifact)` from its parts. An
  invariant can only require facts, not compute them.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | Composition needs an evidence *graph* (dependencies between facts) | **False for these invariants.** Flat routing (key → providers) sufficed; there are no edges. The honest name is an evidence *router*. Derived facts (a rule computing `artifact_matches` from two digests) would need the graph itself to attest, and was not needed |
| 2 | Removing cross-domain code removes cross-domain coupling | **False.** The coupling became a shared, untyped vocabulary and a shared value normal form (`content.rs`). It is declarative and auditable, but unchecked |
| 3 | Opening attestation to outside providers keeps `VERIFIED` meaning what it meant | **False.** Before, `VERIFIED` meant "established by a deterministic observer inside v9r-core". Now it means "attested by a provider the host registered". A single trusted liar is believed (1c). The only defence the graph offers is redundancy, and redundancy turns a lie into Blocked, never into the truth |
| 4 | Providers can be stateless answerers for every invariant | **Untested, and doubtful.** The release invariant is a *state* predicate, answerable "now". Transition invariants need a before/after diff of one domain around an effect, and a stateless provider has no "before". That is why `GitGuard` and `GuardedTask` were not migrated: G2 (protected refs unmoved) and I1 (observed writes declared) |
| 5 | Freshness carries over | **Partly.** There is no snapshot, so freshness is by re-evaluation (three collections per release: authorize, execute, POST). Providers observe at *different instants* within one decision, so a decision can mix states from before and after a concurrent change. Re-evaluation narrows that window; nothing closes it. `GitGuard`'s composite observation has the same gap, but pins a basis |
| 6 | "Remove a provider" is an unusual failure | **False.** Missing provider, misspelled kind, claims-only provider and misattributed answer all produce the same thing: an undetermined obligation, so Blocked. That is safe, but the operator needs the `Plan` and the discard log to tell them apart |

## Complexity analysis

- **Per decision.**
  - The cost is O(providers × keys) `answers` checks plus one `provide`
    call per provider that answers anything.
  - Each `provide` observes afresh: the fs provider scans its root; the
    git provider spawns plumbing processes.
  - A release costs three collections. Before, it took one composite
    observation per stage plus basis checks.
  - Not measured. The work is dominated by the same scans and spawns
    as before.
- **Per new domain.**
  - A provider is about 45–70 lines: `answers` plus `provide`, with no
    lifecycle.
  - An invariant that uses it is plain data over keys.
  - No sum type and no routing code.
- **Per new cross-domain relation.** Zero code, *if* the two providers
  already share a value normal form. Otherwise a normal form must be
  agreed and implemented by both sides. That is the real cost of
  composition, and it is not visible in line counts.
- **Trust surface.**
  - Before: v9r-core.
  - After: v9r-core plus every provider registered as `Attesting`.

## Decision

**A, scoped to evidence about state: the evidence graph works and
domains compose.**

- **Why not B** ("composition requires domain-specific glue"):
  - No provider, the registry and the runtime contain no cross-domain
    code.
  - The one cross-domain relation is a 20-line declaration in an
    invariant written outside the crate.
  - A third-party domain plugged in without touching v9r-core.
  - The remaining coupling is a **shared vocabulary**, not glue: it
    names facts and fixes a normal form; it does not route or translate.

  That said, B is the right reading if one counts value normal forms as
  glue. `content.rs` is exactly the per-relation agreement that
  composition cannot avoid, and nothing in the type system enforces it.
- **Why not C:**
  - The boundary (invariants require keys, providers answer keys, the
    kernel decides) held under all six attack classes.
  - It needed no change to the kernel or the runtime.
- **Why "scoped":**
  - Only state predicates on a declaration (release) were composed.
  - The effectful paths still compose fs and git by hand.
  - If transitions can only be expressed by providers that know about
    effects and snapshots, then the boundary is wrong for half of v9r's
    invariants. That would be C.

## Next falsification experiment

**Transition invariants over independent providers.** Re-express two
invariants on the graph:

- **G2**, protected refs unmoved by a command;
- **I1**, observed writes stay inside declared writes.

The effect itself still runs in an acting domain (the workspace). The
providers must keep knowing nothing of the lifecycle.

The crux is "before". Candidate shape: the runtime asks providers for
facts keyed by a *snapshot token* it owns, e.g.
`refs_digest(repo, ns) @ t0` and `@ t1`. Providers answer "the value at
the snapshot you named", without knowing why.

- **Supports A:**
  - G2 and I1 are expressible as obligations over keys with runtime-owned
    snapshot tokens.
  - Providers stay lifecycle-free.
  - The fs and git adapters lose their hand-written composition.
- **Falsifies A (→ C):**
  - providers must hold per-effect state or know the authorization; or
  - "unknown effects" (the `UnknownBelow` names fs receipts produce)
    cannot be expressed as answers to keys; or
  - the snapshot token forces a lifecycle concept into the provider
    API.

Secondary, if transitions compose:

- **Cross-provider snapshot consistency.** A decision whose providers
  observed different instants must be detectable.
- **Typed vocabulary.** Kind schemas that make a normal-form mismatch a
  registration error rather than a silent Deny.

CI-history invariants and external integrations stay deferred until the
transition question is answered.

## Commits

| Commit | Content |
|---|---|
| `77028af` | composition baseline (K1–K15) |
| `aee584f` | `graph.rs`: keys, providers, attestor, registry, declaration domain |
| `eb5ec22` | filesystem and git providers; `GitObserver::answer` refactor |
| `b53d960` | release invariant, third-party CI, provider-removal tests |
| `9f45bb6` | adversarial providers |
