use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::{debug, trace};

use crate::archive::ArchiveError;

/// Statistics gathered during archive creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressionStats {
    /// Number of files included.
    pub file_count: u64,
    /// Total original size in bytes.
    pub original_size: u64,
    /// Compressed output size in bytes.
    pub compressed_size: u64,
}

/// Creates a compressed archive from selected paths within a source directory.
///
/// `include_paths` are relative to `source_dir`. Directories are walked recursively.
///
/// # Errors
///
/// Returns `ArchiveError` on I/O failures or compression errors.
pub async fn create_archive_zip(
    source_dir: &Path,
    target_path: &Path,
    include_paths: &[PathBuf],
) -> Result<CompressionStats, ArchiveError> {
    let source_dir = source_dir.to_path_buf();
    let target_path = target_path.to_path_buf();
    let include_paths = include_paths.to_vec();

    tokio::task::spawn_blocking(move || {
        create_archive_zip_sync(&source_dir, &target_path, &include_paths)
    })
    .await
    .map_err(|e| ArchiveError::Compression(format!("spawn_blocking failed: {e}")))?
}

fn create_archive_zip_sync(
    source_dir: &Path,
    target_path: &Path,
    include_paths: &[PathBuf],
) -> Result<CompressionStats, ArchiveError> {
    let file = std::fs::File::create(target_path)?;
    let mut zip_writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut file_count: u64 = 0;
    let mut original_size: u64 = 0;

    for rel_path in include_paths {
        let full_path = source_dir.join(rel_path);
        if full_path.is_dir() {
            for entry in walkdir::WalkDir::new(&full_path) {
                let entry = entry.map_err(|e| ArchiveError::Io(e.to_string()))?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let relative = path
                    .strip_prefix(source_dir)
                    .map_err(|e| ArchiveError::Io(e.to_string()))?;
                let content = std::fs::read(path)?;
                original_size += content.len() as u64;
                file_count += 1;
                trace!("adding to zip: {}", relative.display());
                zip_writer
                    .start_file(relative.to_string_lossy(), options)
                    .map_err(|e| ArchiveError::Compression(e.to_string()))?;
                zip_writer
                    .write_all(&content)
                    .map_err(|e| ArchiveError::Compression(e.to_string()))?;
            }
        } else if full_path.is_file() {
            let content = std::fs::read(&full_path)?;
            original_size += content.len() as u64;
            file_count += 1;
            trace!("adding to zip: {}", rel_path.display());
            zip_writer
                .start_file(rel_path.to_string_lossy(), options)
                .map_err(|e| ArchiveError::Compression(e.to_string()))?;
            zip_writer
                .write_all(&content)
                .map_err(|e| ArchiveError::Compression(e.to_string()))?;
        } else {
            debug!("skipping non-existent path: {}", full_path.display());
        }
    }

    let finished = zip_writer
        .finish()
        .map_err(|e| ArchiveError::Compression(e.to_string()))?;
    let compressed_size = finished
        .metadata()
        .map_err(|e| ArchiveError::Io(e.to_string()))?
        .len();

    Ok(CompressionStats {
        file_count,
        original_size,
        compressed_size,
    })
}

/// Creates a tar.gz archive from selected paths within a source directory.
///
/// # Errors
///
/// Returns `ArchiveError` on I/O failures or compression errors.
pub async fn create_archive_tar_gz(
    source_dir: &Path,
    target_path: &Path,
    include_paths: &[PathBuf],
) -> Result<CompressionStats, ArchiveError> {
    let source_dir = source_dir.to_path_buf();
    let target_path = target_path.to_path_buf();
    let include_paths = include_paths.to_vec();

    tokio::task::spawn_blocking(move || {
        create_archive_tar_gz_sync(&source_dir, &target_path, &include_paths)
    })
    .await
    .map_err(|e| ArchiveError::Compression(format!("spawn_blocking failed: {e}")))?
}

