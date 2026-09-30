# Git as a Second Evidence Domain (falsification experiment)

Research question: can the invariant kernel represent non-filesystem
state transitions, with `kernel.rs` knowing nothing about Git and using
only the existing Evidence/Fact/Obligation architecture?

The falsification criterion was fixed in `INVARIANT_KERNEL_V0.md` before
this experiment:

> Falsifies A: ancestry-style relations need a new requirement form,
> domain knowledge in the kernel, or unbounded search.

**Result: not falsified by that criterion.**

- There is no new requirement form and no Git knowledge in the kernel.
- There is no search: every relation instance is decided by one bounded
  observer query.
- The kernel did change, though (+50 lines, one crate-private
  combinator).
- The experiment also showed that generality stops at the *runtime*
  layer, not at the kernel.

## Baseline

- Starting commit `0649a71`: 178 tests passing, 2 ignored.
- `kernel.rs` md5 `d5ade4cf…`.
- Git 2.51.2.

## What was built

| File | Lines | Role |
|---|---|---|
| `content.rs` | 90 | shared *content manifest* normal form: sorted `(path, kind, sha256)`, computable from an fs observation **or** a git tree |
| `git.rs` | 586 | hardened plumbing-only observer; `GitSubject` / `GitValue` vocabulary |
| `git_guard.rs` | 748 | the three invariants; `Fs \| Git` sum type; proposal → authorization → execution → judgment |
| `facts.rs` | +7 | fs fact `ContentManifest(dir)` |
| `kernel.rs` | **+50 / −0** | `EvidenceBase::map` (crate-private), see below |
| `tests/git.rs` | 979 | 19 tests |

### Git facts

| Subject | Value | Goes stale? |
|---|---|---|
| `Head{repo}` | `Head{symref, commit}` | yes |
| `Ref{repo, name}` | `Points{oid}` / `Absent` | yes |
| `Refs{repo}` | `Digest` of HEAD and all refs | yes |
| `Descends{repo, ancestor, descendant}` | `Yes` / `No` | **no**: both sides are content-addressed |
| `ContentManifest{repo, commit}` | `Digest` | **no** |
| `Worktree{repo}` | `MatchesHead` / `DiffersFromHead` | yes |

Only *pointer* facts go stale. Facts about content-addressed objects are
timeless, so ancestry and content can never become stale evidence; only
the ref that names them can.

### Invariants, expressed with existing forms only

| Invariant | Obligations |
|---|---|
| **G1** release commit descends from the approved base | `Fact Ref(release) = Points{X}` (pins the witness) ∧ `Fact Descends(base, X) = Yes` |
| **G2** protected refs cannot move | pre: `Within(declared refs, writable namespaces)`; post: `Within(touched refs, writable namespaces)`; rollback: `Fact Refs(repo) = Digest(checkpoint)` |
| **G3** exported artifact corresponds to the verified commit | `Fact Ref(release) = Points{X}` ∧ `Fact git ContentManifest(X) = D` ∧ `Fact fs ContentManifest(artifact) = D` |

A test (`only_existing_requirement_forms_are_used`) checks that every
obligation is `Fact`, `Within` or `AtMost`.

G2 reuses `Within` unchanged: ref names are hierarchical (`repo/refs/heads/…`),
just like paths. "Protected" means *everything outside the writable
namespaces*, which is safer than listing what is protected.

## Measurements

### kernel.rs changes

`git diff 0649a71 -- kernel.rs`: +50 lines, 0 deletions.

- **What was added:** one method, `EvidenceBase::map`: 40 code lines plus
  docs and a compile-fail doctest.
- **What was not touched:** `Requirement`, `Evidence`, `Strength`,
  `evaluate`, the verdict rules.
- **Guard test:** still passes; the kernel imports only `std` and
  `serde`.

**Why `map` was needed.** G3 relates a git fact and an fs fact in one
decision, so the evidence must live in one `EvidenceBase<Fs|Git, …>`. The
fs and git evidence sources produce their own typed bases, and moving
verified evidence into the sum type means relabeling it.

**Why it must be crate-private.** Relabeling verified evidence is
equivalent to attesting it: a public `map` would let any caller turn
`Verified(Exists(x) = Present)` into `Verified(TestsAt(v) = Passed)`. So
domain composition is an *authority-sensitive* operation, not a neutral
utility. A compile-fail doctest pins that it is unreachable from outside
v9r-core.

