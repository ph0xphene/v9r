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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};
use crate::verifiers::{digest_of, hex, observed, walk_tree, Walk, MANIFEST};
use crate::verify::{vouched, Basis, Candidate, Inputs, Step, Verifier};

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

    /// An object's type and body, if stored *and* its bytes hash to `id`.
    pub fn get_verified(&self, id: &str) -> Result<(String, Vec<u8>), String> {
        let stored = self
            .0
            .lock()
            .expect("store")
            .get(id)
            .cloned()
            .ok_or_else(|| format!("object {id} not stored"))?;
        if hex(&Sha256::digest(&stored)) != id {
            return Err(format!("object {id}: stored bytes do not hash to it"));
        }
        let nul = stored
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| format!("object {id}: no header"))?;
        let header = std::str::from_utf8(&stored[..nul]).map_err(|e| e.to_string())?;
        let (kind, _) = header
            .split_once(' ')
            .ok_or_else(|| format!("object {id}: bad header"))?;
        Ok((kind.to_string(), stored[nul + 1..].to_vec()))
    }
}

/// Snapshot `dir` from a transcription made elsewhere (for example by the
/// probe inside a world): the same derivation as `snapshot(dir)`, with
/// every primitive observation taken from `transcript`. An observation
/// the transcript lacks leaves the snapshot incomplete.
pub fn snapshot_from(
    store: &ObjectStore,
    transcript: &BTreeMap<Key, Term>,
    dir: &str,
    vouched_by: &str,
) -> Result<String, String> {
    let verifier = FsSnapshot::new(store.clone());
    let key = Key::new("snapshot", [dir]);
    let mut inputs = Inputs::new();
    for _ in 0..100_000 {
        match verifier.step(&key, &inputs) {
            Step::Derived {
                value: Term::Id(root),
                ..
            } => return Ok(root),
            Step::Derived { value, .. } => return Err(format!("unexpected {value:?}")),
            Step::Incomplete(reason) => return Err(reason),
            Step::Need(keys) => {
                for k in keys {
                    let candidates = transcript
                        .get(&k)
                        .map(|v| {
                            vec![Candidate {
                                value: v.clone(),
                                vouched_by: Some(vouched_by.to_string()),
                            }]
                        })
                        .unwrap_or_default();
                    if inputs.insert(k.clone(), candidates).is_some() {
                        return Err(format!("{k:?} asked twice"));
                    }
                }
            }
        }
    }
    Err("snapshot did not converge".into())
}

/// A snapshot tree loaded from a store, every object already checked.
enum Node {
    Tree(Vec<(String, Node)>),
    File { executable: bool, bytes: Vec<u8> },
    Link(String),
}

fn load(store: &ObjectStore, id: &str, depth: usize) -> Result<Node, String> {
    if depth > 256 {
        return Err("tree too deep".into());
    }
    let (kind, body) = store.get_verified(id)?;
    if kind != "tree" {
        return Err(format!("{id} is a {kind}, not a tree"));
    }
    let mut out = Vec::new();
    let mut rest = &body[..];
    while !rest.is_empty() {
        let sp = rest
            .iter()
            .position(|&b| b == b' ')
            .ok_or("bad tree entry")?;
        let nul = rest.iter().position(|&b| b == 0).ok_or("bad tree entry")?;
        if nul < sp || rest.len() < nul + 33 {
            return Err("bad tree entry".into());
        }
        let mode = std::str::from_utf8(&rest[..sp]).map_err(|e| e.to_string())?;
        let name = std::str::from_utf8(&rest[sp + 1..nul]).map_err(|e| e.to_string())?;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(format!("unsafe entry name {name:?}"));
        }
        let child = hex(&rest[nul + 1..nul + 33]);
        let node = match mode {
            "40000" => load(store, &child, depth + 1)?,
            "100644" | "100755" | "120000" => {
                let (kind, bytes) = store.get_verified(&child)?;
                if kind != "blob" {
                    return Err(format!("{name}: {kind}, expected blob"));
                }
                if mode == "120000" {
                    Node::Link(String::from_utf8(bytes).map_err(|e| e.to_string())?)
                } else {
                    Node::File {
                        executable: mode == "100755",
                        bytes,
                    }
                }
            }
            other => return Err(format!("{name}: mode {other}")),
        };
        out.push((name.to_string(), node));
        rest = &rest[nul + 33..];
    }
    Ok(Node::Tree(out))
}

