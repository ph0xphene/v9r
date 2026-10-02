# v9r External Review Package v0

> **Release status (added at the v1 freeze, 2026-10-02).** This document
> describes `v9r-review-v0`. The current release candidate is
> **`v9r-review-v1`**: the Debloat Phase 1 verifier.
>
> **Revision:**
>
> - Code commit: `6a9c717695e3cf00b72e97b758283af3ab5e707e`.
> - The tag points at the documentation commit that follows it; that
>   commit changes no code. Check with
>   `git diff 6a9c717 v9r-review-v1 -- crates Cargo.toml Cargo.lock`
>   (empty).
>
> **Tests and kernel:**
>
> - `cargo test --workspace`: **86 passed, 0 failed, 4 ignored**.
> - Kernel hash guard: passed. `kernel.rs` sha256 `85badb66…6177f` is
>   unchanged from `v9r-review-v0`.
>
> **Modules:** one crate, `v9r-core`, with **12 modules + `lib.rs`** and
> 4,838 lines of library code:
>
> - claim path: `kernel`, `runtime`, `temporal`, `graph`, `verify`,
>   `snapshot`, `fs_raw`, `verifiers`, `content`;
> - explanations: `provenance`;
> - historical: `counter`, `fs_watch`.
>
> **Trusted computing base:**
>
> - `kernel`, `runtime`, `temporal`;
> - the `graph` registry with `verify`;
> - `snapshot` (`FsSnapshot`, `ObjectStore`);
> - `fs_raw` with the Linux filesystem;
> - every registered observer, the rules, and whoever holds the registry;
> - snapshot capture (no undetected concurrent writer);
> - external crates: `serde`, `serde_json`, `sha2`, `libc` (20 in the
>   build graph).
>
> **Archived:** the capability, delegation, git and product-phase code
> named below was removed from the review tree and is preserved at
> `v9r-archive-v0`. See research/V9R_RELEASE_FREEZE_V1.md and
> README_REVIEWER.md.
>
> **Post-freeze measurement (2026-10-02):** V9R_SELF_REFERENTIAL_OBSERVERS_V0
> measured X1 and X4 of V9R_ADVERSARIAL_REVIEW_V0 on the `v9r-review-v1`
> code (its test is added in `v9r-review-v1.1`, not in `v9r-review-v1`). §1, §4, §5.3 and §6 below are
> corrected to match.


For a systems engineer seeing v9r for the first time. Please read it as
a claim to be attacked, not a product description. Everything stated as
a result was measured on one Linux host (6.12, unprivileged, ext4 and
tmpfs) and is reproducible from the commands given. Everything else is
labelled as design or assumption.

**Revision under review:** tag `v9r-review-v0` on branch
`research/effect-runtime-v0`. Run `git rev-parse v9r-review-v0` for the
hash; a file cannot contain the hash of its own commit.

- **Full suite at that revision** (`cargo test --offline --workspace`):
  315 passed, 0 failed, 6 ignored.
- **`kernel.rs` hash guard:** passed (sha256 85badb66…6177f). The kernel has been unchanged
  since commit `bfebebe`.

---

## 1. One-page summary

**Problem.** An LLM agent's work and its report about that work come
from the same unreliable process. "Tests pass" can mean the code was
fixed, or that the tests were changed. A pipeline that trusts the
report, or only re-runs checks the agent could influence, cannot tell
the difference.

**Idea.** Separate *proposing* from *accepting*.

1. The agent proposes and acts.
2. A runtime that holds the authority observes the state before and
   after, collecting evidence itself from registered observers.
3. A small kernel evaluates declared rules against that evidence.
4. The result is accepted only on **Allow**. Missing evidence gives
   **Blocked**; contradicting evidence gives **Deny**.

The agent's statements are recorded as claims, and a claim cannot
satisfy a rule.

**Guarantees** (measured; conditions in §3):

- an accepted state satisfied every declared rule on evidence attested
  by registered observers. That evidence is as true as its observer, and
  an observer that executes the state is controlled by it (measured: X1,
  V9R_SELF_REFERENTIAL_OBSERVERS_V0);
- changes outside a declared scope, present when the after-snapshot is
  taken, are denied and named: names, content, symlink targets and the
  owner-execute bit (other metadata is not observed);
- input and output states are identified by git-compatible SHA-256 tree
  ids (names, content, symlink targets, owner-execute bits);
- an authorization is single-use and refused if the state changed after
  it was granted;
- missing or contradictory evidence never yields Allow;
- every verdict is explained down to the observer and request of each
  fact.

**Non-goals:**

- intent;
- reasoning quality;
- causal attribution (who produced a state) without receipts from an
  isolating executor;
- sandboxing.

