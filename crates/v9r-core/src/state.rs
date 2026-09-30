//! Deterministic observation of filesystem state under a task workdir.
//!
//! This is the single crawler shared by checkpointing, rollback and
//! effect observation. It records *state*, never events: for every path
//! it knows the entry kind and, for regular files, the byte length and a
//! SHA-256 of the content. Two observations can be diffed to derive a
//! state delta; nothing here claims anything about *how* a state came to
//! be.
//!
//! Symlinks are recorded with their target string and are never
//! followed, neither for traversal nor for reading. Files are opened with
//! `O_NOFOLLOW | O_NONBLOCK` and re-checked against the `lstat` taken
//! during the walk, so a path swapped for a symlink or FIFO mid-scan is
//! reported as unobserved instead of being read through.
//!
//! Anything the walk cannot observe reliably (unreadable entries, entries
//! that vanish or change during the scan, non-UTF-8 names) is recorded in
//! `unobserved`. An unobserved path covers itself and everything below it.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// Top-level names under the workdir that belong to the runtime, not to
/// the task. They are excluded from checkpoints, rollback and effects.
/// `trace.jsonl` is the CLI's execution history; restoring it on rollback
/// would erase the record of what happened during the task.
pub const INTERNAL_PATHS: &[&str] = &[".v9r", "trace.jsonl"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// FIFO, socket or device node. Kind is recorded; content never read.
    Other,
}

/// SHA-256 of a byte string. Serialized as `"sha256:<hex>"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.strip_prefix("sha256:")
            .and_then(Self::from_hex)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid content hash: {s}")))
    }
}

/// Observable state of one path. Two entries are equal exactly when the
/// observable state is equal: same kind, and for files the same length
/// and content hash, for symlinks the same target string. Timestamps and
/// permission bits are deliberately not part of observable state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<ContentHash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_target: Option<String>,
}

impl Entry {
    fn file(bytes: &[u8]) -> Self {
        Self {
            kind: EntryKind::File,
            len: Some(bytes.len() as u64),
            sha256: Some(ContentHash::of(bytes)),
            link_target: None,
        }
    }

    fn bare(kind: EntryKind) -> Self {
        Self {
            kind,
            len: None,
            sha256: None,
            link_target: None,
        }
    }
}

/// A path whose state could not be established. Covers the path itself
/// and every path below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unobserved {
    pub path: String,
    pub reason: String,
}

/// Normalized state of a directory tree. Keys are `/`-separated paths
/// relative to the observed root. Ordering is lexicographic, so a parent
/// always sorts before its children.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsState {
    pub(crate) entries: BTreeMap<String, Entry>,
    #[serde(default)]
    pub(crate) unobserved: Vec<Unobserved>,
}

