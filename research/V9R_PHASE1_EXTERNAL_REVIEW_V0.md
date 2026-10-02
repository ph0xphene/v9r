# v9r Phase 1: External Review Package v0

*For one external reviewer. It replaces V9R_EXTERNAL_REVIEW_PACKAGE_V0
for the Phase 1 tree. Every claim below names the test that backs it;
run them (§7) rather than trusting this text. Measured 2026-10-02, one
Linux host.*

> **v1.1 wording corrections (2026-10-02).** G1, G3, G4, G6, G8 and G9,
> §1 steps 4 and 6, and the self-referential limitation in §6 were
> narrowed to match measurements (X1, X4:
> V9R_SELF_REFERENTIAL_OBSERVERS_V0) and code facts (`fs_raw.rs:73`,
> `temporal.rs:526`), as identified in V9R_RELEASE_AUDIT_V0. The test
> names are unchanged. The counts in §3 and §7 are those of the
> pre-freeze tree; `v9r-review-v1.1` gives 88 passed, 0 failed,
> 4 ignored. The formal claim and its boundary:
> V9R_VERIFICATION_BOUNDARY_V0.

**The one question:** does §1 match what the code and the tests show?
§8 lists the places we expect it to be weakest.

## 1. What v9r is now

v9r is a **state transition verifier for untrusted computation**.

1. An actor (typically an LLM agent) proposes a change. A proposal is
   plain data and carries no authority.
2. v9r authorizes it once, against an approved, content-identified
   state S0.
3. Immediately before the actor runs, v9r re-checks the authorization on
   a fresh observation. If the state changed, it refuses.
4. When the actor's call returns, v9r captures the result S1 and asks
   its registered observers about it. It evaluates declared rules
   against their attestations. An attestation is as true as its
   observer; an observer that executes S1 is controlled by S1 (X1).
   Changes made after the capture are not judged (X4).
5. Each rule is Satisfied, Violated or Undetermined. The verdict is
   **Deny** if any rule is violated, else **Blocked** if any is
   undetermined, else **Allow**. Unknown is never treated as true or
   false. An actor's statements enter only as claims, and a claim never
   satisfies a rule.
6. On Allow, the captured S1 becomes the runtime's last accepted
   snapshot. It may already differ from the live state (X4). Otherwise
   that runtime process **holds** and authorizes nothing further; the
   hold is in memory only.

**v9r verifies states, not histories.** A decision depends on S0, S1,
the rules, the observers' answers and the ordering of the window. It does
not depend on who wrote S1:

- an unauthorized writer that produces byte-identical S1 inside the
  window gets **identical decisions**;
- "identical" means the complete PRE and POST decisions and the journal,
  up to counter values.

## 2. What was removed from `v9r-review-v0`

`v9r-review-v0` is commit `da6d694` (also tagged `v9r-archive-v0`). Everything below is
still there.

| Removed | What it was | Report it belonged to |
|---|---|---|
| `fs_provider` and the legacy stack (`effect`, `facts`, `vfs`, `execution`, `trace`, `task`, `bundle`, `context`, `trusted`, `manifest`, `state`, `policy`, `guarded`, `adapter`) | the earlier "transactional shell": checkpoints, rollback, bundles, LLM clients, the first effect domain | Effect Runtime v0, Invariant Kernel v0 (domain), Effect Runtime v1 |
| `git`, `git_provider`, `git_guard`; git verifiers (`GitObjects`, `ContentEquality`, `SnapshotCommitEquality`) | git as an evidence domain and a second effect domain | Git Evidence Domain v0, Effect Runtime v1, Evidence Graph v0, Verifiable Observers v0 |
| `capability`, `delegation`, `authority`, `object_identity` | worlds built from manifests, delegated and signed grants, object identity | Capability Manifest / Delegation / Root of Trust / Object Identity / Content Identity / Snapshot Boundary v0 |
| `crates/v9r-cli`, `examples/*`, `agents/`, `task.toml` | the shell's CLI/REPL, Wasm agent examples, a committed `.wasm` | product phase |
| ~1.04 GB of tracked build output | committed `target/` directories | — |
| 213 integration and unit tests and 7 doctests of the removed code | — | the reports above |

