use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::effect::{Action, EffectReceipt, ExecutionOutcome, Observation};
use crate::state::{self, ContentHash, Entry, EntryKind, FsState, ObserveError};
use crate::trace::{TaskEvent, TraceLogger};

const V9R_DIR: &str = ".v9r";
const BACKUPS_DIR: &str = "backups";
const SNAPSHOT_MANIFEST: &str = "manifest.json";
const SNAPSHOT_MANIFEST_VERSION: u32 = 2;
const SAFETY_ERROR: &str = "Safety Error: Cannot use a project root (or a subfolder of one) as a mutable workdir. Place the workdir somewhere outside any version-controlled tree, or opt in by creating a `.v9r-workdir` file inside it.";

/// Files/dirs that mark `dir` as a project root. `.git` is checked by
/// presence, not type, so it catches both worktrees (`.git/` dir) and
/// submodules / linked-worktrees (`.git` file).
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];

/// Presence of this file at the workdir's top level means the user has
/// explicitly opted in: "yes, manage this directory as an agent workdir,
/// even though an ancestor is a project root." Without it we refuse to
/// rollback inside any version-controlled tree.
const OPT_IN_MARKER: &str = ".v9r-workdir";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub Uuid);

#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    #[error("unknown task: {0}")]
    UnknownTask(Uuid),
    #[error("task filesystem is blocked: {0}")]
    FilesystemBlocked(Uuid),
    #[error("symbolic links are not supported in task snapshots: {0}")]
    SymlinkUnsupported(PathBuf),
    #[error("{0}")]
    Safety(String),
    #[error("cannot observe {path}: {reason}")]
    Unobservable { path: PathBuf, reason: String },
    #[error("io at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("checkpoint worker failed: {0}")]
    Join(String),
    #[error("trace: {0}")]
    Trace(#[from] crate::trace::TraceError),
}

impl From<ObserveError> for TransactionError {
    fn from(err: ObserveError) -> Self {
        match err {
            ObserveError::Io { path, source } => TransactionError::Io { path, source },
        }
    }
}

pub type Result<T> = std::result::Result<T, TransactionError>;

#[derive(Clone, Debug)]
struct TaskFsState {
    workdir: PathBuf,
    blocked: bool,
}

/// On-disk checkpoint manifest. Version 1 (`files`/`dirs` with FNV-64)
/// is no longer read: FNV-64 is not collision resistant, so a crafted
/// modification could make rollback skip a file.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SnapshotManifest {
    version: u32,
    state: FsState,
}

static TASKS: OnceLock<Mutex<HashMap<Uuid, TaskFsState>>> = OnceLock::new();

pub fn register_task(task_id: Uuid, workdir: PathBuf) {
    let mut tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tasks
        .entry(task_id)
        .and_modify(|state| state.workdir = workdir.clone())
        .or_insert(TaskFsState {
            workdir,
            blocked: false,
        });
}

fn has_marker_at(dir: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
}

