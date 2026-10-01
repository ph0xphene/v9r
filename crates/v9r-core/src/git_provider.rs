//! Git evidence provider, over the hardened plumbing observer of
//! `crate::git`.
//!
//! | Kind | Args | Value |
//! |---|---|---|
//! | `ref` | `repo`, `refname` | `Id(oid)` or `Absent` |
//! | `descends` | `repo`, `ancestor oid`, `descendant oid` | `Bool` |
//! | `tree_content` | `repo`, `commit oid` | `Digest`: the `crate::content` manifest of the commit's tree |
//!
//! Object ids are validated before anything reaches `git`; a key with a
//! malformed id is not answered.

use std::path::Path;

use crate::git::{GitObserver, GitRepo, GitSubject, GitValue, Oid};
use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};

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
            ("tree_content", [repo, commit]) => GitSubject::ContentManifest {
                repo: repo.to_string(),
                commit: Oid::parse(commit)?,
            },
            _ => return None,
        };
        known(args[0]).then_some(subject)
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
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let observation = self.observer.observe();
        keys.iter()
            .filter_map(|key| {
                let subject = self.subject(key)?;
                let (value, basis) = self.observer.answer(&observation, None, &subject)?;
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    term(value)?,
                    basis,
                )))
            })
            .collect()
    }
}
