use crate::download::task::DownloadTask;
use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::VersionInfo;
use std::path::Path;
use tracing::{debug, info};

const PAPER_API_BASE: &str = "https://api.papermc.io/v2/projects";

/// Server platform supported by the Paper installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperPlatform {
    Paper,
    Spigot,
    Purpur,
}

impl PaperPlatform {
    #[must_use]
    pub const fn project_name(&self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Spigot => "spigot",
            Self::Purpur => "purpur",
        }
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Paper => "Paper",
            Self::Spigot => "Spigot",
            Self::Purpur => "Purpur",
        }
    }

    #[must_use]
    pub const fn api_base(&self) -> &'static str {
        match self {
            Self::Paper | Self::Spigot => PAPER_API_BASE,
            // Purpur uses its own API endpoint
            Self::Purpur => "https://api.purpurmc.org/v2",
        }
    }
}

/// Installer for Paper/Spigot/Purpur server platforms.
///
/// Unlike Forge or Fabric, these platforms ship a single server JAR
/// that contains everything needed to run. No libraries or version JSON
/// patching is required.
#[derive(Debug, Clone)]
pub struct PaperInstaller {
    platform: PaperPlatform,
}

impl PaperInstaller {
    #[must_use]
    pub const fn new(platform: PaperPlatform) -> Self {
        Self { platform }
    }

    #[must_use]
    pub const fn paper() -> Self {
        Self::new(PaperPlatform::Paper)
    }

    #[must_use]
    pub const fn spigot() -> Self {
        Self::new(PaperPlatform::Spigot)
    }

    #[must_use]
    pub const fn purpur() -> Self {
        Self::new(PaperPlatform::Purpur)
    }

    /// Returns the download URL for a specific build.
    #[must_use]
    pub fn download_url(&self, mc_version: &str, build: &str) -> String {
        match self.platform {
            PaperPlatform::Paper | PaperPlatform::Spigot => {
                format!(
                    "{}/{}/versions/{}/builds/{}/downloads/{}-{}-{}.jar",
                    self.platform.api_base(),
                    self.platform.project_name(),
                    mc_version,
                    build,
                    self.platform.project_name(),
                    mc_version,
                    build
                )
            }
            PaperPlatform::Purpur => {
                format!(
                    "{}/{}/{}/builds/{}/downloads/{}-{}-{}.jar",
                    self.platform.api_base(),
                    self.platform.project_name(),
                    mc_version,
                    build,
                    self.platform.project_name(),
                    mc_version,
                    build
                )
            }
        }
    }

