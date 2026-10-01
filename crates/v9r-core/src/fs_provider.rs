//! Filesystem evidence provider.
//!
//! | Kind | Args | Value |
//! |---|---|---|
//! | `dir_content` | `dir` (relative to the root) | `Digest`: the `crate::content` manifest of the directory |
//! | `entries` | `dir`, or `.` for the root | `Map`: every entry below it, root-relative path → `file:<sha256>`, `dir`, `symlink:<target>` or `other` |
//!
//! Each request observes the tree afresh. A directory that cannot be
//! observed, or does not exist, yields no answer; neither does `entries`
//! if any part of the tree below it could not be observed.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::content;
use crate::effect::Observation;
use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};
use crate::state::{EntryKind, FsState};

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
        match (key.kind.as_str(), key.args.as_slice()) {
            ("dir_content", [dir]) => relative_dir(dir),
            ("entries", [dir]) => dir == "." || relative_dir(dir),
            _ => false,
        }
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let Ok(observation) = Observation::capture(&self.root) else {
            return Vec::new();
        };
        keys.iter()
            .filter_map(|key| {
                let value = match key.kind.as_str() {
                    "entries" => Term::Map(entries(observation.state(), &key.args[0])?),
                    _ => Term::Digest(
                        content::from_fs_state(observation.state(), &key.args[0], &[])?.to_string(),
                    ),
                };
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    value,
                    format!("tree observation {}", observation.digest()),
                )))
            })
            .collect()
    }
}

/// Every entry strictly below `dir` (`.`: the whole tree), or `None` if
/// part of it was not observed.
fn entries(state: &FsState, dir: &str) -> Option<BTreeMap<String, String>> {
    let below = |path: &str| {
        dir == "."
            || path
                .strip_prefix(dir)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    let hidden = state
        .unobserved()
        .iter()
        .any(|u| dir == "." || u.path == dir || below(&u.path));
    if hidden || (dir != "." && state.unobserved_cover(dir).is_some()) {
        return None;
    }
    Some(
        state
            .entries()
            .iter()
            .filter(|(path, _)| below(path))
            .map(|(path, entry)| {
                let value = match entry.kind {
                    EntryKind::File => match entry.sha256 {
                        Some(sha256) => format!("file:{}", sha256.to_hex()),
                        None => "other".to_string(),
                    },
                    EntryKind::Dir => "dir".to_string(),
                    EntryKind::Symlink => format!(
                        "symlink:{}",
                        entry.link_target.as_deref().unwrap_or_default()
                    ),
                    EntryKind::Other => "other".to_string(),
                };
                (path.clone(), value)
            })
            .collect(),
    )
}