fn write_out(node: &Node, dest: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let err = |e: std::io::Error| format!("{dest:?}: {e}");
    match node {
        Node::Tree(entries) => {
            std::fs::create_dir(dest).map_err(err)?;
            for (name, child) in entries {
                write_out(child, &dest.join(name))?;
            }
        }
        Node::File { executable, bytes } => {
            std::fs::write(dest, bytes).map_err(err)?;
            let mode = if *executable { 0o555 } else { 0o444 };
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode)).map_err(err)?;
        }
        Node::Link(target) => std::os::unix::fs::symlink(target, dest).map_err(err)?,
    }
    Ok(())
}

/// One entry of a snapshot, relative to its root, parents before
/// children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapEntry {
    Dir(String),
    File {
        path: String,
        executable: bool,
        bytes: Vec<u8>,
    },
    Link {
        path: String,
        target: String,
    },
}

/// Every entry of snapshot `root`, each object checked against its id
/// before any entry is returned.
pub fn entries(store: &ObjectStore, root: &str) -> Result<Vec<SnapEntry>, String> {
    fn flatten(node: &Node, prefix: &str, out: &mut Vec<SnapEntry>) {
        let Node::Tree(children) = node else { return };
        for (name, child) in children {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match child {
                Node::Tree(_) => {
                    out.push(SnapEntry::Dir(path.clone()));
                    flatten(child, &path, out);
                }
                Node::File { executable, bytes } => out.push(SnapEntry::File {
                    path,
                    executable: *executable,
                    bytes: bytes.clone(),
                }),
                Node::Link(target) => out.push(SnapEntry::Link {
                    path,
                    target: target.clone(),
                }),
            }
        }
    }
    let tree = load(store, root, 0)?;
    let mut out = Vec::new();
    flatten(&tree, "", &mut out);
    Ok(out)
}

/// Write the snapshot `root` out of `store` under `dest` (which must not
/// exist), with git's modes. The whole tree is loaded and every object
/// checked against its id first: a snapshot that fails writes nothing.
pub fn materialize(store: &ObjectStore, root: &str, dest: &std::path::Path) -> Result<(), String> {
    let tree = load(store, root, 0)?;
    write_out(&tree, dest)
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

/// git's mode for an `lstat` result (`fs_meta`).
fn git_mode(meta: &BTreeMap<String, String>) -> String {
    let mode = meta
        .get("mode")
        .and_then(|m| u32::from_str_radix(m, 8).ok())
        .unwrap_or(0);
    match mode & 0o170000 {
        0o040000 => "40000",
        0o120000 => "120000",
        0o100000 if mode & 0o100 != 0 => "100755",
        0o100000 => "100644",
        _ => "other",
    }
    .to_string()
}

/// How a snapshot is captured. The strategies combine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capture {
    /// A: walk a second time afterwards; require the same root id.
    pub double_walk: bool,
    /// B: `lstat` every directory and entry before reading it and again
    /// after the walk; require identical metadata (inode, mode, size, link
    /// count, mtime, ctime).
    pub recheck: bool,
    /// Change journal: watch each directory (inotify) before listing it;
    /// require that no change event occurred until the end of capture.
    pub watch: bool,
}

/// Derives `snapshot(dir)` = root tree id from primitive observations,
/// storing the objects. The one step that rests on an observer.
pub struct FsSnapshot {
    store: ObjectStore,
    capture: Capture,
}

static SESSIONS: AtomicU64 = AtomicU64::new(1);