v9r verifies **states, not histories**.

---

## 2. Architecture

```text
            agent (untrusted)
               │  proposal: plain data, no authority
               ▼
 ┌──────────────────────── runtime (runtime.rs, temporal.rs) ────────────────────────┐
 │ authorize ─ PRE rules on the trusted state S0 ─▶ single-use authorization         │
 │ execute   ─ re-check PRE on a fresh observation (changed ⇒ refuse, hold on drift) │
 │             agent acts ─▶ observe after (each snapshot read twice)                │
 │ decide    ─ POST rules ─▶ Allow: S1 becomes trusted │ else: hold                  │
 └───────────────┬───────────────────────────────────────────────┬───────────────────┘
                 │ asks for keys                                   │ obligations
                 ▼                                                 ▼
 ┌──── evidence (graph.rs registry) ────┐        ┌──── kernel (kernel.rs) ────┐
 │ observers answer typed keys;         │──────▶ │ Fact / Within / AtMost      │
 │ answers bound to provider+request;   │evidence│ Allow / Deny / Blocked      │
 │ verifiers derive (snapshot = tree id)│        │ unknown is never true/false │
 │ lineage: established vs claimed      │        └──────────────┬──────────────┘
 └──────────────────────────────────────┘                       ▼
                                                   decision record: verdict +
                                                   reason per rule, journaled
```

The capability layer (`capability.rs`, `delegation.rs`, `authority.rs`)
sits beside the runtime. It builds the agent's world from a manifest and
checks delegated grants. It uses the same kernel and adds no kernel
concepts.

---

## 3. Key experimental evidence

Each row: what was asked, what was measured, and under what condition
the result holds. Reports are in `research/`.

| Area | Question | Measured | Holds only if |
|---|---|---|---|
| **Invariant kernel** | can every rule be decided by a tiny, domain-free kernel? | three requirement forms covered every rule in every later experiment; `kernel.rs` stayed byte-identical (sha256 `85badb66…`, test-guarded) | the obligation-deriving code is correct. `Within` and `AtMost` judge the values they are given (Delegation v0: removing any one obligation group let 1–4 attacks through, invisibly to the kernel) |
| **Runtime generality** | one lifecycle for unrelated domains? | filesystem, git and an in-memory counter share it, with 0 lifecycle code in adapters. The counter exposed a defect both real guards shared | domains observe synchronously, and live inside the trusted crate (Effect Runtime v1) |
| **Temporal evidence** | can before/after transitions be judged? | replayed answers Blocked; out-of-band change between authorization and execution refused and held; out-of-scope writes denied | providers report the present. A provider that caches and re-attests gave a **false Allow** (Temporal v0, Provenance v0) |
| **Capability model** | can authority be constructed, delegated and checked? | world observed ⊆ manifest (9/9 predictions); 22/22 malicious grant records refused; forged issuers blocked by signatures; object identity right 6/6 with file handles (inode numbers reused 500/500 on ext4); snapshot grants exact when materialized (9/9) | the anchor, the runtime's keys and its in-memory witness are trusted. A **stolen key** and a **runtime restored from backup** both gave Allow |
| **State vs causality** | does v9r distinguish an authorized agent from an unauthorized writer of identical bytes? | **No.** Verdict, findings, observers, receipts, journal and both tree ids were identical; only counters differed | — this is the boundary of the claim, not a condition |

Reproduce each: `cargo test --offline -p v9r-core --test <name> -- --nocapture`,
with `<name>` ∈ `temporal`, `counter`, `capability_manifest`,
`object_identity`, `content_identity`, `snapshot_boundary`,
`state_vs_causality`. The delegation and root-of-trust tests are in
`--lib delegation` and `--lib authority`. Some need unprivileged user
namespaces, `python3` and `nix-store`.

---

## 4. Minimal demo

```
cargo run --offline -p v9r-core --example v9r_demo
```

It needs `python3`, and takes about one second. It is deterministic:
two runs gave identical tree ids. It shows five scripted agents on a
two-file repository whose test fails:

| Run | Verdict | What to look at |
|---|---|---|
| agent edits `src/greet.py` | ALLOW | every ✓ cites an observer, never the agent |
| agent edits the protected test | DENY | the tests **pass**, and the scope rule still refuses |
| agent claims success, changes nothing, no test runner | BLOCKED | the kernel's reason names the agent's claim as `proposed` |
| same, runner present | DENY | v9r runs the test itself |
| someone edits the code after approval | REFUSED | the authorization is bound to the approved state |

Skeptic's note: the demo agents are scripted, and the "test runner"
evidence executes the code under test (see §5.3). Measured: an agent
whose `greet.py` is `sys.exit(0)` also gets ALLOW, with `greet` absent.
The ALLOW establishes that the test command exited 0 on S1, not that
the code is correct.

