//! A primitive filesystem observer: it transcribes single system calls
//! and interprets nothing.
//!
//! | Kind | Args | Value | Call |
//! |---|---|---|---|
//! | `fs_dir` | `path` | `Map`: entry name → mode (`40000`, `100644`, `100755`, `120000`, `other`) | `getdents` on an opened directory + `fstatat` per entry |
//! | `fs_stat` | `path` | `Map`: `nlink` → link count of the directory | `fstatat` |
//! | `fs_file` | `path` | `Bytes`: the file's content | `openat` + `read` |
//! | `fs_link` | `path` | `Text`: the symlink's target | `readlinkat` |
//! | `fs_meta` | `path` | `Map`: `ino`, `mode`, `size`, `nlink`, `mtime_ns`, `ctime_ns` | `fstatat` |
//!
//! Every kind also accepts a second, opaque argument (a reading tag). It
//! is ignored: it only lets a caller ask for the same observation twice,
//! as two distinct facts.
//!
//! It never walks, hashes, normalizes or decides what counts: the
//! snapshot verifier does all of that.
//!
//! **Correspondence.** Every call is relative to a file descriptor
//! obtained by `openat(…, O_NOFOLLOW)` one component at a time from the
//! root, and the final operation acts on that descriptor (or on a name
//! relative to it). No path is resolved twice, so a component swapped
//! for a symlink between a check and a use cannot redirect the observer
//! outside the root: the swap makes the call fail instead.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};

pub struct RawFsObserver {
    id: String,
    root: PathBuf,
}

fn openat(dir: &OwnedFd, name: &str, flags: libc::c_int) -> Option<OwnedFd> {
    let name = CString::new(name).ok()?;
    // SAFETY: dir is an open descriptor, name a valid C string.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    // SAFETY: a non-negative result is a descriptor we now own.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `lstat` of `name` relative to `dir` (or of `dir` itself if `name` is
/// empty), without following a final symlink.
fn fstatat(dir: &OwnedFd, name: &str) -> Option<libc::stat> {
    let c_name = CString::new(name).ok()?;
    let flags = if name.is_empty() {
        libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };
    // SAFETY: stat is plain data; the call fills it on success.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::fstatat(dir.as_raw_fd(), c_name.as_ptr(), &mut st, flags) };
    (rc == 0).then_some(st)
}

fn git_mode(st: &libc::stat) -> &'static str {
    match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => "40000",
        libc::S_IFLNK => "120000",
        libc::S_IFREG if st.st_mode & 0o100 != 0 => "100755",
        libc::S_IFREG => "100644",
        _ => "other",
    }
}

fn is(st: &libc::stat, kind: libc::mode_t) -> bool {
    st.st_mode & libc::S_IFMT == kind
}

impl RawFsObserver {
    pub fn new(id: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            root: root.into(),
        }
    }

    /// The directory containing `path` (opened component by component,
    /// never through a symlink) and `path`'s last component.
    fn parent(&self, path: &str) -> Option<(OwnedFd, String)> {
        let components: Vec<&str> = Path::new(path)
            .components()
            .map(|c| match c {
                Component::Normal(n) => n.to_str(),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let (last, dirs) = components.split_last()?;
        let root = CString::new(self.root.as_os_str().as_bytes()).ok()?;
        // SAFETY: root is a valid C string; the result is checked.
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        // SAFETY: a non-negative result is a descriptor we now own.
        let mut dir = (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })?;
        for name in dirs {
            dir = openat(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        }
        Some((dir, last.to_string()))
    }

    /// `path` opened as a directory, never through a symlink.
    pub(crate) fn open_dir(&self, path: &str) -> Option<OwnedFd> {
        let (parent, name) = self.parent(path)?;
        openat(&parent, &name, libc::O_RDONLY | libc::O_DIRECTORY)
    }

    fn observe(&self, key: &Key) -> Option<Term> {
        let path = key.args.first()?;
        Some(match key.kind.as_str() {
            "fs_dir" => {
                let dir = self.open_dir(path)?;
                // Enumerate the opened directory itself, not a path to it.
                let listing =
                    std::fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd())).ok()?;
                let mut entries = BTreeMap::new();
                for entry in listing {
                    let name = entry.ok()?.file_name().to_str()?.to_string();
                    let st = fstatat(&dir, &name)?;
                    entries.insert(name, git_mode(&st).to_string());
                }
                Term::Map(entries)
            }
            "fs_stat" => {
                let (parent, name) = self.parent(path)?;
                let st = fstatat(&parent, &name)?;
                if !is(&st, libc::S_IFDIR) {
                    return None;
                }
                Term::Map(BTreeMap::from([(
                    "nlink".to_string(),
                    st.st_nlink.to_string(),
                )]))
            }
            "fs_meta" => {
                let (parent, name) = self.parent(path)?;
                let st = fstatat(&parent, &name)?;
                let ns = |s: i64, n: i64| (s as i128 * 1_000_000_000 + n as i128).to_string();
                Term::Map(BTreeMap::from([
                    ("ino".to_string(), st.st_ino.to_string()),
                    ("mode".to_string(), format!("{:o}", st.st_mode)),
                    ("size".to_string(), st.st_size.to_string()),
                    ("nlink".to_string(), st.st_nlink.to_string()),
                    ("mtime_ns".to_string(), ns(st.st_mtime, st.st_mtime_nsec)),
                    ("ctime_ns".to_string(), ns(st.st_ctime, st.st_ctime_nsec)),
                ]))
            }
            "fs_file" => {
                let (parent, name) = self.parent(path)?;
                let fd = openat(&parent, &name, libc::O_RDONLY)?;
                if !is(&fstatat(&fd, "")?, libc::S_IFREG) {
                    return None;
                }
                let mut bytes = Vec::new();
                std::fs::File::from(fd).read_to_end(&mut bytes).ok()?;
                Term::Bytes(bytes)
            }
            "fs_link" => {
                let (parent, name) = self.parent(path)?;
                let c_name = CString::new(name).ok()?;
                let mut buf = vec![0u8; 4096];
                // SAFETY: buf is writable for its length; the result is checked.
                let n = unsafe {
                    libc::readlinkat(
                        parent.as_raw_fd(),
                        c_name.as_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    )
                };
                if n < 0 || n as usize >= buf.len() {
                    return None;
                }
                buf.truncate(n as usize);
                Term::Text(String::from_utf8(buf).ok()?)
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
            "fs_dir" | "fs_stat" | "fs_file" | "fs_link" | "fs_meta"
        ) && matches!(key.args.len(), 1 | 2)
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
