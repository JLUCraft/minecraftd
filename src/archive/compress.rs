use crate::archive::ArchiveError;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Resource limits shared by archive creation and extraction.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// Maximum input, output, and expanded bytes per archive.
    pub bytes: u64,
    /// Maximum number of entries, including directories.
    pub entries: u64,
}
impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            bytes: 10 * 1024 * 1024 * 1024,
            entries: 100_000,
        }
    }
}

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

struct Budget {
    limits: ArchiveLimits,
    bytes: u64,
    entries: u64,
    files: u64,
}
impl Budget {
    const fn new(limits: ArchiveLimits) -> Self {
        Self {
            limits,
            bytes: 0,
            entries: 0,
            files: 0,
        }
    }
    fn entry(&mut self) -> Result<(), ArchiveError> {
        self.entries = self.entries.checked_add(1).ok_or(ArchiveError::Quota)?;
        if self.entries > self.limits.entries {
            return Err(ArchiveError::Quota);
        }
        Ok(())
    }
    fn copy(&mut self, input: &mut impl Read, output: &mut impl Write) -> Result<(), ArchiveError> {
        let remaining = self.limits.bytes.saturating_sub(self.bytes);
        let written = std::io::copy(&mut input.take(remaining), output)?;
        self.bytes += written;
        let mut extra = [0u8; 1];
        if input.read(&mut extra)? != 0 {
            return Err(ArchiveError::Quota);
        }
        self.files += 1;
        Ok(())
    }
}

struct BoundedFile {
    file: std::fs::File,
    position: u64,
    limit: u64,
}
impl BoundedFile {
    fn create(path: &Path, limit: u64) -> Result<Self, ArchiveError> {
        Ok(Self {
            file: std::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(path)?,
            position: 0,
            limit,
        })
    }
}
impl Write for BoundedFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = u64::try_from(bytes.len()).map_err(std::io::Error::other)?;
        if length > self.limit.saturating_sub(self.position) {
            return Err(std::io::Error::other("archive byte quota exceeded"));
        }
        let written = self.file.write(bytes)?;
        self.position += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for BoundedFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let next = self.file.seek(position)?;
        if next > self.limit {
            return Err(std::io::Error::other("archive byte quota exceeded"));
        }
        self.position = next;
        Ok(next)
    }
}

fn relative_path(path: &Path) -> Result<(), ArchiveError> {
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(ArchiveError::Compression(
            "archive path escapes the instance directory".into(),
        ));
    }
    Ok(())
}

