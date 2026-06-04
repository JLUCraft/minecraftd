use crate::download::task::DownloadTask;
use crate::minecraft::loader::installer::{
    LoaderInstallError, ModLoaderInstaller, find_java_executable, run_installer_jar,
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
        run_installer_jar(&java_path, &installer_jar, game_dir).await?;

        debug!("Forge installer completed successfully");

        // Extract version info from the installer jar (server installer doesn't create versions/)
        let version_info = Self::extract_version_json_from_jar(&installer_jar)?;

        // Clean up installer jar (optional, but keeps game dir clean)
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
    async fn test_forge_extract_version_json_from_jar() {
        let tmpdir = tempfile::tempdir().unwrap();
        let jar_path = tmpdir.path().join("forge-installer.jar");

        // Download a real Forge installer
        let url = ForgeInstaller::installer_url("1.20.1", "47.2.0");
        let task = DownloadTask::new(url.clone(), jar_path.clone());
        let download_result = task.execute().await;
        if download_result.is_err() {
            eprintln!("SKIP: network unavailable");
            return;
        }

        let version_info = ForgeInstaller::extract_version_json_from_jar(&jar_path).unwrap();
        assert_eq!(version_info.id, "1.20.1-forge-47.2.0");
        assert_eq!(version_info.inherits_from, Some("1.20.1".to_string()));
        assert!(!version_info.main_class.as_ref().unwrap().is_empty());
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

    /// Tests the full install flow using a fake Java binary that simulates
    /// the Forge installer. The version.json is extracted from the fake jar.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_forge_install_with_fake_java() {
        let tmpdir = tempfile::tempdir().unwrap();
        let game_dir = tmpdir.path().join("game");
        std::fs::create_dir_all(&game_dir).unwrap();

        // Create a fake installer jar WITH embedded version.json
        let installer_jar = game_dir.join("forge-1.20.1-47.2.0-installer.jar");
        {
            let file = std::fs::File::create(&installer_jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("version.json", options).unwrap();
            zip.write_all(br#"{"id":"1.20.1-forge-47.2.0","type":"release","mainClass":"net.minecraft.launchwrapper.Launch","libraries":[],"inheritsFrom":"1.20.1"}"#).unwrap();
            zip.finish().unwrap();
        }

        // Create a fake Java binary that just exits 0 (libraries are "already there")
        let fake_java = tmpdir.path().join("fake-java");
        std::fs::write(&fake_java, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_java).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_java, perms).unwrap();
        }

        // Test run_installer_jar directly with the fake Java
        let result = run_installer_jar(&fake_java, &installer_jar, &game_dir).await;
        assert!(result.is_ok(), "fake installer should succeed: {result:?}");

        // Extract version info from the jar
        let version_info = ForgeInstaller::extract_version_json_from_jar(&installer_jar).unwrap();
        assert_eq!(version_info.id, "1.20.1-forge-47.2.0");
        assert_eq!(
            version_info.main_class,
            Some("net.minecraft.launchwrapper.Launch".to_string())
        );
    }

    /// Tests that `run_installer_jar` correctly reports failure when Java exits non-zero.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_forge_installer_jar_failure() {
        let tmpdir = tempfile::tempdir().unwrap();
        let fake_java = tmpdir.path().join("fake-java-fail");
        std::fs::write(&fake_java, "#!/bin/sh\necho 'out of memory' >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_java).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_java, perms).unwrap();
        }

        let installer_jar = tmpdir.path().join("installer.jar");
        std::fs::write(&installer_jar, b"").unwrap();

        let result = run_installer_jar(&fake_java, &installer_jar, tmpdir.path()).await;
        assert!(result.is_err(), "should fail when Java exits non-zero");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("out of memory"),
            "error should contain stderr: {err}"
        );
    }
}