fn ancestor_has_marker(dir: &Path) -> bool {
    let mut cur = dir.parent();
    while let Some(p) = cur {
        if has_marker_at(p) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// A directory is safe to use as a mutable agent workdir when:
///   1. It does NOT itself contain a project-root marker (.git, Cargo.toml, etc.)
///   2. AND either no ancestor contains a marker, OR the workdir contains
///      the explicit opt-in file `.v9r-workdir`.
///
/// Rule (2) is the one that catches the "agent is rooted at a subfolder
/// inside my git repo" footgun. The opt-in is a file the user creates
/// when they really do want the agent operating inside their repo.
pub fn is_safe_directory(workdir: &Path) -> bool {
    if has_marker_at(workdir) {
        return false;
    }
    if workdir.join(OPT_IN_MARKER).is_file() {
        return true;
    }
    !ancestor_has_marker(workdir)
}

pub fn ensure_safe_directory(workdir: &Path) -> Result<()> {
    if is_safe_directory(workdir) {
        Ok(())
    } else {
        Err(TransactionError::Safety(SAFETY_ERROR.to_string()))
    }
}

/// Reject relative paths read out of an on-disk snapshot manifest that
/// contain anything other than ordinary segments — no `..`, no `/foo`
/// absolute paths, no `.`. The manifest is regenerated each checkpoint,
/// but it lives on disk between checkpoint and rollback, so this is a
/// hardening against tampering or corruption.
fn safe_relative_path(rel: &Path) -> Result<()> {
    use std::path::Component;
    if rel.as_os_str().is_empty() {
        return Err(TransactionError::Safety(
            "snapshot manifest contained an empty path".to_string(),
        ));
    }
    for c in rel.components() {
        match c {
            Component::Normal(_) => continue,
            other => {
                return Err(TransactionError::Safety(format!(
                    "snapshot manifest path is not relative-normal: {other:?} in {}",
                    rel.display()
                )))
            }
        }
    }
    Ok(())
}

pub fn block_task_fs(task_id: Uuid) {
    let mut tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(state) = tasks.get_mut(&task_id) {
        state.blocked = true;
    }
}

pub fn ensure_task_fs_unblocked(task_id: Uuid) -> Result<()> {
    let tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = tasks
        .get(&task_id)
        .ok_or(TransactionError::UnknownTask(task_id))?;
    if state.blocked {
        return Err(TransactionError::FilesystemBlocked(task_id));
    }
    Ok(())
}

pub async fn checkpoint(task_id: Uuid, trace: &TraceLogger) -> Result<CheckpointId> {
    let checkpoint = tokio::task::spawn_blocking(move || checkpoint_untraced(task_id))
        .await
        .map_err(|err| TransactionError::Join(err.to_string()))??;
    trace
        .log_event(TaskEvent::CheckpointCreated { id: checkpoint })
        .await?;
    Ok(checkpoint)
}

pub async fn rollback(task_id: Uuid, checkpoint: CheckpointId, trace: &TraceLogger) -> Result<()> {
    tokio::task::spawn_blocking(move || rollback_untraced(task_id, checkpoint))
        .await
        .map_err(|err| TransactionError::Join(err.to_string()))??;
    trace
        .log_event(TaskEvent::RollbackPerformed { id: checkpoint })
        .await?;
    Ok(())
}

/// A rollback bracketed by observations. The receipt's effects are the
/// changes the rollback itself made; effects observed during the task
/// remain in their own receipts and are not erased by this.
#[derive(Debug)]
pub struct ObservedRollback {
    pub result: Result<()>,
    pub receipt: EffectReceipt,
}

/// Like [`rollback`], but also logs a `TaskEvent::EffectObserved` for the
/// rollback. Returns `Err` only if the pre-observation or the trace write
/// fails; a failed rollback is reported in `ObservedRollback::result`.
pub async fn rollback_observed(
    task_id: Uuid,
    checkpoint: CheckpointId,
    trace: &TraceLogger,
) -> Result<ObservedRollback> {
    let workdir = registered_workdir(task_id, false)?;
    let capture = |root: PathBuf| async move {
        tokio::task::spawn_blocking(move || Observation::capture(&root))
            .await
            .map_err(|err| TransactionError::Join(err.to_string()))
    };
    let pre = capture(workdir.clone()).await??;
    let result = rollback(task_id, checkpoint, trace).await;
    let outcome = match &result {
        Ok(()) => ExecutionOutcome::Completed,
        Err(err) => ExecutionOutcome::Failed {
            reason: err.to_string(),
        },
    };
    let post = match capture(workdir).await {
        Ok(Ok(post)) => Ok(post),
        Ok(Err(err)) => Err(err.to_string()),
        Err(err) => Err(err.to_string()),
    };
    let receipt = EffectReceipt::from_observations(
        Action::Rollback { checkpoint },
        None,
        outcome,
        &pre,
        post.as_ref().map_err(Clone::clone),
    );
    trace
        .log_event(TaskEvent::EffectObserved {
            receipt: Box::new(receipt.to_record()),
        })
        .await?;
    Ok(ObservedRollback { result, receipt })
}

fn checkpoint_untraced(task_id: Uuid) -> Result<CheckpointId> {
    let workdir = registered_workdir(task_id, true)?;
    ensure_safe_directory(&workdir)?;
    let checkpoint = CheckpointId(Uuid::new_v4());
    let backup_dir = backup_dir(&workdir, task_id, checkpoint);
    let tmp_path = backup_dir.with_file_name(format!(".tmp-{}", checkpoint.0));

    remove_dir_if_exists(&tmp_path)?;
    create_dir_all(&tmp_path)?;
    let manifest = match backup_workdir(&workdir, &tmp_path) {
        Ok(manifest) => manifest,
        Err(err) => {
            let _ = fs::remove_dir_all(&tmp_path);
            return Err(err);
        }
    };
    write_snapshot_manifest(&tmp_path, &manifest)?;
    if let Some(parent) = backup_dir.parent() {
        create_dir_all(parent)?;
    }
    rename(&tmp_path, &backup_dir)?;

    Ok(checkpoint)
}

fn rollback_untraced(task_id: Uuid, checkpoint: CheckpointId) -> Result<()> {
    let workdir = registered_workdir(task_id, false)?;

    // Defense-in-depth. `checkpoint_untraced` validated at snapshot time,
    // but the directory could have grown a project marker since then
    // (e.g. `git init` ran inside the workdir during the task). Re-check
    // before we touch anything. Idempotency note: calling rollback twice
    // is safe because the second call sees the workdir already restored
    // and `selective_rollback` becomes a no-op diff.
    ensure_safe_directory(&workdir)?;

    let backup_dir = backup_dir(&workdir, task_id, checkpoint);
    if !backup_dir.is_dir() {
        return Err(TransactionError::Io {
            path: backup_dir,
            source: io::Error::new(io::ErrorKind::NotFound, "checkpoint not found"),
        });
    }
    let manifest = read_snapshot_manifest(&backup_dir)?;
    selective_rollback(&workdir, &backup_dir, &manifest)?;
    Ok(())
}

fn tasks() -> &'static Mutex<HashMap<Uuid, TaskFsState>> {
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_workdir(task_id: Uuid, require_unblocked: bool) -> Result<PathBuf> {
    let tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = tasks
        .get(&task_id)
        .ok_or(TransactionError::UnknownTask(task_id))?;
    if require_unblocked && state.blocked {
        return Err(TransactionError::FilesystemBlocked(task_id));
    }
    Ok(state.workdir.clone())
}

fn backup_dir(workdir: &Path, task_id: Uuid, checkpoint: CheckpointId) -> PathBuf {
    workdir
        .join(V9R_DIR)
        .join(BACKUPS_DIR)
        .join(task_id.to_string())
        .join(checkpoint.0.to_string())
}

fn backup_workdir(workdir: &Path, backup_dir: &Path) -> Result<SnapshotManifest> {
    // One pass: the bytes written to the backup are exactly the bytes
    // whose hash lands in the manifest.
    let state = state::observe_with(workdir, |relative_path, bytes| {
        let dst_path = backup_dir.join(relative_path);
        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ObserveError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&dst_path, bytes).map_err(|source| ObserveError::Io {
            path: dst_path,
            source,
        })
    })?;
    if let Some((path, _)) = state
        .entries()
        .iter()
        .find(|(_, entry)| entry.kind == EntryKind::Symlink)
    {
        return Err(TransactionError::SymlinkUnsupported(workdir.join(path)));
    }
    if let Some(unobserved) = state.unobserved().first() {
        return Err(TransactionError::Unobservable {
            path: workdir.join(&unobserved.path),
            reason: unobserved.reason.clone(),
        });
    }
    Ok(SnapshotManifest {
        version: SNAPSHOT_MANIFEST_VERSION,
        state,
    })
}

/// Restore the workdir to the checkpointed state, touching only paths
/// whose observable state differs from the snapshot.
///
/// Order matters for safety:
///   1. every non-directory that differs from the snapshot is unlinked
///      (this includes symlinks and FIFOs the task created — unlinking a
///      symlink never touches its target);
///   2. directories not in the snapshot are removed deepest-first, but
///      only when already empty (`remove_dir`, never `remove_dir_all`);
///   3. snapshot directories and files are recreated parent-first, files
///      via `create_new` (`O_EXCL`), which refuses to write through any
///      symlink that raced into place.
///
/// After step 1 no symlink remains under the workdir, so step 3 cannot
/// be redirected outside it by a planted link.
fn selective_rollback(
    workdir: &Path,
    backup_dir: &Path,
    manifest: &SnapshotManifest,
) -> Result<()> {
    if manifest.version != SNAPSHOT_MANIFEST_VERSION {
        return Err(TransactionError::Safety(format!(
            "unsupported snapshot manifest version: {}",
            manifest.version
        )));
    }
    // Validate every relative path in the manifest BEFORE any join, so a
    // tampered manifest can't smuggle `..` into a delete-or-restore path.
    let snapshot = &manifest.state;
    for (relative_path, entry) in snapshot.entries() {
        safe_relative_path(Path::new(relative_path))?;
        if entry.kind == EntryKind::Symlink {
            return Err(TransactionError::Safety(format!(
                "snapshot manifest contains a symlink: {relative_path}"
            )));
        }
    }

    let current = state::observe(workdir)?;
    if let Some(unobserved) = current.unobserved().first() {
        return Err(TransactionError::Unobservable {
            path: workdir.join(&unobserved.path),
            reason: unobserved.reason.clone(),
        });
    }

    // Verify every backup we will need before mutating anything, so a
    // corrupted checkpoint fails closed instead of half-restoring.
    for (relative_path, original) in snapshot.entries() {
        if original.kind == EntryKind::File && current.get(relative_path) != Some(original) {
            load_backup(backup_dir, relative_path, original)?;
        }
    }

    let mut stray_dirs = Vec::new();
    for (relative_path, entry) in current.entries() {
        // `current` came from our own walk, which only yields plain
        // segments. Belt-and-suspenders: re-validate.
        safe_relative_path(Path::new(relative_path))?;
        let path = workdir.join(relative_path);
        let original = snapshot.get(relative_path);
        if entry.kind == EntryKind::Dir {
            if original.map(|e| e.kind) != Some(EntryKind::Dir) {
                stray_dirs.push(path);
            }
        } else if original != Some(entry) {
            remove_file_if_exists(&path)?;
        }
    }

    stray_dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in stray_dirs {
        // A non-empty dir we don't know about is left alone rather than
        // recursively wiped.
        let _ = fs::remove_dir(&path);
    }

    for (relative_path, original) in snapshot.entries() {
        let path = workdir.join(relative_path);
        match original.kind {
            EntryKind::Dir => match fs::create_dir(&path) {
                Ok(()) => {}
                Err(source)
                    if source.kind() == io::ErrorKind::AlreadyExists
                        && fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) => {}
                Err(source) => return Err(TransactionError::Io { path, source }),
            },
            EntryKind::File => {
                if current.get(relative_path) != Some(original) {
                    restore_file(workdir, backup_dir, relative_path, original)?;
                }
            }
            // Special files cannot be restored from a backup; they were
            // never copied. Symlinks were rejected above.
            EntryKind::Other | EntryKind::Symlink => {}
        }
    }
    Ok(())
}