Alternatives, rejected:

- Put Git variants into the fs `Subject` enum. This needs no kernel
  change, but every domain would be edited into one closed vocabulary.
- Re-attest in `git_guard`. This works too, but it spreads attestation
  into a third module.

### New requirement types needed

**None.**

### Is `Relation` required?

**Not for these invariants.** Three mechanisms cover what a relation
type would have provided:

1. **Relation instances as keyed facts.** The observer decides
   `Descends{a, d}` for exactly the oids an obligation names (a single
   `merge-base --is-ancestor`), and the result is an ordinary fact. The
   kernel never computes a closure.
2. **Existentials via witnesses.** "There is a release commit X such that
   the ref points at X and X descends from base" becomes: the proposal
   supplies X, and a *binding obligation* `Ref(release) = X` pins it.
   Policy derivation stays evidence-free, and a wrong witness is denied
   by verified evidence.
3. **Cross-domain equality via a shared normal form plus a witness.**
   "The artifact equals commit X" becomes ∃D: both manifests equal D. The
   kernel compares values; the *meaning* of "same content" lives in
   `content.rs`, the one piece of shared vocabulary both observers must
   agree on.

What *would* force a relation or quantifier form (untested; the next
experiment):

- universal quantification over an evidence-derived set, e.g. "every
  commit in `base..X` is signed" or "no commit in `base..X` touches
  `ci/`";
- negative existentials over history;
- equality of two unknown values when nobody supplies a witness. Today
  that is Blocked, by design.

Each of these would need either an observer-materialized aggregate fact
(policy semantics leaking into the observer) or a bounded quantifier in
the kernel.

### Performance (release build, medians)

| Operation | Time |
|---|---|
| ref observation, 2 repos (6 `git` spawns) | 7.6 ms |
| release authorization (fs scan + refs + `Descends` + `ContentManifest`) | 14.7 ms |
| guarded git command (authorize + execute) | 28.5 ms |

The cost is dominated by process spawns. Kernel evaluation is still
about 1 µs.

## Adversarial results

