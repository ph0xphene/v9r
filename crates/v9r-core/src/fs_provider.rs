//! Filesystem evidence provider.
//!
//! | Kind | Args | Value |
//! |---|---|---|
//! | `dir_content` | `dir` (relative to the root) | `Digest`: the `crate::content` manifest of the directory |
//!
//! Each request observes the tree afresh. A directory that cannot be
//! observed, or does not exist, yields no answer.

use std::path::{Component, Path, PathBuf};

use crate::content;
use crate::effect::Observation;
use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};

pub struct FilesystemEvidenceProvider {
    id: String,
    root: PathBuf,
}

impl FilesystemEvidenceProvider {
    pub fn new(id: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            root: root.into(),
        }
    }
}

/// A non-empty relative path with only normal components.
fn relative_dir(arg: &str) -> bool {
    !arg.is_empty()
        && Path::new(arg)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

impl EvidenceProvider for FilesystemEvidenceProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn answers(&self, key: &Key) -> bool {
        key.kind == "dir_content" && key.args.len() == 1 && relative_dir(&key.args[0])
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let Ok(observation) = Observation::capture(&self.root) else {
            return Vec::new();
        };
        keys.iter()
            .filter_map(|key| {
                let digest = content::from_fs_state(observation.state(), &key.args[0], &[])?;
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    Term::Digest(digest.to_string()),
                    format!("tree observation {}", observation.digest()),
                )))
            })
            .collect()
    }
}
