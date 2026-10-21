//! Port of `src/core/tools/file-mutation-queue.ts`.
//!
//! Serialize file mutation operations targeting the same file; operations
//! for different files still run in parallel. The queue key is the file's
//! real path (`realpath`) when it exists, and the resolved path otherwise
//! (ENOENT/ENOTDIR fall back), so a symlink and its target share a queue.
//!
//! TS keeps module-level state (`fileMutationQueues`); here a
//! [`FileMutationQueues`] value owns it, and [`with_file_mutation_queue`]
//! drives a process-global instance for parity with the module singleton.
//!
//! Divergence (harness-only relevance): the TS registration queue exists
//! because `realpath` is async and concurrent registrations could interleave
//! map reads/writes; here the key is computed synchronously before the map
//! lock is taken, so no registration serialization is needed. Observable
//! ordering (per-file serialization, cross-file parallelism) is identical.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Mutex as AsyncMutex;

/// Per-process queues, mirroring the TS module-level `fileMutationQueues`.
static QUEUES: OnceLock<FileMutationQueues> = OnceLock::new();

/// `isMissingPathError`: ENOENT or ENOTDIR.
fn is_missing_path_error(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(error.kind(), NotFound | NotADirectory)
}

/// `getMutationQueueKey`: resolved path, realpath'd when it exists. Missing
/// paths (ENOENT/ENOTDIR) fall back to the resolved path; other errors
/// propagate.
pub fn get_mutation_queue_key(file_path: &str, cwd: &Path) -> Result<String, String> {
    let resolved = pi_core::paths::resolve_path(file_path, &cwd.to_string_lossy(), &pi_core::paths::PathInputOptions::default());
    match std::fs::canonicalize(&resolved) {
        Ok(real) => Ok(real.to_string_lossy().into_owned()),
        Err(error) if is_missing_path_error(&error) => Ok(resolved),
        Err(error) => Err(error.to_string()),
    }
}

/// The mutation-queue state: one async lock per queue key.
#[derive(Default)]
pub struct FileMutationQueues {
    queues: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl FileMutationQueues {
    pub fn new() -> Self {
        Self::default()
    }

    /// `withFileMutationQueue` on an explicit instance: run `f` while holding
    /// this file's queue. Same-file operations serialize; different files run
    /// in parallel. The queue entry is dropped once the last holder releases
    /// it (matching the TS delete when the map still holds the chained
    /// queue).
    pub async fn with_queue<T, F>(&self, file_path: &str, cwd: &Path, f: F) -> Result<T, String>
    where
        F: Future<Output = T>,
    {
        let key = get_mutation_queue_key(file_path, cwd)?;
        let lock = {
            let mut map = self.queues.lock().expect("file mutation queue map");
            Arc::clone(map.entry(key.clone()).or_insert_with(|| Arc::new(AsyncMutex::new(()))))
        };
        let _guard = lock.lock().await;
        let result = f.await;
        let mut map = self.queues.lock().expect("file mutation queue map");
        if let Some(existing) = map.get(&key) {
            if Arc::ptr_eq(existing, &lock) {
                map.remove(&key);
            }
        }
        Ok(result)
    }
}

/// `withFileMutationQueue` against the process-global queues (the TS module
/// singleton).
pub async fn with_file_mutation_queue<T, F>(file_path: &str, cwd: &Path, f: F) -> Result<T, String>
where
    F: Future<Output = T>,
{
    let queues = QUEUES.get_or_init(FileMutationQueues::new);
    queues.with_queue(file_path, cwd, f).await
}
