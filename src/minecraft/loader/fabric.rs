use crate::download::task::DownloadTask;
use crate::instance::path::InstanceSubdir;
use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::{LibraryInfo, VersionInfo};
use serde::Deserialize;
use std::path::Path;
use tracing::{debug, info};

const META_URL: &str = "https://meta.fabricmc.net/v2/versions/loader";

/// Installer for Fabric mod loader.
#[derive(Debug, Clone, Default)]
pub struct FabricInstaller;

impl FabricInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Fetches the Fabric loader version list for a Minecraft version.
    #[cfg(test)]
    async fn fetch_loader_versions(
        &self,
        mc_version: &str,
    ) -> Result<Vec<FabricLoaderVersion>, LoaderInstallError> {
        let url = format!("{META_URL}/{mc_version}");
        debug!("fetching Fabric versions from {}", url);
        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            return Err(LoaderInstallError::Network(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }
        let versions: Vec<FabricLoaderVersion> = response.json().await?;
        Ok(versions)
    }

    /// Fetches the launch profile (version JSON) for a specific loader version.
    async fn fetch_launch_profile(
        &self,
        mc_version: &str,
        loader_version: &str,
    ) -> Result<FabricLaunchProfile, LoaderInstallError> {
        let url = format!("{META_URL}/{mc_version}/{loader_version}/profile/json");
        debug!("fetching Fabric launch profile from {}", url);
        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            return Err(LoaderInstallError::Network(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }
        let profile: FabricLaunchProfile = response.json().await?;
        Ok(profile)
    }
}

#[async_trait::async_trait]
impl ModLoaderInstaller for FabricInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Fabric
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        info!(
            "installing Fabric {} for Minecraft {}",
            loader_version, mc_version
        );

        let profile = self
            .fetch_launch_profile(mc_version, loader_version)
            .await?;

        // Download libraries (Fabric uses a different format than Mojang)
        let libraries_dir = InstanceSubdir::Libraries.resolve(game_dir);
        let mut tasks = Vec::new();
        for lib in &profile.libraries {
            if let Some(task) = lib.to_download_task(&libraries_dir) {
                tasks.push(task);
            }
        }

        if !tasks.is_empty() {
            let queue = crate::download::queue::DownloadQueue::new(8);
            queue
                .download_all(tasks)
                .await
                .map_err(|e| LoaderInstallError::Network(e.to_string()))?;
        }

        // Convert profile to VersionInfo
        let version_info = profile.into_version_info(mc_version);
        Ok(version_info)
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Deserialize)]
struct FabricLoaderVersion {
    #[serde(rename = "loader")]
    loader: FabricLoaderMeta,
}
#[cfg(test)]
#[derive(Debug, Clone, Deserialize)]
struct FabricLoaderMeta {
    version: String,
}

/// Fabric's library format differs from Mojang's: it has `url`, `sha1`, `size`
/// at the top level instead of nested under `downloads.artifact`.
#[derive(Debug, Clone, Deserialize)]
struct FabricLibrary {
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    sha1: String,
    #[serde(default)]
    size: i64,
}

impl FabricLibrary {
    /// Builds the full Maven URL for this library.
    fn maven_url(&self) -> Option<String> {
        if self.url.is_empty() {
            return None;
        }
        let artifact_path = crate::minecraft::library::artifact_path(&self.name);
        let base = if self.url.ends_with('/') {
            self.url.clone()
        } else {
            format!("{}/", self.url)
        };
        Some(format!("{}{}", base, artifact_path.to_string_lossy()))
    }

    /// Converts to a `DownloadTask` for downloading this library.
    fn to_download_task(&self, libraries_dir: &Path) -> Option<DownloadTask> {
        let url = self.maven_url()?;
        let path = libraries_dir.join(crate::minecraft::library::artifact_path(&self.name));
        if path.exists() {
            return None;
        }
        let mut task = DownloadTask::new(url, path);
        if !self.sha1.is_empty() {
            task = task.with_sha1(self.sha1.clone());
        }
        Some(task)
    }

    /// Converts to a `LibraryInfo` for use in VersionInfo/classpath generation.
    fn to_library_info(&self) -> LibraryInfo {
        use crate::minecraft::version::{DownloadInfo, LibraryDownloads};
        let artifact = if self.url.is_empty() {
            None
        } else {
            Some(DownloadInfo {
                url: self.url.clone(),
                sha1: if self.sha1.is_empty() {
                    String::new()
                } else {
                    self.sha1.clone()
                },
                size: if self.size > 0 { self.size as u64 } else { 0 },
            })
        };
        LibraryInfo {
            name: self.name.clone(),
            downloads: Some(LibraryDownloads {
                artifact,
                classifiers: None,
            }),
            rules: None,
            natives: None,
            extract: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct FabricLaunchProfile {
    id: String,
    #[serde(default)]
    libraries: Vec<FabricLibrary>,
    #[serde(rename = "mainClass")]
    main_class: String,
}

impl FabricLaunchProfile {
    fn into_version_info(self, mc_version: &str) -> VersionInfo {
        VersionInfo {
            id: format!("fabric-loader-{}-{}", mc_version, self.id),
            version_type: "release".into(),
            main_class: Some(self.main_class),
            libraries: self
                .libraries
                .into_iter()
                .map(|l| l.to_library_info())
                .collect(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fabric_fetch_loader_versions() {
        let installer = FabricInstaller::new();
        let result = installer.fetch_loader_versions("1.20.4").await;
        assert!(
            result.is_ok(),
            "fetch_loader_versions should succeed with network: {result:?}"
        );
        let versions = result.unwrap();
        assert!(!versions.is_empty());
        // Verify deserialization: fields in FabricLoaderVersion and FabricLoaderMeta are populated
        assert!(!versions[0].loader.version.is_empty());
    }

    #[tokio::test]
    async fn test_fabric_fetch_launch_profile() {
        let installer = FabricInstaller::new();
        let result = installer.fetch_launch_profile("1.20.4", "0.15.6").await;
        assert!(
            result.is_ok(),
            "fetch_launch_profile should succeed with network: {result:?}"
        );
        assert!(!result.unwrap().main_class.is_empty());
    }

    #[tokio::test]
    async fn test_fabric_fetch_invalid_version() {
        let installer = FabricInstaller::new();
        let result = installer.fetch_loader_versions("0.0.0-invalid").await;
        assert!(
            result.is_err(),
            "fetch_loader_versions should fail for invalid version"
        );
    }

    #[test]
    fn test_fabric_installer_type() {
        let installer = FabricInstaller::new();
        assert_eq!(installer.loader_type(), ModLoaderType::Fabric);
    }
}