impl FsState {
    pub fn entries(&self) -> &BTreeMap<String, Entry> {
        &self.entries
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    pub fn unobserved(&self) -> &[Unobserved] {
        &self.unobserved
    }

    /// True when every path under the root was observed.
    pub fn is_complete(&self) -> bool {
        self.unobserved.is_empty()
    }

    /// The unobserved record covering `path` (the path itself or an
    /// ancestor), if any.
    pub fn unobserved_cover(&self, path: &str) -> Option<&Unobserved> {
        self.unobserved.iter().find(|u| {
            path == u.path
                || (path.len() > u.path.len()
                    && path.starts_with(u.path.as_str())
                    && path.as_bytes()[u.path.len()] == b'/')
        })
    }

    /// Deterministic digest of the whole observation. Equal states have
    /// equal digests regardless of when or where they were observed.
    pub fn digest(&self) -> ContentHash {
        let bytes = serde_json::to_vec(self).expect("FsState serialization is infallible");
        ContentHash::of(&bytes)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ObserveError {
    #[error("observation io at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// Observe the tree under `root`. Fails only if the root itself cannot be
/// listed; per-entry problems are recorded as `unobserved`.
pub fn observe(root: &Path) -> Result<FsState, ObserveError> {
    observe_with(root, |_, _| Ok(()))
}

/// Like [`observe`], but hands every regular file's bytes to `on_file`
/// exactly as they were hashed. Checkpointing uses this to copy backups
/// in the same pass, so the stored hash and the stored bytes cannot
/// disagree.
pub fn observe_with<F>(root: &Path, mut on_file: F) -> Result<FsState, ObserveError>
where
    F: FnMut(&str, &[u8]) -> Result<(), ObserveError>,
{
    let mut state = FsState::default();
    let root_meta = fs::symlink_metadata(root).map_err(|source| ObserveError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !root_meta.is_dir() {
        return Err(ObserveError::Io {
            path: root.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "root is not a directory"),
        });
    }
    walk(root, "", &root_meta, &mut state, &mut on_file)?;
    state.unobserved.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(state)
}

fn walk<F>(
    dir: &Path,
    dir_rel: &str,
    dir_meta: &fs::Metadata,
    state: &mut FsState,
    on_file: &mut F,
) -> Result<(), ObserveError>
where
    F: FnMut(&str, &[u8]) -> Result<(), ObserveError>,
{
    let is_root = dir_rel.is_empty();
    let listing = fs::read_dir(dir).and_then(|entries| {
        entries
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()
    });
    let mut names = match listing {
        Ok(names) => names,
        Err(source) if is_root => {
            return Err(ObserveError::Io {
                path: dir.to_path_buf(),
                source,
            })
        }
        Err(err) => {
            state.entries.remove(dir_rel);
            state.unobserved.push(Unobserved {
                path: dir_rel.to_string(),
                reason: format!("directory not listable: {err}"),
            });
            return Ok(());
        }
    };
    // If the directory was swapped (e.g. for a symlink) while we listed
    // it, the names may belong to some other tree. Refuse to use them.
    if !same_node(dir, dir_meta) {
        if is_root {
            return Err(ObserveError::Io {
                path: dir.to_path_buf(),
                source: io::Error::other("root changed during observation"),
            });
        }
        state.entries.remove(dir_rel);
        state.unobserved.push(Unobserved {
            path: dir_rel.to_string(),
            reason: "directory changed during observation".to_string(),
        });
        return Ok(());
    }
    names.sort();

    for name in names {
        let Some(name_str) = name.to_str() else {
            state.unobserved.push(Unobserved {
                path: join_rel(dir_rel, &name.to_string_lossy()),
                reason: "non-UTF-8 file name".to_string(),
            });
            continue;
        };
        if is_root && INTERNAL_PATHS.contains(&name_str) {
            continue;
        }
        let rel = join_rel(dir_rel, name_str);
        let abs = dir.join(&name);
        let meta = match fs::symlink_metadata(&abs) {
            Ok(meta) => meta,
            Err(err) => {
                let reason = if err.kind() == io::ErrorKind::NotFound {
                    "vanished during observation".to_string()
                } else {
                    format!("metadata unreadable: {err}")
                };
                state.unobserved.push(Unobserved { path: rel, reason });
                continue;
            }
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            match fs::read_link(&abs) {
                Ok(target) => match target.to_str() {
                    Some(target) => {
                        let mut entry = Entry::bare(EntryKind::Symlink);
                        entry.link_target = Some(target.to_string());
                        state.entries.insert(rel, entry);
                    }
                    None => state.unobserved.push(Unobserved {
                        path: rel,
                        reason: "non-UTF-8 symlink target".to_string(),
                    }),
                },
                Err(err) => state.unobserved.push(Unobserved {
                    path: rel,
                    reason: format!("symlink unreadable: {err}"),
                }),
            }
        } else if file_type.is_dir() {
            state
                .entries
                .insert(rel.clone(), Entry::bare(EntryKind::Dir));
            walk(&abs, &rel, &meta, state, on_file)?;
        } else if file_type.is_file() {
            match read_file_stable(&abs, &meta) {
                Ok(bytes) => {
                    on_file(&rel, &bytes)?;
                    state.entries.insert(rel, Entry::file(&bytes));
                }
                Err(reason) => state.unobserved.push(Unobserved { path: rel, reason }),
            }
        } else {
            state.entries.insert(rel, Entry::bare(EntryKind::Other));
        }
    }
    Ok(())
}

fn join_rel(dir_rel: &str, name: &str) -> String {
    if dir_rel.is_empty() {
        name.to_string()
    } else {
        format!("{dir_rel}/{name}")
    }
}

#[cfg(unix)]
fn same_node(path: &Path, expected: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).is_ok_and(|now| {
        now.file_type() == expected.file_type()
            && now.dev() == expected.dev()
            && now.ino() == expected.ino()
    })
}

#[cfg(not(unix))]
fn same_node(path: &Path, expected: &fs::Metadata) -> bool {
    fs::symlink_metadata(path).is_ok_and(|now| now.file_type() == expected.file_type())
}

/// Read a regular file without following symlinks and without blocking
/// on FIFOs, and reject the read if the file is not the one `lstat` saw
/// or if it changed while being read.
#[cfg(unix)]
pub(crate) fn read_file_stable(path: &Path, lstat: &fs::Metadata) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|err| format!("file unreadable: {err}"))?;
    let before = file
        .metadata()
        .map_err(|err| format!("file unreadable: {err}"))?;
    if !before.is_file() || before.dev() != lstat.dev() || before.ino() != lstat.ino() {
        return Err("file changed during observation".to_string());
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|err| format!("file unreadable: {err}"))?;
    let after = file
        .metadata()
        .map_err(|err| format!("file unreadable: {err}"))?;
    if after.len() != bytes.len() as u64
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
    {
        return Err("file changed during observation".to_string());
    }
    Ok(bytes)
}

#[cfg(not(unix))]
pub(crate) fn read_file_stable(path: &Path, _lstat: &fs::Metadata) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|err| format!("file unreadable: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("v9r-state-test-{name}-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn observes_files_dirs_and_empty_files() {
        let tmp = TempDir::new("basic");
        fs::create_dir_all(tmp.0.join("a/b")).unwrap();
        fs::write(tmp.0.join("a/b/f.txt"), "hi").unwrap();
        fs::write(tmp.0.join("empty"), "").unwrap();
        let state = observe(&tmp.0).unwrap();
        let keys: Vec<_> = state.entries().keys().cloned().collect();
        assert_eq!(keys, ["a", "a/b", "a/b/f.txt", "empty"]);
        assert_eq!(state.get("a/b/f.txt").unwrap().len, Some(2));
        assert_eq!(state.get("empty").unwrap().len, Some(0));
        assert_eq!(
            state.get("empty").unwrap().sha256,
            Some(ContentHash::of(b""))
        );
        assert!(state.is_complete());
    }

    #[test]
    fn excludes_internal_paths_only_at_top_level() {
        let tmp = TempDir::new("internal");
        fs::create_dir_all(tmp.0.join(".v9r/backups")).unwrap();
        fs::write(tmp.0.join(".v9r/backups/x"), "x").unwrap();
        fs::write(tmp.0.join("trace.jsonl"), "{}").unwrap();
        fs::create_dir_all(tmp.0.join("sub")).unwrap();
        fs::write(tmp.0.join("sub/trace.jsonl"), "agent file").unwrap();
        let state = observe(&tmp.0).unwrap();
        let keys: Vec<_> = state.entries().keys().cloned().collect();
        assert_eq!(keys, ["sub", "sub/trace.jsonl"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_recorded_not_followed() {
        let tmp = TempDir::new("symlink");
        let outside = TempDir::new("symlink-outside");
        fs::write(outside.0.join("secret"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside.0, tmp.0.join("dirlink")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret"), tmp.0.join("filelink")).unwrap();
        let state = observe(&tmp.0).unwrap();
        let keys: Vec<_> = state.entries().keys().cloned().collect();
        assert_eq!(keys, ["dirlink", "filelink"]);
        let link = state.get("filelink").unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
        assert_eq!(link.sha256, None, "symlink target content must not be read");
        assert_eq!(
            link.link_target.as_deref(),
            Some(outside.0.join("secret").to_str().unwrap())
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_recorded_without_blocking() {
        let tmp = TempDir::new("fifo");
        let path = std::ffi::CString::new(tmp.0.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let state = observe(&tmp.0).unwrap();
        assert_eq!(state.get("pipe").unwrap().kind, EntryKind::Other);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_unobserved_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("unreadable");
        fs::create_dir_all(tmp.0.join("locked")).unwrap();
        fs::write(tmp.0.join("locked/f"), "x").unwrap();
        fs::set_permissions(tmp.0.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let listable = fs::read_dir(tmp.0.join("locked")).is_ok();
        let state = observe(&tmp.0);
        fs::set_permissions(tmp.0.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        if listable {
            return; // running with CAP_DAC_OVERRIDE (root); nothing to test
        }
        let state = state.unwrap();
        assert!(state.get("locked").is_none());
        assert!(state.unobserved_cover("locked/f").is_some());
        assert!(state.unobserved_cover("lockedX").is_none());
    }

    #[test]
    fn digest_is_deterministic_across_trees() {
        let left = TempDir::new("digest-l");
        let right = TempDir::new("digest-r");
        for dir in [&left.0, &right.0] {
            fs::create_dir_all(dir.join("d")).unwrap();
            fs::write(dir.join("d/x"), "same").unwrap();
        }
        let l = observe(&left.0).unwrap();
        let r = observe(&right.0).unwrap();
        assert_eq!(l, r);
        assert_eq!(l.digest(), r.digest());
        fs::write(right.0.join("d/x"), "diff").unwrap();
        assert_ne!(l.digest(), observe(&right.0).unwrap().digest());
    }

    #[test]
    fn content_hash_round_trips() {
        let hash = ContentHash::of(b"abc");
        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(
            json,
            "\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\""
        );
        assert_eq!(serde_json::from_str::<ContentHash>(&json).unwrap(), hash);
    }
}
