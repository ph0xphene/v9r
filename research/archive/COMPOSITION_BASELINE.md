# Composition baseline

Note written *before* the evidence-graph experiment. It records where
domain knowledge lives and where domains are composed by hand.

## Baseline

- Branch `research/effect-runtime-v0`, commit `631ff46` (effect runtime
  v1 report), clean tree.
- `cargo test --workspace`: **211 passing, 2 ignored** (perf benches).
- `kernel.rs` sha256
  `85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f`,
  unchanged since `bfebebe`.

## 1. What is generic

| Module | What | Knows about domains? |
|---|---|---|
| `kernel.rs` | facts, evidence classes, obligations (`Fact`/`Within`/`AtMost`), `evaluate` | no (test-enforced) |
| `runtime.rs` | lifecycle: authorize, freshness, execute, observe, judge, hold, compensate, journal | no (test-enforced) |
| `runtime::RtSubject/RtValue` | runtime facts ⊕ *one* domain's facts | no |
| `runtime::MemoryJournal`, `Journal` | decision journal | no |

The runtime composes exactly two vocabularies: its own and one domain's.
It has no notion of a second domain.

## 2. What is domain-specific

| Module | Domain |
|---|---|
| `facts.rs`, `policy.rs`, `effect.rs`, `state.rs`, `vfs.rs`, `execution.rs` | filesystem: observation, receipts, seals, I1/I3/I4, export gate |
| `git.rs` | git: hardened plumbing observer, `GitSubject`/`GitValue` |
| `counter.rs` | in-memory counter |
| `guarded.rs` | fs adapter + `Workspace` effect machinery (shared with git) |
| `git_guard.rs` | git adapter **and** the fs⊕git composition |

## 3. Where manual composition happens

### Every place git code knows about the filesystem

Line numbers are at `631ff46`.

| # | Where | What git code knows about fs |
|---|---|---|
| K1 | `git_guard.rs:55-72` | `CrossSubject = Fs(Subject) \| Git(GitSubject)`, same for values: a hand-written sum vocabulary |
| K2 | `git_guard.rs:291-294` | I4 is stated as the **fs** fact `CrossSubject::Fs(Subject::TrustedState)` |
| K3 | `git_guard.rs:345-349` | G3 names the fs fact `Subject::ContentManifest(artifact)` and the fs value `Value::Digest` |
| K4 | `git_guard.rs:401-407` | I3 names the fs fact `Subject::Workspace` |
| K5 | `git_guard.rs:273, 332` | artifact paths are normalized with fs's `policy::workspace_name` |
| K6 | `git_guard.rs:428-431` | composite observation `GitWorld { workspace: Observation, git: GitObservation }` |
| K7 | `git_guard.rs:486-500` | composite basis: fs workspace digest ∪ each repo's refs digest |
| K8 | `git_guard.rs:533-567` | **evidence routing**: split subjects by tag, ask the fs evidence code and the git observer, relabel both with `EvidenceBase::map`, merge |
| K9 | `git_guard.rs:570-626` | effects are fs `WorkspaceEffect`s; the receipt is fs `WorkspaceReceipt`; fs durable evidence is relabeled into `CrossSubject` |
| K10 | `git_guard.rs:527, 623` | compensation is the fs checkpoint rollback, and I3 uses the fs checkpoint digest |
| K11 | `git_guard.rs:666-706` | start: `Workspace::open`; the git observer is rooted at the fs workdir; `query` scans the workspace |
| K12 | `git_guard.rs:132` | `GitPolicy` embeds the fs exec `Manifest` |
| K13 | `git.rs:513-531` | the git **observer** answers `Worktree` by comparing a commit manifest with an **fs observation** (`content::from_fs_state`) |
| K14 | `git.rs:300-314` | the observer confines repos to the workspace root (fs path semantics) |
| K15 | `content.rs:20, 46` | the shared normal form imports the fs type `FsState` |

Measured: **110 of 635** code lines in `git_guard.rs` name fs or
composite types directly, and the 181-line `GitDomain` impl exists only
to compose.

### Other manual wiring

- **`guarded::Workspace`.** The effect machinery shared by fs and git
  (trace, checkpoint, commands, rollback receipts). It is shared by
  *reuse*, not by composition: git calls fs code.
- **`content.rs`.** The one deliberately shared vocabulary: a normal
  form for "same content" that both observers must compute identically.
- **Attestation.** `Verified::attest` is crate-private, so every
  observer must live in `v9r-core`. There is no way to register an
  outside source of verified facts.

## Summary

- **Generic.** The kernel (deciding) and the runtime (lifecycle).
- **Domain-specific.** Observers and invariants.
- **Hand-composed.** Composing two domains is done entirely by hand, in
  one adapter that knows both: vocabulary, observation, basis, evidence
  routing, effects and compensation (K1–K12).
- **Leaks below the adapter.** The observer layer (K13–K15) leaks too:
  the git observer reads fs observations, and the shared normal form is
  typed by an fs structure.

## Hypothesis for the experiment

An **evidence graph** between the runtime and the observers can replace
K1 and K8. Its job:

- route each required fact to whichever registered provider declares it
  can answer;
- with providers that know neither each other nor the lifecycle.

K3/K4 should then become facts in a shared vocabulary, rather than "fs
facts named by git code". K13–K15 should disappear from providers that
do not need them.

What the graph is *not* expected to replace:

- effects and compensation (K9, K10);
- composite freshness (K7).

Those are lifecycle concerns. Whether that is a limitation or the right
boundary is part of the question.
