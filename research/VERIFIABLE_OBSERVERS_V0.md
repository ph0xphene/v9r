# Verifiable Observers v0: from trusted observers to verifiable ones

Research question: **can v9r move from trusting what observers *conclude*
to checking it, so that observer trust shrinks to raw observation, or
disappears?**

Method: separate three roles.

1. **Observation**: providers report raw data (bytes, listings, stored
   objects).
2. **Derivation**: trusted, deterministic verifier functions compute
   higher-level facts from that data.
3. **Verification**: only verifier output may become verified evidence.
   Each derivation states how it relied on every input, and the
   registry enforces the class.

The invariant is "artifact matches verified commit". `kernel.rs` and
`runtime.rs` are unchanged.

Short answer: **it depends on what is observed, and the split is
sharp.**

- **Derivations: trust reduced to raw observation (A).** A provider that
  lies about a derived digest, or derives it wrongly, is overruled.
- **Content-addressed data: trust reduced to zero (stronger than A).**
  Git objects are checked against the ids they were requested by. The
  git provider is registered as *untrusted*, a third-party object mirror
  outside v9r-core is safe, and forged objects never verify.
- **Mutable state: observer trust is unavoidable (B).** "What
  `dist/` contains now" is checkable only for its derivation. Its
  completeness (a file hidden from the listing) and its freshness
  (cached bytes attested anew) remain the observer's word. Both give a
  **false ALLOW**.

Verdict: **B**, with the boundary now located exactly. The
abstraction did not need a stronger formal model, but it needed one
explicit classification of inputs (see *Decision*).

## Baseline

| | |
|---|---|
| commit | `2010bf5` (evidence provenance report), clean tree |
| tests | **249 passing, 2 ignored** |
| `kernel.rs` | sha256 `85badb66…6177f`, unchanged since `bfebebe` |
| trust before | the git provider *claimed* `tree_content(X) = D`; the fs provider *claimed* `dir_content(dist) = D`; both trusted as `Attesting` |

## Design

### Roles

```text
 OBSERVATION (providers)                DERIVATION + VERIFICATION (trusted)
 ───────────────────────                ─────────────────────────────────────────────────
 git:  git_object(repo, oid) ─bytes─┐   GitObjects
                                    ├──▶  commit(repo, X)           hash(bytes) = X ?  ─┐
                                    │     tree_digest(repo, X, def) walk commit→trees→   │ SelfCertified
                                    │                               blobs, every object  │ (no observer)
                                    │                               checked by its id   ─┘
 fs:   fs_listing(dir)  ─names──────┤   FsContent
       fs_file(path)    ─bytes──────┼──▶  dir_digest(dir, def)      every listed entry   ── VouchedBy provider:fs
       fs_link(path)    ─target─────┘                               needs content           (observer trust)
                                        ContentEquality(def)
                                          content_equal(repo, X, dir) tree_digest = dir_digest ── VouchedBy verifier:*

 Registry: a derived fact is Verified (observer "verifier:<id>") only if no input was Unvouched;
           providers' answers for derived kinds are demoted to claims; contradicted claims are logged.
 Kernel:   unchanged; the invariant requires only commit(repo, X) = true ∧ content_equal(repo, X, dist) = true
```

### The contract (`verify.rs`, 44 lines, domain-free)

- **`Verifier::step(key, inputs)`** returns one of three steps:
  - `Need(keys)`: fetch these inputs and ask again;
  - `Derived { value, basis }`: the derivation, with how each input was
    relied on;
  - `Incomplete(reason)`: cannot be derived from what is available.
- **Inputs arrive as candidates.** Every candidate value comes with who
  vouched for it: `provider:<id>`, `verifier:<id>`, or nobody.
- **Basis per input**, as declared by the verifier:
  - `SelfCertified`: the verifier checked the input itself;
  - `VouchedBy(observer)`;
  - `Unvouched`.
- **The registry enforces the class:**
  - any `Unvouched` input makes the result a claim;
  - otherwise the result is verified, with provenance
    `"<n> self-certified input(s); trusts <observers>"`.

This removed the need for a witness. Earlier experiments compared two
unknown digests through a value `D` supplied in the proposal.
`content_equal` is now itself a derived fact, so the comparison moved
from "kernel + witness" into a trusted function.

### Definitions

The definition is part of the derived key: `tree_digest(repo, X, def)`,
`dir_digest(dir, def)`. Values under different definitions can
therefore never meet as the same fact.

Two definitions are implemented:

- the content manifest (`v9r-content-manifest/1`);
- a deliberately weak `names/1` (paths and kinds only).

## Results

**262 passing, 2 ignored.** That is 249 plus 12 in
`tests/verifiable.rs` and the `verify.rs` guard.

