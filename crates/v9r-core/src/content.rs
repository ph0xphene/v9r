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

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

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
