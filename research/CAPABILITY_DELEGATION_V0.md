# Capability Delegation v0 (X2): can the unchanged kernel validate a delegation ledger?

Research question: **can capability provenance live as a trusted layer
above `kernel.rs`, with the kernel deciding every delegation rule
through its existing requirement forms?**

This is experiment X2 of `CAPABILITY_COMPOSITION_DESIGN_V0.md`. It is
synthetic: no worlds are built, and nothing touches the OS.

Short answer: **yes, and the experiment says exactly what the layer
has to be trusted for.**

- **The kernel decides every rule. Nothing about delegation went into
  it.**
  - Honest chains: Allow (3 of 3).
  - Malicious chains and stored records: Deny or Blocked (22 of 22).
    Each was attributed to the invariant that should catch it.
  - The kernel used only `Fact`, `Within` and `AtMost`.
  - `kernel.rs` sha256 `85badb66…6177f`: unchanged, and checked by a
    test.
- **The kernel can only judge the obligations it is given.**
  - **Obligation groups:** leaving out any one of the eight groups
    lets 1–4 attacks through as **Allow**. The kernel cannot tell that
    an obligation is missing.
  - **The ledger's word:** a ledger replica that missed a revocation is
    believed (Allow). Two replicas that disagree give Blocked.
  - **The issuer's identity:** a record that lies about who issued it
    is **allowed**. D2 checks that the grantor field matches the
    parent's grantee, but it cannot check who actually wrote the record.
- **Verdict: B.** Provenance belongs above the kernel. The layer is
  trusted for three things:
  - **completeness**: it emits every obligation, pinned;
  - **faithful state**: records, revocations, roots, clock;
  - **issuer authentication**.

  The kernel is trusted for the verdict. Unexpectedly, **the code that
  issues grants does not need to be trusted for R1–R5**: those rules
  are checked at use, against whatever the store holds.

## Baseline

| | |
|---|---|
| tree | `8a49c4b` plus the uncommitted Capability Manifest / Inventory / Controlled Domain / Composition Design files |
| tests before | 291 passed, 6 ignored (Capability Manifest v0 report) |
| `kernel.rs` | sha256 `85badb66…6177f` |
| toolchain | rustc/cargo 1.98.1 (nix store) |

## The system

`crates/v9r-core/src/delegation.rs`: about 400 non-blank, non-comment
lines. Tests are in `src/delegation/tests.rs`.

```text
  GrantLedger (trusted state)              compile() (trusted, pure)
  insert(grant)  ── stores ANYTHING        claimed chain (untrusted) + use + claimed tick
  insert_root    ── trusted configuration          │
  revoke, advance                                  ▼
  evidence(): Verified facts about its      obligations: Fact pins, Within, AtMost
    own state only                                 │
        └─────────────────▶ kernel::evaluate ◀─────┘   (unchanged)
```

### Grant

```rust
Grant { parent: Root | Grant(id), grantor, grantee /* child world */,
        objects: [canonical path], operations: [read|write],
        constraints: {key → value}, not_after: tick, depth: u8 }
id = "sha256:" + sha256(canonical JSON of the record)
```

The brief's field list has no `grantor`. It had to be added: without
it, a record cannot say who claims to hold the parent (see R2).

### Ledger (trusted state, not a validator)

The ledger **stores whatever it is given**. That models a careless or
malicious issuer: an attack is just a record the issuer should not have
written. The ledger attests only facts about its own state:

| Subject | Value |
|---|---|
| `record(id)` | the digest of the record it holds under `id` |
| `grantee(id)` | that record's grantee |
| `status(id)` | `live` or `revoked` |
| `root(id)` | `trusted`, for configured roots only |
| `clock` | its tick |

It never judges a chain.

### Compiler (trusted, pure)

The input is a **claimed** chain (the records a caller supplies, keyed
by the ids the caller claims), a use (world and capability names), and
a **claimed** tick. The compiler walks up from the leaf, at most
`MAX_CHAIN = 32` records, and emits:

| Id | Rule | Kernel form |
|---|---|---|
| `D0.canonical` | every object is a canonical absolute path | `Within([noncanonical:<o>], [])`: always violated |
| `D1.chain_pinned` | each claimed record is the stored one; the top is a configured root; a missing record is a gap | `Fact(record(id) = sha256(claimed))`, `Fact(root(id) = trusted)`, `Within([UnknownBelow(grant/<id>)], [])` |
| `D2.issuer_holds_parent` | grantor = the parent's grantee | `Fact(grantee(parent) = grantor)` |
| `D3.attenuates` | the child's names lie within the parent's | `Within(child names, parent names)` |
| `D4.depth` | depth strictly decreases; the chain is bounded; a cycle is unbounded | `AtMost(depth(child) + 1 ≤ depth(parent))`, `AtMost(length ≤ 32)` |
| `D5.unexpired` | every grant on the chain is unexpired at the ledger's tick | `Fact(clock = t)`, `AtMost(t ≤ not_after)` per grant |
| `D6.live` | no grant on the chain is revoked | `Fact(status(id) = live)` per grant |
| `D7.use_within_leaf` | the user is the leaf's grantee; the use lies within the leaf | `Fact(grantee(leaf) = world)`, `Within(use, leaf names)` |
| `D8.constraints` | every constraint on the chain holds in the use's context | `Fact(context(k) = v)` |

