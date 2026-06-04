pub mod curseforge;
pub mod modrinth;

use crate::download::task::DownloadTask;
use crate::minecraft::loaders::ModLoaderType;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during modpack operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ModpackError {
    #[error("io error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unknown modpack format")]
    UnknownFormat,
    #[error("network error: {0}")]
    Network(String),
    #[error("api error: {0}")]
    Api(String),
}

impl From<std::io::Error> for ModpackError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for ModpackError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

impl From<crate::resource::ResourceError> for ModpackError {
    fn from(e: crate::resource::ResourceError) -> Self {
        Self::Api(e.to_string())
    }
}

/// Recognized modpack formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModpackFormat {
    CurseForge,
    Modrinth,
    MultiMc,
}

/// Core trait for modpack manifests.
#[async_trait::async_trait]
pub trait ModpackManifest: Send + Sync {
    /// Parses the manifest from a zip archive.
    ///
    /// # Errors
    ///
    /// Returns `ModpackError` if the archive is unreadable or the manifest is invalid.
    fn from_archive(path: &Path) -> Result<Self, ModpackError>
    where
        Self: Sized;

    /// Returns the Minecraft client version required by this modpack.
    fn client_version(&self) -> Result<String, ModpackError>;

    /// Returns the mod loader type and version, if specified.
    fn mod_loader(&self) -> Result<Option<(ModLoaderType, String)>, ModpackError>;

    /// Generates download tasks for all files listed in the manifest.
    ///
    /// `instance_path` is the root directory where files should be saved.
    ///
    /// # Errors
    ///
    /// Returns `ModpackError` on network or API failures.
    async fn download_tasks(&self, instance_path: &Path)
    -> Result<Vec<DownloadTask>, ModpackError>;

    /// Returns the overrides directory prefix inside the archive.
    fn overrides_path(&self) -> &str;
}

/// Detects the modpack format by inspecting the zip archive contents.
///
/// # Errors
///
/// Returns `ModpackError::UnknownFormat` if no recognized manifest is found.
pub fn detect_format(path: &Path) -> Result<ModpackFormat, ModpackError> {
    let file = std::fs::File::open(path)?;
    let archive = zip::ZipArchive::new(file)
        .map_err(|e| ModpackError::Parse(format!("invalid zip archive: {e}")))?;

    for name in archive.file_names() {
        if name == "manifest.json" {
            return Ok(ModpackFormat::CurseForge);
        }
        if name == "modrinth.index.json" {
            return Ok(ModpackFormat::Modrinth);
        }
        if name == "instance.cfg" {
            return Ok(ModpackFormat::MultiMc);
        }
    }

    Err(ModpackError::UnknownFormat)
}

/// Extracts the overrides directory from a modpack archive into an instance directory.
///
/// # Errors
///
/// Returns `ModpackError` on I/O failures.
pub fn extract_overrides(
    archive_path: &Path,
    instance_path: &Path,
    overrides_prefix: &str,
) -> Result<(), ModpackError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ModpackError::Parse(format!("invalid zip archive: {e}")))?;

    let prefix = format!("{}/", overrides_prefix.trim_end_matches('/'));

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ModpackError::Io(e.to_string()))?;
        let path = entry.mangled_name();

        if let Ok(rel) = path.strip_prefix(&prefix) {
            let out = instance_path.join(rel);
            if entry.is_file() {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out_file = std::fs::File::create(&out)?;
                std::io::copy(&mut entry, &mut out_file)
                    .map_err(|e| ModpackError::Io(e.to_string()))?;
            }
        }
    }

    Ok(())
}

/// Blacklisted file/directory names for modpack export.
///
/// Mirrored from SJMCL's export blacklist.
static EXPORT_BLACKLIST: &[&str] = &[
    ".DS_Store",
    "desktop.ini",
    "Thumbs.db",
    "usernamecache.json",
    "usercache.json",
    "jars",
    "logs",
    "versions",
    "assets",
    "libraries",
    "crash-reports",
    "NVIDIA",
    "AMD",
    "screenshots",
    "natives",
    "native",
    "$native",
    "$natives",
    "server-resource-packs",
    "command_history.txt",
    "launcher_profiles.json",
    "launcher.pack.lzma",
    "launcher_accounts.json",
    "launcher_cef_log.txt",
    "launcher_log.txt",
    "launcher_msa_credentials.bin",
    "launcher_settings.json",
    "launcher_ui_state.json",
    "realms_persistence.json",
    "webcache2",
    "treatment_tags.json",
    "clientId.txt",
    "PCL.ini",
    "backup",
    "pack.json",
    "launcher.jar",
    "cache",
    "modpack.cfg",
    "log4j2.xml",
    "hmclversion.cfg",
    "install_profile.json",
    "sjmclcfg.json",
    "manifest.json",
    "minecraftinstance.json",
    ".curseclient",
    "modrinth.index.json",
    ".fabric",
    ".mixin.out",
    ".optifine",
    "downloads",
    "essential",
    "asm",
    "backups",
    "TCNodeTracker",
    "CustomDISkins",
    "data",
    "CustomSkinLoader/caches",
    "debug",
    ".replay_cache",
    "replay_recordings",
    "replay_videos",
    "irisUpdateInfo.json",
    "modernfix",
    "modtranslations",
    "schematics",
    "journeymap/data",
    "mods/.connector",
];

