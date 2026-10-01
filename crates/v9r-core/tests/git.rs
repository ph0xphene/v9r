//! Git as a second evidence domain under the unchanged kernel forms.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};

use uuid::Uuid;
use v9r_core::execution::CommandSpec;
use v9r_core::git::{GitRepo, GitSubject, GitValue, Oid};
use v9r_core::git_guard::{
    CrossDecision, GitAction, GitAuthorize, GitGuard, GitPolicy, GitProposal, GitReport,
    ReleasePolicy,
};
use v9r_core::kernel::{Requirement, Status, Verdict};
use v9r_core::manifest::Manifest;
use v9r_core::state::ContentHash;
use v9r_core::task::Task;
use v9r_core::trusted::StateRoot;

static HERMETIC: Once = Once::new();

/// Fixed identity and dates make commit ids reproducible; no user config.
fn hermetic_git() {
    HERMETIC.call_once(|| {
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_AUTHOR_NAME", "agent"),
            ("GIT_AUTHOR_EMAIL", "agent@example.invalid"),
            ("GIT_COMMITTER_NAME", "agent"),
            ("GIT_COMMITTER_EMAIL", "agent@example.invalid"),
            ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z"),
        ] {
            std::env::set_var(key, value);
        }
    });
}

struct Env {
    base: PathBuf,
    work: PathBuf,
    root: StateRoot,
    approved_base: Oid,
}

