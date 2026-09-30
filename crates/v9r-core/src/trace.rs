use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::effect::ReceiptRecord;
use crate::kernel::DecisionRecord;
use crate::manifest::{AccessType, Manifest};
use crate::state::ContentHash;
use crate::task::TaskStatus;
use crate::trusted::StateRoot;
use crate::vfs::CheckpointId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    TaskStarted {
        timestamp: DateTime<Utc>,
        manifest: Manifest,
    },
    CheckpointCreated {
        id: CheckpointId,
    },
    RollbackPerformed {
        id: CheckpointId,
    },
    CommandExecuted {
        command: String,
        exit_code: i32,
    },
    FileAccess {
        path: PathBuf,
        access: AccessType,
        allowed: bool,
    },
    ViolationOccurred {
        reason: String,
    },
    TaskFinished {
        status: TaskStatus,
    },
    TaskHandoverReceived {
        timestamp: DateTime<Utc>,
        source_task_id: Uuid,
    },
    /// Derived claim about the state transition across one action. Unlike
    /// `CommandExecuted`, this is not execution history but a comparison
    /// of observed pre/post state; see `crate::effect`.
    EffectObserved {
        receipt: Box<ReceiptRecord>,
    },
    /// An invariant-kernel decision on the guarded path.
    InvariantDecision {
        record: Box<DecisionRecord>,
    },
}

impl TaskEvent {
    pub fn task_started(manifest: Manifest) -> Self {
        Self::TaskStarted {
            timestamp: Utc::now(),
            manifest,
        }
    }

    pub fn task_handover_received(source_task_id: Uuid) -> Self {
        Self::TaskHandoverReceived {
            timestamp: Utc::now(),
            source_task_id,
        }
    }
}

/// Append-only trace writer. It keeps a running SHA-256 and length of
/// everything the file should contain (pre-existing content at open, plus
/// every line written through this logger), so edits made behind its back
/// are detectable with [`TraceLogger::verify_integrity`].
#[derive(Clone)]
pub struct TraceLogger {
    path: PathBuf,
    file: Arc<Mutex<SealedFile>>,
}

struct SealedFile {
    file: File,
    hasher: Sha256,
    len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceIntegrity {
    Intact,
    Tampered { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("trace io at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("trace serialization: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, TraceError>;

impl TraceLogger {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| TraceError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|source| TraceError::Io {
                path: path.clone(),
                source,
            })?;
        let existing = tokio::fs::read(&path)
            .await
            .map_err(|source| TraceError::Io {
                path: path.clone(),
                source,
            })?;
        let mut hasher = Sha256::new();
        hasher.update(&existing);

        Ok(Self {
            path,
            file: Arc::new(Mutex::new(SealedFile {
                file,
                hasher,
                len: existing.len() as u64,
            })),
        })
    }

    /// Open the trace of `task_id` under the trusted state root.
    pub async fn for_task(root: &StateRoot, task_id: Uuid) -> Result<Self> {
        Self::new(root.trace_path(task_id)).await
    }

    /// Compare the file on disk with what this logger has written.
    pub async fn verify_integrity(&self) -> Result<TraceIntegrity> {
        let sealed = self.file.lock().await;
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TraceIntegrity::Tampered {
                    reason: "trace file is missing".to_string(),
                })
            }
            Err(source) => {
                return Err(TraceError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        if bytes.len() as u64 != sealed.len {
            return Ok(TraceIntegrity::Tampered {
                reason: format!(
                    "trace length is {} bytes, runtime wrote {}",
                    bytes.len(),
                    sealed.len
                ),
            });
        }
        let expected: [u8; 32] = sealed.hasher.clone().finalize().into();
        if ContentHash::of(&bytes) != ContentHash::from_bytes(expected) {
            return Ok(TraceIntegrity::Tampered {
                reason: "trace content differs from what the runtime wrote".to_string(),
            });
        }
        Ok(TraceIntegrity::Intact)
    }

    pub async fn log_event(&self, event: TaskEvent) -> Result<()> {
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');

        let mut sealed = self.file.lock().await;
        sealed
            .file
            .write_all(&line)
            .await
            .map_err(|source| TraceError::Io {
                path: self.path.clone(),
                source,
            })?;
        sealed.file.flush().await.map_err(|source| TraceError::Io {
            path: self.path.clone(),
            source,
        })?;
        sealed.hasher.update(&line);
        sealed.len += line.len() as u64;
        Ok(())
    }

    pub async fn recent_events(&self, limit: usize) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let text = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|source| TraceError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut lines: Vec<String> = text.lines().rev().take(limit).map(str::to_string).collect();
        lines.reverse();
        Ok(lines)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seal_detects_edits_appends_and_deletion() {
        let dir = std::env::temp_dir().join(format!("v9r-trace-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trace.jsonl");
        std::fs::write(&path, "{\"imported\":true}\n").unwrap();
        let trace = TraceLogger::new(&path).await.unwrap();
        let event = TaskEvent::ViolationOccurred {
            reason: "x".to_string(),
        };
        trace.log_event(event.clone()).await.unwrap();
        assert_eq!(
            trace.verify_integrity().await.unwrap(),
            TraceIntegrity::Intact
        );

        // Same length, different bytes.
        let original = std::fs::read(&path).unwrap();
        let mut edited = original.clone();
        let last = edited.len() - 3;
        edited[last] = b'y';
        std::fs::write(&path, &edited).unwrap();
        assert!(matches!(
            trace.verify_integrity().await.unwrap(),
            TraceIntegrity::Tampered { .. }
        ));

        // Appended by someone else.
        std::fs::write(&path, [original.as_slice(), b"{}\n"].concat()).unwrap();
        assert!(matches!(
            trace.verify_integrity().await.unwrap(),
            TraceIntegrity::Tampered { .. }
        ));

        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            trace.verify_integrity().await.unwrap(),
            TraceIntegrity::Tampered { .. }
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