/// Read a backup copy and check it against the manifest entry. The
/// backup lives inside the workdir, so it is not trusted blindly.
fn load_backup(backup_dir: &Path, relative_path: &str, original: &Entry) -> Result<Vec<u8>> {
    let src = backup_dir.join(relative_path);
    let src_meta = fs::symlink_metadata(&src).map_err(|source| TransactionError::Io {
        path: src.clone(),
        source,
    })?;
    let bytes = state::read_file_stable(&src, &src_meta)
        .map_err(|reason| TransactionError::Unobservable { path: src, reason })?;
    if original.len != Some(bytes.len() as u64) || original.sha256 != Some(ContentHash::of(&bytes))
    {
        return Err(TransactionError::Safety(format!(
            "checkpoint backup does not match its manifest: {relative_path}"
        )));
    }
    Ok(bytes)
}

fn restore_file(
    workdir: &Path,
    backup_dir: &Path,
    relative_path: &str,
    original: &Entry,
) -> Result<()> {
    let bytes = load_backup(backup_dir, relative_path, original)?;
    let dst = workdir.join(relative_path);
    remove_file_if_exists(&dst)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dst)
        .map_err(|source| TransactionError::Io {
            path: dst.clone(),
            source,
        })?;
    file.write_all(&bytes)
        .map_err(|source| TransactionError::Io { path: dst, source })
}