Names use the Capability Manifest v0 vocabulary: `fs/read<path>`,
`fs/write<path>`, and a final `/.` for an exact directory entry.

**The pattern that makes it work: every value the compiler reads from
untrusted input is pinned.** `Within` and `AtMost` look at no evidence
(Design F2). They judge whatever names and numbers they are given.
`Fact(record(id) = sha256(claimed record))` ties every field of a
claimed record to the stored record. `Fact(clock = t)` ties the tick
that `AtMost` compares against to the ledger's clock. After pinning,
the evidence-free checks are about stored values.

**Two design choices, both changes from the design document:**

- **Expiry is checked per grant against the clock, not as an edge
  attenuation** (`child.not_after ≤ parent.not_after`). An expired
  ancestor then fails its own `D5`, so edge attenuation of expiry is
  unnecessary (case R4b).
- **Constraints are conditions on use, checked for every grant on the
  chain.** A child that drops its parent's constraint does not escape
  it (case "constraint dropped"), so constraints need no edge check at
  all.

## Results

Measured on 2026-10-02.

```
cargo test -p v9r-core --lib delegation
test result: ok. 10 passed; 0 failed

cargo test --workspace     (before the grantor-lie test was added)
300 passed, 0 failed, 6 ignored     (291 + 9)
```

After the tenth test was added, the full suite was run twice:

```
run 1: atomic_capture  8 passed; 1 failed; 3 ignored   (cargo stopped there)
run 2: 301 passed, 0 failed, 6 ignored
```

The failure in run 1 is in `tests/atomic_capture.rs`. That file does
not use `delegation`. Its name was not captured, and it did not recur
in 15 further runs of that file (15/15 ok). It is recorded as an
**unexplained intermittent failure in a timing-sensitive suite**, not
attributed to this change.

### Positive (honest ledger output)

| Case | Verdict |
|---|---|
| root → A read `/data`; A → B read `/data/reports`; B reads `/data/reports/q3.csv` | **Allow** |
| three levels, depths 2 → 1 → 0 | **Allow** |
| exact entries: `/nix/.` within `/nix/.`, `/nix/store/x/.` within prefix `/nix/store` (Design H2) | **Allow** |

### Negative (malicious ledger output), compiler and kernel intact

| Rule | Case | Verdict | Caught by |
|---|---|---|---|
| R1 | child names a sibling object `/srv` | **Deny** | D3 |
| R1 | child names `/database` under parent `/data` (prefix spelling) | **Deny** | D3 (component-bounded) |
| R1 | child adds `write` | **Deny** | D3 |
| R1 | child names `/data/reports/../../etc` | **Deny** | **D0 only**: `Within` accepts it (see below) |
| R1 | child names prefix `/nix` under exact parent `/nix/.` | **Deny** | D3 |
| R2 | forged parent record (wider scope) presented under the parent's real id | **Deny** | D1 (digest mismatch) |
| R2 | parent id that does not exist | **Blocked** | D1 (gap) |
| R2 | issuer X hangs a grant off A's grant, writing grantor = X | **Deny** | D2 |
| R2 | self-declared root (`parent: Root`, not configured) | **Blocked** | D1 |
| R2 | honest record the ledger does not hold ("missing evidence pin") | **Blocked** | D1, D6, D7: pins with no evidence |
| R3 | depth not decremented (2 → 2) | **Deny** | D4 |
| R3 | redelegation below depth 0 | **Deny** | D4 |
| R3 | 40-grant chain with valid depths (255 → 216) | **Deny** | D4 (length > 32) |
| R3 | claimed cycle (`x → y → x`, ids made up) | **Deny** | D4; also D1/D2/D6/D7 undetermined |
| R4 | expired leaf | **Deny** | D5 |
| R4 | expired ancestor, unexpired leaf | **Deny** | D5 on the ancestor |
| R4 | caller claims a stale tick (10 instead of 60) | **Deny** | D5: the clock pin |
| R5 | revoked ancestor | **Deny** | D6 |
| R5 | revoked leaf | **Deny** | D6 |
| use | another world uses B's grant | **Deny** | D7 |
| use | use outside the leaf (`/data/payroll`) | **Deny** | D7 |
| constraint | root requires `commit = c0ffee`; child drops it; context says `deadbeef` | **Deny** | D8 (`c0ffee`: Allow; no context: Blocked) |