/// Privileged setup: `repo` (on branch `release`, at the approved base,
/// with `main` protected) and a bare `origin.git` it pushes to.
impl Env {
    fn new(name: &str) -> Self {
        hermetic_git();
        let base = std::env::temp_dir().join(format!("v9r-git-{name}-{}", Uuid::new_v4()));
        let work = base.join("work");
        fs::create_dir_all(&work).unwrap();
        let repo = work.join("repo");
        let setup = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        setup(&work, &["init", "-q", "-b", "main", "repo"]);
        fs::write(repo.join("README"), "approved base\n").unwrap();
        setup(&repo, &["add", "README"]);
        setup(&repo, &["commit", "-q", "-m", "approved base"]);
        let approved_base = Oid::parse(&setup(&repo, &["rev-parse", "HEAD"])).unwrap();
        setup(&repo, &["checkout", "-q", "-b", "release"]);
        setup(&work, &["clone", "-q", "--bare", "repo", "origin.git"]);
        setup(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                work.join("origin.git").to_str().unwrap(),
            ],
        );
        let root = StateRoot::open(&base.join("state")).unwrap();
        Self {
            base,
            work,
            root,
            approved_base,
        }
    }

    fn policy(&self) -> GitPolicy {
        GitPolicy::standard(
            Manifest {
                allow_read: vec![self.work.clone()],
                allow_write: vec![self.work.clone()],
                allow_exec: ["git", "mkdir", "touch", "cp", "tar"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                token_limit: 1,
                max_steps: 64,
                timeout_ms: 30_000,
                mandatory_artifacts: Vec::new(),
                test_commands: Vec::new(),
            },
            vec![
                GitRepo {
                    path: "repo".into(),
                    bare: false,
                },
                GitRepo {
                    path: "origin.git".into(),
                    bare: true,
                },
            ],
            ReleasePolicy {
                repo: "repo".into(),
                release_ref: "refs/heads/release".into(),
                approved_base: self.approved_base.clone(),
            },
            vec![
                "repo/HEAD".into(),
                "repo/refs/heads/release".into(),
                "repo/refs/heads/task".into(),
                "origin.git/refs/heads/task".into(),
            ],
        )
    }

    async fn start(&self) -> GitGuard {
        let policy = self.policy();
        let task = Task::new(Manifest::clone(&dummy_manifest()), self.work.clone());
        GitGuard::start(task, Arc::new(policy), self.root.clone())
            .await
            .unwrap()
    }

    /// An out-of-band actor (not the agent, not the runtime).
    fn out_of_band(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(self.work.join("repo"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn dummy_manifest() -> Manifest {
    Manifest {
        allow_read: Vec::new(),
        allow_write: Vec::new(),
        allow_exec: Vec::new(),
        token_limit: 1,
        max_steps: 1,
        timeout_ms: 1,
        mandatory_artifacts: Vec::new(),
        test_commands: Vec::new(),
    }
}

fn cmd(argv: &[&str], declared_refs: &[&str]) -> GitProposal {
    GitProposal {
        action: GitAction::Command(CommandSpec {
            program: argv[0].to_string(),
            args: argv[1..].iter().map(|s| s.to_string()).collect(),
            cwd: None,
            reads: Vec::new(),
            writes: Vec::new(),
        }),
        declared_refs: declared_refs.iter().map(|s| s.to_string()).collect(),
        proposer: "agent".into(),
    }
}

fn release(commit: &str, artifact: &str, manifest: ContentHash) -> GitProposal {
    GitProposal {
        action: GitAction::Release {
            commit: commit.to_string(),
            artifact: artifact.to_string(),
            manifest,
        },
        declared_refs: Vec::new(),
        proposer: "agent".into(),
    }
}

async fn run(guard: &mut GitGuard, proposal: GitProposal) -> (Verdict, Option<GitReport>) {
    match guard.authorize(proposal).await.unwrap() {
        GitAuthorize::Allowed(auth) => (Verdict::Allow, Some(guard.execute(*auth).await.unwrap())),
        other => (other.verdict(), None),
    }
}

/// Run and require PRE and POST to allow.
async fn ok(guard: &mut GitGuard, argv: &[&str], refs: &[&str]) {
    let (pre, report) = run(guard, cmd(argv, refs)).await;
    assert_eq!(pre, Verdict::Allow, "{argv:?}");
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Allow,
        "{argv:?}\n{}",
        report.decision
    );
}

const MOVES_RELEASE: &[&str] = &["repo/refs/heads/release", "repo/HEAD"];

/// Agent work: add a feature commit on `release`.
async fn commit_feature(guard: &mut GitGuard, file: &str) {
    ok(guard, &["touch", &format!("repo/{file}")], &[]).await;
    ok(guard, &["git", "-C", "repo", "add", file], &[]).await;
    ok(
        guard,
        &["git", "-C", "repo", "commit", "-q", "-m", file],
        MOVES_RELEASE,
    )
    .await;
}

/// Agent work: export `commit` into `dir` via git archive + tar.
async fn export(guard: &mut GitGuard, commit: &Oid, dir: &str) {
    let tar = format!("{dir}.tar");
    ok(
        guard,
        &[
            "git",
            "--git-dir=repo/.git",
            "archive",
            "-o",
            &tar,
            commit.as_str(),
        ],
        &[],
    )
    .await;
    ok(guard, &["mkdir", dir], &[]).await;
    ok(guard, &["tar", "-xf", &tar, "-C", dir], &[]).await;
}

async fn release_head(guard: &GitGuard) -> Oid {
    match guard
        .query(&GitSubject::Ref {
            repo: "repo".into(),
            name: "refs/heads/release".into(),
        })
        .await
    {
        Some(GitValue::Points { oid }) => oid,
        other => panic!("release ref: {other:?}"),
    }
}

async fn manifest_of(guard: &GitGuard, commit: &Oid) -> ContentHash {
    match guard
        .query(&GitSubject::ContentManifest {
            repo: "repo".into(),
            commit: commit.clone(),
        })
        .await
    {
        Some(GitValue::Digest { sha256 }) => sha256,
        other => panic!("manifest: {other:?}"),
    }
}

fn status<'a>(decision: &'a CrossDecision, invariant: &str) -> Vec<&'a Status> {
    let found: Vec<_> = decision
        .findings
        .iter()
        .filter(|f| f.obligation.invariant == invariant)
        .map(|f| &f.status)
        .collect();
    assert!(!found.is_empty(), "no {invariant} in\n{decision}");
    found
}

fn all_satisfied(decision: &CrossDecision, invariant: &str) -> bool {
    status(decision, invariant)
        .iter()
        .all(|s| matches!(s, Status::Satisfied(_)))
}

fn any_violated(decision: &CrossDecision, invariant: &str) -> bool {
    status(decision, invariant)
        .iter()
        .any(|s| matches!(s, Status::Violated(_)))
}

fn main_ref(env: &Env) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "refs/heads/main"])
        .current_dir(env.work.join("repo"))
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

