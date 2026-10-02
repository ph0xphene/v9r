# Effect Runtime v1, Phase 1: lifecycle baseline

Note written *before* any refactor. It records where the lifecycle code
lived and what was duplicated between the two guards.

## Baseline

- Branch `research/effect-runtime-v0`, commit `bfebebe` (git evidence
  domain report), clean tree.
- `cargo test --workspace`: **199 passing, 2 ignored** (cap 11, core 93,
  effects 21, git 19, guarded 20, orchestrator 11, runtime 8, vfs 7,
  doctests 9; the 2 ignored tests are the perf benches).
- `crates/v9r-core/src/kernel.rs`:
  - 842 lines;
  - sha256 `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`;
  - md5 `7017d69f2445a7c940b99773623d5fc0`.
- Toolchain: the rustup toolchain on this host cannot execute (NixOS, no
  dynamic loader). Everything ran under `nix shell nixpkgs#cargo
  nixpkgs#rustc nixpkgs#gcc` (cargo 1.98.0).

## The two guards

| | `guarded.rs` (fs) | `git_guard.rs` (fs + git) |
|---|---|---|
| lines | 486 | 748 |
| lifecycle code (authorize / execute / rollback / evidence / log / state) | ~300 | ~300 |
| invariant derivation | in `policy.rs` (435) | inline (~230) |

Shared function names: `start`, `task`, `trace`, `is_accepting`,
`authorize`, `execute`, `rollback`, `evidence`, `log`. Shared shapes:

- `Authorization` / `GitAuthorization`: `{task_id, policy digest,
  proposal, decision, …}`, private fields, no `Clone`, no `Deserialize`.
- `Authorize` / `GitAuthorize`: `Allowed | Denied | Blocked`, with
  `verdict()` and `decision()`.
- `StepReport` / `GitReport`: `{executed, result, decision, <domain
  output>}`.

## Universal lifecycle logic (duplicated)

1. **Start.** Install the policy's manifest, register the task under the
   state root, open the sealed trace, log `task_started`, take the
   checkpoint.
2. **Authorize.**
   - Derive PRE obligations from the policy and the proposal.
   - Gather evidence.
   - `evaluate(Pre)`, then log the decision.
   - Mint a single-use authorization only on Allow.
3. **Execute.**
   - Reject a foreign authorization (task id or policy digest).
   - Re-observe, then check freshness against the authorization's
     basis.
   - If stale: log, mark the task unaccepted, run nothing.
   - Otherwise run the effect, observe the result, derive POST
     obligations, gather evidence, `evaluate(Post)` and log.
   - Accept (advance the trusted state) or reject (hold the task).
4. **Rollback.**
   - Restore the checkpoint.
   - Derive the restoration obligations, evaluate and log.
   - On Allow, clear the hold and reset the trusted state; otherwise
     keep holding.
5. **Acceptance state.** `unaccepted: Option<String>`, surfaced to the
   kernel as the verified fact `Transitions = Accepted | Unaccepted`
   (invariant `workspace_accepted`).
6. **Evidence assembly.**
   - Collect the subjects of every `Fact` obligation.
   - Route each subject to the observer that can answer it.
   - Add the runtime's own facts (`TrustedState`, `Transitions`).
7. **Journal.** Every decision becomes a `DecisionRecord` in the trace.

## Domain-specific logic

- **Proposal and action types.** `ActionProposal{Command|Export}`;
  `GitProposal{Command|Release}`, plus declared refs and witnesses.
- **Invariants.** The I1/I3/export gate/expectations set and the
  G1/G2/G3 set.
- **Observation.**
  - fs: a workspace tree scan with digest.
  - git: the fs scan *plus* a `GitObservation` of refs.
- **What an effect touched.**
  - fs: the `receipt_names` of an `EffectReceipt`.
  - git: `touched_refs(pre, post)`.
- **Effect-derived evidence.**
  - fs: the `TestsAt(version)` outcome of a test command, which is
    durable;
  - fs: the receipt's post-state digest.
- **Outputs that may be released only on acceptance.**
  - fs: the export bundle.
  - git: the released `(commit, manifest)`.
- **Compensation.** Both guards use the same filesystem checkpoint and
  rollback. Git relies on its repos living inside the workspace.

## Divergences between the two copies

These are not cosmetic. They are places where the "same" lifecycle
already meant different things.

| # | Aspect | fs guard | git guard |
|---|---|---|---|
| D1 | **authorization basis** | last *accepted* observation (cached) | a fresh scan at authorize time |
| D2 | **out-of-band change between steps** | detected at the next execute (basis ≠ reality) | **silently absorbed**: the next authorize re-bases on whatever is there |
| D3 | freshness re-check at execute | basis digest only | basis **plus** every PRE obligation, re-evaluated |
| D4 | when staleness marks the task unaccepted | any non-Allow freshness verdict | only if the basis obligation is *violated* |
| D5 | post-state unobservable | unaccepted ("post-state unobservable") | ignored; evaluated with missing evidence |
| D6 | rejection reason | the invariant ids that failed | the verdict only |
| D7 | rollback acceptance | kernel verdict | kernel verdict **and** `rollback` returned `Ok` |
| D8 | durable evidence (test outcomes, semantic oracle) | yes | none |
| D9 | release / export report decision | POST decision | the *execute-time PRE* re-evaluation, logged twice |

D2 is a latent defect in `git_guard.rs`. An actor outside v9r's
mediation can change the workspace between two steps, and the next
authorization treats that change as the trusted baseline.

## Runtime facts living in a domain vocabulary

Two facts are about the runtime itself, not about either domain:

- `Transitions` (the acceptance history);
- part of `TrustedState` (the trace seal).

Both are variants of the **filesystem** `Subject` enum. The git guard
reaches them through `CrossSubject::Fs(Subject::Transitions)`. The runtime
layer had no vocabulary of its own, so it borrowed the first domain's.

## Hypothesis for Phase 2

A domain-neutral runtime should own items 2–7 above and resolve D1–D9
once. A domain should provide only:

- an observer;
- the effect;
- the evidence the effect and observer can produce;
- its invariants;
- optionally, a compensation.

The test of the hypothesis has two parts:

- both guards become adapters with no authorize/execute/rollback logic
  of their own;
- an in-memory counter domain runs through the same runtime unchanged.
