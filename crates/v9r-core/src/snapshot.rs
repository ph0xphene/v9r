//! Content-addressed filesystem state.
//!
//! A snapshot turns "a directory contains these files" into git objects:
//! blobs (`blob <len>\0<bytes>`) and trees (`tree <len>\0` then
//! `<mode> <name>\0<32-byte id>` per entry, in git's order), identified
//! by SHA-256, exactly as git does with `--object-format=sha256`. A
//! snapshot's identity is its root tree id.
//!
//! ```text
//!   RawFsObserver ── readdir / lstat / read / readlink ──┐   (the only trusted observation)
//!                                                         ▼
//!   FsSnapshot (verifier) ── builds objects ── ObjectStore ── snapshot(dir) = root
//!                                                         │
//!   SnapshotObjects (verifier) ◀── snap_object(id) ───────┘   (pure: every object checked by id)
//!     snapshot_valid(root), snapshot_digest(root, def)
//!   MatchesSnapshot(def)        matches_snapshot(dir, approved)
//!   SnapshotCommitEquality      snapshot_matches_commit(root, repo, commit)
//! ```
//!
//! After a snapshot exists, nothing about it needs an observer: anyone may
//! store and serve its objects, and a wrong byte fails the hash. Only
//! `snapshot(dir)` (what a live directory contains now) rests on the
//! observer, and the walk cross-checks each directory listing against
//! the directory's link count (`2 + subdirectories` on filesystems that
//! keep it), so a listing cannot silently drop a subdirectory there.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};
use crate::verifiers::{digest_of, hex, observed, walk_tree, Walk, MANIFEST};
use crate::verify::{vouched, Basis, Inputs, Step, Verifier};

/// Definition under which two snapshots are the same iff their root ids
/// are: names, content and git file modes all count.
pub const IDENTITY: &str = "git-tree-sha256/1";

/// Store objects as git would, return the id.
fn object_id(kind: &str, body: &[u8]) -> (String, Vec<u8>) {
    let mut stored = format!("{kind} {}\0", body.len()).into_bytes();
    stored.extend_from_slice(body);
    (hex(&Sha256::digest(&stored)), stored)
}

fn unhex(id: &str) -> Option<Vec<u8>> {
    (0..id.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(id.get(i..i + 2)?, 16).ok())
        .collect()
}

fn snap_key(id: &str) -> Key {
    Key::new("snap_object", [id])
}

// ------------------------------------------------------------ store

/// Content-addressed objects. As a provider it is untrusted by design:
/// what it serves is checked against the id it was asked for.
#[derive(Clone, Default)]
pub struct ObjectStore(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

impl ObjectStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn put(&self, kind: &str, body: &[u8]) -> String {
        let (id, stored) = object_id(kind, body);
        self.0.lock().expect("store").insert(id.clone(), stored);
        id
    }

    /// Remove an object (simulates loss).
    pub fn remove(&self, id: &str) -> bool {
        self.0.lock().expect("store").remove(id).is_some()
    }

    /// Overwrite an object's bytes (simulates tampering).
    pub fn overwrite(&self, id: &str, bytes: Vec<u8>) {
        self.0.lock().expect("store").insert(id.to_string(), bytes);
    }

    pub fn ids(&self) -> Vec<String> {
        self.0.lock().expect("store").keys().cloned().collect()
    }
}

impl EvidenceProvider for ObjectStore {
    fn id(&self) -> &str {
        "object-store"
    }

    fn answers(&self, key: &Key) -> bool {
        key.kind == "snap_object" && key.args.len() == 1
    }

    fn provide(&self, keys: &[&Key], _: &Attestor) -> Vec<Answer> {
        let objects = self.0.lock().expect("store");
        keys.iter()
            .filter_map(|key| {
                Some(Answer::Proposed {
                    key: (*key).clone(),
                    value: Term::Bytes(objects.get(&key.args[0])?.clone()),
                    note: "stored object".into(),
                })
            })
            .collect()
    }
}

// ------------------------------------------------------------ snapshot creation

/// Derives `snapshot(dir)` = root tree id from primitive observations,
/// storing the objects. The one step that rests on an observer.
pub struct FsSnapshot {
    store: ObjectStore,
}

impl FsSnapshot {
    pub fn new(store: ObjectStore) -> Self {
        Self { store }
    }

