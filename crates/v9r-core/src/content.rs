//! A normal form for "the content of a tree", shared by domains.
//!
//! Two observers that describe the same content in different ways (a
//! directory on disk, a git tree) can only be compared if both reduce to
//! the same value. The kernel compares values; *what counts as the same
//! content* is defined here, once:
//!
//! * the set of `(path, kind, sha256(content))` for every regular file and
//!   symlink, paths `/`-separated and relative to the tree root;
//! * symlink content is its target string;
//! * directories are implied by paths (empty directories do not count,
//!   as in git);
//! * permission bits do not count (the filesystem observer does not
//!   record them), so an executable bit difference is invisible;
//! * anything else (special files, git submodules) makes the tree
//!   non-representable: no manifest, hence no fact.

use serde::Serialize;

use crate::state::{ContentHash, EntryKind, FsState};

/// Name of this normal form, for provenance: two digests are comparable
/// only if both were computed by it.
pub const DEFINITION: &str = "v9r-content-manifest/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    File,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Item {
    pub path: String,
    pub kind: ItemKind,
    pub sha256: ContentHash,
}

/// Digest of a set of items (order-independent).
pub fn digest(mut items: Vec<Item>) -> ContentHash {
    items.sort();
    ContentHash::of(&serde_json::to_vec(&items).expect("manifest serialization is infallible"))
}

/// Content manifest of the subtree `dir` ("" for the whole state),
/// skipping any top-level names in `exclude`. `None` if part of the
/// subtree is unobserved, contains special files, or `dir` is not a
/// directory.
pub fn from_fs_state(state: &FsState, dir: &str, exclude: &[&str]) -> Option<ContentHash> {
    if !dir.is_empty() && state.get(dir).map(|e| e.kind) != Some(EntryKind::Dir) {
        return None;
    }
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let excluded =
        |relative: &str| exclude.contains(&relative.split('/').next().unwrap_or_default());
    if state.unobserved_cover(dir).is_some()
        || state.unobserved().iter().any(|u| {
            u.path
                .strip_prefix(&prefix)
                .is_some_and(|relative| !excluded(relative))
        })
    {
        return None;
    }
    let mut items = Vec::new();
    for (path, entry) in state.entries() {
        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };
        if excluded(relative) {
            continue;
        }
        let (kind, sha256) = match entry.kind {
            EntryKind::Dir => continue,
            EntryKind::File => (ItemKind::File, entry.sha256?),
            EntryKind::Symlink => (
                ItemKind::Symlink,
                ContentHash::of(entry.link_target.as_deref()?.as_bytes()),
            ),
            EntryKind::Other => return None,
        };
        items.push(Item {
            path: relative.to_string(),
            kind,
            sha256,
        });
    }
    Some(digest(items))
}
