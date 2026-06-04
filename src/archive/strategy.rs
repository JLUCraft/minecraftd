use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use crate::archive::ArchiveError;
use crate::archive::ArchiveResult;
use crate::archive::metadata::ArchiveMetadata;

/// Strategy for selecting files and triggering archive operations.
///
/// Different strategies can implement different policies:
/// - Full: always archives everything
/// - Incremental: only changed files since last archive
/// - Timed: archives on a schedule
#[async_trait]
pub trait ArchiveStrategy: Send + Sync + Debug {
    /// Human-readable name.
    fn name(&self) -> &str;

    /// Select which paths (relative to instance dir) to include in the archive.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the instance directory cannot be read.
    fn select_paths(
        &self,
        instance_dir: &Path,
        existing_archives: &[ArchiveMetadata],
    ) -> ArchiveResult<Vec<PathBuf>>;

    /// Returns `true` if an archive should be created now.
    async fn should_archive(&self, _instance_id: &str) -> bool {
        true
    }

    /// Cleanup old archives. Returns the number removed.
    ///
    /// Called after a successful archive creation.
    async fn cleanup(
        &self,
        instance_id: &str,
        archives: &[ArchiveMetadata],
        archives_dir: &Path,
    ) -> ArchiveResult<usize> {
        let _ = (instance_id, archives, archives_dir);
        Ok(0)
    }
}

/// Archives the entire instance directory (all files).
#[derive(Debug, Clone, Default)]
pub struct FullArchiveStrategy;

impl FullArchiveStrategy {
    /// Creates a new full archive strategy.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ArchiveStrategy for FullArchiveStrategy {
    fn name(&self) -> &str {
        "full"
    }

    fn select_paths(
        &self,
        instance_dir: &Path,
        _existing_archives: &[ArchiveMetadata],
    ) -> ArchiveResult<Vec<PathBuf>> {
        // Archive the entire directory by selecting "."
        if !instance_dir.exists() {
            return Err(ArchiveError::Io(format!(
                "instance directory does not exist: {}",
                instance_dir.display()
            )));
        }
        Ok(vec![PathBuf::from(".")])
    }
}

/// Archives only specific world directories (e.g. `world`, `world_nether`, `world_the_end`).
#[derive(Debug, Clone)]
pub struct WorldsArchiveStrategy {
    /// World directory names to include.
    pub world_names: Vec<String>,
}

impl WorldsArchiveStrategy {
    /// Creates a strategy that archives the given world directories.
    #[must_use]
    pub const fn new(world_names: Vec<String>) -> Self {
        Self { world_names }
    }
}

#[async_trait]
impl ArchiveStrategy for WorldsArchiveStrategy {
    fn name(&self) -> &str {
        "worlds"
    }

    fn select_paths(
        &self,
        instance_dir: &Path,
        _existing_archives: &[ArchiveMetadata],
    ) -> ArchiveResult<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for world_name in &self.world_names {
            let world_path = instance_dir.join(world_name);
            if world_path.exists() {
                paths.push(PathBuf::from(world_name));
            } else {
                debug!(
                    "world directory '{}' not found in {}, skipping",
                    world_name,
                    instance_dir.display()
                );
            }
        }
        if paths.is_empty() {
            return Err(ArchiveError::Io(format!(
                "no world directories found in {}",
                instance_dir.display()
            )));
        }
        Ok(paths)
    }
}

/// Archives user-specified subdirectories.
#[derive(Debug, Clone)]
pub struct CustomPathsStrategy {
    /// Relative paths to include.
    pub paths: Vec<String>,
}

impl CustomPathsStrategy {
    /// Creates a strategy that archives the given paths.
    #[must_use]
    pub const fn new(paths: Vec<String>) -> Self {
        Self { paths }
    }
}

#[async_trait]
impl ArchiveStrategy for CustomPathsStrategy {
    fn name(&self) -> &str {
        "custom"
    }

    fn select_paths(
        &self,
        instance_dir: &Path,
        _existing_archives: &[ArchiveMetadata],
    ) -> ArchiveResult<Vec<PathBuf>> {
        let mut result: Vec<PathBuf> = Vec::new();
        for p in &self.paths {
            if instance_dir.join(p).exists() {
                result.push(PathBuf::from(p));
            } else {
                debug!("path '{}' not found, skipping", p);
            }
        }
        if result.is_empty() {
            return Err(ArchiveError::Io(format!(
                "none of the specified paths exist in {}",
                instance_dir.display()
            )));
        }
        Ok(result)
    }
}

/// Maximum retention strategy: keeps only the most recent N archives.
///
/// Removes oldest archives first when the count exceeds the limit.
#[derive(Debug, Clone)]
pub struct MaxRetentionCleanup {
    /// Maximum number of archives to retain (per instance).
    pub max_archives: usize,
}

impl MaxRetentionCleanup {
    /// Creates a new retention policy.
    #[must_use]
    pub const fn new(max_archives: usize) -> Self {
        Self { max_archives }
    }

    /// Removes excess archives. Archives are expected to be sorted oldest-first.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if file deletion fails.
    pub async fn cleanup(
        &self,
        archives: &[ArchiveMetadata],
        archives_dir: &Path,
    ) -> ArchiveResult<usize> {
        if archives.len() <= self.max_archives {
            return Ok(0);
        }

        let excess = archives.len() - self.max_archives;
        let mut removed: usize = 0;

        // Archives are sorted oldest-first
        for meta in archives.iter().take(excess) {
            let archive_path = archive_path_for(archives_dir, meta);
            let meta_path = meta_path_for(archives_dir, meta);

            debug!("removing old archive: {}", meta.id);
            if archive_path.exists() {
                tokio::fs::remove_file(&archive_path).await?;
            }
            if meta_path.exists() {
                tokio::fs::remove_file(&meta_path).await?;
            }
            removed += 1;
        }

        Ok(removed)
    }
}