| # | Case | Verdict | What decided it |
|---|---|---|---|
| — | honest release, SHA-1 and SHA-256 repos, untrusted git provider | **ALLOW** | `tree_digest`: "self-certified; trusts no observer"; `dir_digest`: "trusts provider:fs". Both equal the digests the trusted observers used to claim |
| — | tampered artifact | **DENY** | `content_equal = false` |
| 1 | **provider lies about a derived digest** (claims `dir_digest`/`content_equal` to match; registered `Attesting`) | **DENY** | its answers are demoted to claims; the verifier derives the real digest; "derivation mismatch" logged |
| 1 | provider supplies **forged object bytes** (even registered `Attesting`) | **BLOCKED** | "no supplied bytes hash to `<oid>`" |
| 2 | **provider omits an input file's content** (listed, not supplied) | **BLOCKED** | "listed but no observation of `fs_file(…)`" |
| 2 | provider withholds a git blob | **BLOCKED** | "object `<oid>` not supplied": a tree names every entry, so nothing can be left out silently |
| 2 | provider hides a payload **from the listing itself** | **false ALLOW** | completeness of mutable state is the observer's word |
| 3 | **two definitions disagree** (manifest says unequal, names says equal, on a tampered artifact) | **BLOCKED** | contradictory verified facts |
| 3 | the weak definition alone | **false ALLOW** | definitions are trusted code |
| 4 | **stale raw observation replayed** (attestation from an earlier request) | refused, **BLOCKED** | request binding ("temporal verification failure") |
| 4 | stale raw content **re-attested freshly** after tampering | **false ALLOW** | freshness of an observation is not derivable |
| 5 | **valid raw data, wrong derivation** claimed by a provider | **ALLOW** | the verifier's own derivation is used; the provider's claim logged as a mismatch |
| — | **third-party object mirror** (test crate, plain `git cat-file`, untrusted) | **ALLOW** | every object self-certified |
| — | the same mirror forging blobs | **BLOCKED** | hash mismatch |
| — | **untrusted fs observer** | **BLOCKED** | `dir_digest` built on unvouched input stays a claim |

Mutation checks. Each change below was applied and confirmed to fail
the named tests:

| Mutation | Fails |
|---|---|
| no hash check | forged objects; third-party mirror |
| no demotion of provider-supplied derived facts | derived-digest liar; wrong derivation; forged objects |
| unvouched inputs treated as verified | untrusted fs observer |
| listed files need no content | omitted input file |
| tree entries need no blob | withheld object; forged-blob mirror |

## Measurements

### `kernel.rs` changes

**None.** It is byte-identical (sha256 `85badb66…`), and so is
`runtime.rs`.

The verifier layer uses the same crate-private affordance as before
(`Verified::attest`, inside the registry).

### Amount of trusted code

Code lines, counted without blanks, comments or tests:

| Component | Role | Lines |
|---|---|---|
| `verify.rs` | contract | 44 |
| `verifiers.rs` | git object checking and tree walking, fs content, equality, two definitions | 340 |
| `content.rs` | the manifest normal form | 64 |
| `graph.rs` | registry, verifier driving and class enforcement | 729 (was 569) |
| `sha1`, `sha2` | external crates, RustCrypto | — |

The verifiers are deterministic functions of their inputs. They run no
processes and read no files. Every test in `tests/verifiable.rs`
exercises them through the registry; they have no unit tests of their
own.

### What remains in the TCB

For "artifact matches verified commit":

| Fact | TCB before | TCB now |
|---|---|---|
| `commit(repo, X)`, content of X | kernel, runtime, registry, **git provider (169 lines), git observer `git.rs` (556 lines, much of it hardening), the `git` binary, `PATH`, repo config handling** | kernel, runtime, registry, **verifiers + SHA-1/SHA-256** |
| content of `dist/` | kernel, runtime, registry, fs provider, observation walk (`effect.rs`/`state.rs`), OS | **the same**, plus verifiers. The observer still decides *what exists* and *what it contained when* |
| `content_equal` | the witness `D` in the proposal, plus both observers | verifiers over the two derived facts |

The hardening the git report needed is no longer needed *for content*:

- forged ancestry through grafts or replace refs;
- `core.fsmonitor`;
- filter drivers.

Bytes that do not hash to the requested id are rejected whatever
produced them. It is still needed for anything git says about *mutable*
state (refs).

### Can third-party providers exist safely?

**Yes for self-certifying data; no for observations of mutable state.**

- **Self-certifying data.** A third-party object mirror outside v9r-core
  is safe while *untrusted*. The worst it can do is withhold, which
  gives BLOCKED.
