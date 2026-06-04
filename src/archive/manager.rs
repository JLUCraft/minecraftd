use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::archive::compress;
use crate::archive::metadata::{ArchiveMetadata, ArchiveScope, CompressionAlgorithm};
use crate::archive::strategy::{ArchiveStrategy, archive_path_for, meta_path_for};
use crate::archive::{ArchiveError, ArchiveResult};
use crate::core::state::InstanceState;

/// Configuration for the archive manager.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveConfig {
    /// Root directory where all archives are stored.
    pub base_dir: PathBuf,
    /// Compression algorithm to use.
    pub algorithm: CompressionAlgorithm,
    /// What to include in archives by default.
    pub default_scope: ArchiveScope,
    /// Maximum archives per instance (0 = unlimited).
    pub max_archives_per_instance: usize,
    /// Whether to auto-cleanup after each archive.
    pub auto_cleanup: bool,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("./archives"),
            algorithm: CompressionAlgorithm::Zip,
            default_scope: ArchiveScope::Full,
            max_archives_per_instance: 10,
            auto_cleanup: true,
        }
    }
}

/// Core archive manager for creating, listing, restoring, and deleting archives.
///
/// Archives are stored in `{base_dir}/{instance_id}/` as:
/// - `{archive_id}.{ext}` — the compressed data
/// - `{archive_id}.meta.json` — metadata
#[derive(Debug, Clone)]
pub struct ArchiveManager {
    config: ArchiveConfig,
}

impl ArchiveManager {
    /// Creates a new archive manager with the given configuration.
    #[must_use]
    pub const fn new(config: ArchiveConfig) -> Self {
        Self { config }
    }

    /// Returns the archive directory for a specific instance.
    fn instance_archives_dir(&self, instance_id: &str) -> PathBuf {
        self.config.base_dir.join(instance_id)
    }

    /// Lists all archives for an instance, sorted by creation time (oldest first).
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the archive directory cannot be read or metadata is corrupt.
    pub async fn list(&self, instance_id: &str) -> ArchiveResult<Vec<ArchiveMetadata>> {
        let dir = self.instance_archives_dir(instance_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = tokio::fs::read_dir(&dir).await?;
        let mut metadatas: Vec<ArchiveMetadata> = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(content) = tokio::fs::read_to_string(&path).await
            {
                match serde_json::from_str::<ArchiveMetadata>(&content) {
                    Ok(meta) => metadatas.push(meta),
                    Err(e) => {
                        warn!("failed to parse archive metadata {}: {}", path.display(), e);
                    }
                }
            }
        }

        // Sort oldest first by creation time
        metadatas.sort_by_key(|a| a.created_at);

        Ok(metadatas)
    }

    /// Finds archive metadata by ID.
    async fn find_metadata(&self, archive_id: &str) -> ArchiveResult<ArchiveMetadata> {
        // Search all instance subdirectories
        if !self.config.base_dir.exists() {
            return Err(ArchiveError::NotFound(archive_id.to_string()));
        }

        let mut entries = tokio::fs::read_dir(&self.config.base_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let meta_path = entry.path().join(format!("{archive_id}.meta.json"));
                if meta_path.exists() {
                    let content = tokio::fs::read_to_string(&meta_path).await?;
                    return serde_json::from_str(&content)
                        .map_err(|e| ArchiveError::MetadataParse(e.to_string()));
                }
            }
        }

