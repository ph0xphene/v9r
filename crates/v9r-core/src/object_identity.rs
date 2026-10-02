//! Object identity for grants: which object a name resolves to, in which
//! view, under which notion of identity.
//!
//! ```text
//!   grant (grantor's view)            world (grantee's view)
//!   objects: [P], identities {P: id}  runtime binds P's object at T
//!         │                                   │ probe: resolve(T)
//!         └──── D9: Fact(object(P) = id) ◀────┘ evidence keyed by P,
//!                                               value observed at T
//! ```
//!
//! Grant names stay in the grantor's namespace (they are what D3/D7
//! attenuate). Where the grantee sees the object is the runtime's
//! choice, recorded as a mapping `P → T`. The probe in the grantee's
//! world resolves `T`, and [`evidence`] attests `object(P)` from it.
//!
//! Three schemes, compared by the X1 experiment:
//!
//! | Scheme | Identity | Notes |
//! |---|---|---|
//! | [`IdScheme::Name`] | none: the name is trusted to mean the object | X2's behaviour (no D9) |
//! | [`IdScheme::DevIno`] | `(st_dev, st_ino)` | inode numbers are reused after deletion |
//! | [`IdScheme::Handle`] | `st_dev` + `name_to_handle_at` handle | on ext4 and tmpfs the handle carries the inode generation |

use std::ffi::CString;

use crate::capability::{Observation, Resolved};
use crate::delegation::{DEvidence, DSubject, DValue};
use crate::kernel::{Fact, Provenance, Verified};
use crate::snapshot::{snapshot_from, ObjectStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdScheme {
    Name,
    DevIno,
    Handle,
}

/// The identity string of a resolved object, or `None` if the scheme has
/// none or the resolution failed (unknown, never absent).
pub fn object_id(resolved: &Resolved, scheme: IdScheme) -> Option<String> {
    let Resolved::Seen {
        dev, ino, handle, ..
    } = resolved
    else {
        return None;
    };
    match scheme {
        IdScheme::Name => None,
        IdScheme::DevIno => Some(format!("devino:{dev}:{ino}")),
        IdScheme::Handle => handle.as_ref().ok().map(|h| format!("fh:{dev}:{h}")),
    }
}

/// Verified `object(P)` facts from a grantee world's probe, for each
/// mapping `(P in the grant, T in the world)`.
pub fn evidence(obs: &Observation, mapping: &[(String, String)], scheme: IdScheme) -> DEvidence {
    let mut ev = DEvidence::new();
    for (grant_path, world_path) in mapping {
        let Some(id) = obs.resolved(world_path).and_then(|r| object_id(r, scheme)) else {
            continue;
        };
        ev.add_verified(
            Verified::attest(Fact {
                subject: DSubject::Object(grant_path.clone()),
                value: DValue::Text(id),
            }),
            Provenance {
                observer: "world-probe".into(),
                basis: format!("probe sha256 {}; resolve {world_path}", obs.probe_digest()),
            },
        );
    }
    ev
}

/// Content identity of a snapshot root: `tree:<sha256 git tree id>`.
pub fn content_id(root: &str) -> String {
    format!("tree:{root}")
}

/// Verified `object(key) = tree:<root>` facts: for each `(key, world
/// dir)`, the snapshot root of what the world's probe transcribed under
/// that directory. A transcription that does not yield a snapshot gives
/// no fact (and its reason).
pub fn content_evidence(
    obs: &Observation,
    mapping: &[(String, String)],
    store: &ObjectStore,
) -> (DEvidence, Vec<String>) {
    let transcript = obs.transcript();
    let mut ev = DEvidence::new();
    let mut failed = Vec::new();
    for (key, dir) in mapping {
        match snapshot_from(store, &transcript, dir, "provider:world-probe") {
            Ok(root) => ev.add_verified(
                Verified::attest(Fact {
                    subject: DSubject::Object(key.clone()),
                    value: DValue::Text(content_id(&root)),
                }),
                Provenance {
                    observer: "world-probe+fs-snapshot".into(),
                    basis: format!("probe sha256 {}; snapshot {dir}", obs.probe_digest()),
                },
            ),
            Err(e) => failed.push(format!("{dir}: {e}")),
        }
    }
    (ev, failed)
}

// ------------------------------------------------------------ host side

#[repr(C)]
struct FileHandle {
    handle_bytes: u32,
    handle_type: i32,
    f_handle: [u8; 128],
}

/// The runtime's own resolution of a host path (no world involved), in
/// the same formats as the probe's.
pub fn host_resolve(path: &str) -> Resolved {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Resolved::Error {
                error: e.raw_os_error().map(errno_name).unwrap_or_default(),
            }
        }
    };
    let (handle, mount) = handle_of(path);
    Resolved::Seen {
        kind: if meta.is_dir() { "d" } else { "f" }.into(),
        dev: meta.dev(),
        ino: meta.ino(),
        generation: generation_of(path),
        handle: handle.map(|(t, b)| format!("{t}:{}", crate::verifiers::hex(&b))),
        mount,
    }
}