Details: V9R_DEBLOAT_PHASE1_REPORT_V0 and V9R_PHASE1_CORE_AUDIT_V0.

**Guarantees no longer claimed** (they were rows of the old Overview):

- delegated authority (attenuation, authenticity, liveness, expiry);
- exact read-only snapshot grants.

Both are now results of archived experiments, not properties of v9r.

## 3. What survived

`crates/v9r-core`, 5,079 lines of library code:

| Module | LOC | Role |
|---|---|---|
| `kernel` | 604 | the verdict. **Byte-identical since `bfebebe`** (sha256 `85badb66…6177f`) |
| `runtime` | 689 | lifecycle: authorize → re-check → act → judge → accept or hold |
| `temporal` | 529 | snapshots on one clock, read twice; before/after rules |
| `graph` | 943 | evidence registry: binds answers to provider and request; kind rules; lineage |
| `verify` | 96 | protocol for derived facts |
| `snapshot` | 818 | `FsSnapshot`: git-compatible SHA-256 tree id and `snapshot_entries` from one walk; object store |
| `fs_raw` | 222 | the filesystem observer (`openat`, `O_NOFOLLOW` per component) |
| `verifiers`, `content` | 296 | content-addressed helpers, content-manifest definition |
| `provenance` | 275 | explanations from lineage |
| `counter`, `fs_watch` | 445 | historical: domain-neutrality falsifier; inotify capture strategy |

A demo (`examples/v9r_demo.rs`) and 128 tests remain. Four crates from
the product phase (`v9r-vfs`, `v9r-cap`, `v9r-runtime`,
`v9r-orchestrator`) are still in the workspace, marked for archiving.
None of v9r-core depends on them; they depend on `v9r-core`'s `lib.rs`
types only.

## 4. Current trusted computing base

For an **Allow** to mean what §1 says, all of the following must be
correct or faithful.

| Component | Trusted for | Size / note |
|---|---|---|
| `kernel` | evaluation semantics | 604 lines; `std` + `serde` |
| `runtime`, `temporal` | lifecycle, freshness, pairing, hold | 1,218 lines |
| `graph` registry, `verify`, `verifiers` (helpers) | binding answers to requests; vouching | ~800 lines on the claim path |
| `snapshot` (`FsSnapshot`, `ObjectStore`) | state identity and scope entries | ~560 lines on the claim path |
| `fs_raw` | **observation of the filesystem**: complete listings, true contents | 222 lines + Linux syscalls |
| the host OS and filesystem | `openat`/`getdents`/`fstatat`/`read` semantics; directory link counts (used to detect omitted subdirectories) | external |
| `sha2`, `serde`, `serde_json`, `libc`, `rustc` | hashing, serialization, FFI | external |
| **every registered provider** | faithful, complete, current answers, independent of the state it observes. The demo's test runner is **not** independent: it runs S1's code (X1) | per deployment. A registered liar is believed (`a_trusted_liar_is_believed_alone_…`) |
| **the invariants** (the rules) | expressing what "acceptable" means | written by the host; trusted configuration |
| **whoever holds the `Registry`** | deciding which observers count | privileged |
| capture | no concurrent writer while a snapshot is taken, unless detected by a double reading | trusted (Atomic Capture v0) |

**Not trusted:**

- the actor and its statements;
- the object store's bytes (checked by hash);
- answers bound to another provider or request (dropped);
- answers about keys nobody asked (dropped).

## 5. Guarantees, with conditions