    /// The tree id of `dir` (`None` if it holds no files: git stores no
    /// empty trees), and the observations it rests on.
    fn build(&self, inputs: &Inputs, dir: &str) -> Walk<(Option<String>, Vec<Basis>)> {
        let listing_key = Key::new("fs_dir", [dir]);
        let stat_key = Key::new("fs_stat", [dir]);
        let (listing, stat, mut basis) =
            match (observed(inputs, &listing_key), observed(inputs, &stat_key)) {
                (Walk::Done((Term::Map(l), b1)), Walk::Done((Term::Map(s), b2))) => {
                    (l, s, vec![b1, b2])
                }
                (Walk::Fail(r), _) | (_, Walk::Fail(r)) => return Walk::Fail(r),
                (Walk::Done(_), Walk::Done(_)) => return Walk::Fail(format!("{dir}: malformed")),
                _ => return Walk::Need(vec![listing_key, stat_key]),
            };
        // Completeness check independent of the listing: a directory's link
        // count is 2 + its subdirectories (where the filesystem keeps it).
        let subdirs = listing.values().filter(|m| *m == "40000").count();
        match stat.get("nlink").and_then(|n| n.parse::<usize>().ok()) {
            Some(n) if n >= 2 && n != 2 + subdirs => {
                return Walk::Fail(format!(
                    "{dir}: link count {n} implies {} subdirectories, listing shows {subdirs}",
                    n - 2
                ))
            }
            None => return Walk::Fail(format!("{dir}: no link count")),
            _ => {}
        }
        let mut entries = Vec::new();
        let mut need = Vec::new();
        for (name, mode) in &listing {
            let path = format!("{dir}/{name}");
            let id = match mode.as_str() {
                "40000" => match self.build(inputs, &path) {
                    Walk::Done((Some(id), more)) => {
                        basis.extend(more);
                        id
                    }
                    Walk::Done((None, more)) => {
                        basis.extend(more);
                        continue;
                    }
                    Walk::Need(k) => {
                        need.extend(k);
                        continue;
                    }
                    Walk::Fail(r) => return Walk::Fail(r),
                },
                "100644" | "100755" | "120000" => {
                    let input = if mode == "120000" {
                        Key::new("fs_link", [path.as_str()])
                    } else {
                        Key::new("fs_file", [path.as_str()])
                    };
                    match observed(inputs, &input) {
                        Walk::Done((Term::Bytes(b), via)) if mode != "120000" => {
                            basis.push(via);
                            self.store.put("blob", &b)
                        }
                        Walk::Done((Term::Text(t), via)) if mode == "120000" => {
                            basis.push(via);
                            self.store.put("blob", t.as_bytes())
                        }
                        Walk::Done(_) => return Walk::Fail(format!("{path}: unexpected value")),
                        Walk::Need(k) => {
                            need.extend(k);
                            continue;
                        }
                        Walk::Fail(r) => return Walk::Fail(format!("{path}: {r}")),
                    }
                }
                other => return Walk::Fail(format!("{path}: mode {other} not representable")),
            };
            entries.push((mode.clone(), name.clone(), id));
        }
        if !need.is_empty() {
            return Walk::Need(need);
        }
        if entries.is_empty() {
            return Walk::Done((None, basis));
        }
        // git's order: names compared as if trees ended in '/'.
        entries.sort_by_key(|(mode, name, _)| {
            let mut sort = name.clone().into_bytes();
            if mode == "40000" {
                sort.push(b'/');
            }
            sort
        });
        let mut body = Vec::new();
        for (mode, name, id) in &entries {
            body.extend(format!("{mode} {name}\0").into_bytes());
            body.extend(unhex(id).expect("our own hex"));
        }
        Walk::Done((Some(self.store.put("tree", &body)), basis))
    }
}

impl Verifier for FsSnapshot {
    fn id(&self) -> &str {
        "fs-snapshot"
    }

    fn derives(&self, key: &Key) -> bool {
        key.kind == "snapshot" && key.args.len() == 1
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        match self.build(inputs, &key.args[0]) {
            Walk::Done((root, basis)) => Step::Derived {
                value: Term::Id(root.unwrap_or_else(|| self.store.put("tree", b""))),
                basis,
            },
            Walk::Need(k) => Step::Need(k),
            Walk::Fail(r) => Step::Incomplete(r),
        }
    }
}

