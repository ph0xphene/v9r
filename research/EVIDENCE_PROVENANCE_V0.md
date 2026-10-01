# Evidence Provenance v0: can v9r know *why* evidence should be trusted?

Research question: **can every verified fact carry enough provenance to
explain who observed it, in which state, by what method, and from what
other facts, and does that make evidence compositional?**

Short answer: **provenance splits in two, and only one half can be
checked.**

- **The registry can establish** who attested a fact, in which request,
  collection round and snapshot, and that its claimed dependencies come
  from the same response.
- **Only the provider can claim** how the fact was established (method,
  definition) and from which state.
- **What the layer can enforce:** that facts do not combine across
  requests, snapshots, declared definitions or declared observers.
- **What the layer cannot check:** whether a claim is true.
- **The result:**
  - a stale provider, or one that copies evidence, produces a **false
    ALLOW with a consistent explanation**;
  - an undeclared definition mismatch produces a **wrong DENY**, which
    only the explanation exposes.

Verdict: **B**, *insufficient without trusted observers*, with
pressure towards C (see *Decision*). `kernel.rs` and `runtime.rs` are
unchanged.

## Baseline

| | |
|---|---|
| commit | `000facc` (temporal evidence report), clean tree |
| tests | **240 passing, 2 ignored** |
| `kernel.rs` | sha256 `85badb66…6177f`, unchanged since `bfebebe` |
| provenance before | kernel `Provenance { observer, basis }`, two free-text strings; graph attestations had `basis: String` |

## Design

### Lineage, split into established and claimed

```text
Lineage L<n>
  established by the registry ── id, provider, request, collection round,
                                 snapshot (assigned by the temporal layer
                                 for the reading it keeps), supporting?,
                                 depends_on (checked: same response)
  claimed by the provider ────── method name, definition (e.g. a digest
                                 normal form), observed state token(s)
```

- **Providers** build lineage with
  `attestor.attest(key, value, Method::new(..).defined_as(..)).observed(..).depends_on(&other)`.
- **They show their work.** A provider may return facts nobody asked
  for, as **supporting facts**, if an accepted answer depends on them.
  - git's `tree_content(X)` and `descends(…, X)` depend on a supporting
    `commit(X)`.
  - fs's `dir_content(d)` depends on a supporting `entries(d)` from the
    same tree walk.
- **The kernel is not changed.** The kernel's `Provenance.basis` carries
  a `lineage:L<n>` token, and the registry keeps the lineage store.
  `EvidenceBase::map`, `merge`, the runtime's `RtSubject` lifting and
  the temporal relabeling already preserve `Provenance`, so lineage
  survives composition without new plumbing.

### Registry rules

All rules are domain-neutral; the host declares them per kind.

| Rule | Effect |
|---|---|
| attestations bound to provider + request | carried over from the graph experiment |
| dependencies must be in the same response | discards facts derived from another request's or snapshot's evidence |
| unasked facts kept only as support | volunteering is still discarded |
| `define(kind, definition)` | answers claiming another (or no) definition are discarded |
| `restrict(kind, observers)` | answers from other providers are discarded |

### Explanation (`provenance.rs`)

`explain(decision, registry, snapshot_of)`:

- **Resolves the lineage tokens** in the kernel's finding reasons.
- **Follows dependencies.**
- **Separates the two halves**: established versus claimed.
- **Audits** the decision for:
  - provider evidence without resolvable lineage;
  - evidence for snapshot *n* whose lineage names another snapshot;
  - one invariant requiring the same value from facts under different
    definitions.

It imports only `std`, the kernel and the graph, and names no domain
(test-enforced).

## The explanation

"Release artifact is valid" means `commit(repo, X) = true` ∧
`tree_content(repo, X) = D` ∧ `dir_content(dist) = D` ∧
`tests(repo, X) = passed`. The invariants and the CI provider live
outside v9r-core.

Actual output of the POST decision (ids vary per run):

```text
ALLOW (Post) because:
  [ok] release.commit_exists: commit(repo, 8957b323…) = true
    evidence L84: commit(repo, 8957b323…) = true
      established: provider git · request 42 · round 19 · no snapshot
      claimed:     method `cat-file -e, cat-file -t` · observed object store of repo
  [ok] release.artifact_matches: tree_content(repo, 8957b323…) = sha256:a70cc77a…
    evidence L85: tree_content(repo, 8957b323…) = sha256:a70cc77a…
      established: provider git · request 42 · round 19 · no snapshot
      claimed:     method `ls-tree + cat-file` · definition v9r-content-manifest/1 · observed object store of repo
        └ depends on L86: commit(repo, 8957b323…) = true
          established: provider git · request 42 · round 19 · no snapshot
          claimed:     method `cat-file -e, cat-file -t` · observed object store of repo
  [ok] release.artifact_matches: dir_content(dist) = sha256:a70cc77a…
    evidence L88: dir_content(dist) = sha256:a70cc77a…
      established: provider fs · request 43 · round 19 · no snapshot
      claimed:     method `content manifest of the tree walk` · definition v9r-content-manifest/1 · observed tree sha256:67b5e4d4…
        └ depends on L87: entries(dist) = {2 entries, #696ecd95eb4cc53e}
          established: provider fs · request 43 · round 19 · no snapshot · supporting fact
          claimed:     method `tree walk (symlinks not followed)` · observed tree sha256:67b5e4d4…
  [ok] release.tests_passed: tests(repo, 8957b323…) = passed
    evidence L89: tests(repo, 8957b323…) = passed
      established: provider ci · request 44 · round 19 · no snapshot
      claimed:     method `pipeline result` · observed pipeline run #7 at 8957b323…
provenance: consistent
```

