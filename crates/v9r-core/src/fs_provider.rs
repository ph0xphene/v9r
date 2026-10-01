//! Filesystem evidence provider.
//!
//! | Kind | Args | Value |
//! |---|---|---|
//! | `dir_content` | `dir` (relative to the root) | `Digest`: the `crate::content` manifest of the directory |
//! | `fs_listing` | `dir`, or `.` | `Map`: every entry below it → `file`, `dir`, `symlink` or `other` (raw observation) |
//! | `fs_file` | `path` | `Bytes`: a regular file's content (raw observation) |
//! | `fs_link` | `path` | `Text`: a symlink's target (raw observation) |
//! | `entries` | `dir`, or `.` for the root | `Map`: every entry below it, root-relative path → `file:<sha256>`, `dir`, `symlink:<target>` or `other` |
//!
//! Lineage: every answer names its method and the tree digest it read;
//! `dir_content` claims the `crate::content` definition and depends on a
//! supporting `entries` fact for the same directory from the same walk.
//!
//! Each request observes the tree afresh. A directory that cannot be
//! observed, or does not exist, yields no answer; neither does `entries`
//! if any part of the tree below it could not be observed.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::content;
use crate::effect::Observation;
use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Method, Term};
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
            ("entries" | "fs_listing", [dir]) => dir == "." || relative_dir(dir),
            ("fs_file" | "fs_link", [path]) => relative_dir(path),
            _ => false,
        }
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let Ok(observation) = Observation::capture(&self.root) else {
            return Vec::new();
        };
        let state = format!("tree {}", observation.digest());
        let walk = |dir: &str| {
            Some(
                attestor
                    .attest(
                        Key::new("entries", [dir]),
                        Term::Map(entries(observation.state(), dir)?),
                        "tree walk (symlinks not followed)",
                    )
                    .observed(state.clone()),
            )
        };
        let mut out = Vec::new();
        for key in keys {
            let dir = key.args[0].as_str();
            match key.kind.as_str() {
                "entries" => out.extend(walk(dir).map(Answer::Verified)),
                "fs_listing" => {
                    let Some(Term::Map(entries)) = walk(dir).map(|a| a.fact().value.clone()) else {
                        continue;
                    };
                    let kinds = entries
                        .into_iter()
                        .map(|(path, value)| {
                            let kind = value.split(':').next().unwrap_or("other").to_string();
                            (path, kind)
                        })
                        .collect();
                    out.push(Answer::Verified(
                        attestor
                            .attest((*key).clone(), Term::Map(kinds), "directory walk")
                            .observed(state.clone()),
                    ));
                }
                "fs_file" | "fs_link" => {
                    let full = self.root.join(dir);
                    let Ok(meta) = std::fs::symlink_metadata(&full) else {
                        continue;
                    };
                    let value = match key.kind.as_str() {
                        "fs_file" if meta.file_type().is_file() => {
                            std::fs::read(&full).ok().map(Term::Bytes)
                        }
                        "fs_link" if meta.file_type().is_symlink() => std::fs::read_link(&full)
                            .ok()
                            .and_then(|t| t.to_str().map(|t| Term::Text(t.to_string()))),
                        _ => None,
                    };
                    if let Some(value) = value {
                        out.push(Answer::Verified(attestor.attest(
                            (*key).clone(),
                            value,
                            "read",
                        )));
                    }
                }
                _ => {
                    let (Some(support), Some(digest)) = (
                        walk(dir),
                        content::from_fs_state(observation.state(), dir, &[]),
                    ) else {
                        continue;
                    };
                    let attested = attestor
                        .attest(
                            (*key).clone(),
                            Term::Digest(digest.to_string()),
                            Method::new("content manifest of the tree walk")
                                .defined_as(content::DEFINITION),
                        )
                        .observed(state.clone())
                        .depends_on(&support);
                    out.push(Answer::Verified(support));
                    out.push(Answer::Verified(attested));
                }
            }
        }
        out
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