/// One walk's parameters.
struct Walker<'a> {
    /// Reading tag, so that a second walk is a second set of observations.
    tag: Option<&'a str>,
    meta: bool,
    session: Option<&'a str>,
}

impl Walker<'_> {
    fn key(&self, kind: &str, path: &str) -> Key {
        match self.tag {
            Some(tag) => Key::new(kind, [path, tag]),
            None => Key::new(kind, [path]),
        }
    }
}

/// A finished walk: root tree (`None` if no files), what it rests on, and
/// every path whose metadata was taken.
struct Built {
    root: Option<String>,
    basis: Vec<Basis>,
    paths: Vec<String>,
}

/// Take `key` in order: `Need` it alone if absent.
macro_rules! first {
    ($inputs:expr, $key:expr, $basis:ident) => {
        match observed($inputs, &$key) {
            Walk::Done((value, via)) => {
                $basis.push(via);
                value
            }
            Walk::Need(k) => return Walk::Need(k),
            Walk::Fail(r) => return Walk::Fail(r),
        }
    };
}

impl FsSnapshot {
    pub fn new(store: ObjectStore) -> Self {
        Self::with(store, Capture::default())
    }

    pub fn with(store: ObjectStore, capture: Capture) -> Self {
        Self { store, capture }
    }

    fn build(&self, inputs: &Inputs, dir: &str, w: &Walker) -> Walk<Built> {
        let mut basis = Vec::new();
        let mut paths = Vec::new();
        // Watch before anything is read, then metadata, then the listing.
        if let Some(session) = w.session {
            if first!(inputs, Key::new("fs_watch", [dir, session]), basis) != Term::Bool(true) {
                return Walk::Fail(format!("{dir}: not watched"));
            }
        }
        if w.meta {
            first!(inputs, w.key("fs_meta", dir), basis);
            paths.push(dir.to_string());
        }
        let listing_key = w.key("fs_dir", dir);
        let stat_key = w.key("fs_stat", dir);
        let (mut listing, stat) =
            match (observed(inputs, &listing_key), observed(inputs, &stat_key)) {
                (Walk::Done((Term::Map(l), b1)), Walk::Done((Term::Map(s), b2))) => {
                    basis.extend([b1, b2]);
                    (l, s)
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
        // Every entry's metadata before any entry's content.
        if w.meta {
            let metas: Vec<Key> = listing
                .iter()
                .filter(|(_, mode)| *mode != "40000")
                .map(|(name, _)| w.key("fs_meta", &format!("{dir}/{name}")))
                .collect();
            let missing: Vec<Key> = metas
                .iter()
                .filter(|k| !inputs.contains_key(*k))
                .cloned()
                .collect();
            if !missing.is_empty() {
                return Walk::Need(missing);
            }
            for key in &metas {
                let Term::Map(meta) = first!(inputs, key.clone(), basis) else {
                    return Walk::Fail(format!("{}: malformed metadata", key.args[0]));
                };
                paths.push(key.args[0].clone());
                // Only values from inside the bracket may be used: the mode
                // comes from this lstat, not from the earlier listing.
                let name = key.args[0]
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let bracketed = git_mode(&meta);
                let listed = &listing[&name];
                let kind = |m: &str| if m == "120000" { "link" } else { "file" };
                if kind(&bracketed) != kind(listed) || bracketed == "other" {
                    return Walk::Fail(format!(
                        "{}: changed from {listed} to {bracketed} between listing and lstat",
                        key.args[0]
                    ));
                }
                listing.insert(name, bracketed);
            }
        }
        let mut entries = Vec::new();
        let mut need = Vec::new();
        for (name, mode) in &listing {
            let path = format!("{dir}/{name}");
            let id = match mode.as_str() {
                "40000" => match self.build(inputs, &path, w) {
                    Walk::Done(sub) => {
                        basis.extend(sub.basis);
                        paths.extend(sub.paths);
                        match sub.root {
                            Some(id) => id,
                            None => continue,
                        }
                    }
                    Walk::Need(k) => {
                        need.extend(k);
                        continue;
                    }
                    Walk::Fail(r) => return Walk::Fail(r),
                },
                "100644" | "100755" | "120000" => {
                    let input = if mode == "120000" {
                        w.key("fs_link", &path)
                    } else {
                        w.key("fs_file", &path)
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
            return Walk::Done(Built {
                root: None,
                basis,
                paths,
            });
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
        Walk::Done(Built {
            root: Some(self.store.put("tree", &body)),
            basis,
            paths,
        })
    }

    fn root(&self, built: &Built) -> String {
        built
            .root
            .clone()
            .unwrap_or_else(|| self.store.put("tree", b""))
    }

    fn capture(&self, inputs: &Inputs, dir: &str) -> Result<(String, Vec<Basis>), Step> {
        let c = self.capture;
        let plain = !(c.double_walk || c.recheck || c.watch);
        // One session per capture: reuse the one already in the inputs.
        let session = c.watch.then(|| {
            inputs
                .keys()
                .find(|k| k.kind == "fs_watch")
                .map(|k| k.args[1].clone())
                .unwrap_or_else(|| format!("s{}", SESSIONS.fetch_add(1, Ordering::Relaxed)))
        });
        let walker = Walker {
            tag: (!plain).then_some("w1"),
            meta: c.recheck,
            session: session.as_deref(),
        };
        let lift = |w: Walk<Built>| match w {
            Walk::Done(b) => Ok(b),
            Walk::Need(k) => Err(Step::Need(k)),
            Walk::Fail(r) => Err(Step::Incomplete(r)),
        };
        let built = lift(self.build(inputs, dir, &walker))?;
        let root = self.root(&built);
        let mut basis = built.basis;

        if c.recheck {
            let again: Vec<Key> = built
                .paths
                .iter()
                .map(|p| Key::new("fs_meta", [p.as_str(), "recheck"]))
                .collect();
            let missing: Vec<Key> = again
                .iter()
                .filter(|k| !inputs.contains_key(*k))
                .cloned()
                .collect();
            if !missing.is_empty() {
                return Err(Step::Need(missing));
            }
            for (path, key) in built.paths.iter().zip(&again) {
                let before = observed(inputs, &Key::new("fs_meta", [path.as_str(), "w1"]));
                match (before, observed(inputs, key)) {
                    (Walk::Done((a, _)), Walk::Done((b, via))) => {
                        if a != b {
                            return Err(Step::Incomplete(format!(
                                "{path} changed during capture: {a:?} -> {b:?}"
                            )));
                        }
                        basis.push(via);
                    }
                    _ => return Err(Step::Incomplete(format!("{path} vanished during capture"))),
                }
            }
        }
        if c.double_walk {
            let second = Walker {
                tag: Some("w2"),
                meta: false,
                session: None,
            };
            let again = lift(self.build(inputs, dir, &second))?;
            let other = self.root(&again);
            if other != root {
                return Err(Step::Incomplete(format!(
                    "second walk found {other}, first found {root}"
                )));
            }
            basis.extend(again.basis);
        }
        if let Some(session) = &session {
            let key = Key::new("fs_changes", [session.as_str()]);
            match observed(inputs, &key) {
                Walk::Done((Term::Map(m), via)) => {
                    let events = m.get("events").map_or("?", String::as_str);
                    if events != "0" || m.get("overflow").map(String::as_str) != Some("false") {
                        return Err(Step::Incomplete(format!(
                            "{events} change event(s) during capture"
                        )));
                    }
                    basis.push(via);
                }
                Walk::Need(k) => return Err(Step::Need(k)),
                _ => return Err(Step::Incomplete("change journal unavailable".into())),
            }
        }
        Ok((root, basis))
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
        match self.capture(inputs, &key.args[0]) {
            Ok((root, basis)) => Step::Derived {
                value: Term::Id(root),
                basis,
            },
            Err(step) => step,
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