// ------------------------------------------------------------ pure verification

/// Derives facts about stored snapshots from their objects alone.
pub struct SnapshotObjects;

impl Verifier for SnapshotObjects {
    fn id(&self) -> &str {
        "snapshot-objects"
    }

    fn derives(&self, key: &Key) -> bool {
        matches!(
            (key.kind.as_str(), key.args.len()),
            ("snapshot_valid", 1) | ("snapshot_digest", 2)
        )
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        let def = key.args.get(1).map_or(MANIFEST, String::as_str);
        match walk_tree(inputs, &snap_key, &key.args[0], def) {
            Walk::Done((items, basis)) => {
                let value = if key.kind == "snapshot_valid" {
                    Term::Bool(true)
                } else {
                    match digest_of(def, items) {
                        Some(d) => Term::Digest(d),
                        None => return Step::Incomplete(format!("unknown definition {def}")),
                    }
                };
                Step::Derived { value, basis }
            }
            Walk::Need(k) => Step::Need(k),
            Walk::Fail(r) => Step::Incomplete(r),
        }
    }
}

/// `matches_snapshot(dir, approved)`: the directory's snapshot now is the
/// approved one, under a definition.
pub struct MatchesSnapshot {
    id: String,
    definition: String,
}

impl MatchesSnapshot {
    pub fn new(definition: &str) -> Self {
        Self {
            id: format!("matches/{definition}"),
            definition: definition.to_string(),
        }
    }
}

/// A derived input that some verifier vouched for.
fn derived(inputs: &Inputs, key: &Key) -> Walk<(Term, Basis)> {
    if !inputs.contains_key(key) {
        return Walk::Need(vec![key.clone()]);
    }
    match vouched(inputs, key) {
        Ok((value, by)) => Walk::Done((value, Basis::VouchedBy(key.clone(), by))),
        Err(reason) => Walk::Fail(reason),
    }
}

macro_rules! take {
    ($walk:expr, $basis:ident) => {
        match $walk {
            Walk::Done((value, via)) => {
                $basis.push(via);
                value
            }
            Walk::Need(k) => return Step::Need(k),
            Walk::Fail(r) => return Step::Incomplete(r),
        }
    };
}

impl Verifier for MatchesSnapshot {
    fn id(&self) -> &str {
        &self.id
    }

    fn derives(&self, key: &Key) -> bool {
        key.kind == "matches_snapshot" && key.args.len() == 2
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        let (dir, approved) = (&key.args[0], &key.args[1]);
        let mut basis = Vec::new();
        let Term::Id(now) = take!(
            derived(inputs, &Key::new("snapshot", [dir.as_str()])),
            basis
        ) else {
            return Step::Incomplete("snapshot is not an id".into());
        };
        let value = if self.definition == IDENTITY {
            take!(
                derived(inputs, &Key::new("snapshot_valid", [approved.as_str()])),
                basis
            );
            Term::Bool(&now == approved)
        } else {
            let def = self.definition.as_str();
            let a = take!(
                derived(inputs, &Key::new("snapshot_digest", [now.as_str(), def])),
                basis
            );
            let b = take!(
                derived(
                    inputs,
                    &Key::new("snapshot_digest", [approved.as_str(), def])
                ),
                basis
            );
            Term::Bool(a == b)
        };
        Step::Derived { value, basis }
    }
}

/// `snapshot_matches_commit(root, repo, commit)`: a stored snapshot has the
/// content of a git commit. No observer is involved on either side.
pub struct SnapshotCommitEquality;

impl Verifier for SnapshotCommitEquality {
    fn id(&self) -> &str {
        "snapshot-commit-equality"
    }

    fn derives(&self, key: &Key) -> bool {
        key.kind == "snapshot_matches_commit" && key.args.len() == 3
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        let mut basis = Vec::new();
        let a = take!(
            derived(
                inputs,
                &Key::new("snapshot_digest", [key.args[0].as_str(), MANIFEST])
            ),
            basis
        );
        let b = take!(
            derived(
                inputs,
                &Key::new(
                    "tree_digest",
                    [key.args[1].as_str(), &key.args[2], MANIFEST]
                )
            ),
            basis
        );
        Step::Derived {
            value: Term::Bool(a == b),
            basis,
        }
    }
}
