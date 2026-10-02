# Capability Root of Trust v0: can delegation authority be rooted in a verifiable identity?

Research question: **can the authority to delegate be rooted in a
verifiable identity, so that the ledger which stores grants no longer
has to be trusted for *who* issued them?**

Capability Delegation v0 (X2) ended with one open hole: a record that
lies about its grantor was **allowed**. The kernel could check that a
record is consistent, but not that it is authentic. This experiment
roots issuer identity in Ed25519 keys and a runtime-owned anchor.

Short answer: **A, issuer trust can be represented above the kernel.
One part of freshness is the exception: across a runtime restart it
needs a primitive v9r does not have.**

- **Authenticity: solved above the kernel, by changing who produces
  evidence.**
  - The ledger is now **untrusted bytes**. A pure verifier, configured
    with one public key (the anchor), emits facts only for records whose
    signatures verify.
  - X2's lying issuer is now **Blocked**.
  - An unknown root, a rival runtime's ledger, a copied-and-re-signed
    ledger, an edited grant and an unsigned tail are all **Blocked**.
  - A copy verifies with nothing but the anchor string. No runtime is
    needed.
  - No new obligation was needed. The signer pins added for
    authenticity turned out to be **redundant** (mutation: removing them
    changes no verdict).
- **Freshness: detected, but only against a live witness.**
  - The runtime holds its current log head, the *witness*. A stale copy
    is **Deny**. Stale and current copies together are **Blocked**. Two
    heads the root signed at one sequence number (a fork) are
    **Blocked**.
  - Signatures cannot prove that no newer head exists. **A runtime
    restored from a backup holds an old witness and accepts the old
    ledger (Allow).** That is undetectable inside the model.
- **What remains trusted:**
  - **key custody**: whoever holds a world's private key issues as that
    world (measured: Allow);
  - **the witness**: one hash and counter, held by the runtime.

`kernel.rs` is unchanged (sha256 `85badb66…6177f`, checked by a test).
No kernel concept was added.

## Baseline

| | |
|---|---|
| tree | `8a49c4b` plus the uncommitted capability files and X2 (`delegation.rs`) |
| tests before | 301 passed, 6 ignored (X2 report) |
| `kernel.rs` | sha256 `85badb66…6177f` |
| new dependency | `ring` 0.17.14 (Ed25519). It was already in `Cargo.lock` through rustls, so it builds `--offline`. `Cargo.lock` changed by one line |

## The system

`crates/v9r-core/src/authority.rs` (about 500 non-blank, non-comment
lines). Tests are in `src/authority/tests.rs`.

```text
  Authority (runtime-owned, trusted)                 any verifier
  ──────────────────────────────────                 ────────────
  root key R  (private; never leaves)                anchor = R's public key   (configuration)
  world keys  (held FOR worlds; channel-bound)       LedgerCopy                (untrusted bytes)
  append-only log ─ sha256 hash chain ─ head(seq, h)      │
  publish(): head signed by R ──────────────────────▶ verify(anchor, copy) ─▶ Verified facts
  witness(): current "seq:h"   (trusted, fresh)           │   record, signer, grantee,
  clock                                                   │   status, root, head
        │                                                 ▼
        └─ obligations:  delegation::compile   D0–D9   grant correctness   (X2, unchanged)
                         A1 signer(id) = grantor        issuer authenticity (redundant, kept)
                         A2 head = witness              freshness
                                    └────────────▶ kernel::evaluate  (unchanged)
```

### Identities

| Identity | What it is | Where the private key lives |
|---|---|---|
| **runtime root** R | an Ed25519 key | inside `Authority` only. Its public key is the **anchor**, which a verifier is configured with |
| **world** W | an Ed25519 key, `ed25519:<hex>` | inside `Authority`, **held for** the world. A world issues by asking on its own channel (`delegate(requester, grant)`); the runtime signs only if `grant.grantor` is the requester's key |
| **anyone else** | any key | wherever. It can sign records, and the runtime appends them if their signatures verify (`submit`). What that key may *do* is decided at use |

### Grant (X2's record, with two changes)

```text
Grant { parent: Root | Grant(sha256), grantor: KeyId /* issuer */, grantee: KeyId,
        objects: [path], identities: {path → object id}, operations, constraints,
        not_after, depth }
SignedGrant { grant, signature: by grantor over "v9r-grant-v0\0" ‖ id }
Revoke { grant, by, signature }   // valid if `by` is the grant's issuer or the anchor
```

