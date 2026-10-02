//! Content-addressed derivation helpers shared by the snapshot verifiers.
//!
//! Definitions (`def`) of "the content of a tree":
//! * [`MANIFEST`] = [`content::DEFINITION`]: `(path, kind, sha256(content))`
//!   for every file and symlink, as in `crate::content`;
//! * [`NAMES`]: `(path, kind)` only, a deliberately weaker definition.
//!
//! Stored objects are content-addressed as in git with
//! `--object-format=sha256`: an object's id is the SHA-256 of its stored
//! bytes (`<type> <size>\0<content>`), and a tree lists every entry with
//! its id. So a snapshot's whole content can be checked from bytes
//! supplied by anyone: wrong bytes fail the hash, a withheld object leaves
//! the derivation incomplete, and an omitted entry is impossible (the
//! tree names it).
//!
//! Debloat Phase 1 removed the git-domain verifiers that lived here
//! (`GitObjects`, `ContentEquality`, SHA-1 object ids) and `FsContent`,
//! which read `fs_provider`'s `fs_listing`. They are kept at the tag
//! `v9r-archive-v0`.

use sha2::Digest as _;

use crate::content::{self, ContentHash, Item, ItemKind};
use crate::graph::{Key, Term};
use crate::verify::{vouched, Basis, Inputs};

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

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The id of stored object bytes: SHA-256, for 64-hex ids.
fn object_id(oid_len: usize, stored: &[u8]) -> Option<String> {
    (oid_len == 64).then(|| hex(&sha2::Sha256::digest(stored)))
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
