//! Git evidence provider, over the hardened plumbing observer of
//! `crate::git`.
//!
//! | Kind | Args | Value |
//! |---|---|---|
//! | `ref` | `repo`, `refname` | `Id(oid)` or `Absent` |
//! | `descends` | `repo`, `ancestor oid`, `descendant oid` | `Bool` |
//! | `tree_content` | `repo`, `commit oid` | `Digest`: the `crate::content` manifest of the commit's tree |
//! | `refs` | `repo` | `Map`: every ref name (and `HEAD`) → target |
//! | `commit` | `repo`, `oid` | `Bool`: the object exists and is a commit |
//! | `git_object` | `repo`, `oid` | `Bytes`: the stored object, `<type> <size>\0<content>` (raw observation) |
//!
//! Lineage: every answer names its plumbing method and the state it read
//! (the repo's ref digest, or its object store for content-addressed
//! facts). `tree_content` claims the `crate::content` definition. Facts
//! about a commit (`tree_content`, `descends`) depend on a supporting
//! `commit` fact from the same response.
//!
//! Object ids are validated before anything reaches `git`; a key with a
//! malformed id is not answered.

use std::path::Path;

use crate::content;
use crate::git::{GitObserver, GitRepo, GitSubject, GitValue, Oid};
use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Method, Term};

pub struct GitEvidenceProvider {
    id: String,
    observer: GitObserver,
}

impl GitEvidenceProvider {
    /// `repos` are relative to `root`.
    pub fn new(id: impl Into<String>, root: &Path, repos: Vec<GitRepo>) -> Self {
        Self {
            id: id.into(),
            observer: GitObserver::new(root, repos),
        }
    }

    fn subject(&self, key: &Key) -> Option<GitSubject> {
        let args: Vec<&str> = key.args.iter().map(String::as_str).collect();
        let known = |repo: &str| self.observer.repos().iter().any(|r| r.path == repo);
        let subject = match (key.kind.as_str(), args.as_slice()) {
            ("ref", [repo, name]) => GitSubject::Ref {
                repo: repo.to_string(),
                name: name.to_string(),
            },
            ("descends", [repo, ancestor, descendant]) => GitSubject::Descends {
                repo: repo.to_string(),
                ancestor: Oid::parse(ancestor)?,
                descendant: Oid::parse(descendant)?,
            },
            ("commit", [repo, oid]) => GitSubject::IsCommit {
                repo: repo.to_string(),
                oid: Oid::parse(oid)?,
            },
            ("tree_content", [repo, commit]) => GitSubject::ContentManifest {
                repo: repo.to_string(),
                commit: Oid::parse(commit)?,
            },
            _ => return None,
        };
        known(args[0]).then_some(subject)
    }

    /// The repo and id of a `git_object(repo, oid)` key.
    fn object_key<'k>(&self, key: &'k Key) -> Option<(&'k str, Oid)> {
        match (key.kind.as_str(), key.args.as_slice()) {
            ("git_object", [repo, oid])
                if self.observer.repos().iter().any(|r| &r.path == repo) =>
            {
                Some((repo, Oid::parse(oid)?))
            }
            _ => None,
        }
    }

    /// The repo of a `refs(repo)` key.
    fn refs_repo<'k>(&self, key: &'k Key) -> Option<&'k str> {
        match (key.kind.as_str(), key.args.as_slice()) {
            ("refs", [repo]) if self.observer.repos().iter().any(|r| &r.path == repo) => Some(repo),
            _ => None,
        }
    }
}

fn method(subject: &GitSubject) -> Method {
    match subject {
        GitSubject::IsCommit { .. } => Method::new("cat-file -e, cat-file -t"),
        GitSubject::Descends { .. } => {
            Method::new("merge-base --is-ancestor (replace refs and grafts disabled)")
        }
        GitSubject::ContentManifest { .. } => {
            Method::new("ls-tree + cat-file").defined_as(content::DEFINITION)
        }
        _ => Method::new("for-each-ref"),
    }
}

/// The commit a fact is about, for facts that presuppose one.
fn about_commit(subject: &GitSubject) -> Option<(&str, &Oid)> {
    match subject {
        GitSubject::ContentManifest { repo, commit } => Some((repo, commit)),
        GitSubject::Descends {
            repo, descendant, ..
        } => Some((repo, descendant)),
        _ => None,
    }
}

fn term(value: GitValue) -> Option<Term> {
    Some(match value {
        GitValue::Points { oid } => Term::Id(oid.as_str().to_string()),
        GitValue::Absent => Term::Absent,
        GitValue::Yes => Term::Bool(true),
        GitValue::No => Term::Bool(false),
        GitValue::Digest { sha256 } => Term::Digest(sha256.to_string()),
        GitValue::Head { .. } | GitValue::MatchesHead | GitValue::DiffersFromHead => return None,
    })
}

impl EvidenceProvider for GitEvidenceProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn answers(&self, key: &Key) -> bool {
        self.subject(key).is_some()
            || self.refs_repo(key).is_some()
            || self.object_key(key).is_some()
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let observation = self.observer.observe();
        let mut out = Vec::new();
        for key in keys {
            if let Some((repo, oid)) = self.object_key(key) {
                // Raw: the bytes the id is the hash of. Checkable by anyone.
                if let Some(stored) = self.observer.object(&observation, repo, &oid) {
                    out.push(Answer::Verified(attestor.attest(
                        (*key).clone(),
                        Term::Bytes(stored),
                        "cat-file --batch",
                    )));
                }
                continue;
            }
            if let Some(repo) = self.refs_repo(key) {
                let (Some(refs), Some(digest)) =
                    (observation.refs_map(repo), observation.refs_digest(repo))
                else {
                    continue;
                };
                out.push(Answer::Verified(
                    attestor
                        .attest((*key).clone(), Term::Map(refs), "for-each-ref")
                        .observed(format!("refs {digest}")),
                ));
                continue;
            }
            let Some(subject) = self.subject(key) else {
                continue;
            };
            let Some(value) = self
                .observer
                .answer(&observation, None, &subject)
                .and_then(|(value, _)| term(value))
            else {
                continue;
            };
            let state = |repo: &str| match &subject {
                GitSubject::Ref { .. } => observation
                    .refs_digest(repo)
                    .map_or("refs unknown".to_string(), |d| format!("refs {d}")),
                // Content-addressed: no ref state is involved.
                _ => format!("object store of {repo}"),
            };
            let mut attested = attestor
                .attest((*key).clone(), value, method(&subject))
                .observed(state(&key.args[0]));
            // Show the work: the commit the fact is about exists.
            if let Some((repo, oid)) = about_commit(&subject) {
                let support = GitSubject::IsCommit {
                    repo: repo.to_string(),
                    oid: oid.clone(),
                };
                if let Some(value) = self
                    .observer
                    .answer(&observation, None, &support)
                    .and_then(|(value, _)| term(value))
                {
                    let support = attestor
                        .attest(
                            Key::new("commit", [repo, oid.as_str()]),
                            value,
                            method(&support),
                        )
                        .observed(state(repo));
                    attested = attested.depends_on(&support);
                    out.push(Answer::Verified(support));
                }
            }
            out.push(Answer::Verified(attested));
        }
        out
    }
}
