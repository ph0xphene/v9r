//! Trusted derivations over raw observations.
//!
//! | Verifier | Derives | From | Trust needed |
//! |---|---|---|---|
//! | [`GitObjects`] | `commit(repo, oid)`, `tree_digest(repo, commit, def)` | `git_object` bytes | **none**: every object is checked against the id it was requested by |
//! | [`FsContent`] | `dir_digest(dir, def)` | `fs_listing`, `fs_file`, `fs_link` | the filesystem observer (listing completeness, content, freshness) |
//! | [`ContentEquality`] | `content_equal(repo, commit, dir)` | `tree_digest`, `dir_digest` under one definition | whatever its inputs needed |
//!
//! Definitions (`def`):
//! * [`MANIFEST`] = [`content::DEFINITION`]: `(path, kind, sha256(content))`
//!   for every file and symlink, as in `crate::content`;
//! * [`NAMES`]: `(path, kind)` only, a deliberately weaker definition.
//!
//! Git objects are content-addressed: an object's id is the hash of its
//! stored bytes (`<type> <size>\0<content>`), SHA-1 for 40-hex ids and
//! SHA-256 for 64-hex ids. A tree lists every entry with its id. So a
//! commit's whole content can be checked from bytes supplied by anyone:
//! wrong bytes fail the hash, a withheld object leaves the derivation
//! incomplete, and an omitted entry is impossible (the tree names it).
//! What can *not* be derived this way is anything about mutable state:
//! which commit a ref points at, what a directory contains now.

use sha1::Digest as _;

use crate::content::{self, Item, ItemKind};
use crate::graph::{Key, Term};
use crate::state::ContentHash;
use crate::verify::{vouched, Basis, Inputs, Step, Verifier};

pub const MANIFEST: &str = content::DEFINITION;
pub const NAMES: &str = "names/1";

/// What a walk still needs, or why it cannot finish.
pub(crate) enum Walk<T> {
    Done(T),
    Need(Vec<Key>),
    Fail(String),
}

fn names_digest(mut items: Vec<(String, ItemKind)>) -> ContentHash {
    items.sort();
    ContentHash::of(&serde_json::to_vec(&items).expect("infallible"))
}

pub(crate) fn digest_of(def: &str, items: Vec<Item>) -> Option<String> {
    Some(
        match def {
            MANIFEST => content::digest(items),
            NAMES => names_digest(items.into_iter().map(|i| (i.path, i.kind)).collect()),
            _ => return None,
        }
        .to_string(),
    )
}

// ------------------------------------------------------------ git objects

pub struct GitObjects;

fn object_key(repo: &str, oid: &str) -> Key {
    Key::new("git_object", [repo, oid])
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The id of stored object bytes, in the hash the id length implies.
fn object_id(oid_len: usize, stored: &[u8]) -> Option<String> {
    match oid_len {
        40 => Some(hex(&sha1::Sha1::digest(stored))),
        64 => Some(hex(&sha2::Sha256::digest(stored))),
        _ => None,
    }
}

/// The checked `(type, content)` of `oid`: any candidate whose bytes hash
/// to it, whoever supplied it.
fn object(inputs: &Inputs, repo: &str, oid: &str) -> Walk<(String, Vec<u8>)> {
    object_at(inputs, object_key(repo, oid), oid)
}

/// The checked `(type, content)` of `oid`, fetched as `key`.
pub(crate) fn object_at(inputs: &Inputs, key: Key, oid: &str) -> Walk<(String, Vec<u8>)> {
    let Some(candidates) = inputs.get(&key) else {
        return Walk::Need(vec![key]);
    };
    for candidate in candidates {
        let Term::Bytes(stored) = &candidate.value else {
            continue;
        };
        if object_id(oid.len(), stored).as_deref() != Some(oid) {
            continue;
        }
        let Some(nul) = stored.iter().position(|b| *b == 0) else {
            continue;
        };
        let Ok(header) = std::str::from_utf8(&stored[..nul]) else {
            continue;
        };
        let Some((kind, size)) = header.split_once(' ') else {
            continue;
        };
        let body = &stored[nul + 1..];
        if size.parse::<usize>().ok() != Some(body.len()) {
            continue;
        }
        return Walk::Done((kind.to_string(), body.to_vec()));
    }
    Walk::Fail(if candidates.is_empty() {
        format!("object {oid} not supplied")
    } else {
        format!("no supplied bytes hash to {oid}")
    })
}

/// `(mode, name, id)` entries of a tree object.
pub(crate) fn tree_entries(body: &[u8], id_len: usize) -> Option<Vec<(String, String, String)>> {
    let raw_len = id_len / 2;
    let mut entries = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let space = rest.iter().position(|b| *b == b' ')?;
        let nul = rest.iter().position(|b| *b == 0)?;
        let mode = std::str::from_utf8(&rest[..space]).ok()?.to_string();
        let name = std::str::from_utf8(&rest[space + 1..nul]).ok()?.to_string();
        let id = hex(rest.get(nul + 1..nul + 1 + raw_len)?);
        entries.push((mode, name, id));
        rest = &rest[nul + 1 + raw_len..];
    }
    Some(entries)
}