fn create_archive_tar_gz_sync(
    source_dir: &Path,
    target_path: &Path,
    include_paths: &[PathBuf],
) -> Result<CompressionStats, ArchiveError> {
    let file = std::fs::File::create(target_path)?;
    let gz_encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar_builder = tar::Builder::new(gz_encoder);

    let mut file_count: u64 = 0;
    let mut original_size: u64 = 0;

    for rel_path in include_paths {
        let full_path = source_dir.join(rel_path);
        if full_path.is_dir() {
            for entry in walkdir::WalkDir::new(&full_path) {
                let entry = entry.map_err(|e| ArchiveError::Io(e.to_string()))?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(source_dir)
                    .map_err(|e| ArchiveError::Io(e.to_string()))?;

                if path.is_file() {
                    let meta = std::fs::metadata(path)?;
                    original_size += meta.len();
                    file_count += 1;
                    trace!("adding to tar: {}", relative.display());
                    tar_builder
                        .append_path_with_name(path, relative)
                        .map_err(|e| ArchiveError::Compression(e.to_string()))?;
                } else if path.is_dir() {
                    tar_builder
                        .append_dir(relative, path)
                        .map_err(|e| ArchiveError::Compression(e.to_string()))?;
                }
            }
        } else if full_path.is_file() {
            let meta = std::fs::metadata(&full_path)?;
            original_size += meta.len();
            file_count += 1;
            trace!("adding to tar: {}", rel_path.display());
            tar_builder
                .append_path_with_name(&full_path, rel_path)
                .map_err(|e| ArchiveError::Compression(e.to_string()))?;
        } else {
            debug!("skipping non-existent path: {}", full_path.display());
        }
    }

    let gz_encoder = tar_builder
        .into_inner()
        .map_err(|e| ArchiveError::Compression(e.to_string()))?;
    let file = gz_encoder
        .finish()
        .map_err(|e| ArchiveError::Compression(e.to_string()))?;
    let compressed_size = file
        .metadata()
        .map_err(|e| ArchiveError::Io(e.to_string()))?
        .len();

    Ok(CompressionStats {
        file_count,
        original_size,
        compressed_size,
    })
}

/// Extracts a zip archive to a target directory.
///
/// # Errors
///
/// Returns `ArchiveError` if the archive is invalid or I/O errors occur.
pub async fn extract_archive_zip(
    archive_path: &Path,
    target_dir: &Path,
) -> Result<(), ArchiveError> {
    let archive_path = archive_path.to_path_buf();
    let target_dir = target_dir.to_path_buf();

    tokio::task::spawn_blocking(move || extract_archive_zip_sync(&archive_path, &target_dir))
        .await
        .map_err(|e| ArchiveError::Compression(format!("spawn_blocking failed: {e}")))?
}

fn extract_archive_zip_sync(archive_path: &Path, target_dir: &Path) -> Result<(), ArchiveError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| ArchiveError::Compression(e.to_string()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ArchiveError::Compression(e.to_string()))?;
        let out_path = target_dir.join(entry.name());

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outfile = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut outfile)?;
        }
    }

    Ok(())
}

/// Extracts a tar.gz archive to a target directory.
///
/// # Errors
///
/// Returns `ArchiveError` if the archive is invalid or I/O errors occur.
pub async fn extract_archive_tar_gz(
    archive_path: &Path,
    target_dir: &Path,
) -> Result<(), ArchiveError> {
    let archive_path = archive_path.to_path_buf();
    let target_dir = target_dir.to_path_buf();

    tokio::task::spawn_blocking(move || extract_archive_tar_gz_sync(&archive_path, &target_dir))
        .await
        .map_err(|e| ArchiveError::Compression(format!("spawn_blocking failed: {e}")))?
}

fn extract_archive_tar_gz_sync(archive_path: &Path, target_dir: &Path) -> Result<(), ArchiveError> {
    let file = std::fs::File::open(archive_path)?;
    let gz_decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz_decoder);

    archive
        .unpack(target_dir)
        .map_err(|e| ArchiveError::Compression(e.to_string()))?;

    Ok(())
}

/// Computes the SHA-256 hash of a file.
///
/// # Errors
///
/// Returns `ArchiveError` on I/O failures.
pub async fn compute_sha256(path: &Path) -> Result<String, ArchiveError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || compute_sha256_sync(&path))
        .await
        .map_err(|e| ArchiveError::Compression(format!("spawn_blocking failed: {e}")))?
}