### Ledger correctness, separated from kernel correctness

**1. Each obligation group is necessary (mutation matrix).** Each run
leaves out one group and replays every attack. The kernel is the same;
only the obligations differ.

| Group left out | Attacks that become **Allow** |
|---|---|
| Canonical (D0) | dot-dot path |
| Record + root pins (D1) | forged parent record; self-declared root |
| Issuer pins (D2) | issuer does not hold the parent |
| Attenuation (D3) | sibling; `/database`; added operation; prefix below exact |
| Depth (D4) | not decremented; below depth 0; longer than 32 |
| Expiry (D5 bounds) | expired leaf; expired ancestor |
| Clock pin (D5) | stale tick |
| Liveness (D6) | revoked ancestor; revoked leaf |

Every group is load-bearing, and **no kernel-side signal reveals that
one is missing**. The verdict on an incomplete set of obligations is a
correct verdict on the wrong question.

**2. The ledger's state is believed.**

| Evidence source | Verdict |
|---|---|
| the ledger, after A's grant was revoked | **Deny** |
| a replica that missed the revocation | **Allow** |
| both merged | **Blocked**: "contradictory verified evidence for status(A)" |

This is Evidence Graph v0's single trusted liar, reproduced. The only
defence is redundancy, which turns a lie into Blocked, never into the
truth.

**3. The issuer's identity is not checkable here.** X, holding no
grant, writes a record `{parent: A's grant, grantor: "A", grantee:
"X"}`. Every obligation is Satisfied, and the use is **Allow**.

- D2 checks that the record is *consistent*: the grantor field names
  the parent's grantee.
- It cannot check that the record is *authentic*: that A actually
  wrote it.
- Nothing the kernel sees can carry that. It is a property of the path
  by which records enter the ledger.

**4. The compiler's walk is bounded by the compiler, not by the
records.**

- A stored chain of 10,000 grants: the ledger's honest supplier returns
  33 records, and the compiler stops at 33.
- Result: **196 obligations, Deny, 207 ms** in a debug build. Most of
  that time is building ledger evidence for 10,000 records (30,000
  facts), not compiling.
- A claimed cycle terminates on a revisit.

Termination and the bound are compiler properties. The kernel never
walks a chain.

## Does X2 falsify R1–R10?

R1–R5 below are the **brief's** rules; R1–R10 in the design document
are numbered differently. Both are covered.

### The brief's rules

| Rule | Result |
|---|---|
| R1 child ⊆ parent | **Held** (D3), plus one precondition: names must be canonical (D0). The kernel's prefix `Within` accepts `…/reports/../../etc` as inside `/data` |
| R2 valid parent chain | **Held** for structure (D1, D2, gaps). **Not held** for authenticity: a record that lies about its grantor passes |
| R3 depth bounds | **Held** (D4), including chains with valid depths longer than the compiler's bound, and cycles |
| R4 expiry | **Held** (D5), on the condition that the clock is pinned. Without the pin, a stale tick passes |
| R5 revoked ancestor | **Held** (D6), on the condition that the ledger's revocation state is current. A stale replica is believed |

### The design's rules

| Design rule | Status after X2 |
|---|---|
| R1 only the ledger issues; agent requests are `Proposed` | **Refined, and still required.** R1–R5 above were enforced at use against an *unvalidated* store, so the issuer's validation logic is not trusted for them. But issuer **authentication** (case 3) is. R1 now means: the path into the ledger authenticates the grantor. Validating at issue is defence in depth |
| R2 grantor = parent.grantee; parent live | **Supported** as a consistency check (D2, D6). Authenticity moves to R1 |
| R3 attenuation of scope, expiry, depth | **Supported for scope and depth.** For expiry, **superseded**: a per-grant check against the pinned clock replaces edge attenuation |
| R4 objects resolved in the grantor's view | **Not tested** (X1). X2 adds a weaker sibling: even *syntactic* name normal form is a precondition the kernel cannot check (D0). Design H3 (names are not identities) is not tested |
| R5 world manifest derived from live grants | not tested |
| R6 revocation = teardown | **Not tested.** X2 tests revocation *at authorization*. Enforcement on a running world is X3 |
| R7 deputy intersection worlds | not tested (X4) |
| R8 pipes only | not tested (X7) |
| R9 write-sharing component is one domain | not tested (X5) |
| R10 residue | not tested |
| H1 every edge check fits `Fact`/`Within`/`AtMost` | **Supported**: 9 invariant groups, 3 forms, no new form |
| H2 per-edge `Within` composes, exact `/.` entries included | **Supported** (two exact-entry cases) |
| "missing `live` is Blocked, never Allow" | **Supported** (pin without evidence) |