// ---------------------------------------------------------------- baseline

#[tokio::test]
async fn honest_release_is_allowed() {
    let env = Env::new("honest");
    let mut guard = env.start().await;
    commit_feature(&mut guard, "feature.txt").await;
    let head = release_head(&guard).await;
    export(&mut guard, &head, "dist").await;
    let digest = manifest_of(&guard, &head).await;

    let (pre, report) = run(&mut guard, release(head.as_str(), "dist", digest)).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Allow,
        "{}",
        report.decision
    );
    assert!(all_satisfied(
        &report.decision,
        "G1.release_descends_from_base"
    ));
    assert!(all_satisfied(
        &report.decision,
        "G3.artifact_matches_verified_commit"
    ));
    assert_eq!(report.released, Some((head, digest)));
}

#[tokio::test]
async fn only_existing_requirement_forms_are_used() {
    let env = Env::new("forms");
    let mut guard = env.start().await;
    let head = release_head(&guard).await;
    let decision = guard
        .authorize(release(head.as_str(), "dist", ContentHash::of(b"")))
        .await
        .unwrap()
        .decision()
        .clone();
    for finding in &decision.findings {
        assert!(matches!(
            finding.obligation.requirement,
            Requirement::Fact { .. } | Requirement::Within { .. } | Requirement::AtMost { .. }
        ));
    }
}

#[tokio::test]
async fn worktree_state_is_computed_not_asked_of_git_status() {
    let env = Env::new("worktree");
    let guard = env.start().await;
    let subject = GitSubject::Worktree {
        repo: "repo".into(),
    };
    assert_eq!(guard.query(&subject).await, Some(GitValue::MatchesHead));
    fs::write(env.work.join("repo/untracked.txt"), "x").unwrap();
    assert_eq!(guard.query(&subject).await, Some(GitValue::DiffersFromHead));
}

// -------------------------------------------------------- G1 / rewritten history

#[tokio::test]
async fn rewritten_history_does_not_descend_from_approved_base() {
    let env = Env::new("amend");
    let mut guard = env.start().await;
    // `release` is at the approved base; amending rewrites that commit.
    ok(
        &mut guard,
        &[
            "git",
            "-C",
            "repo",
            "commit",
            "-q",
            "--amend",
            "-m",
            "rewritten base",
        ],
        MOVES_RELEASE,
    )
    .await;
    let head = release_head(&guard).await;
    assert_ne!(head, env.approved_base);
    export(&mut guard, &head, "dist").await;
    let digest = manifest_of(&guard, &head).await;

    let result = guard
        .authorize(release(head.as_str(), "dist", digest))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny, "{}", result.decision());
    assert!(any_violated(
        result.decision(),
        "G1.release_descends_from_base"
    ));
    // Content correspondence alone was fine: the verdict is about ancestry.
    assert!(all_satisfied(
        result.decision(),
        "G3.artifact_matches_verified_commit"
    ));
}

#[tokio::test]
async fn replace_ref_forging_ancestry_is_caught_twice() {
    let env = Env::new("replace");
    let mut guard = env.start().await;
    ok(
        &mut guard,
        &[
            "git",
            "-C",
            "repo",
            "commit",
            "-q",
            "--amend",
            "-m",
            "rewritten base",
        ],
        MOVES_RELEASE,
    )
    .await;
    let head = release_head(&guard).await;
    // Forge ancestry: make the rewritten commit claim the approved base as parent.
    let (pre, report) = run(
        &mut guard,
        cmd(
            &[
                "git",
                "-C",
                "repo",
                "replace",
                "--graft",
                head.as_str(),
                env.approved_base.as_str(),
            ],
            &[],
        ),
    )
    .await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    // 1. Creating refs/replace/* moves an unwritable ref.
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(any_violated(&report.decision, "G2.protected_refs_unmoved"));
    // 2. Even taken at face value, the observer ignores replace refs.
    let descends = GitSubject::Descends {
        repo: "repo".into(),
        ancestor: env.approved_base.clone(),
        descendant: head,
    };
    assert_eq!(guard.query(&descends).await, Some(GitValue::No));
}