fn compute_sha256_sync(path: &Path) -> Result<String, ArchiveError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        use std::io::Read;
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        Digest::update(&mut hasher, &buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_zip_roundtrip() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = tmpdir.path().join("source");
        std::fs::create_dir_all(source.join("world")).unwrap();
        std::fs::create_dir_all(source.join("world/subdir")).unwrap();
        std::fs::write(source.join("world/level.dat"), b"level data").unwrap();
        std::fs::write(source.join("world/subdir/region.mca"), b"region data").unwrap();
        std::fs::write(source.join("server.properties"), b"server props").unwrap();

        let archive_path = tmpdir.path().join("backup.zip");
        let include: Vec<PathBuf> =
            vec![PathBuf::from("world"), PathBuf::from("server.properties")];

        let stats = create_archive_zip(&source, &archive_path, &include)
            .await
            .unwrap();

        assert_eq!(stats.file_count, 3);
        assert!(stats.compressed_size > 0);

        // Verify SHA256
        let hash = compute_sha256(&archive_path).await.unwrap();
        assert!(!hash.is_empty());

        // Extract and verify
        let extract_dir = tmpdir.path().join("restored");
        extract_archive_zip(&archive_path, &extract_dir)
            .await
            .unwrap();

        assert!(extract_dir.join("world/level.dat").exists());
        assert!(extract_dir.join("world/subdir/region.mca").exists());
        assert!(extract_dir.join("server.properties").exists());
    }

    #[tokio::test]
    async fn test_tar_gz_roundtrip() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = tmpdir.path().join("source");
        std::fs::create_dir_all(source.join("world/data")).unwrap();
        std::fs::write(source.join("world/data/file.txt"), b"hello world").unwrap();
        std::fs::write(source.join("config.yml"), b"config: true").unwrap();

        let archive_path = tmpdir.path().join("backup.tar.gz");
        let include: Vec<PathBuf> = vec![PathBuf::from("world"), PathBuf::from("config.yml")];

        let stats = create_archive_tar_gz(&source, &archive_path, &include)
            .await
            .unwrap();

        assert_eq!(stats.file_count, 2);
        assert!(stats.compressed_size > 0);

        let extract_dir = tmpdir.path().join("restored");
        extract_archive_tar_gz(&archive_path, &extract_dir)
            .await
            .unwrap();

        assert!(extract_dir.join("world/data/file.txt").exists());
        assert!(extract_dir.join("config.yml").exists());
    }

    #[tokio::test]
    async fn test_compute_sha256() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("test.bin");
        std::fs::write(&path, b"test content for sha256").unwrap();

        let hash = compute_sha256(&path).await.unwrap();
        assert_eq!(hash.len(), 64); // SHA-256 is 64 hex chars
        // Same content should produce same hash
        let hash2 = compute_sha256(&path).await.unwrap();
        assert_eq!(hash, hash2);
    }

    #[tokio::test]
    async fn test_skip_non_existent_paths() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = tmpdir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("exists.txt"), b"data").unwrap();

        let archive_path = tmpdir.path().join("backup.zip");
        let include: Vec<PathBuf> =
            vec![PathBuf::from("exists.txt"), PathBuf::from("does-not-exist")];

        let stats = create_archive_zip(&source, &archive_path, &include)
            .await
            .unwrap();
        assert_eq!(stats.file_count, 1);
    }

    #[tokio::test]
    async fn test_extract_invalid_zip() {
        let tmpdir = tempfile::tempdir().unwrap();
        let bad_zip = tmpdir.path().join("not-a-zip.zip");
        std::fs::write(&bad_zip, b"this is not a valid zip file").unwrap();

        let out = tmpdir.path().join("out");
        let result = extract_archive_zip(&bad_zip, &out).await;
        assert!(result.is_err(), "extracting invalid zip should fail");
    }

    #[tokio::test]
    async fn test_extract_nonexistent_archive() {
        let tmpdir = tempfile::tempdir().unwrap();
        let missing = tmpdir.path().join("missing.zip");

        let out = tmpdir.path().join("out");
        let result = extract_archive_zip(&missing, &out).await;
        assert!(
            result.is_err(),
            "extracting nonexistent archive should fail"
        );
    }

    #[tokio::test]
    async fn test_compute_sha256_missing_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let missing = tmpdir.path().join("missing.bin");

        let result = compute_sha256(&missing).await;
        assert!(result.is_err(), "sha256 of missing file should fail");
    }

    #[tokio::test]
    async fn test_create_archive_empty_include() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = tmpdir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();

        let archive_path = tmpdir.path().join("empty.zip");
        let include: Vec<PathBuf> = vec![];

        let stats = create_archive_zip(&source, &archive_path, &include)
            .await
            .unwrap();
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.original_size, 0);
    }
}