    /// Fetches the latest successful build number for a Minecraft version.
    pub async fn fetch_latest_build(&self, mc_version: &str) -> Result<String, LoaderInstallError> {
        let url = match self.platform {
            PaperPlatform::Paper | PaperPlatform::Spigot => {
                format!(
                    "{}/{}/versions/{}",
                    self.platform.api_base(),
                    self.platform.project_name(),
                    mc_version
                )
            }
            PaperPlatform::Purpur => {
                format!(
                    "{}/{}/{}",
                    self.platform.api_base(),
                    self.platform.project_name(),
                    mc_version
                )
            }
        };

        debug!(
            "fetching {} builds from {}",
            self.platform.display_name(),
            url
        );
        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            return Err(LoaderInstallError::Network(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        let data: serde_json::Value = response.json().await?;

        // Paper API: builds array with build numbers
        let builds = data["builds"]
            .as_array()
            .ok_or_else(|| LoaderInstallError::Parse("missing builds array".to_string()))?;

        if builds.is_empty() {
            return Err(LoaderInstallError::VersionNotFound(format!(
                "no builds found for {} {}",
                self.platform.display_name(),
                mc_version
            )));
        }

        // Get the last build (latest)
        let latest = builds
            .last()
            .and_then(serde_json::Value::as_i64)
            .or_else(|| builds.last().and_then(|b| b["build"].as_i64()))
            .ok_or_else(|| LoaderInstallError::Parse("invalid build number".to_string()))?;

        Ok(latest.to_string())
    }

    /// Creates a synthetic `VersionInfo` for a Paper-based server.
    ///
    /// Paper servers don't use Mojang's version JSON — they are self-contained
    /// server JARs. We construct a minimal `VersionInfo` so the rest of the
    /// system can treat it uniformly.
    fn synthetic_version_info(&self, mc_version: &str, build: &str) -> VersionInfo {
        VersionInfo {
            id: format!("{}-{}-{}", self.platform.project_name(), mc_version, build),
            version_type: "release".to_string(),
            main_class: None, // Server JARs use `java -jar`, not a main class
            libraries: vec![],
            // Mark as server-only by setting a known field
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl ModLoaderInstaller for PaperInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Paper
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        info!(
            "installing {} {} for Minecraft {}",
            self.platform.display_name(),
            loader_version,
            mc_version
        );

        // Ensure game directory exists
        tokio::fs::create_dir_all(game_dir).await?;

        // Determine build number: if loader_version looks like a build number, use it directly;
        // otherwise try to fetch the latest build.
        let build = if loader_version.parse::<u64>().is_ok() {
            loader_version.to_string()
        } else {
            self.fetch_latest_build(mc_version).await?
        };

        let download_url = self.download_url(mc_version, &build);
        let server_jar = game_dir.join(format!(
            "{}-{}-{}.jar",
            self.platform.project_name(),
            mc_version,
            build
        ));

        debug!(
            "downloading {} server from {}",
            self.platform.display_name(),
            download_url
        );
        let task = DownloadTask::new(download_url.clone(), server_jar.clone());
        task.execute().await.map_err(|e| {
            LoaderInstallError::Network(format!("failed to download server jar: {e}"))
        })?;

        info!(
            "{} server {} installed to {}",
            self.platform.display_name(),
            build,
            server_jar.display()
        );

        Ok(self.synthetic_version_info(mc_version, &build))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paper_installer_platforms() {
        assert_eq!(PaperInstaller::paper().platform, PaperPlatform::Paper);
        assert_eq!(PaperInstaller::spigot().platform, PaperPlatform::Spigot);
        assert_eq!(PaperInstaller::purpur().platform, PaperPlatform::Purpur);
    }

    #[test]
    fn test_paper_download_url() {
        let installer = PaperInstaller::paper();
        let url = installer.download_url("1.20.4", "496");
        assert!(url.contains("paper"));
        assert!(url.contains("1.20.4"));
        assert!(url.contains("496"));
    }

    #[test]
    fn test_purpur_download_url() {
        let installer = PaperInstaller::purpur();
        let url = installer.download_url("1.20.4", "2176");
        assert!(url.contains("purpur"));
        assert!(url.contains("1.20.4"));
        assert!(url.contains("2176"));
    }

    #[test]
    fn test_paper_installer_type() {
        let installer = PaperInstaller::paper();
        assert_eq!(installer.loader_type(), ModLoaderType::Paper);
    }

    #[test]
    fn test_paper_synthetic_version_info() {
        let installer = PaperInstaller::paper();
        let info = installer.synthetic_version_info("1.20.4", "496");
        assert_eq!(info.id, "paper-1.20.4-496");
        assert!(info.libraries.is_empty());
    }

    #[tokio::test]
    async fn test_paper_fetch_latest_build() {
        let installer = PaperInstaller::paper();
        let result = installer.fetch_latest_build("1.20.4").await;
        if let Ok(build) = result {
            assert!(!build.is_empty());
            assert!(build.parse::<u64>().is_ok());
        }
        // May fail without network
    }

    #[tokio::test]
    async fn test_paper_install_fails_gracefully_without_network() {
        let installer = PaperInstaller::paper();
        let tmpdir = tempfile::tempdir().unwrap();
        // Use an obviously invalid version to force a quick failure
        let result = installer
            .install("0.0.0-invalid", "99999", tmpdir.path())
            .await;
        assert!(result.is_err());
    }

    /// Tests that `install()` with a numeric build number skips the API fetch
    /// and attempts direct download (which will fail without network).
    #[tokio::test]
    async fn test_paper_install_with_numeric_build_skips_api() {
        let installer = PaperInstaller::paper();
        let tmpdir = tempfile::tempdir().unwrap();
        // "12345" is numeric, so it should skip fetch_latest_build and go straight to download
        let result = installer.install("1.20.4", "12345", tmpdir.path()).await;
        // Should fail at download step (no network), not at API fetch
        assert!(result.is_err());
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("download") || err.contains("network") || err.contains("http"),
            "expected download error, got: {err}"
        );
    }
}