/// Collects candidate files for modpack export from an instance directory.
///
/// Filters out blacklisted directories and files.
pub fn collect_export_files(instance_path: &Path) -> Result<Vec<PathBuf>, ModpackError> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(instance_path)
        .into_iter()
        .filter_entry(|e| {
            let rel = e
                .path()
                .strip_prefix(instance_path)
                .unwrap_or_else(|_| e.path());
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            !EXPORT_BLACKLIST.contains(&rel_str.as_str())
                && !rel_str.ends_with(".log")
                && !rel_str.contains("-natives")
                && !rel_str.starts_with("._")
        })
    {
        let entry = entry.map_err(|e| ModpackError::Io(e.to_string()))?;
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }
    Ok(files)
}

/// Creates a modpack zip archive from an export bundle.
///
/// `overrides_prefix` is the directory name inside the zip (e.g. `"overrides"`).
/// `extra_files` are additional files such as generated manifests.
///
/// # Errors
///
/// Returns `ModpackError` on I/O failures.
pub async fn create_modpack_zip(
    save_path: &Path,
    source_dir: &Path,
    overrides_prefix: &str,
    selected_files: &[(String, PathBuf)],
    extra_files: &[(&str, String)],
) -> Result<(), ModpackError> {
    let save_path = save_path.to_path_buf();
    let _source_dir = source_dir.to_path_buf();
    let overrides_prefix = overrides_prefix.to_string();
    let selected_files = selected_files.to_vec();
    let extra_files: Vec<(String, String)> = extra_files
        .iter()
        .map(|(name, content)| (name.to_string(), content.clone()))
        .collect();

    tokio::task::spawn_blocking(move || {
        let output = std::fs::File::create(&save_path)
            .map_err(|e| ModpackError::Io(format!("failed to create zip file: {e}")))?;
        let output = std::io::BufWriter::with_capacity(1024 * 1024, output);
        let mut writer = zip::ZipWriter::new(output);
        let deflate_options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let store_options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, content) in extra_files {
            writer
                .start_file(name, deflate_options)
                .map_err(|e| ModpackError::Io(format!("failed to create zip entry: {e}")))?;
            writer
                .write_all(content.as_bytes())
                .map_err(|e| ModpackError::Io(format!("failed to write extra file to zip: {e}")))?;
        }

        for (rel, full) in selected_files {
            let entry_path = format!("{}/{}", overrides_prefix, rel);
            let options = if should_store(&entry_path) {
                store_options
            } else {
                deflate_options
            };
            writer
                .start_file(entry_path, options)
                .map_err(|e| ModpackError::Io(format!("failed to create zip entry: {e}")))?;

            let file = std::fs::File::open(&full).map_err(|e| {
                ModpackError::Io(format!("failed to open file {}: {e}", full.display()))
            })?;
            let mut file = std::io::BufReader::with_capacity(1024 * 1024, file);
            std::io::copy(&mut file, &mut writer).map_err(|e| {
                ModpackError::Io(format!(
                    "failed to copy file {} to zip: {e}",
                    full.display()
                ))
            })?;
        }

        writer
            .finish()
            .map_err(|e| ModpackError::Io(format!("failed to finalize zip file: {e}")))?;
        Ok(())
    })
    .await
    .map_err(|e| ModpackError::Io(format!("spawn_blocking failed: {e}")))?
}

fn should_store(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "jar"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "zip"
            | "gz"
            | "xz"
            | "ogg"
            | "mp3"
            | "mp4"
            | "nbt"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_curseforge() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("pack.zip");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(b"{}").unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(detect_format(&path).unwrap(), ModpackFormat::CurseForge);
    }

    #[test]
    fn test_detect_format_modrinth() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("pack.zip");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("modrinth.index.json", options).unwrap();
            zip.write_all(b"{}").unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(detect_format(&path).unwrap(), ModpackFormat::Modrinth);
    }

    #[test]
    fn test_extract_overrides() {
        let tmpdir = tempfile::tempdir().unwrap();
        let archive = tmpdir.path().join("pack.zip");
        let out = tmpdir.path().join("instance");

        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("overrides/config/test.cfg", options)
                .unwrap();
            zip.write_all(b"key=value").unwrap();
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(b"{}").unwrap();
            zip.finish().unwrap();
        }

        extract_overrides(&archive, &out, "overrides").unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("config/test.cfg")).unwrap(),
            "key=value"
        );
    }

    #[test]
    fn test_should_store() {
        assert!(should_store("mods/test.jar"));
        assert!(!should_store("config/test.toml"));
    }
}