- **Grantor and grantee are keys**, no longer free-form strings.
- **Object identity** is a new field, `identities`. It is pinned at use
  against the runtime's resolver as `D9.object_identity`:
  `Fact(object(path) = id)`. In this experiment the resolver is
  synthetic. Resolving objects in the grantor's view is still X1.

### Ledger: an untrusted log with a signed head

- `head = (seq, h_seq)`, where `h_0 = sha256("v9r-ledger-v0")` and
  `h_i = sha256(h_{i-1} ‖ sha256(entry_i))`.
- `publish()` signs `"v9r-head-v0\0" ‖ seq:h` with R.
- A `LedgerCopy` is plain serde data. Anyone may copy or edit it.

### Verifier: pure, trusts no holder

`verify(anchor, copy)` works in four steps:

1. **Head.** The head signature must verify under the anchor, and the
   first `seq` entries must hash to `h`. If either fails, the copy gives
   **no facts at all**. Entries past `seq` are an unsigned tail and are
   ignored.
2. **Grants.** For each grant, the signature must verify under
   `grant.grantor`. Only then does the verifier emit `record`, `signer`,
   `grantee` and `status`. It emits `root(id) = trusted` only if
   `parent = Root` **and** `grantor = anchor`.
3. **Revocations.** A revocation counts only if it is signed by the
   grant's issuer or by the anchor.
4. **Head fact.** It emits `head = "seq:h"`.

Separately, `equivocates(anchor, a, b)` reports two anchor-signed heads
with the same `seq` and different hashes.

### Decision

The runtime contributes three things: the anchor, the witness, and its
clock (a verified `clock` fact). Everything in the copies and the
claimed chain is checked. The obligations:

- **Grant correctness:** `delegation::compile` (D0–D9), unchanged from
  X2.
- **Authenticity:** `A1` signer pins.
- **Freshness:** `A2.fresh_ledger`, i.e. `Fact(head = witness)`.

## Results

Measured on 2026-10-02.

```
cargo test --offline -p v9r-core --lib authority      7 passed
cargo test --offline --workspace --no-fail-fast       308 passed, 0 failed, 6 ignored   (two runs)
```

### The five questions

The **Layer** column is the layer that must catch the case, declared
in the test before it runs. The **Caught by** column is the measured
non-satisfied invariants.

