//! A primitive filesystem observer: it transcribes single system calls
//! and interprets nothing.
//!
//! | Kind | Args | Value | Call |
//! |---|---|---|---|
//! | `fs_dir` | `path` | `Map`: entry name → mode (`40000`, `100644`, `100755`, `120000`, `other`) | `readdir` + `lstat` per entry |
//! | `fs_stat` | `path` | `Map`: `nlink` → link count of the directory | `lstat` |
//! | `fs_file` | `path` | `Bytes`: the file's content | `read` |
//! | `fs_link` | `path` | `Text`: the symlink's target | `readlink` |
//!
//! It never walks, hashes, normalizes or decides what counts: the
//! snapshot verifier does all of that. Paths are root-relative, and no
//! component may be a symlink (it never follows one).

use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};

pub struct RawFsObserver {
    id: String,
    root: PathBuf,
}

impl RawFsObserver {
    pub fn new(id: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            root: root.into(),
        }
    }

    /// `root/path`, if `path` is relative and no component of it is a
    /// symlink (checked component by component).
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let mut full = self.root.clone();
        let relative = Path::new(path);
        let components: Vec<_> = relative.components().collect();
        if path.is_empty()
            || components
                .iter()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return None;
        }
        for (i, component) in components.iter().enumerate() {
            full.push(component);
            let meta = std::fs::symlink_metadata(&full).ok()?;
            if meta.file_type().is_symlink() && i + 1 < components.len() {
                return None;
            }
        }
        Some(full)
    }

    fn observe(&self, key: &Key) -> Option<Term> {
        let path = self.resolve(key.args.first()?)?;
        let meta = std::fs::symlink_metadata(&path).ok()?;
        Some(match key.kind.as_str() {
            "fs_dir" if meta.is_dir() => {
                let mut entries = BTreeMap::new();
                for entry in std::fs::read_dir(&path).ok()? {
                    let entry = entry.ok()?;
                    let meta = entry.metadata().ok()?;
                    let kind = meta.file_type();
                    let mode = if kind.is_dir() {
                        "40000"
                    } else if kind.is_symlink() {
                        "120000"
                    } else if kind.is_file() && meta.permissions().mode() & 0o100 != 0 {
                        "100755"
                    } else if kind.is_file() {
                        "100644"
                    } else {
                        "other"
                    };
                    entries.insert(entry.file_name().to_str()?.to_string(), mode.to_string());
                }
                Term::Map(entries)
            }
            "fs_stat" if meta.is_dir() => Term::Map(BTreeMap::from([(
                "nlink".to_string(),
                meta.nlink().to_string(),
            )])),
            "fs_file" if meta.is_file() => Term::Bytes(std::fs::read(&path).ok()?),
            "fs_link" if meta.file_type().is_symlink() => {
                Term::Text(std::fs::read_link(&path).ok()?.to_str()?.to_string())
            }
            _ => return None,
        })
    }
}

impl EvidenceProvider for RawFsObserver {
    fn id(&self) -> &str {
        &self.id
    }

    fn answers(&self, key: &Key) -> bool {
        matches!(
            key.kind.as_str(),
            "fs_dir" | "fs_stat" | "fs_file" | "fs_link"
        ) && key.args.len() == 1
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let value = self.observe(key)?;
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    value,
                    "syscall",
                )))
            })
            .collect()
    }
}