| # | Guarantee | Holds when | Test |
|---|---|---|---|
| G1 | A transition is accepted only if every declared rule is satisfied by evidence **attested by registered observers** in reply to v9r's own requests; claims never satisfy a rule. An attestation is as true as its observer | TCB (§4) correct; observers faithful, complete, current **and independent of the judged state**. Not met by the demo's test runner: X1 gives Allow with `greet` absent | kernel unit tests; `claims_instead_of_evidence_block`; demo run 3. Limit: `x1_judged_program_decides_the_test_runners_report` |
| G2 | Missing or contradictory evidence gives **Blocked**, never Allow | — | `removing_any_observer_…`, `disagreeing_observers_block`, `observer_disappearing_…`, `observer_unavailable_after_effect_…`, `observers_reading_different_versions_block` |
| G3 | Changes outside a declared scope, present when the after-snapshot is taken, are **denied and named**; scope covers names, file content, symlink targets, directories (empty ones too) and the **owner-execute bit** only (`fs_raw.rs:73`). Other metadata is not observed | the observer sees the whole scope; special files (FIFO, socket, device) make the state unrepresentable → **Blocked**, not Deny | `scope_entries` (18 cases); demo run 2 |
| G4 | S0 and S1 are identified by the SHA-256 git tree id of the captured tree, the same id `git` computes: names, content, symlink targets, owner-execute bits | after capture; capture itself is trusted | `a_snapshot_is_the_git_tree_of_the_same_content` (runs `git`); `a_claimed_snapshot_id_is_recomputed_not_believed` |
| G5 | An omitted subdirectory, an unreadable file, or a lost or altered stored object makes the identity **incomplete** (Blocked) | the filesystem keeps directory link counts; an omitted *file* is caught only with an independent second observer | `hidden_directory_…`, `hidden_file_…_only_with_an_independent_observer`, `unreadable_file_…`, `approved_snapshot_with_a_lost_or_altered_object_…` |
| G6 | An authorization is single-use and **refused** if the watched state differs, at the start of execution, from the one it was granted on; drift **holds** that runtime process. The hold is in memory; a new runtime takes the current state as its baseline (`temporal.rs:526`) | the change is visible to the watched keys | `artifact_changed_after_authorization_is_refused`, `external_modification_between_authorization_and_execution_refuses`; demo run 5 |
| G7 | Answers are bound to the provider and request that produced them: replayed, forwarded, volunteered or anachronistic attestations are dropped | — | `volunteered_attestations_…`, `replayed_attestations_…`, `replayed_raw_observation_fails_binding`, `before_snapshot_replayed_as_after_…`, `evidence_claiming_an_earlier_state_…` |
| G8 | A change written by anyone between the fresh observation at the start of execution and the after-snapshot (taken when the actor's call returns) is **attributed to the effect**. Changes after the after-snapshot are not judged, even while the decision is still running | — | `external_modification_during_the_effect_is_attributed_to_it`. Limit: `x4_watched_state_changes_after_observation_before_completion` (40/40 accepted a stale S1) |
| G9 | Facts bind to the state they name (a test result for another tree does not count). Binding is not independence: an observer that executes that state is controlled by it (X1) | rules name the tree | `test_success_for_another_tree_does_not_count` |
| G10 | Decisions are explained per rule, naming the observer; provider facts resolve to provider, request, round and snapshot | for provider facts; verifier facts name the verifier and the observers it trusted | `release_is_explained_…`, `snapshot_evidence_carries_its_snapshot_in_lineage` |
| G11 | **Boundary:** identical S1 from an authorized actor or an unauthorized writer gets identical decisions | by design | `state_vs_causality`, `identical_s1_gets_identical_decisions_whoever_wrote_it` (complete decisions; control world with different bytes differs) |
| G12 | The kernel is unchanged since review | — | `kernel_guard` |

## 6. Known limitations

**Trust:**

- **Observers are believed.** A registered liar alone gives a false
  Allow. A second, independent observer turns the disagreement into
  Blocked, never into the truth. Tests:
  `a_trusted_liar_is_believed_alone_…`,
  `stale_raw_content_attested_freshly_is_believed`,
  `stale_state_attested_freshly_…_unless_independently_witnessed`.
- **One filesystem observer.** Since Phase 1, scope and identity rest on
  the same `fs_raw` observations. They cannot disagree, and they do not
  check each other.
- **Capture is trusted** against a concurrent writer of the same uid. The
  double reading detects changes between readings, not changes it
  happens to read consistently twice.
- **Checks that run the code being judged are controlled by it.**
  The demo's I3 runs a test that imports the judged code. Measured:
  `sys.exit(0)` on import gives Allow with `greet` absent; the same
  module without the exit is denied (X1).
- **The decision covers S1 as captured.** A process left behind by the
  actor rewrote the protected test while v9r was deciding; the earlier
  S1 was accepted in 40/40 transitions (X4).
- **Freshness across restarts.** The runtime's state (trusted snapshot,
  hold) is in memory.

**Behaviour:**

- **No rollback.** A rejected transition holds the runtime; nothing
  restores S0.
