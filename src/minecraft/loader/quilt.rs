use crate::download::mirror::DownloadSource;
use crate::download::task::DownloadTask;
use crate::instance::path::InstanceSubdir;
use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::{DownloadInfo, LibraryDownloads, LibraryInfo, VersionInfo};
use serde::Deserialize;
use std::path::Path;
use tracing::{debug, info};

const META_URL: &str = "https://meta.quiltmc.org/v3/versions/loader";

/// Installer for Quilt mod loader.
#[derive(Debug, Clone, Default)]
pub struct QuiltInstaller;

impl QuiltInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    async fn fetch_loader_meta(
        &self,
        mc_version: &str,
        loader_version: &str,
    ) -> Result<QuiltMeta, LoaderInstallError> {
        let url = format!("{META_URL}/{mc_version}/{loader_version}");
        debug!("fetching Quilt loader meta from {}", url);
        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            return Err(LoaderInstallError::Network(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }
        let meta: QuiltMeta = response.json().await?;
        Ok(meta)
    }

    fn resolve_maven_root(coord: &str) -> &'static str {
        if coord.starts_with("net.fabricmc") {
            "https://maven.fabricmc.net/"
        } else {
            "https://maven.quiltmc.org/repository/release/"
        }
    }
}

#[async_trait::async_trait]
impl ModLoaderInstaller for QuiltInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Quilt
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        info!(
            "installing Quilt {} for Minecraft {}",
            loader_version, mc_version
        );

        let meta = self.fetch_loader_meta(mc_version, loader_version).await?;

        let libraries_dir = InstanceSubdir::Libraries.resolve(game_dir);
        let mut tasks = Vec::new();
        let mut library_infos = Vec::new();

        // Core maven coordinates (loader, intermediary, hashed)
        let core_coords = [
            meta.loader.maven.as_str(),
            meta.intermediary.maven.as_str(),
            meta.hashed.maven.as_str(),
        ];

        for coord in &core_coords {
            library_infos.push(library_info_from_coord(coord));
            if let Some(task) = download_task_for_maven(coord, &libraries_dir) {
                tasks.push(task);
            }
        }

        // launcherMeta libraries
        let sides = ["common", "client", "server", "development"];
        for side in &sides {
            let arr = match *side {
                "common" => meta.launcher_meta.libraries.common.as_ref(),
                "client" => meta.launcher_meta.libraries.client.as_ref(),
                "server" => meta.launcher_meta.libraries.server.as_ref(),
                "development" => meta.launcher_meta.libraries.development.as_ref(),
                _ => None,
            };
            if let Some(libs) = arr {
                for lib in libs {
                    let base_url = lib
                        .url
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| Self::resolve_maven_root(&lib.name).to_string());
                    library_infos.push(library_info_from_coord(&lib.name));
                    if let Some(task) =
                        download_task_for_maven_with_url(&lib.name, &base_url, &libraries_dir)
                    {
                        tasks.push(task);
                    }
                }
            }
        }

        if !tasks.is_empty() {
            let queue = crate::download::queue::DownloadQueue::new(8);
            queue
                .download_all(tasks)
                .await
                .map_err(|e| LoaderInstallError::Network(e.to_string()))?;
        }

        let version_info = VersionInfo {
            id: format!("quilt-loader-{loader_version}-{mc_version}"),
            version_type: "release".into(),
            main_class: Some(meta.launcher_meta.main_class.client),
            libraries: library_infos,
            ..Default::default()
        };

        Ok(version_info)
    }
}

// ============================================================================
// Data models
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
struct QuiltMeta {
    loader: QuiltMavenEntry,
    intermediary: QuiltMavenEntry,
    hashed: QuiltMavenEntry,
    #[serde(rename = "launcherMeta")]
    launcher_meta: QuiltLauncherMeta,
}

#[derive(Debug, Clone, Deserialize)]
struct QuiltMavenEntry {
    maven: String,
}

#[derive(Debug, Clone, Deserialize)]
struct QuiltLauncherMeta {
    #[serde(rename = "mainClass")]
    main_class: QuiltMainClass,
    libraries: QuiltLibraries,
}

#[derive(Debug, Clone, Deserialize)]
struct QuiltMainClass {
    client: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct QuiltLibraries {
    common: Option<Vec<QuiltLibrary>>,
    client: Option<Vec<QuiltLibrary>>,
    server: Option<Vec<QuiltLibrary>>,
    development: Option<Vec<QuiltLibrary>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuiltLibrary {
    name: String,
    url: Option<String>,
}

// ============================================================================
// Helpers
// ============================================================================

fn library_info_from_coord(name: &str) -> LibraryInfo {
    LibraryInfo {
        name: name.to_string(),
        downloads: Some(LibraryDownloads {
            artifact: Some(DownloadInfo {
                url: String::new(),
                sha1: String::new(),
                size: 0,
            }),
            classifiers: None,
        }),
        rules: None,
        natives: None,
        extract: None,
    }
}

fn download_task_for_maven(coord: &str, libraries_dir: &Path) -> Option<DownloadTask> {
    let root = QuiltInstaller::resolve_maven_root(coord);
    download_task_for_maven_with_url(coord, root, libraries_dir)
}

fn download_task_for_maven_with_url(
    coord: &str,
    base_url: &str,
    libraries_dir: &Path,
) -> Option<DownloadTask> {
    let artifact_path = crate::minecraft::library::artifact_path(coord);
    let base = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{}/", base_url)
    };
    let url = format!("{}{}", base, artifact_path.to_string_lossy());
    let path = libraries_dir.join(&artifact_path);
    if path.exists() {
        return None;
    }
    Some(
        DownloadTask::new(url, path)
            .with_sources(vec![DownloadSource::Bmclapi, DownloadSource::Official]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quilt_installer_type() {
        let installer = QuiltInstaller::new();
        assert_eq!(installer.loader_type(), ModLoaderType::Quilt);
    }

    #[test]
    fn test_resolve_maven_root() {
        assert_eq!(
            QuiltInstaller::resolve_maven_root("net.fabricmc:fabric-loader:0.15.6"),
            "https://maven.fabricmc.net/"
        );
        assert_eq!(
            QuiltInstaller::resolve_maven_root("org.quiltmc:quilt-loader:0.23.2"),
            "https://maven.quiltmc.org/repository/release/"
        );
    }

    #[tokio::test]
    async fn test_quilt_fetch_loader_meta() {
        let installer = QuiltInstaller::new();
        // Use a known-valid version pair; skip if network returns 404
        let result = installer.fetch_loader_meta("1.20.1", "0.21.2").await;
        if let Err(LoaderInstallError::Network(ref msg)) = result
            && msg.contains("404")
        {
            eprintln!("SKIP: Quilt loader version not found (404)");
            return;
        }
        assert!(
            result.is_ok(),
            "fetch_loader_meta should succeed with network: {result:?}"
        );
        let meta = result.unwrap();
        assert!(!meta.loader.maven.is_empty());
        assert!(!meta.launcher_meta.main_class.client.is_empty());
    }

    #[tokio::test]
    async fn test_quilt_fetch_invalid_version() {
        let installer = QuiltInstaller::new();
        let result = installer.fetch_loader_meta("0.0.0-invalid", "0.0.0").await;
        assert!(
            result.is_err(),
            "fetch_loader_meta should fail for invalid version"
        );
    }
}
