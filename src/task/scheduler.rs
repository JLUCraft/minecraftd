use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Fallback concurrency when `available_parallelism()` fails.
pub const FALLBACK_CONCURRENCY: usize = 4;

/// Errors that can occur during task scheduling.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    #[error("task execution failed: {0}")]
    Execution(String),
    #[error("task was cancelled")]
    Cancelled,
    #[error("task timed out")]
    Timeout,
    #[error("scheduler is shutting down")]
    ShuttingDown,
}

/// A boxed future that returns a `TaskResult`.
pub type TaskFuture = Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send>>;

/// Trait for task schedulers.
///
/// Task schedulers manage the execution of background tasks such as
/// file downloads, installations, and maintenance operations.
///
/// Inspired by SJMCL's `TaskMonitor`.
#[async_trait]
pub trait TaskScheduler: Send + Sync {
    /// Submits a task for execution.
    async fn submit(&self, name: String, task: TaskFuture) -> Result<u64, TaskError>;

    /// Cancels a task by ID.
    fn cancel(&self, task_id: u64) -> Result<(), TaskError>;

    /// Returns the number of active tasks.
    fn active_count(&self) -> usize;

    /// Returns the number of queued tasks.
    fn queued_count(&self) -> usize;

    /// Shuts down the scheduler, cancelling all pending tasks.
    async fn shutdown(&self);
}

/// A task scheduler backed by tokio with concurrency limiting.
///
/// Uses a semaphore to control the maximum number of concurrent tasks.
#[derive(Debug)]
pub struct TokioTaskScheduler {
    semaphore: Arc<Semaphore>,
    task_counter: AtomicU64,
    max_concurrency: usize,
}

impl TokioTaskScheduler {
    #[must_use]
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            task_counter: AtomicU64::new(0),
            max_concurrency,
        }
    }

    #[must_use]
    pub fn with_default_concurrency() -> Self {
        let cpus = std::thread::available_parallelism()
            .map_or(FALLBACK_CONCURRENCY, std::num::NonZero::get);
        Self::new(cpus)
    }
}

impl Default for TokioTaskScheduler {
    fn default() -> Self {
        Self::with_default_concurrency()
    }
}

#[async_trait]
impl TaskScheduler for TokioTaskScheduler {
    async fn submit(&self, name: String, task: TaskFuture) -> Result<u64, TaskError> {
        let task_id = self.task_counter.fetch_add(1, Ordering::SeqCst);
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| TaskError::Execution(e.to_string()))?;

        debug!("submitting task {} ({})", task_id, name);

        tokio::spawn(async move {
            let _permit = permit;
            debug!("task {} ({}) started", task_id, name);

            match task.await {
                Ok(()) => {
                    debug!("task {} ({}) completed", task_id, name);
                }
                Err(e) => {
                    warn!("task {} ({}) failed: {}", task_id, name, e);
                }
            }
        });

        Ok(task_id)
    }

    fn cancel(&self, _task_id: u64) -> Result<(), TaskError> {
        Err(TaskError::Execution(
            "task cancellation is not yet implemented".to_string(),
        ))
    }

    fn active_count(&self) -> usize {
        // The number of available permits indicates how many slots are free
        self.max_concurrency - self.semaphore.available_permits()
    }

    fn queued_count(&self) -> usize {
        // With tokio::spawn, tasks are either active or not yet polled
        // A more sophisticated implementation would track a queue separately
        0
    }

    async fn shutdown(&self) {
        info!("shutting down task scheduler");
        // Close the semaphore to prevent new tasks
        self.semaphore.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tokio_scheduler() {
        let scheduler = TokioTaskScheduler::new(2);

        let task = Box::pin(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            Ok(())
        });

        let id = scheduler.submit("test".into(), task).await.unwrap();
        assert_eq!(id, 0);

        // Wait for task to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_tokio_scheduler_active_count_and_shutdown() {
        let scheduler = TokioTaskScheduler::new(1);

        let task = Box::pin(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            Ok(())
        });

        scheduler.submit("slow".into(), task).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(scheduler.active_count(), 1);
        assert_eq!(scheduler.queued_count(), 0);

        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn test_tokio_scheduler_cancel_not_implemented() {
        let scheduler = TokioTaskScheduler::new(2);
        let result = scheduler.cancel(999);
        assert!(matches!(result, Err(TaskError::Execution(_))));
    }

    #[test]
    fn test_tokio_scheduler_default() {
        let scheduler = TokioTaskScheduler::default();
        assert!(scheduler.max_concurrency > 0);
    }

    #[tokio::test]
    async fn test_tokio_scheduler_failed_task() {
        let scheduler = TokioTaskScheduler::new(2);
        let task = Box::pin(async move { Err(TaskError::Execution("boom".into())) });
        let id = scheduler.submit("fail".into(), task).await.unwrap();
        assert_eq!(id, 0);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
}
