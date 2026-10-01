//! An OS-assisted change journal for snapshot capture (inotify).
//!
//! | Kind | Args | Value |
//! |---|---|---|
//! | `fs_watch` | `dir`, `session` | `Bool(true)` once `dir` is watched in `session` |
//! | `fs_changes` | `session` | `Map`: `events` (count), `overflow` (`true`/`false`); ends the session |
//!
//! Unlike every other provider, this one keeps state between requests: a
//! session is an inotify instance that accumulates change events for the
//! directories watched so far. That is the point of the experiment: a
//! change journal needs a session, which stateless observation cannot
//! express.
//!
//! Watched events: content written, attributes changed, entries created,
//! deleted or moved, the directory itself deleted or moved. Reads (ours
//! included) are not watched. Not reported by inotify at all: writes
//! through a hard link from a directory outside the session, and writes
//! through a shared memory mapping.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;

use crate::fs_raw::RawFsObserver;
use crate::graph::{Answer, Attestor, EvidenceProvider, Key, Term};

const MASK: u32 = libc::IN_MODIFY
    | libc::IN_ATTRIB
    | libc::IN_CLOSE_WRITE
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_DELETE_SELF
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR;

struct Session {
    fd: libc::c_int,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: fd is an inotify descriptor this session owns.
        unsafe { libc::close(self.fd) };
    }
}

pub struct WatchObserver {
    id: String,
    paths: RawFsObserver,
    sessions: Mutex<BTreeMap<String, Session>>,
}

impl WatchObserver {
    pub fn new(id: impl Into<String>, root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            id: id.into(),
            paths: RawFsObserver::new("paths", root),
            sessions: Mutex::default(),
        }
    }

    fn watch(&self, dir: &str, session: &str) -> Option<Term> {
        // Watch the directory as opened (never through a symlink), via its
        // descriptor's /proc link, not by re-resolving its path.
        let opened = self.paths.open_dir(dir)?;
        let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", opened.as_raw_fd()));
        let mut sessions = self.sessions.lock().expect("sessions");
        let fd = match sessions.get(session) {
            Some(s) => s.fd,
            None => {
                // SAFETY: plain syscall; the result is checked.
                let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
                if fd < 0 {
                    return None;
                }
                sessions.insert(session.to_string(), Session { fd });
                fd
            }
        };
        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: fd is a live inotify descriptor, c_path a valid C string.
        let wd = unsafe { libc::inotify_add_watch(fd, c_path.as_ptr(), MASK) };
        (wd >= 0).then_some(Term::Bool(true))
    }

    fn changes(&self, session: &str) -> Option<Term> {
        let session = self.sessions.lock().expect("sessions").remove(session)?;
        let mut events = 0u64;
        let mut overflow = false;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: buf is a writable buffer of the stated length.
            let n = unsafe { libc::read(session.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            let mut offset = 0usize;
            while offset < n as usize {
                // SAFETY: the kernel wrote whole inotify_event records.
                let event = unsafe {
                    std::ptr::read_unaligned(buf[offset..].as_ptr().cast::<libc::inotify_event>())
                };
                if event.mask & libc::IN_Q_OVERFLOW != 0 {
                    overflow = true;
                }
                if event.mask & (MASK & !libc::IN_ONLYDIR) != 0 {
                    events += 1;
                }
                offset += std::mem::size_of::<libc::inotify_event>() + event.len as usize;
            }
        }
        Some(Term::Map(BTreeMap::from([
            ("events".to_string(), events.to_string()),
            ("overflow".to_string(), overflow.to_string()),
        ])))
    }
}

impl EvidenceProvider for WatchObserver {
    fn id(&self) -> &str {
        &self.id
    }

    fn answers(&self, key: &Key) -> bool {
        matches!(
            (key.kind.as_str(), key.args.len()),
            ("fs_watch", 2) | ("fs_changes", 1)
        )
    }

    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        keys.iter()
            .filter_map(|key| {
                let value = match key.kind.as_str() {
                    "fs_watch" => self.watch(&key.args[0], &key.args[1])?,
                    _ => self.changes(&key.args[0])?,
                };
                Some(Answer::Verified(attestor.attest(
                    (*key).clone(),
                    value,
                    "inotify",
                )))
            })
            .collect()
    }
}