fn sources(
    source: &Path,
    selected: &[PathBuf],
    limits: ArchiveLimits,
) -> Result<Vec<PathBuf>, ArchiveError> {
    if !std::fs::symlink_metadata(source)?.is_dir() {
        return Err(ArchiveError::Compression(
            "archive source is not a directory".into(),
        ));
    }
    let mut paths = std::collections::BTreeSet::new();
    for relative in selected {
        relative_path(relative)?;
        let mut root = source.to_path_buf();
        for component in relative.components() {
            if let Component::Normal(name) = component {
                root.push(name);
                let metadata = std::fs::symlink_metadata(&root)?;
                if metadata.is_symlink() {
                    return Err(ArchiveError::Compression(
                        "archive source contains a link".into(),
                    ));
                }
            }
        }
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = entry.map_err(|error| ArchiveError::Io(error.to_string()))?;
            let kind = entry.file_type();
            if !kind.is_file() && !kind.is_dir() {
                return Err(ArchiveError::Compression(
                    "archive source contains a link or special file".into(),
                ));
            }
            let relative = entry
                .path()
                .strip_prefix(source)
                .map_err(|error| ArchiveError::Io(error.to_string()))?;
            if relative == Path::new("") || relative == Path::new(".") {
                continue;
            }
            relative_path(relative)?;
            paths.insert(relative.to_path_buf());
            if paths.len() as u64 > limits.entries {
                return Err(ArchiveError::Quota);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub(crate) fn open_regular(path: &Path) -> Result<std::fs::File, ArchiveError> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(ArchiveError::Compression(
            "archive source is not a regular file".into(),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ArchiveError::Compression(
            "archive source is not a regular file".into(),
        ));
    }
    Ok(file)
}

/// Creates a bounded ZIP from controlled source paths. Links and special files are rejected.
///
/// # Errors
/// Returns an archive error for invalid paths, exceeded quotas, or I/O failures.
pub async fn create_archive_zip(
    source_dir: &Path,
    target_path: &Path,
    include_paths: &[PathBuf],
    limits: ArchiveLimits,
) -> Result<CompressionStats, ArchiveError> {
    let source = source_dir.to_path_buf();
    let target = target_path.to_path_buf();
    let selected = include_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let paths = sources(&source, &selected, limits)?;
        let mut writer = zip::ZipWriter::new(BoundedFile::create(&target, limits.bytes)?);
        let mut budget = Budget::new(limits);
        for relative in paths {
            budget.entry()?;
            let path = source.join(&relative);
            let name = relative
                .to_str()
                .ok_or_else(|| ArchiveError::Compression("archive path is not UTF-8".into()))?;
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            if std::fs::symlink_metadata(&path)?.is_dir() {
                writer
                    .add_directory(name, options)
                    .map_err(|error| ArchiveError::Compression(error.to_string()))?;
            } else {
                let mut file = open_regular(&path)?;
                writer
                    .start_file(name, options)
                    .map_err(|error| ArchiveError::Compression(error.to_string()))?;
                budget.copy(&mut file, &mut writer)?;
            }
        }
        let output = writer
            .finish()
            .map_err(|error| ArchiveError::Compression(error.to_string()))?;
        output.file.sync_all()?;
        Ok(CompressionStats {
            file_count: budget.files,
            original_size: budget.bytes,
            compressed_size: output.file.metadata()?.len(),
        })
    })
    .await
    .map_err(|error| ArchiveError::Compression(error.to_string()))?
}

/// Creates a bounded tar.gz from controlled source paths.
///
/// # Errors
/// Returns an archive error for links, special files, exceeded quotas, or I/O failures.
pub async fn create_archive_tar_gz(
    source_dir: &Path,
    target_path: &Path,
    include_paths: &[PathBuf],
    limits: ArchiveLimits,
) -> Result<CompressionStats, ArchiveError> {
    let source = source_dir.to_path_buf();
    let target = target_path.to_path_buf();
    let selected = include_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let paths = sources(&source, &selected, limits)?;
        let encoder = flate2::write::GzEncoder::new(
            BoundedFile::create(&target, limits.bytes)?,
            flate2::Compression::default(),
        );
        let mut writer = tar::Builder::new(encoder);
        let mut budget = Budget::new(limits);
        for relative in paths {
            budget.entry()?;
            let path = source.join(&relative);
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                writer.append_dir(&relative, &path)?;
            } else {
                let mut file = open_regular(&path)?;
                let size = file.metadata()?.len();
                if size > limits.bytes.saturating_sub(budget.bytes) {
                    return Err(ArchiveError::Quota);
                }
                let mut header = tar::Header::new_gnu();
                header.set_metadata(&metadata);
                header.set_size(size);
                header.set_cksum();
                writer.append_data(&mut header, &relative, &mut file)?;
                budget.bytes += size;
                budget.files += 1;
            }
        }
        let output = writer.into_inner()?.finish()?;
        output.file.sync_all()?;
        Ok(CompressionStats {
            file_count: budget.files,
            original_size: budget.bytes,
            compressed_size: output.file.metadata()?.len(),
        })
    })
    .await
    .map_err(|error| ArchiveError::Compression(error.to_string()))?
}

