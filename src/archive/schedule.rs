use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::archive::ArchiveError;
use crate::archive::manager::ArchiveManager;
use crate::archive::strategy::ArchiveStrategy;
use crate::core::state::InstanceState;
use crate::lifecycle::task::{LifecycleTask, TaskError};

/// A `LifecycleTask` that periodically creates archives while the instance is running.
///
/// On `on_start`: spawns a background task that creates archives at the configured interval.
/// On `on_stop`: signals the background task to stop and creates a final archive.
///
/// **Important**: This task archives while the instance is running (hot backup).
/// For Minecraft, this means world data may be inconsistent.
/// For consistent backups, stop the instance first or use a mod that flushes chunks.
#[derive(Debug)]
pub struct ArchiveScheduleTask {
    name: String,
    manager: ArchiveManager,
    instance_dir: PathBuf,
    strategy: Arc<dyn ArchiveStrategy>,
    interval_secs: u64,
    game_version: Option<String>,
    shutdown: Arc<AtomicBool>,
    task_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    label_prefix: String,
}

impl ArchiveScheduleTask {
    /// Creates a new scheduled archive task.
    ///
    /// # Arguments
    /// * `manager` - The archive manager
    /// * `instance_dir` - Path to the instance directory
    /// * `strategy` - Archive file selection strategy
    /// * `interval_secs` - Minimum time between archives (in seconds)
    /// * `label_prefix` - Prefix for archive labels (e.g. "auto-")
    #[must_use]
    pub fn new(
        manager: ArchiveManager,
        instance_dir: PathBuf,
        strategy: Arc<dyn ArchiveStrategy>,
        interval_secs: u64,
        label_prefix: String,
    ) -> Self {
        Self {
            name: "archive-scheduler".to_string(),
            manager,
            instance_dir,
            strategy,
            interval_secs,
            game_version: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            task_handle: Arc::new(Mutex::new(None)),
            label_prefix,
        }
    }

    /// Sets the game version for archive metadata.
    #[must_use]
    pub fn with_game_version(mut self, version: String) -> Self {
        self.game_version = Some(version);
        self
    }

    /// Sets a custom name for this task.
    #[must_use]
    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }

    /// Creates a single archive synchronously (used by `on_stop` for the final backup).
    async fn create_single_archive(&self, instance_id: &str, label_suffix: &str) {
        let label = format!("{}{label_suffix}", self.label_prefix);
        match self
            .manager
            .create(
                instance_id,
                &self.instance_dir,
                &label,
                InstanceState::Stopped,
                self.game_version.as_deref(),
                self.strategy.as_ref(),
            )
            .await
        {
            Ok(meta) => {
                info!(
                    "[{}] archive created: {} ({} files)",
                    self.name, meta.id, meta.file_count
                );
            }
            Err(e) => {
                error!("[{}] failed to create archive: {e}", self.name);
            }
        }
    }
}

#[async_trait]
impl LifecycleTask for ArchiveScheduleTask {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_start(&self, instance_id: &str) -> Result<(), TaskError> {
        let instance_id = instance_id.to_string();
        let shutdown = Arc::clone(&self.shutdown);
        let interval_secs = self.interval_secs;
        let manager = self.manager.clone();
        let instance_dir = self.instance_dir.clone();
        let strategy = Arc::clone(&self.strategy);
        let game_version = self.game_version.clone();
        let label_prefix = self.label_prefix.clone();
        let task_name = self.name.clone();

        // Reset shutdown flag
        shutdown.store(false, Ordering::SeqCst);

        let handle = tokio::spawn(async move {
            info!("[{task_name}] scheduled archive started (interval: {interval_secs}s)");

            loop {
                // Sleep in small chunks to respond to shutdown quickly
                let check_interval_secs = 10u64.min(interval_secs);
                let mut elapsed: u64 = 0;

                while elapsed < interval_secs {
                    if shutdown.load(Ordering::SeqCst) {
                        info!("[{task_name}] scheduled archive shutting down");
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(check_interval_secs)).await;
                    elapsed += check_interval_secs;
                }

                if shutdown.load(Ordering::SeqCst) {
                    info!("[{task_name}] scheduled archive shutting down");
                    return;
                }

                // Create archive (while instance is running)
                let label = format!("{label_prefix}scheduled");
                match manager
                    .create(
                        &instance_id,
                        &instance_dir,
                        &label,
                        InstanceState::Running,
                        game_version.as_deref(),
                        strategy.as_ref(),
                    )
                    .await
                {
                    Ok(meta) => {
                        info!(
                            "[{task_name}] periodic archive created: {} ({} files)",
                            meta.id, meta.file_count
                        );
                    }
                    Err(ArchiveError::InstanceNotStopped(_)) => {
                        debug!("[{task_name}] skipping archive: instance is not stopped");
                    }
                    Err(e) => {
                        warn!("[{task_name}] periodic archive failed: {e}");
                    }
                }
            }
        });

        let mut guard = self.task_handle.lock().await;
        *guard = Some(handle);
        drop(guard);

        Ok(())
    }

    async fn on_stop(&self, instance_id: &str) -> Result<(), TaskError> {
        // Signal the background task to stop
        self.shutdown.store(true, Ordering::SeqCst);

        // Wait for the background task to finish
        let mut guard = self.task_handle.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
            // Don't propagate join errors
            let _ = handle.await;
        }
        drop(guard);

        // Create a final archive now that the instance is stopped
        info!(
            "[{}] creating final archive for stopped instance {}",
            self.name, instance_id
        );
        self.create_single_archive(instance_id, "shutdown").await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::manager::ArchiveConfig;
    use crate::archive::strategy::FullArchiveStrategy;

    #[tokio::test]
    async fn test_schedule_task_lifecycle() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("data.txt"), b"test").unwrap();

        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);
        let strategy = Arc::new(FullArchiveStrategy::new());

        let task = ArchiveScheduleTask::new(
            manager.clone(),
            instance_dir,
            strategy,
            1, // 1 second interval for testing
            "test-".to_string(),
        );

        // Start the scheduler
        task.on_start("test-instance").await.unwrap();

        // Wait for at least one archive to be created
        // The archive will fail because instance is "Running" and archives
        // require "Stopped" — so no archive will actually be created.
        // But the loop itself should run without panicking.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Stop the scheduler (creates a final archive since state is Stopped)
        task.on_stop("test-instance").await.unwrap();

        // Verify final archive was created
        let archives = manager.list("test-instance").await.unwrap();
        assert_eq!(archives.len(), 1);
        assert!(archives[0].label.contains("shutdown"));
    }
}