fn write_snapshot_manifest(backup_dir: &Path, manifest: &SnapshotManifest) -> Result<()> {
    let path = backup_dir.join(SNAPSHOT_MANIFEST);
    let bytes = serde_json::to_vec(manifest).map_err(|source| {
        TransactionError::Safety(format!("snapshot manifest encode failed: {source}"))
    })?;
    fs::write(&path, bytes).map_err(|source| TransactionError::Io { path, source })
}

fn read_snapshot_manifest(backup_dir: &Path) -> Result<SnapshotManifest> {
    let path = backup_dir.join(SNAPSHOT_MANIFEST);
    let bytes = fs::read(&path).map_err(|source| TransactionError::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| {
        TransactionError::Safety(format!("snapshot manifest decode failed: {source}"))
    })
}

fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| TransactionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(|source| TransactionError::Io {
        path: from.to_path_buf(),
        source,
    })
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(TransactionError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(TransactionError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("v9r-vfs-test-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_project_roots_as_workdirs() {
        let dir = temp_dir("safety");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"unsafe\"\n").unwrap();

        assert!(!is_safe_directory(&dir));
        let err = ensure_safe_directory(&dir).unwrap_err().to_string();
        assert!(err.contains("Safety Error: Cannot use a project root"));
    }

    #[test]
    fn rollback_restores_modified_files_and_deletes_new_files_only() {
        let dir = temp_dir("rollback");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("keep.txt"), "original\n").unwrap();
        fs::create_dir_all(dir.join("stable-dir")).unwrap();

        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::write(dir.join("keep.txt"), "changed\n").unwrap();
        fs::write(dir.join("new.txt"), "new\n").unwrap();
        fs::create_dir_all(dir.join("new-dir")).unwrap();
        fs::write(dir.join("new-dir/file.txt"), "new nested\n").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("keep.txt")).unwrap(),
            "original\n"
        );
        assert!(!dir.join("new.txt").exists());
        assert!(!dir.join("new-dir/file.txt").exists());
        assert!(dir.join("stable-dir").exists());
        assert!(dir.exists());
    }

    #[test]
    fn rejects_submodule_dot_git_file() {
        // git submodules and linked worktrees have `.git` as a regular
        // file ("gitdir: ..."), not a directory. The old check missed this.
        let dir = temp_dir("submodule");
        fs::write(dir.join(".git"), "gitdir: /elsewhere/.git/modules/x\n").unwrap();
        assert!(!is_safe_directory(&dir));
    }

    #[test]
    fn rejects_node_pyproject_go_projects() {
        for marker in ["package.json", "pyproject.toml", "go.mod"] {
            let dir = temp_dir(&format!("eco-{marker}"));
            fs::write(dir.join(marker), b"x").unwrap();
            assert!(!is_safe_directory(&dir), "{marker} should mark a root");
        }
    }

    #[test]
    fn rejects_subfolder_of_git_repo_unless_opted_in() {
        // Simulate a git repo with an "agent-data" subfolder.
        let repo = temp_dir("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let agent_dir = repo.join("agent-data");
        fs::create_dir_all(&agent_dir).unwrap();

        // Default: rejected because an ancestor has `.git/`.
        assert!(!is_safe_directory(&agent_dir));

        // Opt-in via marker file: now accepted.
        fs::write(agent_dir.join(OPT_IN_MARKER), b"").unwrap();
        assert!(is_safe_directory(&agent_dir));
    }

    #[test]
    fn rollback_is_idempotent() {
        let dir = temp_dir("idempotent");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("a.txt"), "v1\n").unwrap();

        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::write(dir.join("a.txt"), "v2\n").unwrap();
        fs::write(dir.join("b.txt"), "added\n").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();
        // A second rollback must be a no-op, not an error.
        rollback_untraced(task_id, checkpoint).unwrap();
        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "v1\n");
        assert!(!dir.join("b.txt").exists());
    }

    #[test]
    fn rollback_refuses_if_workdir_became_project_root() {
        // If a `git init` (or similar) happens inside the workdir between
        // checkpoint and rollback, refuse to rollback. Otherwise we'd
        // happily delete the brand-new `.git/` as "files not in snapshot."
        let dir = temp_dir("post-checkpoint-git");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("file.txt"), "x").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();

        // Simulate `git init` happening after checkpoint.
        fs::create_dir_all(dir.join(".git")).unwrap();

        let err = rollback_untraced(task_id, checkpoint).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)));
        // And critically: the `.git` we just created is still there.
        assert!(dir.join(".git").exists());
    }

    #[test]
    fn rollback_rejects_tampered_manifest_with_dotdot() {
        let dir = temp_dir("tampered");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("x.txt"), "x").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();

        // Forge a manifest with a `..` path.
        let backup = backup_dir(&dir, task_id, checkpoint);
        let mut state = FsState::default();
        state.entries.insert(
            "../escape.txt".to_string(),
            Entry {
                kind: EntryKind::File,
                len: Some(0),
                sha256: Some(ContentHash::of(b"")),
                link_target: None,
            },
        );
        let bad = SnapshotManifest {
            version: SNAPSHOT_MANIFEST_VERSION,
            state,
        };
        write_snapshot_manifest(&backup, &bad).unwrap();

        let err = rollback_untraced(task_id, checkpoint).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)));
    }

    #[test]
    fn rollback_preserves_trace_history_written_after_checkpoint() {
        // Regression: `trace.jsonl` used to be part of the snapshot, so a
        // rollback restored it to its checkpoint-time content and erased
        // every event logged during the task.
        let dir = temp_dir("trace-history");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("trace.jsonl"), "before\n").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::write(dir.join("trace.jsonl"), "before\nevidence\n").unwrap();
        fs::write(dir.join("new.txt"), "x").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("trace.jsonl")).unwrap(),
            "before\nevidence\n"
        );
        assert!(!dir.join("new.txt").exists());
    }

    #[test]
    fn rollback_restores_file_replaced_by_directory() {
        // Regression: `path.exists()` was true for the directory, so the
        // original file was never restored.
        let dir = temp_dir("file-to-dir");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("a"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::remove_file(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("a/nested")).unwrap();
        fs::write(dir.join("a/nested/inner"), "x").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(fs::read_to_string(dir.join("a")).unwrap(), "orig");
    }

    #[test]
    fn rollback_restores_directory_replaced_by_file() {
        let dir = temp_dir("dir-to-file");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::create_dir_all(dir.join("d")).unwrap();
        fs::write(dir.join("d/x"), "x").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::remove_dir_all(dir.join("d")).unwrap();
        fs::write(dir.join("d"), "now a file").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(fs::read_to_string(dir.join("d/x")).unwrap(), "x");
    }

    #[test]
    fn rollback_recreates_deleted_empty_directory() {
        // Regression: snapshot dirs were only used to decide what not to
        // delete, never restored.
        let dir = temp_dir("empty-dir");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::create_dir_all(dir.join("empty/inner")).unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::remove_dir_all(dir.join("empty")).unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert!(dir.join("empty/inner").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn rollback_never_writes_through_planted_symlink() {
        // Regression: a dangling symlink planted at a snapshotted path made
        // `path.exists()` false, and `fs::copy` then followed the link and
        // created the target outside the workdir.
        let dir = temp_dir("planted-link");
        let outside = temp_dir("planted-link-outside").join("victim.txt");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("a.txt"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::remove_file(dir.join("a.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("a.txt")).unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert!(!outside.exists(), "rollback wrote outside the workdir");
        let meta = fs::symlink_metadata(dir.join("a.txt")).unwrap();
        assert!(meta.is_file());
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "orig");
    }

    #[cfg(unix)]
    #[test]
    fn rollback_removes_symlinks_created_by_task_without_touching_targets() {
        // Regression: task-created symlinks were skipped and survived
        // rollback, including one masking an original file.
        let dir = temp_dir("task-links");
        let outside = temp_dir("task-links-outside");
        fs::write(outside.join("target.txt"), "outside").unwrap();
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("orig.txt"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::remove_file(dir.join("orig.txt")).unwrap();
        std::os::unix::fs::symlink(outside.join("target.txt"), dir.join("orig.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("dirlink")).unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert!(fs::symlink_metadata(dir.join("dirlink")).is_err());
        assert_eq!(fs::read_to_string(dir.join("orig.txt")).unwrap(), "orig");
        assert!(fs::symlink_metadata(dir.join("orig.txt"))
            .unwrap()
            .is_file());
        assert_eq!(
            fs::read_to_string(outside.join("target.txt")).unwrap(),
            "outside"
        );
    }

    #[test]
    fn rollback_refuses_backup_that_does_not_match_manifest() {
        let dir = temp_dir("tampered-backup");
        let task_id = Uuid::new_v4();
        register_task(task_id, dir.clone());
        fs::write(dir.join("a.txt"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap();
        fs::write(dir.join("a.txt"), "changed").unwrap();
        let backup = backup_dir(&dir, task_id, checkpoint);
        fs::write(backup.join("a.txt"), "forged").unwrap();

        fs::write(dir.join("new.txt"), "new").unwrap();

        let err = rollback_untraced(task_id, checkpoint).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)), "{err}");
        // Fails closed: nothing was touched.
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "changed");
        assert!(dir.join("new.txt").exists());
    }
}
