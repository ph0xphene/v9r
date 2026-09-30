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
use crate::trusted::{self, StateRoot, TrustError};

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
    #[error("trusted state: {0}")]
    Trust(#[from] TrustError),
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
    /// Where this task's trusted state lives. Resolved from the
    /// environment at the first checkpoint if not registered explicitly.
    state_root: Option<StateRoot>,
    /// In-memory seals of the checkpoints this process created. The
    /// on-disk manifest is only trusted if it still matches its seal.
    seals: HashMap<CheckpointId, CheckpointSeal>,
}

/// What the runtime remembers about a checkpoint independently of disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointSeal {
    /// SHA-256 of the manifest file as written.
    pub manifest: ContentHash,
    /// Digest of the checkpointed workspace state.
    pub state: ContentHash,
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
            state_root: None,
            seals: HashMap::new(),
        });
}

/// Register a task with an explicit trusted state root. Fails if the
/// workspace and the state root overlap.
pub fn register_task_with_state(
    task_id: Uuid,
    workdir: PathBuf,
    state_root: StateRoot,
) -> Result<()> {
    state_root.check_separation(&workdir)?;
    register_task(task_id, workdir);
    let mut tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(state) = tasks.get_mut(&task_id) {
        state.state_root = Some(state_root);
    }
    Ok(())
}

/// The trusted state root registered for a task, if any.
pub fn task_state_root(task_id: Uuid) -> Option<StateRoot> {
    let tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tasks
        .get(&task_id)
        .and_then(|state| state.state_root.clone())
}

/// The in-memory seal of a checkpoint created by this process.
pub fn checkpoint_seal(task_id: Uuid, checkpoint: CheckpointId) -> Option<CheckpointSeal> {
    let tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tasks
        .get(&task_id)
        .and_then(|state| state.seals.get(&checkpoint).copied())
}

/// Re-hash every sealed checkpoint manifest of a task. Returns the
/// checkpoints whose manifest is missing or no longer matches its seal.
/// Backup payloads are checked against the manifest at rollback time.
pub fn tampered_checkpoints(task_id: Uuid) -> Result<Vec<CheckpointId>> {
    let (root, seals) = {
        let tasks = tasks()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = tasks
            .get(&task_id)
            .ok_or(TransactionError::UnknownTask(task_id))?;
        (state.state_root.clone(), state.seals.clone())
    };
    let Some(root) = root else {
        return Ok(Vec::new());
    };
    let mut tampered: Vec<CheckpointId> = seals
        .iter()
        .filter(|(id, seal)| {
            let path = checkpoint_dir(&root, task_id, **id).join(SNAPSHOT_MANIFEST);
            fs::read(path).map(|bytes| ContentHash::of(&bytes)).ok() != Some(seal.manifest)
        })
        .map(|(id, _)| *id)
        .collect();
    tampered.sort_by_key(|id| id.0);
    Ok(tampered)
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
    Ok(checkpoint_with_state(task_id, trace).await?.0)
}