/// Timed archive strategy: triggers based on elapsed time since last archive.
///
/// Wraps another strategy for path selection and delegates cleanup.
#[derive(Debug)]
pub struct TimedArchiveStrategy {
    /// Underlying strategy for file selection.
    pub inner: Box<dyn ArchiveStrategy>,
    /// Minimum interval between archives in seconds.
    pub interval_secs: u64,
    /// Maximum archives to retain.
    pub max_archives: usize,
    /// Timestamp of the last archive (Unix epoch seconds).
    last_archive_at: Arc<Mutex<u64>>,
}

impl TimedArchiveStrategy {
    /// Creates a new timed strategy.
    #[must_use]
    pub fn new(inner: Box<dyn ArchiveStrategy>, interval_secs: u64, max_archives: usize) -> Self {
        Self {
            inner,
            interval_secs,
            max_archives,
            last_archive_at: Arc::new(Mutex::new(0)),
        }
    }

    /// Mark that an archive was just created, resetting the timer.
    pub async fn mark_archived(&self) {
        let now = current_unix_secs();
        let mut last = self.last_archive_at.lock().await;
        *last = now;
    }
}

#[async_trait]
impl ArchiveStrategy for TimedArchiveStrategy {
    fn name(&self) -> &str {
        "timed"
    }

    fn select_paths(
        &self,
        instance_dir: &Path,
        existing_archives: &[ArchiveMetadata],
    ) -> ArchiveResult<Vec<PathBuf>> {
        self.inner.select_paths(instance_dir, existing_archives)
    }

    async fn should_archive(&self, _instance_id: &str) -> bool {
        let last = *self.last_archive_at.lock().await;
        let now = current_unix_secs();
        now.saturating_sub(last) >= self.interval_secs
    }

    async fn cleanup(
        &self,
        instance_id: &str,
        archives: &[ArchiveMetadata],
        archives_dir: &Path,
    ) -> ArchiveResult<usize> {
        let _ = instance_id;
        if self.max_archives == 0 {
            return Ok(0);
        }
        MaxRetentionCleanup::new(self.max_archives)
            .cleanup(archives, archives_dir)
            .await
    }
}

/// Returns the file path for an archive.
#[must_use]
pub fn archive_path_for(archives_dir: &Path, meta: &ArchiveMetadata) -> PathBuf {
    let extension = meta.algorithm.extension();
    archives_dir.join(format!("{}.{}", meta.id, extension))
}

/// Returns the metadata file path for an archive.
#[must_use]
pub fn meta_path_for(archives_dir: &Path, meta: &ArchiveMetadata) -> PathBuf {
    archives_dir.join(format!("{}.meta.json", meta.id))
}

/// Returns the current Unix timestamp in seconds.
fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_strategy_selects_root() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();

        let strategy = FullArchiveStrategy::new();
        let paths = strategy.select_paths(&instance_dir, &[]).unwrap();
        assert_eq!(paths, vec![PathBuf::from(".")]);
    }

    #[test]
    fn test_worlds_strategy() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(instance_dir.join("world")).unwrap();
        std::fs::create_dir_all(instance_dir.join("world_nether")).unwrap();

        let strategy = WorldsArchiveStrategy::new(vec![
            "world".to_string(),
            "world_nether".to_string(),
            "world_the_end".to_string(),
        ]);
        let paths = strategy.select_paths(&instance_dir, &[]).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&PathBuf::from("world")));
        assert!(paths.contains(&PathBuf::from("world_nether")));
    }

    #[test]
    fn test_worlds_strategy_no_worlds() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();

        let strategy = WorldsArchiveStrategy::new(vec!["world".to_string()]);
        let result = strategy.select_paths(&instance_dir, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_strategy() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(instance_dir.join("config")).unwrap();
        std::fs::write(instance_dir.join("config/app.toml"), b"").unwrap();
        std::fs::create_dir_all(instance_dir.join("mods")).unwrap();

        let strategy = CustomPathsStrategy::new(vec!["config".to_string(), "mods".to_string()]);
        let paths = strategy.select_paths(&instance_dir, &[]).unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[tokio::test]
    async fn test_max_retention_cleanup() {
        use crate::archive::metadata::{ArchiveMetadata, ArchiveScope, CompressionAlgorithm};
        use crate::core::state::InstanceState;
        use chrono::Utc;

        let tmpdir = tempfile::tempdir().unwrap();
        let archives_dir = tmpdir.path().join("archives");
        std::fs::create_dir_all(&archives_dir).unwrap();

        let mut metas: Vec<ArchiveMetadata> = Vec::new();
        for i in 0..5 {
            let id = format!("archive-{i}");
            let path = archives_dir.join(format!("{id}.zip"));
            std::fs::write(&path, b"dummy").unwrap();
            let meta_path = archives_dir.join(format!("{id}.meta.json"));
            std::fs::write(&meta_path, b"{}").unwrap();

            metas.push(ArchiveMetadata {
                id,
                label: format!("backup-{i}"),
                instance_id: "test".to_string(),
                created_at: Utc::now(),
                game_version: None,
                scope: ArchiveScope::Full,
                algorithm: CompressionAlgorithm::Zip,
                instance_state: InstanceState::Stopped,
                file_count: 1,
                original_size: 100,
                compressed_size: 50,
                sha256: "abc".to_string(),
                world_names: vec![],
            });
        }

        let cleanup = MaxRetentionCleanup::new(3);
        let removed = cleanup.cleanup(&metas, &archives_dir).await.unwrap();
        assert_eq!(removed, 2);
    }
}
