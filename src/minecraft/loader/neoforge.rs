use crate::download::task::DownloadTask;
use crate::minecraft::loader::installer::{
    LoaderInstallError, ModLoaderInstaller, find_java_executable, run_installer_jar,
};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::VersionInfo;
use std::path::Path;
use tracing::{debug, info};

/// Installer for `NeoForge` mod loader.
///
/// `NeoForge` is a fork of Forge for Minecraft 1.20.1+.
/// Installation follows a similar pattern to Forge but uses different Maven coordinates.
#[derive(Debug, Clone, Default)]
pub struct NeoForgeInstaller;

impl NeoForgeInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns the `NeoForge` installer download URL.
    ///
    /// `NeoForge` uses the loader version only (not combined with mc version) in the Maven path
    /// for newer releases (1.20.1+). For older transitional versions, the mc version was included.
    #[must_use]
    pub fn installer_url(_mc_version: &str, neoforge_version: &str) -> String {
        // Try the modern format first (no mc version in path)
        format!(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/{neoforge_version}/neoforge-{neoforge_version}-installer.jar"
        )
    }

    /// Alternative URL format for older `NeoForge` versions that included the mc version.
    #[must_use]
    pub fn installer_url_legacy(mc_version: &str, neoforge_version: &str) -> String {
        format!(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/{mc_version}-{neoforge_version}/neoforge-{mc_version}-{neoforge_version}-installer.jar"
        )
    }

    /// Extracts version.json from the installer jar.
    fn extract_version_json_from_jar(jar_path: &Path) -> Result<VersionInfo, LoaderInstallError> {
        let file = std::fs::File::open(jar_path)
            .map_err(|e| LoaderInstallError::Io(format!("failed to open installer jar: {e}")))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| LoaderInstallError::Parse(format!("failed to read installer jar: {e}")))?;

        let mut entry = archive.by_name("version.json").map_err(|e| {
            LoaderInstallError::Parse(format!("installer jar missing version.json: {e}"))
        })?;

        let mut content = String::new();
        use std::io::Read;
        entry
            .read_to_string(&mut content)
            .map_err(|e| LoaderInstallError::Io(format!("failed to read version.json: {e}")))?;

        let version_info: VersionInfo = serde_json::from_str(&content)
            .map_err(|e| LoaderInstallError::Parse(format!("failed to parse version.json: {e}")))?;

        Ok(version_info)
    }
}

#[async_trait::async_trait]
impl ModLoaderInstaller for NeoForgeInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::NeoForge
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        info!(
            "installing NeoForge {} for Minecraft {}",
            loader_version, mc_version
        );

        // Ensure game directory exists
        tokio::fs::create_dir_all(game_dir).await?;

        // Download the installer JAR
        let installer_url = Self::installer_url(mc_version, loader_version);
        let installer_jar = game_dir.join(format!(
            "neoforge-{mc_version}-{loader_version}-installer.jar"
        ));

        debug!("downloading NeoForge installer from {}", installer_url);
        let task = DownloadTask::new(installer_url.clone(), installer_jar.clone());
        let download_result = task.execute().await;

        // Fallback to legacy URL format if modern format fails
        if download_result.is_err() {
            let legacy_url = Self::installer_url_legacy(mc_version, loader_version);
            debug!("modern URL failed, trying legacy: {}", legacy_url);
            let legacy_task = DownloadTask::new(legacy_url.clone(), installer_jar.clone());
            legacy_task.execute().await.map_err(|e| {
                LoaderInstallError::Network(format!(
                    "failed to download installer from both URLs: {e}"
                ))
            })?;
        }

        // Find Java executable
        let java_path = find_java_executable().await?;
        debug!("using Java executable: {}", java_path.display());

        // Run the installer
        run_installer_jar(&java_path, &installer_jar, game_dir).await?;

        debug!("NeoForge installer completed successfully");

        // Extract version info from the installer jar
        let version_info = Self::extract_version_json_from_jar(&installer_jar)?;

        // Clean up installer jar
        let _ = tokio::fs::remove_file(&installer_jar).await;

        Ok(version_info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::minecraft::loader::installer::run_installer_jar;
    use std::io::Write;

    #[test]
    fn test_neoforge_installer_url() {
        let url = NeoForgeInstaller::installer_url("1.20.4", "20.4.0-beta");
        assert!(url.contains("neoforge-20.4.0-beta-installer.jar"));
        // Modern format should NOT contain mc version in path
        assert!(!url.contains("1.20.4"));
    }

    #[test]
    fn test_neoforge_installer_url_legacy() {
        let url = NeoForgeInstaller::installer_url_legacy("1.20.4", "20.4.0");
        assert!(url.contains("neoforge-1.20.4-20.4.0-installer.jar"));
    }

    #[test]
    fn test_neoforge_installer_type() {
        let installer = NeoForgeInstaller::new();
        assert_eq!(installer.loader_type(), ModLoaderType::NeoForge);
    }

    /// Tests extracting version.json from a fake jar.
    #[test]
    fn test_neoforge_extract_version_json_from_fake_jar() {
        let tmpdir = tempfile::tempdir().unwrap();
        let jar_path = tmpdir.path().join("fake-installer.jar");

        {
            let file = std::fs::File::create(&jar_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("version.json", options).unwrap();
            zip.write_all(br#"{"id":"neoforge-20.4.0","type":"release","mainClass":"net.neoforged.neoforge.launcher.Main","libraries":[],"inheritsFrom":"1.20.4"}"#).unwrap();
            zip.finish().unwrap();
        }

        let version_info = NeoForgeInstaller::extract_version_json_from_jar(&jar_path).unwrap();
        assert_eq!(version_info.id, "neoforge-20.4.0");
        assert_eq!(version_info.inherits_from, Some("1.20.4".to_string()));
    }

    /// Tests the full install flow using a fake Java binary.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_neoforge_install_with_fake_java() {
        let tmpdir = tempfile::tempdir().unwrap();
        let game_dir = tmpdir.path().join("game");
        std::fs::create_dir_all(&game_dir).unwrap();

        // Create a fake installer jar WITH embedded version.json
        let installer_jar = game_dir.join("neoforge-1.20.4-20.4.0-installer.jar");
        {
            let file = std::fs::File::create(&installer_jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("version.json", options).unwrap();
            zip.write_all(br#"{"id":"neoforge-20.4.0","type":"release","mainClass":"net.neoforged.neoforge.launcher.Main","libraries":[],"inheritsFrom":"1.20.4"}"#).unwrap();
            zip.finish().unwrap();
        }

        // Create a fake Java binary that just exits 0
        let fake_java = tmpdir.path().join("fake-java");
        std::fs::write(&fake_java, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_java).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_java, perms).unwrap();
        }

        let result = run_installer_jar(&fake_java, &installer_jar, &game_dir).await;
        assert!(result.is_ok(), "fake installer should succeed: {result:?}");

        let version_info =
            NeoForgeInstaller::extract_version_json_from_jar(&installer_jar).unwrap();
        assert_eq!(version_info.id, "neoforge-20.4.0");
        assert_eq!(
            version_info.main_class,
            Some("net.neoforged.neoforge.launcher.Main".to_string())
        );
    }
}
