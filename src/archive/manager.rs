use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::archive::compress;
use crate::archive::metadata::{
    ArchiveMetadata, ArchiveScope, CompressionAlgorithm, METADATA_BYTES, METADATA_FILE,
    data_filename,
};
use crate::archive::strategy::{ArchiveStrategy, archive_path_for};
use crate::archive::{ArchiveError, ArchiveResult};
use crate::core::state::InstanceState;

/// Server-created identity and description for a new archive.
#[derive(Debug)]
pub struct ArchiveRequest<'a> {
    /// Generated archive identifier, available before publication.
    pub id: String,
    /// Instance owning the data.
    pub instance_id: &'a str,
    /// Operator description.
    pub label: &'a str,
    /// Optional game release recorded with the archive.
    pub game_version: Option<&'a str>,
}
impl<'a> ArchiveRequest<'a> {
    /// Allocates an archive identity before filesystem publication.
    #[must_use]
    pub fn new(instance_id: &'a str, label: &'a str) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            instance_id,
            label,
            game_version: None,
        }
    }
}

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
    /// Maximum retained compressed bytes per instance.
    pub max_bytes_per_instance: u64,
    /// Bounds for each archive and its expanded contents.
    pub limits: compress::ArchiveLimits,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("./archives"),
            algorithm: CompressionAlgorithm::Zip,
            default_scope: ArchiveScope::Full,
            max_archives_per_instance: 10,
            auto_cleanup: true,
            max_bytes_per_instance: 100 * 1024 * 1024 * 1024,
            limits: compress::ArchiveLimits::default(),
        }
    }
}

/// Core archive manager for creating, listing, restoring, and deleting archives.
///
/// Each archive is published as one directory containing data and metadata.
/// The caller must hold exclusive ownership of the configured archive store.
#[derive(Debug, Clone)]
pub struct ArchiveManager {
    config: ArchiveConfig,
    gate: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl ArchiveManager {
    /// Creates a new archive manager with the given configuration.
    #[must_use]
    pub fn new(config: ArchiveConfig) -> Self {
        Self {
            config,
            gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
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
        validate_id(instance_id)?;
        let dir = self.instance_archives_dir(instance_id);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut metadata = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ArchiveError::InvalidId("archive filename is not UTF-8".into()))?;
            if name.starts_with(".pending-") || name.starts_with(".deleting-") {
                continue;
            }
            if !entry.file_type().await?.is_dir() {
                return Err(ArchiveError::MetadataParse(
                    "archive store contains an unexpected entry".into(),
                ));
            }
            metadata.push(self.find_metadata(instance_id, &name).await?);
        }
        metadata.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(metadata)
    }

