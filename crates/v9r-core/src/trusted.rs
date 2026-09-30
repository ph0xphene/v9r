//! Trusted runtime state, kept outside the agent workspace.
//!
//! The agent's authority is the workspace: commands run with their cwd
//! there, every path argument is fenced to it, and write actions are
//! canonicalized into it. Anything the runtime relies on to judge the
//! agent (checkpoints, rollback metadata, the trace and the receipts in
//! it) therefore lives under a separate *state root*:
//!
//! ```text
//! <state root>/tasks/<task id>/trace.jsonl
//! <state root>/tasks/<task id>/checkpoints/<checkpoint id>/{manifest.json, <backup files>}
//! ```
//!
//! The state root comes from `V9R_STATE_DIR`, else `$XDG_STATE_HOME/v9r`,
//! else `$HOME/.local/state/v9r`. There is no hard-coded fallback: if none
//! is set, opening fails. The root and the workspace must not contain each
//! other (checked on canonical paths, so a symlinked spelling cannot hide
//! an overlap).
//!
//! This separates *v9r-mediated* authority from trusted state. It is not
//! an OS sandbox: an allowlisted program runs with the same UID and can
//! still open any path it likes. Tampering by such a program is detected
//! (trace and checkpoint seals), not prevented.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

pub const STATE_DIR_ENV: &str = "V9R_STATE_DIR";

/// Names that the pre-separation layout kept at the workspace root. A
/// workspace containing them was used by an older v9r; it is refused
/// rather than having old runtime state silently treated as task data.
pub const LEGACY_TRUSTED_NAMES: &[&str] = &[".v9r", "trace.jsonl"];

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("no trusted state directory: set {STATE_DIR_ENV}, XDG_STATE_HOME or HOME")]
    NoStateDir,
    #[error("trusted state io at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("trusted state root {root} and workspace {workdir} overlap")]
    Overlap { root: PathBuf, workdir: PathBuf },
    #[error(
        "workspace {workdir} contains legacy in-workspace runtime state `{name}` from an older \
         v9r; remove it (runtime state now lives under the trusted state root)"
    )]
    LegacyLayout { workdir: PathBuf, name: String },
}

/// Canonical path of the trusted state root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRoot {
    path: PathBuf,
}

impl StateRoot {
    /// Open (creating if needed) a state root at `path`.
    pub fn open(path: &Path) -> Result<Self, TrustError> {
        create_private_dir(path)?;
        let path = fs::canonicalize(path).map_err(|source| TrustError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self { path })
    }

    /// Resolve the state root from the environment.
    pub fn from_env() -> Result<Self, TrustError> {
        let path = std::env::var_os(STATE_DIR_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .filter(|v| !v.is_empty())
                    .map(|v| PathBuf::from(v).join("v9r"))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(|v| PathBuf::from(v).join(".local/state/v9r"))
            })
            .ok_or(TrustError::NoStateDir)?;
        Self::open(&path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn task_dir(&self, task_id: Uuid) -> PathBuf {
        self.path.join("tasks").join(task_id.to_string())
    }

    pub fn trace_path(&self, task_id: Uuid) -> PathBuf {
        self.task_dir(task_id).join("trace.jsonl")
    }

    pub fn checkpoints_dir(&self, task_id: Uuid) -> PathBuf {
        self.task_dir(task_id).join("checkpoints")
    }

    /// Create the task's private directory.
    pub fn ensure_task_dir(&self, task_id: Uuid) -> Result<PathBuf, TrustError> {
        let dir = self.task_dir(task_id);
        create_private_dir(&dir)?;
        Ok(dir)
    }

    /// Refuse a workspace that contains the state root or lies inside it.
    pub fn check_separation(&self, workdir: &Path) -> Result<(), TrustError> {
        let workdir = fs::canonicalize(workdir).map_err(|source| TrustError::Io {
            path: workdir.to_path_buf(),
            source,
        })?;
        if workdir.starts_with(&self.path) || self.path.starts_with(&workdir) {
            return Err(TrustError::Overlap {
                root: self.path.clone(),
                workdir,
            });
        }
        Ok(())
    }
}

/// Refuse a workspace carrying the pre-separation runtime layout.
pub fn check_legacy_layout(workdir: &Path) -> Result<(), TrustError> {
    for name in LEGACY_TRUSTED_NAMES {
        if fs::symlink_metadata(workdir.join(name)).is_ok() {
            return Err(TrustError::LegacyLayout {
                workdir: workdir.to_path_buf(),
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), TrustError> {
    fs::create_dir_all(path).map_err(|source| TrustError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
            TrustError::Io {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("v9r-trusted-test-{name}-{}", Uuid::new_v4()));
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
    fn separation_rejects_nesting_in_either_direction() {
        let tmp = TempDir::new("nesting");
        let root = StateRoot::open(&tmp.0.join("state")).unwrap();
        fs::create_dir_all(tmp.0.join("state/inner")).unwrap();
        fs::create_dir_all(tmp.0.join("work")).unwrap();
        assert!(root.check_separation(&tmp.0.join("work")).is_ok());
        assert!(root.check_separation(&tmp.0.join("state/inner")).is_err());
        assert!(root.check_separation(&tmp.0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn separation_sees_through_symlinked_spelling() {
        let tmp = TempDir::new("symlinked");
        let root = StateRoot::open(&tmp.0.join("state")).unwrap();
        fs::create_dir_all(tmp.0.join("state/tasks")).unwrap();
        std::os::unix::fs::symlink(tmp.0.join("state/tasks"), tmp.0.join("innocent")).unwrap();
        assert!(matches!(
            root.check_separation(&tmp.0.join("innocent")),
            Err(TrustError::Overlap { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn state_dirs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("private");
        let root = StateRoot::open(&tmp.0.join("state")).unwrap();
        let task = root.ensure_task_dir(Uuid::new_v4()).unwrap();
        for dir in [root.path(), task.as_path()] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn legacy_layout_is_refused_explicitly() {
        let tmp = TempDir::new("legacy");
        assert!(check_legacy_layout(&tmp.0).is_ok());
        fs::create_dir_all(tmp.0.join(".v9r/backups")).unwrap();
        let err = check_legacy_layout(&tmp.0).unwrap_err().to_string();
        assert!(err.contains("legacy"), "{err}");
    }
}