#[tokio::test]
async fn graft_file_does_not_forge_ancestry() {
    let env = Env::new("graft");
    let mut guard = env.start().await;
    ok(
        &mut guard,
        &[
            "git",
            "-C",
            "repo",
            "commit",
            "-q",
            "--amend",
            "-m",
            "rewritten base",
        ],
        MOVES_RELEASE,
    )
    .await;
    let head = release_head(&guard).await;
    // A plain file write (no ref moves), which git itself honours. It is
    // written by a mediated command: an out-of-band write between steps
    // would be drift, and the next step would be refused.
    let graft = format!("--format=%H {}", env.approved_base.as_str());
    ok(
        &mut guard,
        &[
            "git",
            "--git-dir=repo/.git",
            "show",
            "-s",
            &graft,
            "--output=grafts",
            head.as_str(),
        ],
        &[],
    )
    .await;
    ok(&mut guard, &["cp", "grafts", "repo/.git/info/grafts"], &[]).await;
    let honoured = Command::new("git")
        .args([
            "merge-base",
            "--is-ancestor",
            env.approved_base.as_str(),
            head.as_str(),
        ])
        .current_dir(env.work.join("repo"))
        .status()
        .unwrap();
    assert!(honoured.success(), "plain git is fooled by the graft file");
    let descends = GitSubject::Descends {
        repo: "repo".into(),
        ancestor: env.approved_base.clone(),
        descendant: head,
    };
    assert_eq!(guard.query(&descends).await, Some(GitValue::No));
}

// ------------------------------------------------------- G2 / protected refs

#[tokio::test]
async fn honestly_declared_protected_ref_move_is_denied_before_execution() {
    let env = Env::new("protected-pre");
    let mut guard = env.start().await;
    commit_feature(&mut guard, "f.txt").await;
    let head = release_head(&guard).await;
    let before = main_ref(&env);
    let result = guard
        .authorize(cmd(
            &[
                "git",
                "-C",
                "repo",
                "update-ref",
                "refs/heads/main",
                head.as_str(),
            ],
            &["repo/refs/heads/main"],
        ))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(any_violated(result.decision(), "G2.protected_refs_unmoved"));
    assert_eq!(main_ref(&env), before);
}

#[tokio::test]
async fn undeclared_protected_ref_move_is_denied_after_and_rolled_back() {
    let env = Env::new("protected-post");
    let mut guard = env.start().await;
    commit_feature(&mut guard, "f.txt").await;
    let head = release_head(&guard).await;
    let before = main_ref(&env);
    let (_, report) = run(
        &mut guard,
        cmd(
            &[
                "git",
                "-C",
                "repo",
                "update-ref",
                "refs/heads/main",
                head.as_str(),
            ],
            &[],
        ),
    )
    .await;
    let report = report.unwrap();
    assert_eq!(report.decision.verdict, Verdict::Deny);
    let Status::Violated(because) = status(&report.decision, "G2.protected_refs_unmoved")[0] else {
        panic!()
    };
    assert!(because.contains("repo/refs/heads/main"), "{because}");
    assert!(!guard.is_accepting());
    assert_eq!(
        guard
            .authorize(cmd(&["git", "-C", "repo", "status"], &[]))
            .await
            .unwrap()
            .verdict(),
        Verdict::Deny
    );

    let rolled = guard.rollback().await.unwrap();
    assert_eq!(
        rolled.decision.verdict,
        Verdict::Allow,
        "{}",
        rolled.decision
    );
    assert!(all_satisfied(&rolled.decision, "G2.rollback_restores_refs"));
    assert_eq!(main_ref(&env), before);
    assert!(guard.is_accepting());
}