fn destination(root: &Path, relative: &Path, directory: bool) -> Result<PathBuf, ArchiveError> {
    relative_path(relative)?;
    ensure_restore_directory(root)?;
    let mut path = root.to_path_buf();
    let components: Vec<_> = relative
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .collect();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        if directory || index + 1 < components.len() {
            ensure_restore_directory(&path)?;
        } else {
            reject_restore_symlink(&path)?;
        }
    }
    Ok(path)
}
fn is_restore_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn reject_restore_symlink(path: &Path) -> Result<(), ArchiveError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_restore_link(&metadata) => Err(ArchiveError::Compression(
            "restore destination contains a link".into(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn ensure_restore_directory(path: &Path) -> Result<(), ArchiveError> {
    reject_restore_symlink(path)?;
    std::fs::create_dir_all(path)?;
    reject_restore_symlink(path)
}
fn output_file(path: &Path) -> Result<std::fs::File, ArchiveError> {
    reject_restore_symlink(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let output = options.open(path)?;
    if !output.metadata()?.is_file() {
        return Err(ArchiveError::Compression(
            "restore output is not a regular file".into(),
        ));
    }
    output.set_len(0)?;
    Ok(output)
}

/// Extracts a bounded ZIP into a directory held exclusively by the caller.
///
/// # Errors
/// Returns an archive error for invalid entries, links, exceeded quotas, or I/O failures.
pub async fn extract_archive_zip(
    archive_path: &Path,
    target_dir: &Path,
    limits: ArchiveLimits,
) -> Result<(), ArchiveError> {
    let source = archive_path.to_path_buf();
    let target = target_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut archive = zip::ZipArchive::new(open_regular(&source)?)
            .map_err(|error| ArchiveError::Compression(error.to_string()))?;
        let mut budget = Budget::new(limits);
        ensure_restore_directory(&target)?;
        for index in 0..archive.len() {
            budget.entry()?;
            let mut entry = archive
                .by_index(index)
                .map_err(|error| ArchiveError::Compression(error.to_string()))?;
            const FILE_TYPE_MASK: u32 = 0o170000;
            const REGULAR_FILE: u32 = 0o100000;
            const DIRECTORY: u32 = 0o040000;
            let special = entry
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & FILE_TYPE_MASK, 0 | REGULAR_FILE | DIRECTORY));
            if entry.is_symlink() || special {
                return Err(ArchiveError::Compression("archive contains a link".into()));
            }
            let relative = entry.enclosed_name().ok_or_else(|| {
                ArchiveError::Compression("archive path escapes the target".into())
            })?;
            let path = destination(&target, &relative, entry.is_dir())?;
            if !entry.is_dir() {
                let mut output = output_file(&path)?;
                budget.copy(&mut entry, &mut output)?;
                output.sync_all()?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| ArchiveError::Compression(error.to_string()))?
}

/// Extracts bounded regular files and directories from a tar.gz archive.
///
/// # Errors
/// Returns an archive error for invalid paths, links, special files, exceeded quotas, or I/O failures.
pub async fn extract_archive_tar_gz(
    archive_path: &Path,
    target_dir: &Path,
    limits: ArchiveLimits,
) -> Result<(), ArchiveError> {
    let source = archive_path.to_path_buf();
    let target = target_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let decoder = flate2::read::GzDecoder::new(open_regular(&source)?);
        let mut archive = tar::Archive::new(decoder);
        let mut budget = Budget::new(limits);
        ensure_restore_directory(&target)?;
        let mut long_name: Option<PathBuf> = None;
        let mut directory_modes = Vec::new();
        const PATH_BYTES: u64 = 4096;
        for entry in archive.entries()?.raw(true) {
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            if kind.is_gnu_longname() {
                if long_name.is_some() || entry.size() > PATH_BYTES {
                    return Err(ArchiveError::Quota);
                }
                let mut name = Vec::new();
                entry.read_to_end(&mut name)?;
                if name.last() == Some(&0) {
                    name.pop();
                }
                let name = String::from_utf8(name)
                    .map_err(|_| ArchiveError::Compression("archive path is not UTF-8".into()))?;
                long_name = Some(PathBuf::from(name));
                continue;
            }
            budget.entry()?;
            if !kind.is_file() && !kind.is_dir() {
                return Err(ArchiveError::Compression(
                    "archive contains a link or special file".into(),
                ));
            }
            let relative = match long_name.take() {
                Some(path) => path,
                None => entry.path()?.into_owned(),
            };
            let path = destination(&target, &relative, kind.is_dir())?;
            let mode = entry.header().mode()?;
            if kind.is_file() {
                let mut output = output_file(&path)?;
                budget.copy(&mut entry, &mut output)?;
                restore_mode(&path, mode)?;
                output.sync_all()?;
            } else {
                directory_modes.push((path, mode));
            }
        }
        if long_name.is_some() {
            return Err(ArchiveError::Compression(
                "archive ends in an incomplete path entry".into(),
            ));
        }
        for (path, mode) in directory_modes.into_iter().rev() {
            restore_mode(&path, mode)?;
        }
        // Finish the gzip stream so truncated input is reported.
        let mut decoder = archive.into_inner();
        const TRAILING_BYTES: u64 = 64 * 1024;
        let mut tail = Budget::new(ArchiveLimits {
            bytes: TRAILING_BYTES,
            entries: 1,
        });
        tail.copy(&mut decoder, &mut std::io::sink())?;
        Ok(())
    })
    .await
    .map_err(|error| ArchiveError::Compression(error.to_string()))?
}

fn restore_mode(path: &Path, mode: u32) -> Result<(), ArchiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
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

        let stats = create_archive_zip(&source, &archive_path, &include, ArchiveLimits::default())
            .await
            .unwrap();

        assert_eq!(stats.file_count, 3);
        assert!(stats.compressed_size > 0);

        // Verify SHA256
        let hash = compute_sha256(&archive_path).await.unwrap();
        assert!(!hash.is_empty());

        // Extract and verify
        let extract_dir = tmpdir.path().join("restored");
        extract_archive_zip(&archive_path, &extract_dir, ArchiveLimits::default())
            .await
            .unwrap();

        assert!(extract_dir.join("world/level.dat").exists());
        assert!(extract_dir.join("world/subdir/region.mca").exists());
        assert!(extract_dir.join("server.properties").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn zip_restore_rejects_existing_destination_links()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        for link_kind in ["directory", "file", "dangling", "root"] {
            let temp = tempfile::tempdir()?;
            let source = temp.path().join("source");
            std::fs::create_dir_all(source.join("world"))?;
            std::fs::write(source.join("world/level.dat"), b"replacement")?;
            let archive = temp.path().join("backup.zip");
            create_archive_zip(
                &source,
                &archive,
                &[PathBuf::from("world")],
                ArchiveLimits::default(),
            )
            .await?;
            let outside = temp.path().join("outside");
            std::fs::create_dir_all(outside.join("world"))?;
            std::fs::write(outside.join("level.dat"), b"external data")?;
            std::fs::write(outside.join("world/level.dat"), b"external world")?;
            let target = temp.path().join("restored");
            if link_kind == "root" {
                symlink(&outside, &target)?;
            } else {
                std::fs::create_dir_all(&target)?;
                if link_kind == "directory" {
                    symlink(&outside, target.join("world"))?;
                } else {
                    std::fs::create_dir_all(target.join("world"))?;
                    let destination = if link_kind == "dangling" {
                        outside.join("missing.dat")
                    } else {
                        outside.join("level.dat")
                    };
                    symlink(destination, target.join("world/level.dat"))?;
                }
            }
            assert!(
                extract_archive_zip(&archive, &target, ArchiveLimits::default())
                    .await
                    .is_err(),
                "{link_kind}"
            );
            assert_eq!(std::fs::read(outside.join("level.dat"))?, b"external data");
            assert_eq!(
                std::fs::read(outside.join("world/level.dat"))?,
                b"external world"
            );
            assert!(!outside.join("missing.dat").exists());
        }
        Ok(())
    }

    #[tokio::test]
    async fn zip_restore_overwrites_regular_files() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("world"))?;
        std::fs::write(source.join("world/level.dat"), b"new")?;
        let archive = temp.path().join("backup.zip");
        create_archive_zip(
            &source,
            &archive,
            &[PathBuf::from("world")],
            ArchiveLimits::default(),
        )
        .await?;
        let target = temp.path().join("restored");
        std::fs::create_dir_all(target.join("world"))?;
        std::fs::write(target.join("world/level.dat"), b"previous longer contents")?;
        extract_archive_zip(&archive, &target, ArchiveLimits::default()).await?;
        assert_eq!(std::fs::read(target.join("world/level.dat"))?, b"new");
        Ok(())
    }

    #[tokio::test]
    async fn test_tar_gz_roundtrip() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = tmpdir.path().join("source");
        std::fs::create_dir_all(source.join("world/data")).unwrap();
        std::fs::write(source.join("world/data/file.txt"), b"hello world").unwrap();
        std::fs::write(source.join("config.yml"), b"config: true").unwrap();
        let long_directory = PathBuf::from("world").join("long-directory-".repeat(12));
        std::fs::create_dir_all(source.join(&long_directory)).unwrap();
        std::fs::write(
            source.join(&long_directory).join("long-path.txt"),
            b"long path",
        )
        .unwrap();

        let archive_path = tmpdir.path().join("backup.tar.gz");
        let include: Vec<PathBuf> = vec![PathBuf::from("world"), PathBuf::from("config.yml")];

        let stats =
            create_archive_tar_gz(&source, &archive_path, &include, ArchiveLimits::default())
                .await
                .unwrap();

        assert_eq!(stats.file_count, 3);
        assert!(stats.compressed_size > 0);

        let extract_dir = tmpdir.path().join("restored");
        extract_archive_tar_gz(&archive_path, &extract_dir, ArchiveLimits::default())
            .await
            .unwrap();

        assert!(extract_dir.join("world/data/file.txt").exists());
        assert!(extract_dir.join("config.yml").exists());
        assert_eq!(
            std::fs::read(extract_dir.join(long_directory).join("long-path.txt")).unwrap(),
            b"long path"
        );
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
    async fn missing_selected_paths_fail_archive_creation() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let archive = directory.path().join("backup.zip");
        assert!(
            create_archive_zip(
                directory.path(),
                &archive,
                &[PathBuf::from("missing")],
                ArchiveLimits::default()
            )
            .await
            .is_err()
        );
        assert!(!archive.try_exists()?);
        Ok(())
    }

    #[tokio::test]
    async fn test_extract_invalid_zip() {
        let tmpdir = tempfile::tempdir().unwrap();
        let bad_zip = tmpdir.path().join("not-a-zip.zip");
        std::fs::write(&bad_zip, b"this is not a valid zip file").unwrap();

        let out = tmpdir.path().join("out");
        let result = extract_archive_zip(&bad_zip, &out, ArchiveLimits::default()).await;
        assert!(result.is_err(), "extracting invalid zip should fail");
    }

    #[tokio::test]
    async fn test_extract_nonexistent_archive() {
        let tmpdir = tempfile::tempdir().unwrap();
        let missing = tmpdir.path().join("missing.zip");

        let out = tmpdir.path().join("out");
        let result = extract_archive_zip(&missing, &out, ArchiveLimits::default()).await;
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

        let stats = create_archive_zip(&source, &archive_path, &include, ArchiveLimits::default())
            .await
            .unwrap();
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.original_size, 0);
    }
}