| Attack | Result | Test |
|---|---|---|
| **stale authorization**: release ref moved between authorize and execute | not executed; Deny (`G1` pin and `authorization_basis_current` violated); workspace held | `release_authorization_goes_stale_when_ref_moves` |
| stale authorization: an unrelated tag created | not executed (basis = every repo's ref digest) | `command_authorization_goes_stale_when_any_ref_moves` |
| **rewritten history**: `commit --amend` of the approved base | Deny on G1; G3 is still *satisfied*, so the verdict pins down exactly what is wrong | `rewritten_history_does_not_descend_from_approved_base` |
| rewritten history: forged ancestry via `git replace --graft` | caught twice: creating `refs/replace/*` violates G2 (POST Deny), and the observer reports `Descends = No` | `replace_ref_forging_ancestry_is_caught_twice` |
| rewritten history: forged ancestry via `.git/info/grafts` (a plain file write) | plain `git merge-base` **is fooled** (asserted); the observer is not | `graft_file_does_not_forge_ancestry` |
| **force push**: `push --force origin release:main`, declared honestly | PRE Deny; remote untouched | `force_push_to_protected_remote_branch_is_denied` |
| force push declared as a harmless task-ref push | the push *really happens*, then POST Deny naming `origin.git/refs/heads/main`; the kernel-accepted rollback restores the remote | same |
| protected ref moved with `update-ref` (declared / undeclared) | PRE Deny / POST Deny, then rollback | `…protected_ref_move…` (2 tests) |
| **malicious proposal**: witness `--output=pwned` (option injection) | Deny (`well_formed_proposal`); oids are validated before reaching argv; no file created | `option_injection_witness_is_rejected_and_never_reaches_git` |
| malicious proposal: release the approved base under the release name (honest artifact of the wrong commit) | Deny (the ref pin is violated) | `releasing_a_different_commit_under_the_release_name_is_denied` |
| malicious proposal: tampered artifact, with an honest witness | Deny (the fs manifest ≠ D) | `tampered_artifact_is_denied_with_honest_or_forged_witness` |
| malicious proposal: tampered artifact, with a witness forged to match it | Deny (the git manifest ≠ D) | same |
| hostile repo config (`core.fsmonitor`) | the observer never executes it; plain `git status` does (asserted) | `hostile_repo_config_does_not_execute_in_the_observer` |
| repo made unobservable (`objects/info/alternates`) | the step doing it is **Blocked**; a release on such a repo is **Blocked**, not Denied | 2 tests |

## Surprises

1. **`git archive` output is not a function of the commit.**
   Agent-writable `.git/info/attributes` (which no `-c` or environment
   switch disables) and `tar.umask` change the bytes for the same commit
   (probed). Artifact identity therefore had to be defined by v9r's own
   normal form, built from objects only.
2. **Repo-local state can forge ancestry.** Replace refs and the graft
   file both flip `merge-base --is-ancestor` from 1 to 0 (probed). The
   observer disables both. An observer that just "asks git" would have
   given a VERIFIED fact that was false.
3. **Repo config executes code.**
   - `core.fsmonitor` runs on `git status`.
   - Filter drivers run on `git diff`, and whether status runs them
     depends on stat details.
   - The observer uses plumbing only and computes working-tree state
     itself.
4. **The two domains observe the same bytes at different grains.**
   `.git/index` embeds inode and mtime, so two workspaces with identical
   git state have different *filesystem* versions (test
   `filesystem_version_differs_while_git_state_is_identical`).
   Consequences:
   - fs-level freshness is stricter than git semantics need;
   - a mere `git status` by the agent (which refreshes the index) makes
     every fs-based authorization stale;
   - cross-run determinism holds only for git facts.

   Overlapping domains will need either exclusions or the rule that the
   coarser domain's version governs.
5. **Making something unobservable is itself a judged transition.** The
   step that created `objects/info/alternates` was Blocked: after it,
   "protected refs unmoved" can no longer be shown. Unknown propagated
   exactly as designed. The kernel needed no special case, although my
   own test had wrongly expected Allow.

## Limitations

- **The runtime layer is not generic.**
  - `git_guard.rs` re-implements the proposal → authorize → execute →
    judge → rollback flow of `guarded.rs`; 12 function names are shared.
  - Both guards hard-wire their evidence routing (which subject goes to
    which observer).
  - The kernel is reused unchanged; the *guard* is not.
- **Observers must live in the trusted crate.** Attestation is
  crate-private, so a third-party domain cannot contribute verified
  facts without a sealed observer-registration mechanism that does not
  exist yet.
- **The git observer trusts the `git` binary and `PATH`.** Hardening
  covers the vectors found here, not every config key git reads. Plumbing
  commands still parse repo config.
- **Stale pointer facts are handled by re-checking at execution time.**
  The window between that re-check and the command spawn remains, as in
  the fs domain.
- **Content equality ignores permission bits.** The executable bit is
  invisible, because the fs observer does not record modes.
- **Submodules** are unrepresentable: they yield no manifest, so the
  result is Blocked.

## Answer to the research question

**Can the kernel represent non-filesystem state transitions? Yes.** Git
pointer moves, ancestry and content correspondence were all judged by
the same three requirement forms, with Allow / Deny / Blocked semantics
unchanged. Relations and existentials were discharged outside the kernel
by bounded observer queries and pinned witnesses.

**Is v9r a general invariant runtime? Not yet.**

- **The kernel is general** for bounded, witness-dischargeable
  invariants: zero new forms, and one authority-preserving combinator.
- **The runtime around it is not:**
  - each domain combination needs its own guard;
  - evidence routing is hand-written;
  - the domains' notions of "version" can conflict (surprise 4);
  - only in-crate observers can attest.

The generality bottleneck has moved from the kernel to the composition
layer.

## Next falsification experiment

**A universally quantified invariant over history.** Example: "no commit
in `base..release` modifies `ci/`", or "every commit in `base..release`
is authored by an approved identity".

- **Supports the kernel:** it is expressible with the existing forms,
  without an observer-side aggregate fact that encodes the policy.
- **Falsifies the kernel:** it needs a bounded quantifier (or `Relation`)
  in the requirement language.

This targets the one thing the current experiment could route around.