| # | Case | Layer | Verdict | Caught by |
|---|---|---|---|---|
| **1** | root R → A → B; B reads its grant | — | **Allow** | |
| 1 | A reads under the root grant | — | **Allow** | |
| **2a** | unknown key K writes `{parent: Root, grantor: K}` | authenticity | **Blocked** | D1: no `root` fact (K is not the anchor) |
| 2b | root grant naming **the anchor** as grantor, signed by K (written by a faulty appender) | authenticity | **Blocked** | the verifier rejects the signature → no facts |
| 2c | K hangs a grant off A's grant, as itself | correctness | **Deny** | D2: A holds the parent, not K |
| **2d** | **X2's lying issuer**: grantor A, signed by K | authenticity | **Blocked** (X2: **Allow**) | the verifier rejects the signature |
| — | world X asks the runtime to issue with grantor A | channel | refused | `NotTheRequester` |
| **3a** | verbatim copy of the current ledger | — | **Allow** | (a copy grants nothing new) |
| **3b** | a rival runtime's ledger (own root key, same world names) | authenticity | **Blocked** | head signature fails → no facts |
| 3c | the real ledger, extended with a rival root grant and re-signed by another key | authenticity | **Blocked** | head signature fails |
| 3d | an entry appended past the signed head | authenticity | **Blocked** | unsigned tail ignored |
| 3e | a signed grant widened in place | authenticity | **Blocked** | entries no longer hash to the signed head |
| **4a** | a copy from before a revocation | freshness | **Deny** | A2: `head` ≠ witness |
| 4b | revocation cut from the middle of the log, re-hashed | authenticity | **Blocked** | the head signature no longer verifies |
| 4c | revocation by a non-issuer (B revokes A's root grant) | authenticity | **Allow** (ignored) | |
| **5a** | stale and current copies together | freshness | **Blocked** | contradictory `head` and `status` |
| 5b | forked runtime: two heads at one seq, both signed by R | freshness | **Blocked** | contradictory `head`; `equivocates` = true |
| G1 | authentic grant, wider than its parent | correctness | **Deny** | D3 |
| G2 | object behind `/data/reports` replaced | correctness | **Deny** | D9 |
| G3 | authentic grant, expired | correctness | **Deny** | D5 |

**Every expectation the brief set held:**

1. a valid root issuer creates grants;
2. an unknown issuer creates no authority;
3. a copied ledger cannot impersonate the root;
4. a stale ledger is detected;
5. two disagreeing ledgers give Blocked.

### What remains trusted (measured)

| Case | Verdict | Meaning |
|---|---|---|
| a thief holding A's private key issues as A | **Allow** | signatures authenticate **keys**, not principals |
| a runtime restored from a backup (same key, old witness) receives the old copy | **Allow** | the live runtime gives **Deny** for the same copy. Staleness is detectable only against a witness that is itself fresh |
| a third party with the anchor and a copy | verifies everything **except freshness** | it can say "authentic as of head N", never "current" |

### Mutation matrix: which check carries which case

Each run removes one check and replays every case. Listed are the cases
whose verdict changed.

| Removed | Changed verdicts |
|---|---|
| A1 signer pins | **none** |
| A2 freshness | 4a stale copy → **Allow** |
| verifier: grant signatures | 2b forged root → **Allow**; 2d lying issuer → **Allow** |
| verifier: head signature | 3b, 3c, 4b → **Deny** (no longer Blocked: A2 still rejects their heads) |
| verifier: head signature, *and* A2 | 4a → **Allow**; 4b revocation cut out → **Allow** |
| verifier: revocation entitlement | 4c non-issuer revocation → **Deny** (denial of service) |
| D2 issuer pins (X2) | 2c → **Allow** |

What the matrix shows:

1. **Authenticity is carried by the evidence, not the obligations.**
   The verifier emits `signer(id) = grantor` only when the signature
   verifies, and emits nothing otherwise. So `A1` is implied by D1's
   record pin. The fix for X2's hole is a change of **evidence source**:
   the trusted store that attested its own records became a verifier
   that attests only what verifies.
2. **For the live runtime, the witness subsumes the head signature.**
   Without head signatures, a forged or re-hashed head still differs
   from the witness, so the verdict is Deny. The head signature
   matters:
   - for verifiers **without** a witness (third parties, a restarted
     runtime);
   - for turning a forgery into Blocked (it is not a ledger) rather
     than Deny (it is a stale ledger).
3. **Grant correctness and issuer authenticity stay separate.** 2c is
   authentic (K really signed it) and incorrect (K does not hold the
   parent), and only D2 catches it. G1–G3 are authentic and incorrect,
   and only D3, D9 and D5 catch them.

### Cost

10,001 entries in a debug build (ring unoptimized):

- publish (hash chain + one signature): **0.73 s**;
- verify (10,001 signature checks + hash chain): **2.4 s**;
- result: 40,006 facts.

Verification is linear in the log length. Every decision re-verifies
every copy. A real runtime would verify incrementally from the last
verified head, which these numbers do not include.

## Separation of the three concerns

| Concern | Piece | Trusted for | Not trusted for |
|---|---|---|---|
| **Grant correctness** | `delegation::compile` (X2, unchanged except D9) | emitting every obligation, pinned | who issued a record; whether a ledger is current |
| **Issuer authenticity** | `verify` + the `delegate` channel binding + key custody | signature checking (ring); the anchor being R; world keys never leaving the runtime | what a grant permits |
| **Freshness** | the witness (`Authority::witness`) | being the runtime's current head | anything after a restart from older state |
| **Kernel evaluation** | `kernel::evaluate` | the verdict, given obligations and evidence | all of the above. It is the same code with the same three forms, and it never sees a key, a signature or a log |

## Decision

**A: issuer trust can be represented above the kernel.**

- Every requirement is one of the existing forms: `Fact`, `Within`,
  `AtMost`. Every new fact (`signer`, `head`, `object`) is an ordinary
  subject.
- Authenticity needed no new obligation at all. It needed only a
  verifier that emits facts solely from signatures that verify under an
  anchor.
- This is the same move Verifiable Observers v0 made for git objects:
  trust shrank from "the ledger holder" to "the anchor and the verifier
  code".

**Why not B (a new trusted primitive) for authenticity?** Signatures
are not a new *kind* of trust in v9r. They are self-certifying
evidence, like content-addressed objects (Content-Addressed State v0).
The anchor is trusted configuration, like the root manifest. The root
key needs custody, like the runtime's trusted state directory
(`trusted.rs`).

**Where B does appear: freshness across restarts.**

- Within one runtime lifetime, the witness is in-memory runtime state,
  the same class as Temporal v0's snapshot clock. Stale ledgers are
  detected (4a, 5a, 5b).
- A runtime that **restarts from persisted state** must trust that the
  persisted witness is the latest. The restored-backup case shows that
  nothing inside the model can tell.
- That needs a **rollback-protected monotonic counter**: a TPM NV
  counter, or an external append-only log with its own freshness.
  Capability Inventory v0 found `/dev/tpm0` and `/dev/tpmrm0` at mode
  `0600` root: unavailable to this unprivileged runtime.
- So: **B, scoped to persistence**. The model is A for a single runtime
  lifetime.

**Why not C (the provenance model is incomplete)?**

- X2's model was incomplete: it had no way to authenticate an issuer.
  That gap is now closed.
- What remains is a binding the model **relies on but does not
  contain**: *key ↔ world*. The stolen-key case shows that a signature
  identifies a key holder.
- This model binds keys to worlds by custody. The runtime holds every
  world key and signs only for requests on that world's own channel.
  That makes the binding a property of world construction (Capability
  Manifest v0: no runtime state in any world's view, fds closed, env
  constructed), not of provenance.
- If worlds held their own keys, a world could copy its key to another
  world through a shared write path. That would be **delegation outside
  the ledger**, the same class of channel as fd passing (Design X7).
  Runtime custody of keys is therefore part of this decision, not an
  implementation detail.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | Authenticity needs new obligations (A1 signer pins) | **False.** A1 changes no verdict. A verifier that emits facts only for verified signatures makes it implied. Kept as documentation, not load-bearing |
| 2 | The head signature is what stops a forged ledger | **Only without a witness.** With A2, a forged head is denied as stale. The signature matters for third parties, after a restart, and for Blocked-vs-Deny |
| 3 | Cutting a revocation out of a log is a freshness attack | **It is an authenticity failure.** The re-hashed log no longer matches the signed head (Blocked). It becomes a freshness question only if the head signature is not checked (then Deny with A2, Allow without) |
| 4 | With a witness, the kernel can pick the current copy out of two | **False.** Merged evidence is contradictory, so the verdict is Blocked, as the brief required. Choosing the copy whose head equals the witness *before* merging would give a decision. That choice belongs to the layer, and it is a policy (prefer availability) this experiment did not take |
| 5 | Signatures identify worlds | **False: they identify key holders.** A stolen world key issues as the world (Allow). The world binding comes from key custody plus the channel |
| 6 | Each case tests only its own layer | **False in the first run.** The synthetic resolver did not resolve `/etc` and `/srv`, so D9 also blocked 2a, 2b, 3b and 3c, and hid whether authenticity alone stopped them. After the fix, 2b's dependence on grant-signature checking showed up in the mutation matrix (→ Allow) |

## What this does not show

- **Real key custody.** The runtime's keys live in process memory, and
  worlds are not built here. That a world cannot read the runtime's
  memory or state is Capability Manifest v0's claim (separate
  namespaces, nothing of the runtime's in the world's view). It was not
  re-measured with keys present.