On the temporal path, the same explanation names snapshots. Every
`key@sN` fact's lineage records snapshot N
(`snapshot_evidence_carries_its_snapshot_in_lineage`).

## Attacks

Each case runs through the real runtime with an adversarial provider
written outside v9r-core.

| Attack | Without the declaration | With it | Mechanism |
|---|---|---|---|
| **provider claims evidence from another snapshot**: a fact `depends_on` a support kept from an earlier request | refused, **BLOCKED** (discarded: "depends on evidence outside this response") | — | dependency check (always on) |
| …or claims it in free text (`observed "snapshot s1"`) | accepted, **ALLOW**; explanation shows the claim beside the established round and "no snapshot" | — | none: claims are not checkable |
| **provider reuses stale evidence**: caches its first tree walk, attests it freshly (with a matching support) after the artifact is tampered | **false ALLOW**; explanation consistent, audit clean | same | none: see *Decision* |
| …by replaying an attestation object | discarded (graph experiment) | — | request binding |
| **two providers use different definitions of the same digest**: a `concat-sha256/1` provider as the only `dir_content` source | **wrong DENY** on a correct artifact; audit: "compares … under different definitions" | **BLOCKED** (discarded: definition mismatch) | `define` |
| …beside a conforming provider | **BLOCKED** (contradictory verified values); the kernel's reason carries **no provenance**, so the explanation has none | **ALLOW** on the conforming provider only | `define` |
| **evidence copied between domains**: CI also attests `dir_content` from a build log; artifact tampered; no fs provider | **false ALLOW**; explanation shows `dir_content` by `provider ci` | **BLOCKED** (discarded: not an observer of this kind) | `restrict` |
| …with the real fs provider too | — | **DENY** (tampering seen) | `restrict` |
| evidence moved between snapshots (decision relabeled `s0 → s1`) | audit names it | — | snapshot audit |

Mutation checks. Each of the following fails exactly the test aimed at
it:

- the dependency check;
- `define`;
- `restrict`;
- snapshot assignment;
- the snapshot audit;
- the definition audit.

Tests: **249 passing, 2 ignored**. That is 240 plus 7 in
`tests/provenance.rs`, 1 in `tests/temporal.rs` and 1 guard.

## Requirements

| # | Requirement | Met? |
|---|---|---|
| 1 | A verified fact must have lineage | **On the provider path, by construction**: every attestation gets an id, and the registry records it. **Not universally.** The runtime's own `Transitions` fact and the temporal clock's `transition.ordered` carry provenance text but no lineage record (shown as "established by …"). The pre-graph observers (`facts.rs`, `git.rs` on the guard paths) produce provenance strings only |
| 2 | Provenance must survive composition | **Yes.** It crosses `EvidenceBase::map` (runtime lifting, temporal relabeling), `merge`, and three decision stages. Two losses, both in the kernel's decision text: (a) on agreement the kernel reports only the *first* provider's provenance, so redundancy is invisible; (b) a contradiction finding names no provenance at all, so BLOCKED-by-disagreement cannot be explained by lineage |
| 3 | Incompatible provenance must not silently combine | **Only where declared.** Cross-request dependencies are always refused. Definitions and observers are refused *if the host declared them*. Undeclared, a definition mismatch combines and gives a wrong DENY that only the explanation's audit exposes. Snapshot pairing holds by construction, and the audit checks it after the fact |
| 4 | Providers remain unaware of invariants and lifecycle | **Yes.** They gained vocabulary (method, definition, observed state, supporting facts), not lifecycle. The import guard still passes |

## Evaluation

### Is provenance enough to make evidence compositional? (A)

**No.**

- **What provenance does.** It makes composition *explainable*, and it
  lets the layer refuse *structurally* inconsistent combinations:
  across requests, snapshots, declared definitions and declared
  observers.
- **What it cannot do.** It cannot make a combination *sound*.
- **Two attacks show this.** In the stale-evidence and copied-evidence
  attacks, every check provenance can make passes. The explanation is
  complete and consistent, and the verdict is a false ALLOW on a
  tampered artifact.

### Is it insufficient without trusted observers? (B)

**Yes. This is the main finding.** Each half of a lineage record has a
different trust basis:

- **Established half** (who, when, which request, which snapshot):
  verifiable, because the layer did it.
- **Claimed half** (how, from what state, under which definition):
  exactly as trustworthy as the provider.