        Err(ArchiveError::NotFound(archive_id.to_string()))
    }

    /// Creates a new archive for the given instance.
    ///
    /// The instance MUST be in `Stopped` state; this is enforced.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the instance is not stopped, path selection fails,
    /// compression fails, or metadata cannot be written.
    pub async fn create(
        &self,
        instance_id: &str,
        instance_dir: &Path,
        label: &str,
        state: InstanceState,
        game_version: Option<&str>,
        strategy: &dyn ArchiveStrategy,
    ) -> ArchiveResult<ArchiveMetadata> {
        // Instance must be stopped for data consistency
        if state != InstanceState::Stopped {
            return Err(ArchiveError::InstanceNotStopped(state));
        }

        let archives_dir = self.instance_archives_dir(instance_id);
        tokio::fs::create_dir_all(&archives_dir).await?;

        // Generate archive ID (UUID v7 = time-sortable)
        let archive_id = uuid::Uuid::now_v7().to_string();

        // Select paths to include
        let existing = self.list(instance_id).await?;
        let include_paths = strategy.select_paths(instance_dir, &existing)?;

        // Compute world names from included paths
        let world_names = self.detect_world_names(instance_dir, &include_paths);

        // Determine scope from strategy or config
        let scope = self.infer_scope(&include_paths, instance_dir);

        let extension = self.config.algorithm.extension();
        let archive_path = archives_dir.join(format!("{archive_id}.{extension}"));

        info!(
            "creating archive {} for instance {} ({} paths, {:?})",
            archive_id,
            instance_id,
            include_paths.len(),
            self.config.algorithm
        );

        // Create the archive
        let stats = match self.config.algorithm {
            CompressionAlgorithm::Zip => {
                compress::create_archive_zip(instance_dir, &archive_path, &include_paths).await?
            }
            CompressionAlgorithm::TarGz => {
                compress::create_archive_tar_gz(instance_dir, &archive_path, &include_paths).await?
            }
        };

        // Compute SHA-256
        let sha256 = compress::compute_sha256(&archive_path).await?;

        // Build and persist metadata
        let metadata = ArchiveMetadata {
            id: archive_id.clone(),
            label: label.to_string(),
            instance_id: instance_id.to_string(),
            created_at: Utc::now(),
            game_version: game_version.map(std::string::ToString::to_string),
            scope,
            algorithm: self.config.algorithm.clone(),
            instance_state: state,
            file_count: stats.file_count,
            original_size: stats.original_size,
            compressed_size: stats.compressed_size,
            sha256,
            world_names,
        };

        let meta_path = meta_path_for(&archives_dir, &metadata);
        let meta_json =
            serde_json::to_string_pretty(&metadata).map_err(|e| ArchiveError::Io(e.to_string()))?;
        tokio::fs::write(&meta_path, meta_json).await?;

        // Auto-cleanup if configured
        if self.config.auto_cleanup && self.config.max_archives_per_instance > 0 {
            let all = self.list(instance_id).await?;
            if all.len() > self.config.max_archives_per_instance {
                let excess = all.len() - self.config.max_archives_per_instance;
                let mut removed: usize = 0;
                for meta in all.iter().take(excess) {
                    self.remove_files(&archives_dir, meta).await?;
                    removed += 1;
                }
                if removed > 0 {
                    info!("auto-cleanup removed {removed} old archives for {instance_id}");
                }
            }
        }

        info!(
            "archive {} created: {} files, {} → {} bytes (ratio: {:.1}%)",
            archive_id,
            stats.file_count,
            stats.original_size,
            stats.compressed_size,
            if stats.original_size > 0 {
                (stats.compressed_size as f64 / stats.original_size as f64) * 100.0
            } else {
                0.0
            }
        );

        Ok(metadata)
    }

    /// Restores an archive to a target directory (typically the instance directory).
    ///
    /// **Warning**: This will overwrite files in the target directory.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the archive is not found, SHA-256 verification fails,
    /// or extraction fails.
    pub async fn restore(&self, archive_id: &str, target_dir: &Path) -> ArchiveResult<()> {
        let metadata = self.find_metadata(archive_id).await?;

        // Locate the archive file
        let archives_dir = self.instance_archives_dir(&metadata.instance_id);
        let archive_path = archive_path_for(&archives_dir, &metadata);

        if !archive_path.exists() {
            return Err(ArchiveError::NotFound(format!(
                "archive file missing: {}",
                archive_path.display()
            )));
        }

        // Verify integrity
        let actual_sha256 = compress::compute_sha256(&archive_path).await?;
        if actual_sha256 != metadata.sha256 {
            return Err(ArchiveError::HashMismatch {
                expected: metadata.sha256.clone(),
                actual: actual_sha256,
            });
        }

        info!(
            "restoring archive {} to {}",
            archive_id,
            target_dir.display()
        );

        // Ensure target directory exists
        tokio::fs::create_dir_all(target_dir).await?;

        // Extract
        match metadata.algorithm {
            CompressionAlgorithm::Zip => {
                compress::extract_archive_zip(&archive_path, target_dir).await?;
            }
            CompressionAlgorithm::TarGz => {
                compress::extract_archive_tar_gz(&archive_path, target_dir).await?;
            }
        }

        info!(
            "archive {} restored successfully to {}",
            archive_id,
            target_dir.display()
        );

        Ok(())
    }

    /// Deletes an archive and its metadata.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the archive is not found.
    pub async fn delete(&self, archive_id: &str) -> ArchiveResult<()> {
        let metadata = self.find_metadata(archive_id).await?;
        let archives_dir = self.instance_archives_dir(&metadata.instance_id);

        self.remove_files(&archives_dir, &metadata).await?;

        info!("deleted archive {archive_id}");
        Ok(())
    }

    /// Verifies an archive's SHA-256 checksum.
    ///
    /// Returns `true` if the checksum matches, `false` otherwise.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the archive is not found or cannot be read.
    pub async fn verify(&self, archive_id: &str) -> ArchiveResult<bool> {
        let metadata = self.find_metadata(archive_id).await?;
        let archives_dir = self.instance_archives_dir(&metadata.instance_id);
        let archive_path = archive_path_for(&archives_dir, &metadata);

        if !archive_path.exists() {
            return Err(ArchiveError::NotFound(format!(
                "archive file missing: {}",
                archive_path.display()
            )));
        }

        let actual_sha256 = compress::compute_sha256(&archive_path).await?;
        Ok(actual_sha256 == metadata.sha256)
    }

    /// Manually cleans up excess archives for an instance.
    ///
    /// Returns the number of archives removed.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` on I/O errors.
    pub async fn cleanup(&self, instance_id: &str, max_archives: usize) -> ArchiveResult<usize> {
        if max_archives == 0 {
            return Ok(0);
        }

        let all = self.list(instance_id).await?;
        if all.len() <= max_archives {
            return Ok(0);
        }

        let archives_dir = self.instance_archives_dir(instance_id);
        let excess = all.len() - max_archives;
        let mut removed: usize = 0;

        for meta in all.iter().take(excess) {
            self.remove_files(&archives_dir, meta).await?;
            removed += 1;
        }

        info!("cleanup removed {removed} archives for {instance_id}");
        Ok(removed)
    }

    /// Removes the archive file and metadata file.
    async fn remove_files(&self, archives_dir: &Path, meta: &ArchiveMetadata) -> ArchiveResult<()> {
        let archive_path = archive_path_for(archives_dir, meta);
        let meta_path = meta_path_for(archives_dir, meta);

        if archive_path.exists() {
            tokio::fs::remove_file(&archive_path).await?;
        }
        if meta_path.exists() {
            tokio::fs::remove_file(&meta_path).await?;
        }

        Ok(())
    }

    /// Detects world directory names from included paths.
    fn detect_world_names(&self, instance_dir: &Path, include_paths: &[PathBuf]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();

        for p in include_paths {
            if p == Path::new(".") {
                // For full archives, scan for world dirs
                // World dirs typically contain level.dat
                if let Ok(entries) = std::fs::read_dir(instance_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir()
                            && path.join("level.dat").exists()
                            && let Some(name) = path.file_name()
                        {
                            names.push(name.to_string_lossy().to_string());
                        }
                    }
                }
            } else {
                // Single world dir
                let full = instance_dir.join(p);
                if full.is_dir()
                    && full.join("level.dat").exists()
                    && let Some(name) = p.file_name()
                {
                    names.push(name.to_string_lossy().to_string());
                }
            }
        }

        names
    }

    /// Infers the archive scope from the selected paths.
    fn infer_scope(&self, include_paths: &[PathBuf], instance_dir: &Path) -> ArchiveScope {
        if include_paths.len() == 1 && include_paths[0] == Path::new(".") {
            return ArchiveScope::Full;
        }

        // Check if all paths look like world dirs (contain level.dat)
        let all_worlds = include_paths.iter().all(|p| {
            let full = instance_dir.join(p);
            full.is_dir() && full.join("level.dat").exists()
        });

        if all_worlds && !include_paths.is_empty() {
            ArchiveScope::Worlds
        } else {
            ArchiveScope::Custom {
                dirs: include_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::metadata::ArchiveScope;
    use crate::archive::strategy::FullArchiveStrategy;

    #[tokio::test]
    async fn test_list_empty() {
        let tmpdir = tempfile::tempdir().unwrap();
        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);

        let archives = manager.list("test-instance").await.unwrap();
        assert!(archives.is_empty());
    }

    #[tokio::test]
    async fn test_create_and_list() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(instance_dir.join("world")).unwrap();
        std::fs::write(instance_dir.join("world/level.dat"), b"level").unwrap();
        std::fs::write(instance_dir.join("server.properties"), b"props").unwrap();

        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);
        let strategy = FullArchiveStrategy::new();

        let meta = manager
            .create(
                "test-instance",
                &instance_dir,
                "first backup",
                InstanceState::Stopped,
                Some("1.20.4"),
                &strategy,
            )
            .await
            .unwrap();

        assert_eq!(meta.label, "first backup");
        assert_eq!(meta.instance_id, "test-instance");
        assert_eq!(meta.game_version, Some("1.20.4".to_string()));
        assert_eq!(meta.scope, ArchiveScope::Full);
        assert_eq!(meta.instance_state, InstanceState::Stopped);
        assert!(meta.file_count > 0);

        // List should find it
        let archives = manager.list("test-instance").await.unwrap();
        assert_eq!(archives.len(), 1);
        assert_eq!(archives[0].id, meta.id);
    }

    #[tokio::test]
    async fn test_create_rejects_running_instance() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();

        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);
        let strategy = FullArchiveStrategy::new();

        let result = manager
            .create(
                "test",
                &instance_dir,
                "backup",
                InstanceState::Running,
                None,
                &strategy,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ArchiveError::InstanceNotStopped(ref s) if *s == InstanceState::Running),
            "expected InstanceNotStopped(Running), got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_restore_and_verify() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(instance_dir.join("world")).unwrap();
        std::fs::write(instance_dir.join("world/level.dat"), b"level data").unwrap();
        std::fs::write(instance_dir.join("server.properties"), b"props").unwrap();

        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);
        let strategy = FullArchiveStrategy::new();

        let meta = manager
            .create(
                "test",
                &instance_dir,
                "backup",
                InstanceState::Stopped,
                None,
                &strategy,
            )
            .await
            .unwrap();

        // Verify
        assert!(manager.verify(&meta.id).await.unwrap());

        // Restore to a new directory
        let restore_dir = tmpdir.path().join("restored");
        manager.restore(&meta.id, &restore_dir).await.unwrap();

        assert!(restore_dir.join("world/level.dat").exists());
        assert!(restore_dir.join("server.properties").exists());
        assert_eq!(
            std::fs::read_to_string(restore_dir.join("world/level.dat")).unwrap(),
            "level data"
        );
    }

    #[tokio::test]
    async fn test_delete() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("data.txt"), b"data").unwrap();

        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);
        let strategy = FullArchiveStrategy::new();

        let meta = manager
            .create(
                "test",
                &instance_dir,
                "backup",
                InstanceState::Stopped,
                None,
                &strategy,
            )
            .await
            .unwrap();

        assert_eq!(manager.list("test").await.unwrap().len(), 1);

        manager.delete(&meta.id).await.unwrap();
        assert_eq!(manager.list("test").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_auto_cleanup() {
        let tmpdir = tempfile::tempdir().unwrap();
        let instance_dir = tmpdir.path().join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("data.txt"), b"data").unwrap();

        let config = ArchiveConfig {
            base_dir: tmpdir.path().join("archives"),
            max_archives_per_instance: 2,
            auto_cleanup: true,
            ..Default::default()
        };
        let manager = ArchiveManager::new(config);
        let strategy = FullArchiveStrategy::new();

        // Create 3 archives, auto-cleanup should remove the oldest
        for i in 0..3 {
            // Write different data so archives aren't identical
            std::fs::write(instance_dir.join("data.txt"), format!("data{i}")).unwrap();

            manager
                .create(
                    "test",
                    &instance_dir,
                    &format!("backup-{i}"),
                    InstanceState::Stopped,
                    None,
                    &strategy,
                )
                .await
                .unwrap();
        }

        let archives = manager.list("test").await.unwrap();
        assert_eq!(archives.len(), 2);
    }
}
