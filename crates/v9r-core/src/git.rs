//! Git as an evidence domain.
//!
//! The observer turns repositories inside the workspace into facts:
//!
//! | Subject | Value | Stale-prone? |
//! |---|---|---|
//! | `Head { repo }` | `Head { symref, commit }` | yes (mutable pointer) |
//! | `Ref { repo, name }` | `Points { oid }` / `Absent` | yes |
//! | `Refs { repo }` | `Digest` of HEAD + every ref | yes |
//! | `Descends { repo, ancestor, descendant }` | `Yes` / `No` | no: content-addressed |
//! | `ContentManifest { repo, commit }` | `Digest` (see `crate::content`) | no: content-addressed |
//! | `Worktree { repo }` | `MatchesHead` / `DiffersFromHead` | yes |
//!
//! Relations such as ancestry are not a kernel concept: the observer
//! decides one *instance* (`Descends{a, d}`) at a time, for exactly the
//! oids an obligation names, and the result is an ordinary keyed fact.
//!
//! # Hardening
//!
//! The repositories are agent-writable, so their configuration and
//! metadata are hostile input. The observer:
//!
//! * runs only plumbing (`rev-parse`, `symbolic-ref`, `for-each-ref`,
//!   `merge-base`, `ls-tree`, `cat-file`), never porcelain that may run
//!   filter drivers (`git diff` does, as probed);
//! * clears the environment, ignores system/global config, and disables
//!   `core.fsmonitor` (which otherwise executes on `git status`) and hooks;
//! * ignores replace refs and the graft file, both of which can fake
//!   ancestry (probed: `merge-base --is-ancestor` flips to "yes");
//! * refuses repositories whose git dir is a symlink, a gitfile, a linked
//!   worktree (`commondir`), outside the workspace, or that borrow objects
//!   through `objects/info/alternates`;
//! * validates every oid before it reaches argv, so a witness can never
//!   be parsed as an option;
//! * computes working-tree state itself (content manifests) rather than
//!   trusting `git status`.
//!
//! `git archive` is deliberately not used for artifact identity: its
//! output depends on `.git/info/attributes` and `tar.umask`, which are
//! agent-writable and not part of the commit (probed).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};

use crate::content::{self, Item, ItemKind};
use crate::effect::Observation;
use crate::kernel::{EvidenceBase, Fact, Name, Provenance, Verified};
use crate::state::ContentHash;

/// A full hex object id (SHA-1 or SHA-256). Only constructible from a
/// validated string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Oid(String);

impl Oid {
    pub fn parse(s: &str) -> Option<Self> {
        let ok = matches!(s.len(), 40 | 64)
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        ok.then(|| Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Oid {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Oid::parse(&s).ok_or_else(|| format!("not an object id: {s:?}"))
    }
}

impl From<Oid> for String {
    fn from(oid: Oid) -> String {
        oid.0
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.0[..12])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "git", rename_all = "snake_case")]
pub enum GitSubject {
    Head {
        repo: String,
    },
    Ref {
        repo: String,
        name: String,
    },
    Refs {
        repo: String,
    },
    Descends {
        repo: String,
        ancestor: Oid,
        descendant: Oid,
    },
    ContentManifest {
        repo: String,
        commit: Oid,
    },
    Worktree {
        repo: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "value", rename_all = "snake_case")]
pub enum GitValue {
    Head {
        symref: Option<String>,
        commit: Option<Oid>,
    },
    Points {
        oid: Oid,
    },
    Absent,
    Digest {
        sha256: ContentHash,
    },
    Yes,
    No,
    MatchesHead,
    DiffersFromHead,
}

/// A repository inside the workspace, by workspace-relative path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepo {
    pub path: String,
    pub bare: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RepoState {
    git_dir: PathBuf,
    work_tree: Option<PathBuf>,
    head_symref: Option<String>,
    head: Option<Oid>,
    refs: BTreeMap<String, Oid>,
}

impl RepoState {
    fn digest(&self) -> ContentHash {
        let head = (&self.head_symref, self.head.as_ref().map(Oid::as_str));
        let refs: Vec<(&str, &str)> = self
            .refs
            .iter()
            .map(|(name, oid)| (name.as_str(), oid.as_str()))
            .collect();
        ContentHash::of(&serde_json::to_vec(&(head, refs)).expect("infallible"))
    }
}

/// Pointer state of every configured repository at one moment. Repos that
/// could not be observed carry the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitObservation {
    repos: BTreeMap<String, Result<RepoState, String>>,
}