- **Key rotation, root key loss, restarts.** One root key, never
  rotated. Its compromise is total: it can sign root grants and heads.
- **Concurrency.** Witness and decision are one in-memory instant. A
  revocation racing an authorization is still Temporal v0's re-check at
  execution.
- **Object identity against real views.** `identities` is pinned
  against a synthetic resolver. Whether a path resolves to the same
  object in the grantor's and the grantee's views is X1.

## Next falsification experiment

**X1: name vs object attenuation, now with signed object identities.**

The design's prediction stands: binding from a host path gives B a
submount A never saw, while every name-level check passes.

What changes after this experiment: the grant now carries a signed
`identities` map, and D9 pins it. So the test becomes "does the
runtime's fd-relative resolver, run in the grantee's world, report the
identity A's signed grant names?"

- **Supports the model:** arm (a) is **Deny by D9**, with no new
  mechanism.
- **Falsifies it:** the resolver cannot observe the submount's identity
  from inside the grantee's world (for example, `mnt_id` is not
  available without `/proc`). Then object identity needs an outside
  observer, as C3's namespace facts did.

## Reproducing

```
cargo test --offline -p v9r-core --lib authority -- --nocapture --test-threads=1
```

This prints the case table, the mutation matrix and the 10,000-entry
timing. It takes about 8 s in a debug build.