/// Checkpoint and also return the observed workspace state, so callers
/// that need an observation right after checkpointing need not rescan.
pub(crate) async fn checkpoint_with_state(
    task_id: Uuid,
    trace: &TraceLogger,
) -> Result<(CheckpointId, PathBuf, FsState)> {
    let (checkpoint, workdir, state) =
        tokio::task::spawn_blocking(move || checkpoint_untraced(task_id))
            .await
            .map_err(|err| TransactionError::Join(err.to_string()))??;
    trace
        .log_event(TaskEvent::CheckpointCreated { id: checkpoint })
        .await?;
    Ok((checkpoint, workdir, state))
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

fn checkpoint_untraced(task_id: Uuid) -> Result<(CheckpointId, PathBuf, FsState)> {
    let workdir = registered_workdir(task_id, true)?;
    ensure_safe_directory(&workdir)?;
    trusted::check_legacy_layout(&workdir)?;
    let root = resolve_state_root(task_id)?;
    root.check_separation(&workdir)?;
    root.ensure_task_dir(task_id)?;

    let checkpoint = CheckpointId(Uuid::new_v4());
    let backup_dir = checkpoint_dir(&root, task_id, checkpoint);
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
    let manifest_hash = write_snapshot_manifest(&tmp_path, &manifest)?;
    rename(&tmp_path, &backup_dir)?;

    let seal = CheckpointSeal {
        manifest: manifest_hash,
        state: manifest.state.digest(),
    };
    let mut tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(state) = tasks.get_mut(&task_id) {
        state.seals.insert(checkpoint, seal);
    }
    Ok((checkpoint, workdir, manifest.state))
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

    // Only checkpoints sealed by this process are restorable: the seal is
    // what makes the on-disk manifest trustworthy.
    let seal = checkpoint_seal(task_id, checkpoint).ok_or_else(|| {
        TransactionError::Safety(format!(
            "checkpoint {} was not created by this runtime; refusing to restore from it",
            checkpoint.0
        ))
    })?;
    let root = task_state_root(task_id)
        .ok_or_else(|| TransactionError::Safety("task has no trusted state root".to_string()))?;
    let backup_dir = checkpoint_dir(&root, task_id, checkpoint);
    let manifest = read_snapshot_manifest(&backup_dir, seal.manifest)?;
    selective_rollback(&workdir, &backup_dir, &manifest)?;
    Ok(())
}

fn resolve_state_root(task_id: Uuid) -> Result<StateRoot> {
    if let Some(root) = task_state_root(task_id) {
        return Ok(root);
    }
    let root = StateRoot::from_env()?;
    let mut tasks = tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = tasks
        .get_mut(&task_id)
        .ok_or(TransactionError::UnknownTask(task_id))?;
    Ok(state.state_root.get_or_insert(root).clone())
}

fn checkpoint_dir(root: &StateRoot, task_id: Uuid, checkpoint: CheckpointId) -> PathBuf {
    root.checkpoints_dir(task_id).join(checkpoint.0.to_string())
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

fn write_snapshot_manifest(backup_dir: &Path, manifest: &SnapshotManifest) -> Result<ContentHash> {
    let path = backup_dir.join(SNAPSHOT_MANIFEST);
    let bytes = serde_json::to_vec(manifest).map_err(|source| {
        TransactionError::Safety(format!("snapshot manifest encode failed: {source}"))
    })?;
    let hash = ContentHash::of(&bytes);
    fs::write(&path, bytes).map_err(|source| TransactionError::Io { path, source })?;
    Ok(hash)
}

fn read_snapshot_manifest(backup_dir: &Path, sealed: ContentHash) -> Result<SnapshotManifest> {
    let path = backup_dir.join(SNAPSHOT_MANIFEST);
    let bytes = fs::read(&path).map_err(|source| TransactionError::Io {
        path: path.clone(),
        source,
    })?;
    if ContentHash::of(&bytes) != sealed {
        return Err(TransactionError::Safety(format!(
            "checkpoint manifest does not match its seal: {}",
            path.display()
        )));
    }
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

    /// A workspace and a separate trusted state root, registered together.
    fn registered(name: &str) -> (PathBuf, Uuid) {
        let base = temp_dir(name);
        let dir = base.join("work");
        fs::create_dir_all(&dir).unwrap();
        let task_id = Uuid::new_v4();
        let root = StateRoot::open(&base.join("state")).unwrap();
        register_task_with_state(task_id, dir.clone(), root).unwrap();
        (dir, task_id)
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
        let (dir, task_id) = registered("rollback");
        fs::write(dir.join("keep.txt"), "original\n").unwrap();
        fs::create_dir_all(dir.join("stable-dir")).unwrap();

        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
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
        let (dir, task_id) = registered("idempotent");
        fs::write(dir.join("a.txt"), "v1\n").unwrap();

        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
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
        let (dir, task_id) = registered("post-checkpoint-git");
        fs::write(dir.join("file.txt"), "x").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;

        // Simulate `git init` happening after checkpoint.
        fs::create_dir_all(dir.join(".git")).unwrap();

        let err = rollback_untraced(task_id, checkpoint).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)));
        // And critically: the `.git` we just created is still there.
        assert!(dir.join(".git").exists());
    }

    #[test]
    fn rollback_rejects_tampered_manifest_with_dotdot() {
        let (dir, task_id) = registered("tampered");
        fs::write(dir.join("x.txt"), "x").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;

        // Forge a manifest with a `..` path.
        let backup = checkpoint_dir(&task_state_root(task_id).unwrap(), task_id, checkpoint);
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
        // Regression: the trace used to live in the workspace and was
        // restored to its checkpoint-time content, erasing every event
        // logged during the task. It now lives under the state root.
        let (dir, task_id) = registered("trace-history");
        let trace = task_state_root(task_id).unwrap().trace_path(task_id);
        fs::create_dir_all(trace.parent().unwrap()).unwrap();
        fs::write(&trace, "before\n").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        fs::write(&trace, "before\nevidence\n").unwrap();
        fs::write(dir.join("new.txt"), "x").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(fs::read_to_string(&trace).unwrap(), "before\nevidence\n");
        assert!(!dir.join("new.txt").exists());
    }

    #[test]
    fn trusted_state_is_outside_the_workspace() {
        let (dir, task_id) = registered("outside");
        fs::write(dir.join("a.txt"), "a").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        let root = task_state_root(task_id).unwrap();
        let backup = checkpoint_dir(&root, task_id, checkpoint);
        assert!(backup.join(SNAPSHOT_MANIFEST).is_file());
        assert!(!backup.starts_with(&dir));
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["a.txt"]);
    }

    #[test]
    fn workspace_dot_v9r_created_by_task_is_ordinary_content() {
        // With runtime state moved out, `.v9r` in the workspace is task
        // content: it is snapshotted, observed and rolled back like any file.
        let (dir, task_id) = registered("dot-v9r");
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        fs::create_dir_all(dir.join(".v9r/backups")).unwrap();
        fs::write(dir.join(".v9r/backups/manifest.json"), "forged").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert!(!dir.join(".v9r").exists());
    }

    #[test]
    fn legacy_workspace_layout_fails_explicitly() {
        let (dir, task_id) = registered("legacy");
        fs::create_dir_all(dir.join(".v9r/backups")).unwrap();
        let err = checkpoint_untraced(task_id).unwrap_err();
        assert!(
            matches!(
                err,
                TransactionError::Trust(TrustError::LegacyLayout { .. })
            ),
            "{err}"
        );

        let (dir, task_id) = registered("legacy-trace");
        fs::write(dir.join("trace.jsonl"), "{}").unwrap();
        assert!(checkpoint_untraced(task_id).is_err());
    }

    #[test]
    fn rollback_refuses_checkpoint_not_sealed_by_this_runtime() {
        let (_dir, task_id) = registered("unsealed");
        checkpoint_untraced(task_id).unwrap();
        let foreign = CheckpointId(Uuid::new_v4());
        let err = rollback_untraced(task_id, foreign).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)), "{err}");
    }

    #[test]
    fn rollback_refuses_manifest_that_no_longer_matches_seal() {
        let (dir, task_id) = registered("reseal");
        fs::write(dir.join("a.txt"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        fs::write(dir.join("a.txt"), "changed").unwrap();
        let root = task_state_root(task_id).unwrap();
        let backup = checkpoint_dir(&root, task_id, checkpoint);
        // A consistent forgery: backup and manifest both rewritten.
        fs::write(backup.join("a.txt"), "forged").unwrap();
        let mut manifest: SnapshotManifest =
            serde_json::from_slice(&fs::read(backup.join(SNAPSHOT_MANIFEST)).unwrap()).unwrap();
        manifest.state.entries.insert(
            "a.txt".to_string(),
            Entry {
                kind: EntryKind::File,
                len: Some(6),
                sha256: Some(ContentHash::of(b"forged")),
                link_target: None,
            },
        );
        write_snapshot_manifest(&backup, &manifest).unwrap();
        assert_eq!(tampered_checkpoints(task_id).unwrap(), [checkpoint]);

        let err = rollback_untraced(task_id, checkpoint).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)), "{err}");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "changed");
    }

    #[test]
    fn rollback_restores_file_replaced_by_directory() {
        // Regression: `path.exists()` was true for the directory, so the
        // original file was never restored.
        let (dir, task_id) = registered("file-to-dir");
        fs::write(dir.join("a"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        fs::remove_file(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("a/nested")).unwrap();
        fs::write(dir.join("a/nested/inner"), "x").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(fs::read_to_string(dir.join("a")).unwrap(), "orig");
    }

    #[test]
    fn rollback_restores_directory_replaced_by_file() {
        let (dir, task_id) = registered("dir-to-file");
        fs::create_dir_all(dir.join("d")).unwrap();
        fs::write(dir.join("d/x"), "x").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        fs::remove_dir_all(dir.join("d")).unwrap();
        fs::write(dir.join("d"), "now a file").unwrap();

        rollback_untraced(task_id, checkpoint).unwrap();

        assert_eq!(fs::read_to_string(dir.join("d/x")).unwrap(), "x");
    }

    #[test]
    fn rollback_recreates_deleted_empty_directory() {
        // Regression: snapshot dirs were only used to decide what not to
        // delete, never restored.
        let (dir, task_id) = registered("empty-dir");
        fs::create_dir_all(dir.join("empty/inner")).unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
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
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
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
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
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
        let (dir, task_id) = registered("tampered-backup");
        fs::write(dir.join("a.txt"), "orig").unwrap();
        let checkpoint = checkpoint_untraced(task_id).unwrap().0;
        fs::write(dir.join("a.txt"), "changed").unwrap();
        let backup = checkpoint_dir(&task_state_root(task_id).unwrap(), task_id, checkpoint);
        fs::write(backup.join("a.txt"), "forged").unwrap();

        fs::write(dir.join("new.txt"), "new").unwrap();

        let err = rollback_untraced(task_id, checkpoint).unwrap_err();
        assert!(matches!(err, TransactionError::Safety(_)), "{err}");
        // Fails closed: nothing was touched.
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "changed");
        assert!(dir.join("new.txt").exists());
    }
}