    /// Reads metadata bound to an instance and archive identifier.
    ///
    /// # Errors
    /// Returns an archive error for invalid ownership, missing data, or corrupt metadata.
    pub async fn find_metadata(
        &self,
        instance_id: &str,
        archive_id: &str,
    ) -> ArchiveResult<ArchiveMetadata> {
        validate_id(instance_id)?;
        validate_id(archive_id)?;
        let dir = self.instance_archives_dir(instance_id).join(archive_id);
        let kind = tokio::fs::symlink_metadata(&dir).await?;
        if !kind.is_dir() || kind.is_symlink() {
            return Err(ArchiveError::MetadataParse(
                "archive is not a directory".into(),
            ));
        }
        let meta_path = dir.join(METADATA_FILE);
        let file = tokio::fs::symlink_metadata(&meta_path).await?;
        if !file.is_file() || file.is_symlink() || file.len() > METADATA_BYTES {
            return Err(ArchiveError::MetadataParse(
                "invalid archive metadata file".into(),
            ));
        }
        let metadata: ArchiveMetadata = serde_json::from_slice(&tokio::fs::read(&meta_path).await?)
            .map_err(|error| ArchiveError::MetadataParse(error.to_string()))?;
        if metadata.id != archive_id || metadata.instance_id != instance_id {
            return Err(ArchiveError::MetadataParse(
                "archive ownership differs from its storage location".into(),
            ));
        }
        let file = tokio::fs::symlink_metadata(archive_path_for(
            &self.instance_archives_dir(instance_id),
            &metadata,
        ))
        .await?;
        if !file.is_file() || file.is_symlink() || file.len() != metadata.compressed_size {
            return Err(ArchiveError::MetadataParse(
                "archive data differs from its metadata".into(),
            ));
        }
        Ok(metadata)
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
        request: &ArchiveRequest<'_>,
        instance_dir: &Path,
        state: InstanceState,
        strategy: &dyn ArchiveStrategy,
    ) -> ArchiveResult<ArchiveMetadata> {
        let _guard = self.gate.lock().await;
        let instance_id = request.instance_id;
        let label = request.label;
        let game_version = request.game_version;
        validate_id(instance_id)?;
        validate_id(&request.id)?;
        if self.config.max_bytes_per_instance == 0
            || self.config.limits.bytes == 0
            || self.config.limits.entries == 0
        {
            return Err(ArchiveError::Quota);
        }
        // Instance must be stopped for data consistency
        if state != InstanceState::Stopped {
            return Err(ArchiveError::InstanceNotStopped(state));
        }

        let archives_dir = self.instance_archives_dir(instance_id);
        tokio::fs::create_dir_all(&archives_dir).await?;

        self.remove_pending(instance_id).await?;
        // Generate archive ID (UUID v7 = time-sortable)
        let archive_id = request.id.clone();
        if archives_dir.join(&archive_id).try_exists()? {
            return Err(ArchiveError::InvalidId("archive ID already exists".into()));
        }

        // Select paths to include
        let existing = self.list(instance_id).await?;
        let include_paths = strategy.select_paths(instance_dir, &existing)?;

        // Compute world names from included paths
        let world_names = self.detect_world_names(instance_dir, &include_paths)?;

        // Determine scope from strategy or config
        let scope = self.infer_scope(&include_paths, instance_dir);

        let pending = archives_dir.join(format!(".pending-{archive_id}"));
        tokio::fs::create_dir(&pending).await?;
        let result = async {
            let archive_path = pending.join(data_filename(&self.config.algorithm));

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
                    compress::create_archive_zip(
                        instance_dir,
                        &archive_path,
                        &include_paths,
                        self.config.limits,
                    )
                    .await?
                }
                CompressionAlgorithm::TarGz => {
                    compress::create_archive_tar_gz(
                        instance_dir,
                        &archive_path,
                        &include_paths,
                        self.config.limits,
                    )
                    .await?
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

            if metadata.compressed_size > self.config.max_bytes_per_instance {
                return Err(ArchiveError::Quota);
            }
            let meta_path = pending.join(METADATA_FILE);
            let meta_json = serde_json::to_string_pretty(&metadata)
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            if meta_json.len() as u64 > METADATA_BYTES {
                return Err(ArchiveError::Quota);
            }
            tokio::fs::write(&meta_path, meta_json).await?;
            sync_directory(&pending)?;
            std::fs::File::open(&meta_path)?.sync_all()?;
            tokio::fs::rename(&pending, archives_dir.join(&archive_id)).await?;
            sync_directory(&archives_dir)?;

            if self.config.auto_cleanup {
                self.retain(
                    instance_id,
                    self.config.max_archives_per_instance,
                    Some(&metadata.id),
                )
                .await?;
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
        .await;
        finish_pending(&pending, result).await
    }

    /// Opens immutable archive data for an encrypted transfer.
    ///
    /// # Errors
    /// Returns an archive error if ownership or stored data is invalid.
    pub async fn open_data(
        &self,
        instance_id: &str,
        archive_id: &str,
    ) -> ArchiveResult<std::fs::File> {
        let metadata = self.find_metadata(instance_id, archive_id).await?;
        let path = archive_path_for(&self.instance_archives_dir(instance_id), &metadata);
        compress::open_regular(&path)
    }

    /// Publishes an imported archive and its metadata under the instance quota.
    ///
    /// # Errors
    /// Returns an archive error for an occupied ID, invalid ownership, exceeded quotas, or I/O failure.
    pub async fn import(
        &self,
        metadata: ArchiveMetadata,
        source: &Path,
    ) -> ArchiveResult<ArchiveMetadata> {
        let _guard = self.gate.lock().await;
        validate_id(&metadata.id)?;
        validate_id(&metadata.instance_id)?;
        if metadata.compressed_size > self.config.limits.bytes
            || metadata.compressed_size > self.config.max_bytes_per_instance
            || metadata.original_size > self.config.limits.bytes
            || metadata.file_count > self.config.limits.entries
        {
            return Err(ArchiveError::Quota);
        }
        let root = self.instance_archives_dir(&metadata.instance_id);
        tokio::fs::create_dir_all(&root).await?;
        let final_path = root.join(&metadata.id);
        if final_path.try_exists()? {
            return Err(ArchiveError::InvalidId("archive ID already exists".into()));
        }
        self.remove_pending(&metadata.instance_id).await?;
        let pending = root.join(format!(".pending-{}", metadata.id));
        tokio::fs::create_dir(&pending).await?;
        let result = async {
            let data = pending.join(data_filename(&metadata.algorithm));
            let source = source.to_path_buf();
            let output_path = data.clone();
            let bytes = metadata.compressed_size;
            tokio::task::spawn_blocking(move || -> ArchiveResult<()> {
                use std::io::Read;
                let mut input = compress::open_regular(&source)?;
                if input.metadata()?.len() != bytes {
                    return Err(ArchiveError::MetadataParse(
                        "import size differs from metadata".into(),
                    ));
                }
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output_path)?;
                let copied = std::io::copy(&mut (&mut input).take(bytes), &mut output)?;
                let mut extra = [0u8; 1];
                if copied != bytes || input.read(&mut extra)? != 0 {
                    return Err(ArchiveError::Quota);
                }
                output.sync_all()?;
                Ok(())
            })
            .await
            .map_err(|error| ArchiveError::Io(error.to_string()))??;
            let actual = compress::compute_sha256(&data).await?;
            if actual != metadata.sha256 {
                return Err(ArchiveError::HashMismatch {
                    expected: metadata.sha256.clone(),
                    actual,
                });
            }
            let meta_path = pending.join(METADATA_FILE);
            let content = serde_json::to_vec(&metadata)
                .map_err(|error| ArchiveError::MetadataParse(error.to_string()))?;
            if content.len() as u64 > METADATA_BYTES {
                return Err(ArchiveError::Quota);
            }
            tokio::fs::write(&meta_path, content).await?;
            std::fs::File::open(meta_path)?.sync_all()?;
            sync_directory(&pending)?;
            tokio::fs::rename(&pending, final_path).await?;
            sync_directory(&root)?;
            if self.config.auto_cleanup {
                self.retain(
                    &metadata.instance_id,
                    self.config.max_archives_per_instance,
                    Some(&metadata.id),
                )
                .await?;
            }
            Ok(metadata)
        }
        .await;
        finish_pending(&pending, result).await
    }

    /// Restores an archive to a target directory (typically the instance directory).
    ///
    /// **Warning**: This will overwrite files in the target directory.
    ///
    /// # Errors
    ///
    /// Returns `ArchiveError` if the archive is not found, SHA-256 verification fails,
    /// or extraction fails.
    pub async fn restore(
        &self,
        instance_id: &str,
        archive_id: &str,
        target_dir: &Path,
    ) -> ArchiveResult<()> {
        let metadata = self.find_metadata(instance_id, archive_id).await?;

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
                compress::extract_archive_zip(&archive_path, target_dir, self.config.limits)
                    .await?;
            }
            CompressionAlgorithm::TarGz => {
                compress::extract_archive_tar_gz(&archive_path, target_dir, self.config.limits)
                    .await?;
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
    pub async fn delete(&self, instance_id: &str, archive_id: &str) -> ArchiveResult<()> {
        let _guard = self.gate.lock().await;
        let metadata = self.find_metadata(instance_id, archive_id).await?;
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
    pub async fn verify(&self, instance_id: &str, archive_id: &str) -> ArchiveResult<bool> {
        let metadata = self.find_metadata(instance_id, archive_id).await?;
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
        let _guard = self.gate.lock().await;
        self.retain(instance_id, max_archives, None).await
    }

    async fn retain(
        &self,
        instance_id: &str,
        maximum: usize,
        protected: Option<&str>,
    ) -> ArchiveResult<usize> {
        let all = self.list(instance_id).await?;
        let mut bytes = all.iter().try_fold(0u64, |sum, meta| {
            sum.checked_add(meta.compressed_size)
                .ok_or(ArchiveError::Quota)
        })?;
        let mut count = all.len();
        let mut removed = 0;
        let dir = self.instance_archives_dir(instance_id);
        for metadata in &all {
            if protected == Some(metadata.id.as_str()) {
                continue;
            }
            if (maximum == 0 || count <= maximum) && bytes <= self.config.max_bytes_per_instance {
                break;
            }
            self.remove_files(&dir, metadata).await?;
            bytes -= metadata.compressed_size;
            count -= 1;
            removed += 1;
        }
        Ok(removed)
    }

    async fn remove_pending(&self, instance_id: &str) -> ArchiveResult<()> {
        let mut entries = tokio::fs::read_dir(self.instance_archives_dir(instance_id)).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".pending-") || name.starts_with(".deleting-") {
                if !entry.file_type().await?.is_dir() {
                    return Err(ArchiveError::MetadataParse(
                        "invalid pending archive".into(),
                    ));
                }
                tokio::fs::remove_dir_all(entry.path()).await?;
            }
        }
        Ok(())
    }

    async fn remove_files(&self, archives_dir: &Path, meta: &ArchiveMetadata) -> ArchiveResult<()> {
        validate_id(&meta.id)?;
        let deleting = archives_dir.join(format!(".deleting-{}", meta.id));
        tokio::fs::rename(archives_dir.join(&meta.id), &deleting).await?;
        sync_directory(archives_dir)?;
        tokio::fs::remove_dir_all(deleting).await?;
        Ok(())
    }

    /// Detects world directory names from included paths.
    fn detect_world_names(
        &self,
        instance_dir: &Path,
        include_paths: &[PathBuf],
    ) -> ArchiveResult<Vec<String>> {
        let mut names = Vec::new();
        for path in include_paths {
            if path == Path::new(".") {
                for entry in std::fs::read_dir(instance_dir)? {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() && entry.path().join("level.dat").try_exists()? {
                        names.push(entry.file_name().into_string().map_err(|_| {
                            ArchiveError::MetadataParse("world name is not UTF-8".into())
                        })?);
                    }
                }
            } else if instance_dir.join(path).join("level.dat").try_exists()? {
                names.push(
                    path.to_str()
                        .ok_or_else(|| {
                            ArchiveError::MetadataParse("world name is not UTF-8".into())
                        })?
                        .to_owned(),
                );
            }
        }
        names.sort();
        Ok(names)
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

async fn finish_pending<T>(pending: &Path, result: ArchiveResult<T>) -> ArchiveResult<T> {
    let cleanup = match tokio::fs::remove_dir_all(pending).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArchiveError::from(error)),
    };
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(operation), Err(cleanup)) => Err(ArchiveError::Cleanup {
            operation: Box::new(operation),
            cleanup: Box::new(cleanup),
        }),
    }
}