/// `name_to_handle_at(AT_FDCWD, path, …, 0)`: (type, bytes) and mount id.
fn handle_of(path: &str) -> (Result<(i32, Vec<u8>), String>, Result<i64, String>) {
    let c = CString::new(path).expect("no NUL");
    let mut fh = FileHandle {
        handle_bytes: 128,
        handle_type: 0,
        f_handle: [0; 128],
    };
    let mut mount: libc::c_int = 0;
    // SAFETY: valid path, handle buffer of the declared size, out-pointer.
    let r = unsafe {
        libc::syscall(
            libc::SYS_name_to_handle_at,
            libc::AT_FDCWD,
            c.as_ptr(),
            &mut fh as *mut FileHandle,
            &mut mount as *mut libc::c_int,
            0,
        )
    };
    if r != 0 {
        let e = errno_name(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        return (Err(e.clone()), Err(e));
    }
    let n = fh.handle_bytes as usize;
    (
        Ok((fh.handle_type, fh.f_handle[..n].to_vec())),
        Ok(mount as i64),
    )
}

fn generation_of(path: &str) -> Result<u32, String> {
    let c = CString::new(path).expect("no NUL");
    // SAFETY: plain syscalls on a path we own the string of.
    unsafe {
        let fd = libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        );
        if fd < 0 {
            return Err(errno_name(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            ));
        }
        let mut gen: u64 = 0;
        let r = libc::ioctl(fd, 0x8008_7601, &mut gen as *mut u64);
        let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        libc::close(fd);
        if r < 0 {
            Err(errno_name(e))
        } else {
            Ok(gen as u32)
        }
    }
}

/// Can an unprivileged process reach an object by its handle alone
/// (`open_by_handle_at`)? Returns `Ok` or the errno.
pub fn open_by_handle(path: &str) -> Result<(), String> {
    let c = CString::new(path).expect("no NUL");
    let mut fh = FileHandle {
        handle_bytes: 128,
        handle_type: 0,
        f_handle: [0; 128],
    };
    let mut mount: libc::c_int = 0;
    // SAFETY: as in `handle_of`; then open_by_handle_at on a dir fd we own.
    unsafe {
        if libc::syscall(
            libc::SYS_name_to_handle_at,
            libc::AT_FDCWD,
            c.as_ptr(),
            &mut fh as *mut FileHandle,
            &mut mount as *mut libc::c_int,
            0,
        ) != 0
        {
            return Err("name_to_handle_at failed".into());
        }
        let dir = CString::new("/").unwrap();
        let mfd = libc::open(
            dir.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        );
        let fd = libc::syscall(
            libc::SYS_open_by_handle_at,
            mfd,
            &mut fh as *mut FileHandle,
            libc::O_RDONLY | libc::O_CLOEXEC,
        );
        let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        libc::close(mfd);
        if fd < 0 {
            return Err(errno_name(e));
        }
        libc::close(fd as i32);
        Ok(())
    }
}

fn errno_name(e: i32) -> String {
    match e {
        libc::EPERM => "EPERM".into(),
        libc::ENOENT => "ENOENT".into(),
        libc::EACCES => "EACCES".into(),
        libc::ENOTTY => "ENOTTY".into(),
        libc::EOPNOTSUPP => "EOPNOTSUPP".into(),
        libc::ESTALE => "ESTALE".into(),
        libc::EINVAL => "EINVAL".into(),
        libc::EXDEV => "EXDEV".into(),
        other => format!("errno {other}"),
    }
}