---

## 5. Hard questions a reviewer should ask

Each has what is known, and where the answer is weakest.

### 5.1 Observer trust

*If one registered observer lies or misses something, what happens?*

- **Known:** a single observer that drops a file, or answers from a
  cache, produced a false Allow with a fully consistent explanation
  (Content-Addressed State v0, Temporal v0, Provenance v0).
  - Redundancy turns disagreement into Blocked, never into the truth.
  - Content-addressed data (git objects, snapshot trees) needs no
    trusted observer after ingestion.
- **Weakest point:** "registered as attesting" is the trust decision,
  and nothing checks it afterwards.

### 5.2 Atomicity

*Is the "after" state a state that ever existed?*

- **Known:** a snapshot is many system calls. Against a same-uid
  adversarial writer, no detection strategy certified one: second walk,
  metadata bracket, inotify, and all three together were defeated
  (Atomic Capture v0).
  - The cgroup freezer gave 200/200 true snapshots, but only if writers
    cannot leave the cgroup (Controlled Domain v0).
  - The temporal layer reads each snapshot twice. That catches visible
    change, not ABA.
- **Weakest point:** two different observers in one decision read at
  different instants. Whether a wrong verdict can be built from that has
  **not been constructed** either way.

### 5.3 Causality

*Does Allow mean the agent did the work?*

- **Known: no.** State vs Causality v0 measured identical outcomes for
  the agent and an outsider.
- **Second weakness, now measured:** evidence produced by *running* the
  judged state is not independent of it. A test imports the code under
  test, so that code influences its own verdict. v9r records such a
  result as verified, like any other provider fact.

  Measured (V9R_SELF_REFERENTIAL_OBSERVERS_V0, X1): a `greet.py` that
  is `sys.exit(0)` gets ALLOW, `tests(S1) = passed, verified by
  provider:test-runner`, with `greet` absent. The same module without
  the exit is denied. The judged state fully decides the runner's
  report.

  Also measured (X4): a process left behind by the actor rewrote the
  protected test in the live workspace after S1 was captured, while v9r
  was still deciding. The earlier S1 was accepted, 40/40.

  The design review (Agent Transition Trust Model v0) argues that such
  facts should be treated as specification, not as independent
  evidence.

### 5.4 Capability boundaries

*What can the agent actually reach?*

- **Known:**
  - Namespaces do not remove supplementary groups, inherited fds,
    environment or harness tools (Capability Inventory v0, partly
    unmeasured).
  - A constructed world matched its manifest (Manifest v0).
  - A same-uid process that owns the world's user namespace changed a
    sealed read-only view (Snapshot Boundary v0, A5/A6). The change was
    detected by re-verification, not prevented.
  - Unprivileged, the Linux kernel refused a bind that would reveal
    what lies under a locked mount (Object Identity v0).
- **Weakest point:** v9r runs as one uid. Prevention against that uid
  needs privilege v9r does not use.

### 5.5 Malicious providers

*What can a hostile registered provider do?*

- **Can't do (bounded by the registry):**
  - forward or replay another request's attestations;
  - answer keys it was not asked;
  - violate declared definitions or observer restrictions.
- **Can do:**
  - attest a false value freshly. It is believed (§5.1).
  - claim a method or an observed state. Claims are recorded as claims,
    and only explanations show them.
- **Not tested:** a provider that is honest for most keys and lies
  selectively, timed to the decision.

---

## 6. Current limitations

- **Trusted base:** observers, snapshot ingestion, the runtime process
  (keys, ledger witness, journal) and the Linux kernel. Only the kernel
  module is small; none of it is formally verified.
- **No containment:** the demo agent runs unconfined.
- **No rollback** for the generic transition domain: a rejected
  transition holds the runtime.
- **Freshness across restarts:** a restored runtime accepts a stale
  ledger (measured). No rollback-protected counter is available
  unprivileged.
- **Explanations:** derived from text in kernel findings. A
  contradiction names no provenance.
- **Removed test:** `tests/agent_transition.rs` is removed at the freeze.
  Its design (AGENT_TRANSITION_RUNTIME_DESIGN_V0) is historical: its
  verification evidence executes agent-controlled code, and it never
  recorded per-case results.
- **Scope of evidence:** one host, one filesystem type each for
  ext4/tmpfs, research code, mostly single-author tests.

**What would most change this assessment:**

- a constructed false Allow from two observers reading one state at
  different instants (§5.2);
- a measured bound on how far a judged state can steer an executed check
  (§5.3). *Measured since:* for the demo's check, the judged state
  decides the result outright (X1).