- **Mutable state.** A third-party filesystem observer is safe only if
  the host trusts it. Untrusted, its observations cannot produce
  verified facts, so the verdict is BLOCKED.

## Evaluation

### A: can observer trust be reduced to raw observation only?

**Yes, for derivations.** Every attack on the *derivation* failed:

- a lying derived digest;
- a wrong derivation over correct data;
- forged objects;
- an omitted listed file.

The provider is now trusted for exactly one thing: the raw observation.
For content-addressed data, it is not trusted even for that.

### B: is some observer trust unavoidable?

**Yes, for mutable state. This is the main finding.** Three
properties of a raw observation are not derivable from the observation
itself:

1. **Completeness.** A listing that leaves out a file is
   indistinguishable from a directory without it (false ALLOW).
   Content-addressed trees avoid this because the parent object commits
   to its children; a directory does not.
2. **Freshness.** Bytes read earlier and attested now are
   indistinguishable from bytes read now (false ALLOW). Request binding
   catches *replayed attestations*, not *reused observations*.
3. **Correspondence.** That the bytes came from `dist/` on this machine,
   and not from somewhere else, is the observer's word.

Each needs either a trusted observer or a mechanism outside this model:

- a content-addressed or snapshotting store for the observed state;
- an OS that signs reads;
- making the state immutable and content-addressed before it is judged.

### C: does the abstraction require a stronger formal model?

**No stronger model was needed, but one explicit distinction was.**

- **What sufficed.** The existing kernel, the existing evidence classes,
  and a 44-line contract with three input bases (self-certified,
  vouched, unvouched).
- **What that distinction is.** Facts must be classified by
  *certifiability*:
  - **self-certifying**: content-addressed, checkable by anyone;
  - **observation-dependent**: mutable state, needs a trusted observer;
  - **derived**: inherits the weakest class of its inputs.

  The registry enforces the last rule mechanically.
- **What is still trusted.** Definitions: the weak `names/1` alone gave
  a false ALLOW. Verification moves trust from observers to verifiers.
  Verifiers are smaller, deterministic and testable, but they are not
  free.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | "Raw" is a fixed level | **False.** File hashes are already a derivation of bytes. Each level down moves trust from provider to verifier and costs data volume (all blobs were transferred here). The bottom is always "someone read reality" |
| 2 | All observer trust can be made verifiable | **False.** Only for content-addressed data. Mutable state keeps completeness, freshness and correspondence as observer trust |
| 3 | Absence can be verified like presence | **False.** A supplier can show an object exists (bytes hash to its id) but never that it does *not*: withholding looks like absence. `commit(X) = false` is therefore never derived from a missing object, only BLOCKED |
| 4 | Verification removes trust | **False.** It relocates trust into verifiers and definitions. A weak definition is believed (false ALLOW) |
| 5 | Witnesses are needed to relate two unknown values | **False with derived relations.** `content_equal` is a derived fact; the proposal needs no digest |

## Decision

**B: some observer trust is unavoidable, and it is exactly the trust in
observations of mutable state.**

The experiment moved v9r from trusted observers to verifiable ones
wherever the observed data can certify itself:

- **Derivations are always checkable.**
- **Content-addressed data needs no trusted observer at all.** The git
  provider and the `git` binary left the TCB for commit content, and
  third-party mirrors became safe.
- **What remains** is the observer of mutable state, trusted for three
  things: that it saw everything, that it saw it now, and that it saw
  the right thing.

## Next falsification experiment

**Content-address the mutable state before judging it.**

1. Make the artifact self-certifying: the runtime (trusted, already in
   the TCB) commits `dist/` into a content-addressed store, e.g. as a
   git tree in a runtime-owned object store.
2. Judge `content_equal` over two content-addressed trees.
3. The fs observer's role shrinks to one act at one moment: ingesting
   the directory.

- **Supports "observer trust reduces to a single, auditable ingestion
  step":**
  - completeness and correspondence become properties of the ingested
    tree;
  - the omission, caching and stale attacks become attacks on the
    ingester only;
  - every later judgment needs no observer.
- **Falsifies it:**
  - the ingester must itself be trusted for exactly the same three
    properties, so nothing is gained; or
  - the ingested tree diverges from what is later released, because
    the TOCTOU moves rather than shrinks.

CI history and external integrations remain deferred.

## Commits

| Commit | Content |
|---|---|
| `38d5618` | `verify.rs`: the verifier contract; registry drives verifiers and enforces classes |
| `7cb2376` | raw observation kinds; `verifiers.rs` (git objects, fs content, equality, two definitions); `sha1` dependency |
| `2fa526a` | the experiment and attacks |