impl GitObservation {
    /// Digest of one repo's pointers, if observed.
    pub fn refs_digest(&self, repo: &str) -> Option<ContentHash> {
        self.repos.get(repo)?.as_ref().ok().map(RepoState::digest)
    }

    pub fn ref_target(&self, repo: &str, name: &str) -> Option<Option<&Oid>> {
        let state = self.repos.get(repo)?.as_ref().ok()?;
        Some(state.refs.get(name))
    }

    /// Every ref of one repo (and `HEAD`), name → target, if observed.
    pub fn refs_map(&self, repo: &str) -> Option<BTreeMap<String, String>> {
        let state = self.repos.get(repo)?.as_ref().ok()?;
        let mut map: BTreeMap<String, String> = state
            .refs
            .iter()
            .map(|(name, oid)| (name.clone(), oid.as_str().to_string()))
            .collect();
        let head = match (&state.head_symref, &state.head) {
            (Some(symref), _) => format!("ref: {symref}"),
            (None, Some(oid)) => oid.as_str().to_string(),
            (None, None) => "unborn".to_string(),
        };
        map.insert("HEAD".to_string(), head);
        Some(map)
    }

    pub fn unobservable(&self) -> Vec<(&str, &str)> {
        self.repos
            .iter()
            .filter_map(|(repo, state)| state.as_ref().err().map(|e| (repo.as_str(), e.as_str())))
            .collect()
    }

    /// Refs whose target differs between two observations, as
    /// `<repo>/<refname>` names; an unobservable repo is an unknown subtree.
    pub fn touched_refs(pre: &Self, post: &Self) -> Vec<Name> {
        let mut names = Vec::new();
        let repos: std::collections::BTreeSet<&String> =
            pre.repos.keys().chain(post.repos.keys()).collect();
        for repo in repos {
            match (pre.repos.get(repo), post.repos.get(repo)) {
                (Some(Ok(a)), Some(Ok(b))) => {
                    let names_in: std::collections::BTreeSet<&String> =
                        a.refs.keys().chain(b.refs.keys()).collect();
                    for name in names_in {
                        if a.refs.get(name) != b.refs.get(name) {
                            names.push(Name::Known(format!("{repo}/{name}")));
                        }
                    }
                    if a.head_symref != b.head_symref || a.head != b.head {
                        names.push(Name::Known(format!("{repo}/HEAD")));
                    }
                }
                _ => names.push(Name::UnknownBelow(repo.clone())),
            }
        }
        names
    }
}

pub type GitEvidence = EvidenceBase<GitSubject, GitValue>;

pub struct GitObserver {
    workspace: PathBuf,
    repos: Vec<GitRepo>,
}