**Nothing in R1–R10 was falsified.** One rule was refined (R1), one was
superseded by a simpler check (expiry in R3), and one missing
precondition was found (D0, canonical names).

## Does capability provenance belong above the kernel?

**Yes (B).** X2 shows it from both sides.

**It can live there:**

- Every rule became ordinary obligations and ordinary verified
  evidence.
- The kernel has no grant, chain, issuer or revocation concept. A test
  checks its hash and that it names none of them.
- The one structural need, a chain walk, belongs naturally to the
  compiler. `Within` is transitive, so per-edge checks suffice, and the
  kernel never walks a chain.

**It has to live there:** the kernel cannot provide the three things
the layer is trusted for.

| Trust item | Why the kernel cannot provide it | Measured by |
|---|---|---|
| **Obligation completeness**, with every value pinned | an evaluator judges what it is given. A missing obligation is not visible in a verdict | the mutation matrix: 8 of 8 groups load-bearing |
| **Faithful, current ledger state** (records, revocations, roots, clock) | the kernel believes verified evidence. Redundancy gives at most Blocked | stale replica: Allow; merged: Blocked |
| **Issuer authentication** | authenticity is a property of how a record entered the ledger, not of its content | lying grantor: Allow |

None of the three is a gap in the kernel's *forms*. Each concerns where
obligations and evidence come from. That is the same boundary that
Evidence Graph v0 (registration is the trust decision) and Temporal v0
(the layer owns snapshot identity) found.

**What would have meant C (kernel insufficient)** did not happen:

- no rule needed a requirement form beyond the three;
- no rule needed the kernel to compare two unknown values;
- no rule needed time inside one decision.

The temporal-style pin idiom covered every case.

## Failed assumptions

| # | Assumption | Outcome |
|---|---|---|
| 1 | The grant fields in the brief are enough | **False.** Without `grantor`, nothing says who claims to hold the parent, so D2 cannot be stated |
| 2 | `Within` over names enforces attenuation | **Only for canonical names.** `fs/read/data/reports/../../etc` is component-bounded *within* `fs/read/data`. The kernel is right about the names it was given; the names are wrong. D0 was added |
| 3 | A forged parent link is one attack | **False: four.** (a) wrong record under the right id: Deny; (b) id that does not exist: Blocked; (c) issuer does not hold the parent: Deny; (d) a lying grantor: **Allow**. Only (d) needs something outside the kernel's evidence |
| 4 | Expiry needs edge attenuation (design R3) | **False.** A per-grant check against one pinned clock covers ancestors |
| 5 | Constraints need an edge check | **False**, if they are conditions on use checked along the whole chain |
| 6 | The issuer must validate grants for R1–R5 to hold | **False.** Use-time compilation over an unvalidated store enforced all five. Issuer *authentication* is still needed |
| 7 | A word guard ("kernel never mentions `grant`") checks the kernel knows no delegation | **False positive.** `kernel.rs` already says "execution authority is granted" in a doc comment. The guard now checks `delegat`, `ledger`, `revok`, `grantee`, `grantor` |

## What X2 does not show

- **Anything about real objects.** Paths are strings here. Whether a
  name resolves to the same object in two views is X1, and it is the
  next experiment.
- **Enforcement.** An Allow here is an authorization. Whether a world
  built from it holds exactly that, and loses it on revocation, is
  X3 + Manifest v0.
- **Concurrency.** The ledger evidence and the decision are one
  in-memory instant. A revocation racing an authorization is the same
  freshness question as Temporal v0. The answer there was re-check at
  execution, not tested here.
- **An authenticated issue path.** Case 3 shows it is needed. It does
  not exist yet.

## Next falsification experiment

**X1, unchanged from the design:** name attenuation against object
identity, with a submount the grantor cannot see.

X2 sharpens its prediction. Arm (a), binding from the host path, should
pass **all nine** D-groups and still give B the hidden submount. If it
does, the delegation layer needs one more pinned fact per object root:

```text
object(grant, root) = (dev, ino, mnt_id) resolved in the grantor's view
```

That is attested by the runtime's fd-relative resolver and checked as
an ordinary `Fact`. No kernel change is expected there either.

Before X1, the issue-path gap (case 3) should be closed on paper:
`insert` should take the requesting principal from the runtime (the
world's namespace identity, as in C3), not from the record.

## Reproducing

```
cargo test -p v9r-core --lib delegation -- --nocapture --test-threads=1
```

It needs nothing beyond the crate and runs in about 0.4 s.
`--nocapture` prints the attack table and the mutation matrix.
