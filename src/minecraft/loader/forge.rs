use crate::download::task::DownloadTask;
use crate::minecraft::loader::installer::{
    LoaderInstallError, ModLoaderInstaller, find_java_executable,
};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::VersionInfo;
use std::path::Path;
use tracing::{debug, info};

/// Installer for Forge mod loader.
///
/// Forge installation requires running an installer jar which generates
/// a patched version JSON and downloads libraries.
#[derive(Debug, Clone, Default)]
pub struct ForgeInstaller;

impl ForgeInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns the Forge installer download URL for a given version.
    #[must_use]
    pub fn installer_url(mc_version: &str, forge_version: &str) -> String {
        format!(
            "https://maven.minecraftforge.net/net/minecraftforge/forge/{mc_version}-{forge_version}/forge-{mc_version}-{forge_version}-installer.jar"
        )
    }

    /// Extracts version.json from the installer jar.
    ///
    /// The Forge installer embeds the version JSON inside the jar itself.
    /// This is used as a fallback when the installer doesn't create a versions/ directory.
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
impl ModLoaderInstaller for ForgeInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Forge
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        info!(
            "installing Forge {} for Minecraft {}",
            loader_version, mc_version
        );

        // Ensure game directory exists
        tokio::fs::create_dir_all(game_dir).await?;

        // Download the installer JAR
        let installer_url = Self::installer_url(mc_version, loader_version);
        let installer_jar =
            game_dir.join(format!("forge-{mc_version}-{loader_version}-installer.jar"));

        debug!("downloading Forge installer from {}", installer_url);
        let task = DownloadTask::new(installer_url.clone(), installer_jar.clone());
        task.execute().await.map_err(|e| {
            LoaderInstallError::Network(format!("failed to download installer: {e}"))
        })?;

        // Find Java executable
        let java_path = find_java_executable().await?;
        debug!("using Java executable: {}", java_path.display());

        // Run the installer
        crate::minecraft::loader::processors::install_server(
            &java_path,
            &installer_jar,
            game_dir,
            ModLoaderType::Forge,
            mc_version,
            loader_version,
        )
        .await?;

        debug!("Forge installer completed successfully");

        // Extract version info from the installer jar (server installer doesn't create versions/)
        let version_info = Self::extract_version_json_from_jar(&installer_jar)?;

        // Remove the downloaded installer after successful installation.
        tokio::fs::remove_file(&installer_jar).await?;

        Ok(version_info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_forge_installer_url() {
        let url = ForgeInstaller::installer_url("1.20.1", "47.2.0");
        assert!(url.contains("forge-1.20.1-47.2.0-installer.jar"));
    }

    #[test]
    fn test_forge_installer_type() {
        let installer = ForgeInstaller::new();
        assert_eq!(installer.loader_type(), ModLoaderType::Forge);
    }

    /// Tests extracting version.json from a real Forge installer jar.
    /// This test downloads a small installer jar, so it requires network.
    #[tokio::test]
    async fn test_forge_extract_version_json_from_jar() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let jar_path = directory.path().join("forge-installer.jar");
        let task = DownloadTask::new(
            ForgeInstaller::installer_url("1.20.1", "47.2.0"),
            jar_path.clone(),
        );
        task.execute().await?;
        let info = ForgeInstaller::extract_version_json_from_jar(&jar_path)?;
        assert_eq!(info.id, "1.20.1-forge-47.2.0");
        assert_eq!(info.inherits_from.as_deref(), Some("1.20.1"));
        assert!(info.main_class.is_some_and(|name| !name.is_empty()));
        Ok(())
    }

    /// Tests extracting version.json from a fake jar with embedded version.json.
    #[test]
    fn test_forge_extract_version_json_from_fake_jar() {
        let tmpdir = tempfile::tempdir().unwrap();
        let jar_path = tmpdir.path().join("fake-installer.jar");

        // Create a fake zip jar with version.json inside
        {
            let file = std::fs::File::create(&jar_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("version.json", options).unwrap();
            zip.write_all(br#"{"id":"test-forge-1.0","type":"release","mainClass":"test.Main","libraries":[],"inheritsFrom":"1.20.1"}"#).unwrap();
            zip.finish().unwrap();
        }

        let version_info = ForgeInstaller::extract_version_json_from_jar(&jar_path).unwrap();
        assert_eq!(version_info.id, "test-forge-1.0");
        assert_eq!(version_info.inherits_from, Some("1.20.1".to_string()));
        assert_eq!(version_info.main_class, Some("test.Main".to_string()));
    }

    /// Tests that extracting from a jar without version.json fails.
    #[test]
    fn test_forge_extract_version_json_missing() {
        let tmpdir = tempfile::tempdir().unwrap();
        let jar_path = tmpdir.path().join("empty.jar");

        {
            let file = std::fs::File::create(&jar_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("other.txt", options).unwrap();
            zip.write_all(b"hello").unwrap();
            zip.finish().unwrap();
        }

        let result = ForgeInstaller::extract_version_json_from_jar(&jar_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("version.json"));
    }
}