/// Items of the tree `tree` and everything below it, each object fetched
/// as `key_of(id)` and checked against its id; and the basis.
pub(crate) fn walk_tree(
    inputs: &Inputs,
    key_of: &dyn Fn(&str) -> Key,
    tree: &str,
    def: &str,
) -> Walk<(Vec<Item>, Vec<Basis>)> {
    let mut basis = Vec::new();
    let mut items = Vec::new();
    let mut need = Vec::new();
    let mut pending = vec![(String::new(), tree.to_string())];
    while let Some((prefix, tree)) = pending.pop() {
        let body = match object_at(inputs, key_of(&tree), &tree) {
            Walk::Done((kind, body)) if kind == "tree" => body,
            Walk::Done((kind, _)) => return Walk::Fail(format!("{tree} is a {kind}, not a tree")),
            Walk::Need(k) => {
                need.extend(k);
                continue;
            }
            Walk::Fail(r) => return Walk::Fail(r),
        };
        basis.push(Basis::SelfCertified(key_of(&tree)));
        let Some(entries) = tree_entries(&body, tree.len()) else {
            return Walk::Fail(format!("tree {tree} does not parse"));
        };
        for (mode, name, id) in entries {
            let path = format!("{prefix}{name}");
            let kind = match mode.as_str() {
                "40000" => {
                    pending.push((format!("{path}/"), id));
                    continue;
                }
                "100644" | "100755" => ItemKind::File,
                "120000" => ItemKind::Symlink,
                other => return Walk::Fail(format!("{path}: mode {other} not representable")),
            };
            if def == NAMES {
                items.push(Item {
                    path,
                    kind,
                    sha256: ContentHash::of(b""),
                });
                continue;
            }
            match object_at(inputs, key_of(&id), &id) {
                Walk::Done((blob, body)) if blob == "blob" => {
                    basis.push(Basis::SelfCertified(key_of(&id)));
                    items.push(Item {
                        path,
                        kind,
                        sha256: ContentHash::of(&body),
                    });
                }
                Walk::Done((other, _)) => {
                    return Walk::Fail(format!("{path}: {other}, not a blob"))
                }
                Walk::Need(k) => need.extend(k),
                Walk::Fail(r) => return Walk::Fail(r),
            }
        }
    }
    if need.is_empty() {
        Walk::Done((items, basis))
    } else {
        Walk::Need(need)
    }
}

impl GitObjects {
    /// Items of `commit`'s tree, and the objects that certify them.
    fn walk(
        &self,
        inputs: &Inputs,
        repo: &str,
        commit: &str,
        def: &str,
    ) -> Walk<(Vec<Item>, Vec<Basis>)> {
        let mut basis = vec![Basis::SelfCertified(object_key(repo, commit))];
        let (kind, body) = match object(inputs, repo, commit) {
            Walk::Done(o) => o,
            Walk::Need(k) => return Walk::Need(k),
            Walk::Fail(r) => return Walk::Fail(r),
        };
        if kind != "commit" {
            return Walk::Fail(format!("{commit} is a {kind}, not a commit"));
        }
        let Some(tree) = std::str::from_utf8(&body)
            .ok()
            .and_then(|t| t.lines().next())
            .and_then(|l| l.strip_prefix("tree "))
        else {
            return Walk::Fail("commit without a tree line".into());
        };
        match walk_tree(inputs, &|oid| object_key(repo, oid), tree, def) {
            Walk::Done((items, more)) => {
                basis.extend(more);
                Walk::Done((items, basis))
            }
            other => other,
        }
    }
}

impl Verifier for GitObjects {
    fn id(&self) -> &str {
        "git-objects"
    }

    fn derives(&self, key: &Key) -> bool {
        matches!(
            (key.kind.as_str(), key.args.len()),
            ("commit", 2) | ("tree_digest", 3)
        )
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        let repo = key.args[0].as_str();
        match key.kind.as_str() {
            "commit" => match object(inputs, repo, &key.args[1]) {
                Walk::Done((kind, _)) => Step::Derived {
                    value: Term::Bool(kind == "commit"),
                    basis: vec![Basis::SelfCertified(object_key(repo, &key.args[1]))],
                },
                Walk::Need(k) => Step::Need(k),
                // Absence cannot be shown by a supplier: only presence.
                Walk::Fail(r) => Step::Incomplete(r),
            },
            _ => match self.walk(inputs, repo, &key.args[1], &key.args[2]) {
                Walk::Done((items, basis)) => match digest_of(&key.args[2], items) {
                    Some(d) => Step::Derived {
                        value: Term::Digest(d),
                        basis,
                    },
                    None => Step::Incomplete(format!("unknown definition {}", key.args[2])),
                },
                Walk::Need(k) => Step::Need(k),
                Walk::Fail(r) => Step::Incomplete(r),
            },
        }
    }
}