Every attack that succeeded attacked the claimed half:

- a stale observation reported as current;
- a value copied rather than observed;
- a self-described snapshot.

The defences that work are **trust declarations about observers**,
which the host must make:

- `restrict` says *who* may observe a kind;
- `define` says *which method* counts;
- `Trust::ClaimsOnly`/`Attesting` (from the graph experiment) says
  whether a provider's attestations count at all.

Provenance makes these declarations enforceable and their violations
visible. It does not replace them.

### Is it forcing a new abstraction? (C)

**Partly, at two points.**

1. **A typed vocabulary.** `define`/`restrict` are a per-kind schema:
   a definition and the observers of each kind. That is a new
   abstraction (an ontology with trust annotations) that the earlier
   experiments had deferred. Without it, requirement 3 holds only after
   the fact, in the explanation.
2. **Structured evidence references in decisions.** The kernel's
   decision records provenance as text. The explainer recovers lineage
   by parsing `lineage:L<n>` from reasons, which works but is a text
   protocol. It also cannot recover provenance where the kernel drops
   it: contradictions, and agreeing redundant evidence. A
   `Finding { evidence: Vec<ProvenanceRef> }` would fix both, and needs
   `kernel.rs` to change. It was not changed here.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | Lineage is one kind of information | **False.** It is two: what the layer established, and what the provider claims. Only the first is evidence about the evidence |
| 2 | Recording dependencies shows facts are derived correctly | **False.** The registry can check that a dependency *exists in the same response*, not that the value *follows* from it. The stale provider showed consistent work |
| 3 | Incompatible definitions would be caught by provenance | **Only if declared.** Undeclared, they meet in the kernel, which compares values, and the result is a confident wrong DENY |
| 4 | An explanation that is consistent is correct | **False.** The stale and copied attacks produce consistent explanations for false ALLOWs |
| 5 | The kernel's provenance is enough to explain every verdict | **False.** It names one source per satisfied fact and none for contradictions |
| 6 | Snapshot provenance can be claimed by providers | **No, by design.** Providers never see snapshots; the layer assigns them. A provider's free-text "observed snapshot" stays a labelled claim |

## Complexity

Code lines, counted without blanks, comments or tests:

| Piece | Lines |
|---|---|
| `graph.rs` | 374 → 569 (`Method`, `Lineage`, store, kind rules, two-pass ingest) |
| `provenance.rs` | 233 (explainer and audit) |
| `git_provider.rs` | 147 (methods, observed state, supporting `commit`) |
| `fs_provider.rs` | 117 (supporting `entries` walk) |
| `temporal.rs` | +7 (assign the kept round to its snapshot) |
| `git.rs` | +36 (`IsCommit`) |

- **Lineage store.** It grows with every attestation and is never
  pruned. That is unbounded over a long task; acceptable for research,
  not for production.
- **Supporting facts** roughly double what a provider returns for
  derived kinds: `entries(d)` is a full map per `dir_content(d)`.

## Decision

**B: provenance is insufficient without trusted observers.**

- **What the experiment showed provenance is good for:**
  - *explaining* a composite verdict down to who observed what, in which
    round or snapshot, by which claimed method, from which supporting
    facts;
  - *refusing* combinations that are structurally inconsistent.
- **What it is not:** a substitute for trust. The established half of
  lineage is checkable but says nothing about truth; the half that
  speaks to truth (method, source state, definition) is the provider's
  word.
- **The C pressure.** The working defences needed a per-kind schema of
  definitions and observers (a new abstraction). A complete explanation
  needs structured provenance in kernel decisions (a kernel change, not
  made).

## Next falsification experiment

**Re-derivation: make the claimed half checkable for derived facts.**

The fs provider's `dir_content(d)` claims to be the content manifest of
the supporting `entries(d)`. Make that claim verifiable:

- register definitions as **executable** trusted functions, e.g.
  `v9r-content-manifest/1` maps an `entries` map to a digest;
- have the registry **recompute** every derived fact from its
  supporting facts, and refuse it on mismatch.

Trust then shrinks from "the provider derived correctly" to "the
provider observed the raw state correctly".

- **Supports the claim that provenance plus trusted derivation makes
  evidence compositional:**
  - a provider that lies about a derivation is caught;
  - the explanation marks each derived fact as *re-derived*, not merely
    claimed;
  - honest tests are unchanged.
- **Falsifies it:**
  - derivations cannot be recomputed from what providers can expose
    (e.g. git's tree manifest needs the object store, not a map), so
    re-derivation only works for providers whose raw observations are
    portable; or
  - the stale-observation attack is unaffected (expected: it attacks
    the raw observation). That would confirm that the residual trust is
    irreducibly in observers.

CI history and external integrations remain deferred.

## Commits

| Commit | Content |
|---|---|
| `ce2433c` | lineage for every attested fact; registry rules; providers report methods, state, supporting facts; git `commit` kind |
| `f21a08b` | `provenance.rs`: explanation and audit |
| `939f095` | release-artifact explanation and attack tests; temporal snapshot lineage |