- **Special files** in a judged tree block. They are not representable,
  so they are Blocked, not Denied.

**Explanations:**

- They are parsed from kernel reason text. A basis containing `)` is cut
  (the verifier's "input(s)"). A contradiction's explanation names no
  provenance.

**Code:**

- **Stale or idle code in the frozen kernel.** Its doc names the removed
  `crate::policy`. Its `compile_fail` example on `EvidenceBase::map` is
  vacuous (an equivalent one is on `graph::GraphEvidence`). `Semantic`,
  `Strength::Soft` and `AtMost` have no producer on the claim path.
- **Idle code in kept modules:** `snapshot_from`, `snapshot::entries`,
  `Runtime::add_semantic`, `Compensable`. Listed in the audit; left
  until Phase 2.
- **Product-phase crates** are still in the workspace (§3).
- **Research code, measured on one Linux host.**

## 7. Reproduction

Requirements: Rust 1.98, measured with rustc 1.98.1 and cargo 1.98.0 (offline build with the committed `Cargo.lock`),
`python3` (demo), `git` (one test).

```sh
cargo test --offline --workspace --no-fail-fast
#   → 128 passed, 0 failed, 4 ignored

cargo run --offline -p v9r-core --example v9r_demo
#   → ALLOW, DENY, BLOCKED, DENY, REFUSED (also asserted by cargo test)

cargo test --offline -p v9r-core --test state_vs_causality -- --nocapture
#   → World A vs B: every measured field identical; complete decisions identical

cargo test --offline -p v9r-core --test claim_guarantees      # 28: G2, G5–G10 and the limits
cargo test --offline -p v9r-core --test scope_entries          # G3, 18 cases
cargo test --offline -p v9r-core --test content_addressed      # G4, G5
cargo test --offline -p v9r-core --test kernel_guard           # G12

sha256sum crates/v9r-core/src/kernel.rs
#   → 85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f
```

The 4 ignored tests are measurements, not guarantees:

- `atomic_capture`: two timing-dependent races, and one that needs a
  systemd user session with cgroup delegation;
- `controlled_domain`: one that needs a systemd user session and
  unprivileged user namespaces.

Run them with `--ignored --nocapture`.

## 8. Questions for the reviewer to attack

1. **Can an actor get Allow for a state that violates a declared rule,
   with every registered observer honest?** (X1 measured the nearby
   case: every rule satisfied, every observer faithful, and the
   property the rule was meant to express false.) Start at `runtime.rs`
   `execute`/`settle` and `temporal.rs` `evidence`.
2. **Can a fact verified for one moment be used for another?** For
   example, an `@s<before>` value satisfying an `@s<after>` obligation,
   or a `@now` answer standing in for a snapshot. See `temporal.rs`
   `relabel` and `At`.
3. **Is `snapshot_entries` complete for the scope?** Can a change
   outside the scope leave both the tree id and the entries map
   unchanged? Consider hard links, xattrs, ACLs, ownership, timestamps,
   sparse files, case-insensitive filesystems and mount points inside
   the scope.
4. **Does the double reading (`Snapshot::take`) catch what it claims?**
   Both readings may come from one stable but wrong view. Is that a
   different trust assumption than the one stated in §4?
5. **Does the registry's binding (`collect_providers`) have a gap?** For
   example: an attestation minted with a valid `Attestor`, then answered
   for a different key in the same request; `depends_on` chains; or a
   provider that answers a derived kind.
6. **Is the boundary (G11) a property of the model, or of these
   invariants?** Can you write rules, using only `Fact`/`Within`/`AtMost`
   over the existing observers, that tell World A from World B? If you
   can, v9r observes something about histories.
7. **Single use.** `Authorization` is consumed by value and has no
   `Clone`. Can two executions happen under one authorization (two
   runtimes, a cloned domain, a re-entrant actor)?
8. **Hold.** After a rejection, is there any path that accepts new work
   without a new accepted transition?
9. **Kernel semantics.** Are "contradictory verified evidence →
   Undetermined" and "Deny dominates Blocked" the right choices? Can one
   observer turn a Deny into a Blocked by contradicting a true fact, and
   does that matter?
10. **The TCB table (§4).** Is anything missing from it, or listed as
    untrusted that should be trusted?