impl GitObserver {
    pub fn new(workspace: &Path, repos: Vec<GitRepo>) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            repos,
        }
    }

    pub fn repos(&self) -> &[GitRepo] {
        &self.repos
    }

    pub fn observe(&self) -> GitObservation {
        GitObservation {
            repos: self
                .repos
                .iter()
                .map(|repo| (repo.path.clone(), self.observe_repo(repo)))
                .collect(),
        }
    }

    fn observe_repo(&self, repo: &GitRepo) -> Result<RepoState, String> {
        let (git_dir, work_tree) = self.layout(repo)?;
        let head_symref = match self.git(&git_dir, &["symbolic-ref", "-q", "HEAD"], None)? {
            out if out.status.success() => Some(stdout_line(&out)?),
            out if out.status.code() == Some(1) => None,
            out => return Err(failure("symbolic-ref", &out)),
        };
        let head = match self.git(
            &git_dir,
            &["rev-parse", "-q", "--verify", "HEAD^{commit}"],
            None,
        )? {
            out if out.status.success() => Some(parse_oid(&stdout_line(&out)?)?),
            out if out.status.code() == Some(1) => None,
            out => return Err(failure("rev-parse HEAD", &out)),
        };
        let out = self.git(
            &git_dir,
            &["for-each-ref", "--format=%(objectname) %(refname)"],
            None,
        )?;
        if !out.status.success() {
            return Err(failure("for-each-ref", &out));
        }
        let mut refs = BTreeMap::new();
        for line in String::from_utf8(out.stdout)
            .map_err(|_| "non-UTF-8 ref name".to_string())?
            .lines()
        {
            let (oid, name) = line.split_once(' ').ok_or("malformed for-each-ref line")?;
            refs.insert(name.to_string(), parse_oid(oid)?);
        }
        Ok(RepoState {
            git_dir,
            work_tree,
            head_symref,
            head,
            refs,
        })
    }

    /// Resolve and vet a repository's layout. See module docs.
    fn layout(&self, repo: &GitRepo) -> Result<(PathBuf, Option<PathBuf>), String> {
        if repo
            .path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        {
            return Err(format!("invalid repository path {:?}", repo.path));
        }
        let root = self.workspace.join(&repo.path);
        let git_dir = if repo.bare {
            root.clone()
        } else {
            root.join(".git")
        };
        let workspace = fs::canonicalize(&self.workspace).map_err(|e| e.to_string())?;
        for dir in [&root, &git_dir] {
            let meta = fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            if !meta.is_dir() {
                return Err(format!("{} is not a real directory", dir.display()));
            }
            let canonical = fs::canonicalize(dir).map_err(|e| e.to_string())?;
            if !canonical.starts_with(&workspace) {
                return Err(format!("{} resolves outside the workspace", dir.display()));
            }
        }
        for foreign in ["commondir", "objects/info/alternates"] {
            if fs::symlink_metadata(git_dir.join(foreign)).is_ok() {
                return Err(format!("repository borrows state via {foreign}"));
            }
        }
        Ok((git_dir, (!repo.bare).then_some(root)))
    }

    fn git(&self, git_dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Output, String> {
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", "/nonexistent")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_GRAFT_FILE", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg("--no-replace-objects")
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(["-c", "core.untrackedCache=false", "-c", "safe.directory=*"])
            .arg(format!("--git-dir={}", git_dir.display()))
            .args(args)
            .current_dir(git_dir)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("spawn git: {e}"))?;
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(bytes)
                .map_err(|e| format!("git stdin: {e}"))?;
        }
        child.wait_with_output().map_err(|e| format!("git: {e}"))
    }

    /// Is `ancestor` an ancestor of (or equal to) `descendant`? `None` if
    /// git cannot tell (e.g. a missing object): unknown, never "no".
    fn descends(&self, git_dir: &Path, ancestor: &Oid, descendant: &Oid) -> Option<bool> {
        let out = self
            .git(
                git_dir,
                &[
                    "merge-base",
                    "--is-ancestor",
                    ancestor.as_str(),
                    descendant.as_str(),
                ],
                None,
            )
            .ok()?;
        match out.status.code() {
            Some(0) => Some(true),
            Some(1) => Some(false),
            _ => None,
        }
    }

    /// Content manifest of a commit's tree, from objects only.
    fn commit_manifest(&self, git_dir: &Path, commit: &Oid) -> Option<ContentHash> {
        let spec = format!("{}^{{commit}}", commit.as_str());
        let out = self
            .git(git_dir, &["rev-parse", "-q", "--verify", &spec], None)
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let out = self
            .git(
                git_dir,
                &["ls-tree", "-r", "-z", "--full-tree", commit.as_str()],
                None,
            )
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let mut entries = Vec::new();
        for record in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = std::str::from_utf8(record).ok()?;
            let (meta, path) = record.split_once('\t')?;
            let mut parts = meta.split(' ');
            let (mode, kind, oid) = (parts.next()?, parts.next()?, parse_oid(parts.next()?).ok()?);
            let kind = match (mode, kind) {
                ("100644" | "100755", "blob") => ItemKind::File,
                ("120000", "blob") => ItemKind::Symlink,
                _ => return None, // submodules and anything else: not representable
            };
            entries.push((path.to_string(), kind, oid));
        }
        let mut request = Vec::new();
        for (_, _, oid) in &entries {
            request.extend_from_slice(oid.as_str().as_bytes());
            request.push(b'\n');
        }
        let out = self
            .git(git_dir, &["cat-file", "--batch"], Some(&request))
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let mut rest = out.stdout.as_slice();
        let mut items = Vec::with_capacity(entries.len());
        for (path, kind, oid) in entries {
            let newline = rest.iter().position(|b| *b == b'\n')?;
            let header = std::str::from_utf8(&rest[..newline]).ok()?;
            let mut fields = header.split(' ');
            if fields.next()? != oid.as_str() || fields.next()? != "blob" {
                return None;
            }
            let size: usize = fields.next()?.parse().ok()?;
            let body = rest.get(newline + 1..newline + 1 + size)?;
            items.push(Item {
                path,
                kind,
                sha256: ContentHash::of(body),
            });
            rest = rest.get(newline + 2 + size..)?;
        }
        Some(content::digest(items))
    }

    /// Verified facts about `subjects`. `workspace` is needed only for
    /// `Worktree` subjects. Subjects that cannot be established yield no
    /// fact.
    pub(crate) fn evidence<'a>(
        &self,
        observation: &GitObservation,
        workspace: Option<&Observation>,
        subjects: impl IntoIterator<Item = &'a GitSubject>,
    ) -> GitEvidence {
        let mut evidence = GitEvidence::new();
        for subject in subjects {
            if let Some((value, basis)) = self.answer(observation, workspace, subject) {
                evidence.add_verified(
                    Verified::attest(Fact {
                        subject: subject.clone(),
                        value,
                    }),
                    Provenance {
                        observer: "git-plumbing".to_string(),
                        basis,
                    },
                );
            }
        }
        evidence
    }

    /// The value of one subject and the basis it was established on, or
    /// `None` if it cannot be established.
    pub(crate) fn answer(
        &self,
        observation: &GitObservation,
        workspace: Option<&Observation>,
        subject: &GitSubject,
    ) -> Option<(GitValue, String)> {
        let repo_name = match subject {
            GitSubject::Head { repo }
            | GitSubject::Ref { repo, .. }
            | GitSubject::Refs { repo }
            | GitSubject::Descends { repo, .. }
            | GitSubject::ContentManifest { repo, .. }
            | GitSubject::Worktree { repo } => repo,
        };
        let Some(Ok(state)) = observation.repos.get(repo_name) else {
            return None;
        };
        Some(match subject {
            GitSubject::Head { .. } => (
                GitValue::Head {
                    symref: state.head_symref.clone(),
                    commit: state.head.clone(),
                },
                format!("refs {}", state.digest()),
            ),
            GitSubject::Ref { name, .. } => (
                match state.refs.get(name) {
                    Some(oid) => GitValue::Points { oid: oid.clone() },
                    None => GitValue::Absent,
                },
                format!("refs {}", state.digest()),
            ),
            GitSubject::Refs { .. } => (
                GitValue::Digest {
                    sha256: state.digest(),
                },
                format!("refs {}", state.digest()),
            ),
            GitSubject::Descends {
                ancestor,
                descendant,
                ..
            } => {
                let value = match self.descends(&state.git_dir, ancestor, descendant)? {
                    true => GitValue::Yes,
                    false => GitValue::No,
                };
                (value, "merge-base --is-ancestor".to_string())
            }
            GitSubject::ContentManifest { commit, .. } => (
                GitValue::Digest {
                    sha256: self.commit_manifest(&state.git_dir, commit)?,
                },
                "ls-tree + cat-file".to_string(),
            ),
            GitSubject::Worktree { repo } => {
                let (Some(workspace), Some(_), Some(head)) =
                    (workspace, &state.work_tree, &state.head)
                else {
                    return None;
                };
                let tree = content::from_fs_state(workspace.state(), repo, &[".git"])?;
                let committed = self.commit_manifest(&state.git_dir, head)?;
                (
                    if tree == committed {
                        GitValue::MatchesHead
                    } else {
                        GitValue::DiffersFromHead
                    },
                    format!("workspace {} vs HEAD manifest", workspace.digest()),
                )
            }
        })
    }
}

fn stdout_line(out: &Output) -> Result<String, String> {
    String::from_utf8(out.stdout.clone())
        .map(|s| s.trim_end().to_string())
        .map_err(|_| "non-UTF-8 git output".to_string())
}

fn parse_oid(s: &str) -> Result<Oid, String> {
    Oid::parse(s).ok_or_else(|| format!("unexpected object id {s:?}"))
}

fn failure(what: &str, out: &Output) -> String {
    format!(
        "git {what} failed ({:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oids_are_validated_before_use() {
        assert!(Oid::parse(&"a".repeat(40)).is_some());
        assert!(Oid::parse(&"0".repeat(64)).is_some());
        for bad in [
            "HEAD",
            "--output=x",
            "main",
            &"A".repeat(40),
            &"a".repeat(39),
        ] {
            assert!(Oid::parse(bad).is_none(), "{bad}");
        }
        assert!(serde_json::from_str::<Oid>("\"--upload-pack=x\"").is_err());
    }
}