#[tokio::test]
async fn force_push_to_protected_remote_branch_is_denied() {
    let env = Env::new("force-push");
    let mut guard = env.start().await;
    ok(
        &mut guard,
        &[
            "git",
            "-C",
            "repo",
            "commit",
            "-q",
            "--amend",
            "-m",
            "rewritten",
        ],
        MOVES_RELEASE,
    )
    .await;
    let origin_main = |env: &Env| {
        let out = Command::new("git")
            .args(["--git-dir=origin.git", "rev-parse", "refs/heads/main"])
            .current_dir(&env.work)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let before = origin_main(&env);
    let push = [
        "git",
        "-C",
        "repo",
        "push",
        "-q",
        "--force",
        "origin",
        "release:main",
    ];

    // Declared honestly: refused before anything runs.
    let result = guard
        .authorize(cmd(&push, &["origin.git/refs/heads/main"]))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert_eq!(origin_main(&env), before);

    // Declared as a harmless task push: runs, is observed, denied, rolled back.
    let (pre, report) = run(&mut guard, cmd(&push, &["origin.git/refs/heads/task/x"])).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(
        report.result.as_ref().unwrap().is_ok(),
        "the push itself succeeded"
    );
    assert_ne!(
        origin_main(&env),
        before,
        "the forced update really happened"
    );
    assert_eq!(report.decision.verdict, Verdict::Deny);
    let Status::Violated(because) = status(&report.decision, "G2.protected_refs_unmoved")[0] else {
        panic!()
    };
    assert!(because.contains("origin.git/refs/heads/main"), "{because}");

    assert_eq!(
        guard.rollback().await.unwrap().decision.verdict,
        Verdict::Allow
    );
    assert_eq!(origin_main(&env), before);
}

// ------------------------------------------------- stale authorization

#[tokio::test]
async fn release_authorization_goes_stale_when_ref_moves() {
    let env = Env::new("stale-release");
    let mut guard = env.start().await;
    commit_feature(&mut guard, "f.txt").await;
    let head = release_head(&guard).await;
    export(&mut guard, &head, "dist").await;
    let digest = manifest_of(&guard, &head).await;
    let GitAuthorize::Allowed(auth) = guard
        .authorize(release(head.as_str(), "dist", digest))
        .await
        .unwrap()
    else {
        panic!("honest release should be allowed")
    };

    // Between authorize and execute, someone moves the release branch.
    env.out_of_band(&[
        "update-ref",
        "refs/heads/release",
        env.approved_base.as_str(),
    ]);

    let report = guard.execute(*auth).await.unwrap();
    assert!(!report.executed);
    assert_eq!(report.released, None);
    assert_eq!(report.decision.verdict, Verdict::Deny);
    assert!(any_violated(
        &report.decision,
        "G1.release_descends_from_base"
    ));
    assert!(any_violated(
        &report.decision,
        "authorization_basis_current"
    ));
    assert!(!guard.is_accepting());
}

#[tokio::test]
async fn command_authorization_goes_stale_when_any_ref_moves() {
    let env = Env::new("stale-command");
    let mut guard = env.start().await;
    let GitAuthorize::Allowed(auth) = guard
        .authorize(cmd(
            &["git", "-C", "repo", "branch", "task/a"],
            &["repo/refs/heads/task"],
        ))
        .await
        .unwrap()
    else {
        panic!()
    };
    env.out_of_band(&["tag", "sneaky"]);
    let report = guard.execute(*auth).await.unwrap();
    assert!(!report.executed);
    assert!(any_violated(
        &report.decision,
        "authorization_basis_current"
    ));
}

#[tokio::test]
async fn out_of_band_change_between_steps_is_not_absorbed() {
    // Before the shared runtime, each git authorization re-based on a fresh
    // scan, so a change made outside v9r *between* steps became the
    // trusted baseline unnoticed. Now the basis is the last accepted state.
    let env = Env::new("between-steps");
    let mut guard = env.start().await;
    ok(&mut guard, &["touch", "notes.txt"], &[]).await;
    env.out_of_band(&["tag", "planted"]);
    let (pre, report) = run(&mut guard, cmd(&["touch", "more.txt"], &[])).await;
    assert_eq!(pre, Verdict::Allow);
    let report = report.unwrap();
    assert!(!report.executed);
    assert!(any_violated(
        &report.decision,
        "authorization_basis_current"
    ));
    assert!(!guard.is_accepting());
    assert!(!env.work.join("more.txt").exists());
}

// ------------------------------------------------ malicious proposals

#[tokio::test]
async fn option_injection_witness_is_rejected_and_never_reaches_git() {
    let env = Env::new("inject");
    let mut guard = env.start().await;
    let result = guard
        .authorize(release("--output=pwned", "dist", ContentHash::of(b"")))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    assert!(any_violated(result.decision(), "well_formed_proposal"));
    assert!(!env.work.join("pwned").exists());
    assert!(!env.work.join("repo/pwned").exists());

    let result = guard
        .authorize(release("HEAD", "../outside", ContentHash::of(b"")))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
}

#[tokio::test]
async fn releasing_a_different_commit_under_the_release_name_is_denied() {
    let env = Env::new("wrong-commit");
    let mut guard = env.start().await;
    commit_feature(&mut guard, "f.txt").await;
    // The agent's witness is the approved base itself: it descends
    // trivially and its artifact is honest, but it is not the release head.
    let base = env.approved_base.clone();
    export(&mut guard, &base, "dist").await;
    let digest = manifest_of(&guard, &base).await;
    let result = guard
        .authorize(release(base.as_str(), "dist", digest))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny, "{}", result.decision());
    assert!(any_violated(
        result.decision(),
        "G3.artifact_matches_verified_commit"
    ));
}