// ------------------------------------------------------------ filesystem

pub struct FsContent;

/// A raw fs input: vouched for if possible, else a lone claim (unvouched).
pub(crate) fn observed(inputs: &Inputs, key: &Key) -> Walk<(Term, Basis)> {
    if !inputs.contains_key(key) {
        return Walk::Need(vec![key.clone()]);
    }
    match vouched(inputs, key) {
        Ok((value, by)) => Walk::Done((value, Basis::VouchedBy(key.clone(), by))),
        Err(reason) => {
            let claims = &inputs[key];
            match claims.first() {
                Some(c) if claims.iter().all(|o| o.value == c.value) => {
                    Walk::Done((c.value.clone(), Basis::Unvouched(key.clone())))
                }
                _ => Walk::Fail(reason),
            }
        }
    }
}

impl Verifier for FsContent {
    fn id(&self) -> &str {
        "fs-content"
    }

    fn derives(&self, key: &Key) -> bool {
        key.kind == "dir_digest" && key.args.len() == 2
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        let (dir, def) = (key.args[0].as_str(), key.args[1].as_str());
        let listing_key = Key::new("fs_listing", [dir]);
        let (listing, listing_basis) = match observed(inputs, &listing_key) {
            Walk::Done((Term::Map(m), b)) => (m, b),
            Walk::Done(_) => return Step::Incomplete("listing is not a map".into()),
            Walk::Need(k) => return Step::Need(k),
            Walk::Fail(r) => return Step::Incomplete(r),
        };
        let mut basis = vec![listing_basis];
        let mut items = Vec::new();
        let mut need = Vec::new();
        for (path, kind) in &listing {
            let relative = path
                .strip_prefix(&format!("{dir}/"))
                .unwrap_or(path)
                .to_string();
            let (kind, input) = match kind.as_str() {
                "dir" => continue,
                "file" => (ItemKind::File, Key::new("fs_file", [path.as_str()])),
                "symlink" => (ItemKind::Symlink, Key::new("fs_link", [path.as_str()])),
                other => return Step::Incomplete(format!("{path}: {other} not representable")),
            };
            if def == NAMES {
                items.push(Item {
                    path: relative,
                    kind,
                    sha256: ContentHash::of(b""),
                });
                continue;
            }
            // Every listed entry needs its content: a listing is a promise.
            let sha256 = match observed(inputs, &input) {
                Walk::Done((Term::Bytes(b), via)) if kind == ItemKind::File => {
                    basis.push(via);
                    ContentHash::of(&b)
                }
                Walk::Done((Term::Text(t), via)) if kind == ItemKind::Symlink => {
                    basis.push(via);
                    ContentHash::of(t.as_bytes())
                }
                Walk::Done(_) => return Step::Incomplete(format!("{path}: unexpected value")),
                Walk::Need(k) => {
                    need.extend(k);
                    continue;
                }
                Walk::Fail(r) => return Step::Incomplete(format!("listed but {r}")),
            };
            items.push(Item {
                path: relative,
                kind,
                sha256,
            });
        }
        if !need.is_empty() {
            return Step::Need(need);
        }
        match digest_of(def, items) {
            Some(d) => Step::Derived {
                value: Term::Digest(d),
                basis,
            },
            None => Step::Incomplete(format!("unknown definition {def}")),
        }
    }
}

// ------------------------------------------------------------ equality

/// `content_equal(repo, commit, dir)` under one definition.
pub struct ContentEquality {
    id: String,
    definition: String,
}

impl ContentEquality {
    pub fn new(definition: &str) -> Self {
        Self {
            id: format!("content-equality/{definition}"),
            definition: definition.to_string(),
        }
    }
}

impl Verifier for ContentEquality {
    fn id(&self) -> &str {
        &self.id
    }

    fn derives(&self, key: &Key) -> bool {
        key.kind == "content_equal" && key.args.len() == 3
    }

    fn step(&self, key: &Key, inputs: &Inputs) -> Step {
        let def = self.definition.as_str();
        let tree = Key::new("tree_digest", [key.args[0].as_str(), &key.args[1], def]);
        let dir = Key::new("dir_digest", [key.args[2].as_str(), def]);
        let mut values = Vec::new();
        let mut basis = Vec::new();
        for input in [&tree, &dir] {
            match observed(inputs, input) {
                Walk::Done((value, via)) => {
                    values.push(value);
                    basis.push(via);
                }
                Walk::Need(_) => return Step::Need(vec![tree.clone(), dir.clone()]),
                Walk::Fail(r) => return Step::Incomplete(r),
            }
        }
        Step::Derived {
            value: Term::Bool(values[0] == values[1]),
            basis,
        }
    }
}
