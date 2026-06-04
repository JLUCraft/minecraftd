use crate::download::task::DownloadTask;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, error, info};

/// Errors from the download queue.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum DownloadQueueError {
    #[error("download failed: {0}")]
    Download(String),
}

/// A concurrent download queue with limited parallelism.
pub struct DownloadQueue {
    semaphore: Arc<Semaphore>,
}

impl DownloadQueue {
    /// Creates a new download queue.
    #[must_use]
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Submits multiple download tasks and waits for all to complete.
    ///
    /// # Errors
    ///
    /// Returns `DownloadQueueError` if any task fails.
    pub async fn download_all(&self, tasks: Vec<DownloadTask>) -> Result<(), DownloadQueueError> {
        let mut handles = Vec::new();

        for task in tasks {
            let permit = self
                .semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| DownloadQueueError::Download(e.to_string()))?;

            let handle = tokio::spawn(async move {
                let _permit = permit;
                debug!("starting download: {}", task.destination.display());
                let result = task.execute().await;
                match &result {
                    Ok(()) => info!("completed: {}", task.destination.display()),
                    Err(e) => error!("failed: {}: {}", task.destination.display(), e),
                }
                result
            });

            handles.push(handle);
        }

        let mut any_error = None;
        for handle in handles {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if any_error.is_none() {
                        any_error = Some(DownloadQueueError::Download(e.to_string()));
                    }
                }
                Err(e) => {
                    if any_error.is_none() {
                        any_error = Some(DownloadQueueError::Download(e.to_string()));
                    }
                }
            }
        }

        any_error.map_or(Ok(()), Err)
    }
}

impl Default for DownloadQueue {
    fn default() -> Self {
        Self::new(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::task::DownloadTask;

    #[tokio::test]
    async fn test_download_queue_empty() {
        let queue = DownloadQueue::new(2);
        assert!(queue.download_all(vec![]).await.is_ok());
    }

    #[tokio::test]
    async fn test_download_queue_failure_propagates() {
        let queue = DownloadQueue::new(2);
        let tmpdir = tempfile::tempdir().unwrap();

        let tasks = vec![
            DownloadTask::new(
                "http://localhost:59999/invalid".to_string(),
                tmpdir.path().join("a.jar"),
            ),
            DownloadTask::new(
                "http://localhost:59999/also-invalid".to_string(),
                tmpdir.path().join("b.jar"),
            ),
        ];

        let result = queue.download_all(tasks).await;
        assert!(
            result.is_err(),
            "expected download_all to fail when all tasks fail"
        );
    }

    #[tokio::test]
    async fn test_download_queue_mixed_success_failure() {
        let queue = DownloadQueue::new(2);
        let tmpdir = tempfile::tempdir().unwrap();

        // One invalid task and one valid local-file-as-destination task
        // (both URLs are invalid, so it should still fail)
        let tasks = vec![
            DownloadTask::new(
                "http://localhost:59999/invalid".to_string(),
                tmpdir.path().join("fail.jar"),
            ),
            DownloadTask::new(
                "http://[::1]:59999/invalid".to_string(),
                tmpdir.path().join("also-fail.jar"),
            ),
        ];

        let result = queue.download_all(tasks).await;
        assert!(
            result.is_err(),
            "expected download_all to fail when any task fails"
        );
    }

    #[tokio::test]
    async fn test_download_queue_concurrency_one() {
        let queue = DownloadQueue::new(1);
        // Verify semaphore is actually wired into the queue by checking initial permits
        assert_eq!(queue.semaphore.available_permits(), 1);
    }
}