#[tokio::test]
async fn tampered_artifact_is_denied_with_honest_or_forged_witness() {
    let env = Env::new("tampered");
    let mut guard = env.start().await;
    commit_feature(&mut guard, "f.txt").await;
    let head = release_head(&guard).await;
    export(&mut guard, &head, "dist").await;
    let honest = manifest_of(&guard, &head).await;
    ok(&mut guard, &["touch", "dist/backdoor"], &[]).await;

    // Honest witness: the artifact no longer matches it.
    let result = guard
        .authorize(release(head.as_str(), "dist", honest))
        .await
        .unwrap();
    assert_eq!(result.verdict(), Verdict::Deny);
    let g3 = status(result.decision(), "G3.artifact_matches_verified_commit");
    assert!(g3
        .iter()
        .any(|s| matches!(s, Status::Violated(b) if b.contains("ContentManifest(\"dist\")"))));

    // Forged witness matching the tampered artifact: the commit disagrees.
    let GitAuthorize::Denied(decision) = guard
        .authorize(release(
            head.as_str(),
            "dist",
            tampered_manifest(&env, "dist"),
        ))
        .await
        .unwrap()
    else {
        panic!()
    };
    let g3 = status(&decision, "G3.artifact_matches_verified_commit");
    assert!(g3
        .iter()
        .any(|s| matches!(s, Status::Violated(b) if b.contains("ContentManifest { repo"))));
}

/// The agent can compute the manifest of its own tampered directory.
fn tampered_manifest(env: &Env, dir: &str) -> ContentHash {
    let observation = v9r_core::effect::Observation::capture(&env.work).unwrap();
    v9r_core::content::from_fs_state(observation.state(), dir, &[]).unwrap()
}

#[tokio::test]
async fn hostile_repo_config_does_not_execute_in_the_observer() {
    let env = Env::new("fsmonitor");
    let mut guard = env.start().await;
    let marker = env.work.join("repo/observer-pwned");
    ok(
        &mut guard,
        &[
            "git",
            "-C",
            "repo",
            "config",
            "core.fsmonitor",
            "touch observer-pwned; false",
        ],
        &[],
    )
    .await;
    // Observation-only operations from here on.
    let head = release_head(&guard).await;
    let _ = guard
        .query(&GitSubject::Worktree {
            repo: "repo".into(),
        })
        .await;
    let _ = guard
        .authorize(release(head.as_str(), "repo", ContentHash::of(b"")))
        .await;
    assert!(
        !marker.exists(),
        "observer executed repo-configured fsmonitor"
    );
    // Sanity: plain `git status` in that repo would have run it.
    let _ = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(env.work.join("repo"))
        .output();
    assert!(marker.exists());
}