pub(crate) fn validate_id(value: &str) -> ArchiveResult<()> {
    const ID_CHARACTERS: usize = 64;
    if value.is_empty()
        || value.len() > ID_CHARACTERS
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ArchiveError::InvalidId(value.into()));
    }
    Ok(())
}
fn sync_directory(path: &Path) -> ArchiveResult<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
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
                &ArchiveRequest {
                    game_version: Some("1.20.4"),
                    ..ArchiveRequest::new("test-instance", "first backup")
                },
                &instance_dir,
                InstanceState::Stopped,
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
                &ArchiveRequest {
                    game_version: None,
                    ..ArchiveRequest::new("test", "backup")
                },
                &instance_dir,
                InstanceState::Running,
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
                &ArchiveRequest {
                    game_version: None,
                    ..ArchiveRequest::new("test", "backup")
                },
                &instance_dir,
                InstanceState::Stopped,
                &strategy,
            )
            .await
            .unwrap();

        // Verify
        assert!(manager.verify("test", &meta.id).await.unwrap());

        // Restore to a new directory
        let restore_dir = tmpdir.path().join("restored");
        manager
            .restore("test", &meta.id, &restore_dir)
            .await
            .unwrap();

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
                &ArchiveRequest {
                    game_version: None,
                    ..ArchiveRequest::new("test", "backup")
                },
                &instance_dir,
                InstanceState::Stopped,
                &strategy,
            )
            .await
            .unwrap();

        assert_eq!(manager.list("test").await.unwrap().len(), 1);

        manager.delete("test", &meta.id).await.unwrap();
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
                    &ArchiveRequest {
                        game_version: None,
                        ..ArchiveRequest::new("test", &format!("backup-{i}"))
                    },
                    &instance_dir,
                    InstanceState::Stopped,
                    &strategy,
                )
                .await
                .unwrap();
        }

        let archives = manager.list("test").await.unwrap();
        assert_eq!(archives.len(), 2);
    }
}