#[tokio::test]
async fn making_a_repo_unobservable_is_blocked_not_accepted() {
    let env = Env::new("alternates-step");
    let mut guard = env.start().await;
    let (pre, report) = run(
        &mut guard,
        cmd(&["touch", "repo/.git/objects/info/alternates"], &[]),
    )
    .await;
    assert_eq!(pre, Verdict::Allow);
    // After the step the repo cannot be observed, so "protected refs
    // unmoved" cannot be shown either way.
    let report = report.unwrap();
    assert_eq!(
        report.decision.verdict,
        Verdict::Blocked,
        "{}",
        report.decision
    );
    assert!(!guard.is_accepting());
}

#[tokio::test]
async fn borrowed_objects_make_release_unknown_not_denied() {
    let env = Env::new("alternates");
    // Pre-existing (privileged setup): the repo borrows objects.
    fs::write(env.work.join("repo/.git/objects/info/alternates"), "").unwrap();
    fs::create_dir_all(env.work.join("dist")).unwrap();
    fs::write(env.work.join("dist/README"), "approved base\n").unwrap();
    let digest = tampered_manifest(&env, "dist");
    let mut guard = env.start().await;
    let witness = env.approved_base.clone();
    let result = guard
        .authorize(release(witness.as_str(), "dist", digest))
        .await
        .unwrap();
    // No verified git facts exist: nothing can be shown violated or satisfied.
    assert_eq!(result.verdict(), Verdict::Blocked, "{}", result.decision());
    assert!(status(result.decision(), "G1.release_descends_from_base")
        .iter()
        .all(|s| matches!(s, Status::Undetermined(_))));
    // The filesystem half of G3 is established; only the git half is unknown.
    let g3 = status(result.decision(), "G3.artifact_matches_verified_commit");
    assert!(g3
        .iter()
        .any(|s| matches!(s, Status::Satisfied(b) if b.contains("Fs(ContentManifest"))));
}

// -------------------------------------------------------- determinism

#[tokio::test]
async fn git_decisions_are_deterministic_across_repos() {
    let mut records = Vec::new();
    for name in ["det-a", "det-b"] {
        let env = Env::new(name);
        let mut guard = env.start().await;
        commit_feature(&mut guard, "f.txt").await;
        let head = release_head(&guard).await;
        export(&mut guard, &head, "dist").await;
        let digest = manifest_of(&guard, &head).await;
        let mut decision = guard
            .authorize(release(head.as_str(), "dist", digest))
            .await
            .unwrap()
            .decision()
            .record("release");
        // The *filesystem* version differs between the two workspaces even
        // though every git fact is identical: `.git/index` embeds stat data
        // (inode, mtime). Normalize that basis out; everything else must match.
        for finding in &mut decision.findings {
            if let Status::Satisfied(because) = &mut finding.status {
                *because = normalize_workspace_digests(because);
            }
        }
        records.push((head, digest, decision));
    }
    assert_eq!(records[0], records[1]);
}

fn normalize_workspace_digests(text: &str) -> String {
    // Mask `sha256:<64 hex>` tokens that denote the filesystem version.
    let masked = text.contains("Fs(Workspace)");
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("sha256:") {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let hex = &tail[7..];
        let is_digest = hex.len() >= 64 && hex[..64].bytes().all(|b| b.is_ascii_hexdigit());
        if is_digest && (masked || head.ends_with("workspace-observation (")) {
            out.push_str("sha256:<workspace>");
            rest = &hex[64..];
        } else {
            out.push_str("sha256:");
            rest = hex;
        }
    }
    out.push_str(rest);
    out
}

#[tokio::test]
async fn filesystem_version_differs_while_git_state_is_identical() {
    // Documents the cross-domain granularity mismatch rather than hiding it.
    let mut versions = Vec::new();
    let mut git_refs = Vec::new();
    for name in ["gran-a", "gran-b"] {
        let env = Env::new(name);
        let guard = env.start().await;
        let observation = v9r_core::effect::Observation::capture(&env.work).unwrap();
        versions.push(observation.digest());
        git_refs.push(
            guard
                .query(&GitSubject::Refs {
                    repo: "repo".into(),
                })
                .await,
        );
    }
    assert_eq!(git_refs[0], git_refs[1]);
    assert_ne!(versions[0], versions[1]);
}
