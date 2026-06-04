use crate::download::task::DownloadTask;
use crate::minecraft::loader::installer::LoaderInstallError;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Type of server addon: plugin (Bukkit-based) or mod (Forge/Fabric-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonType {
    /// Bukkit/Spigot/Paper plugin (.jar into plugins/)
    Plugin,
    /// Forge/Fabric/NeoForge mod (.jar into mods/)
    Mod,
}

impl AddonType {
    /// Returns the target subdirectory name.
    #[must_use]
    pub const fn directory_name(&self) -> &'static str {
        match self {
            Self::Plugin => "plugins",
            Self::Mod => "mods",
        }
    }
}

/// Installer for server addons (plugins or mods).
///
/// Downloads a .jar file and places it into the correct subdirectory
/// of the game directory:
/// - `plugins/` for Paper/Spigot/Purpur
/// - `mods/` for Forge/Fabric/NeoForge
#[derive(Debug, Clone, Default)]
pub struct AddonInstaller;

impl AddonInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Installs an addon from a URL into the game directory.
    ///
    /// The `file_name` parameter determines the saved file name.
    /// If `None`, the name is derived from the URL path.
    pub async fn install_from_url(
        &self,
        addon_type: AddonType,
        url: &str,
        game_dir: &Path,
        file_name: Option<&str>,
    ) -> Result<PathBuf, LoaderInstallError> {
        let dest_name = file_name
            .map(std::string::ToString::to_string)
            .or_else(|| {
                url.split('/')
                    .next_back()
                    .map(|s| s.split('?').next().unwrap_or(s).to_string())
            })
            .ok_or_else(|| {
                LoaderInstallError::InstallFailed(
                    "could not determine file name from URL".to_string(),
                )
            })?;

        let dest_dir = game_dir.join(addon_type.directory_name());
        tokio::fs::create_dir_all(&dest_dir).await?;

        let dest_path = dest_dir.join(&dest_name);
        if !dest_name.ends_with(".jar") {
            return Err(LoaderInstallError::InstallFailed(format!(
                "addon file must be a .jar, got: {dest_name}"
            )));
        }

        info!(
            "downloading {} from {} to {}",
            addon_type.directory_name(),
            url,
            dest_path.display()
        );

        let task = DownloadTask::new(url.to_string(), dest_path.clone());
        task.execute()
            .await
            .map_err(|e| LoaderInstallError::Network(format!("failed to download addon: {e}")))?;

        debug!(
            "{} installed to {}",
            addon_type.directory_name(),
            dest_path.display()
        );

        // If it's a mod, try to parse metadata
        if addon_type == AddonType::Mod {
            match crate::minecraft::metainfo::parse_mod_metadata(&dest_path).await {
                Ok(Some(meta)) => {
                    info!(
                        "parsed mod metadata: {} v{} ({}",
                        meta.name, meta.version, meta.mod_loader
                    );
                }
                Ok(None) => {
                    debug!("no mod metadata found in {}", dest_path.display());
                }
                Err(e) => {
                    warn!("failed to parse mod metadata: {}", e);
                }
            }
        }

        Ok(dest_path)
    }

    /// Installs an addon from local file path into the game directory.
    ///
    /// Copies the file to the correct subdirectory.
    pub async fn install_from_file(
        &self,
        addon_type: AddonType,
        source: &Path,
        game_dir: &Path,
    ) -> Result<PathBuf, LoaderInstallError> {
        let file_name = source
            .file_name()
            .ok_or_else(|| {
                LoaderInstallError::InstallFailed("source path has no file name".to_string())
            })?
            .to_string_lossy();

        if !file_name.ends_with(".jar") {
            return Err(LoaderInstallError::InstallFailed(format!(
                "addon file must be a .jar, got: {file_name}"
            )));
        }

        let dest_dir = game_dir.join(addon_type.directory_name());
        tokio::fs::create_dir_all(&dest_dir).await?;

        let dest_path = dest_dir.join(&*file_name);
        tokio::fs::copy(source, &dest_path).await?;

        info!(
            "copied {} to {}",
            addon_type.directory_name(),
            dest_path.display()
        );
        Ok(dest_path)
    }

    /// Lists installed addons of a given type.
    pub async fn list_installed(
        &self,
        addon_type: AddonType,
        game_dir: &Path,
    ) -> Result<Vec<PathBuf>, LoaderInstallError> {
        let dir = game_dir.join(addon_type.directory_name());
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut entries = tokio::fs::read_dir(&dir).await?;
        let mut jars = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jar") {
                jars.push(path);
            }
        }
        Ok(jars)
    }

    /// Removes an installed addon by file name.
    pub async fn remove(
        &self,
        addon_type: AddonType,
        game_dir: &Path,
        file_name: &str,
    ) -> Result<(), LoaderInstallError> {
        let path = game_dir.join(addon_type.directory_name()).join(file_name);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
            info!("removed {} from {}", file_name, addon_type.directory_name());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addon_type_directory() {
        assert_eq!(AddonType::Plugin.directory_name(), "plugins");
        assert_eq!(AddonType::Mod.directory_name(), "mods");
    }

    #[tokio::test]
    async fn test_install_from_file_plugin() {
        let tmpdir = tempfile::tempdir().unwrap();
        let game_dir = tmpdir.path().join("server");

        // Create a fake plugin jar
        let source = tmpdir.path().join("TestPlugin.jar");
        std::fs::write(&source, b"fake plugin").unwrap();

        let installer = AddonInstaller::new();
        let dest = installer
            .install_from_file(AddonType::Plugin, &source, &game_dir)
            .await
            .unwrap();

        assert_eq!(dest.file_name().unwrap(), "TestPlugin.jar");
        assert!(dest.exists());
        assert!(dest.to_string_lossy().contains("plugins"));
        let content = tokio::fs::read_to_string(&dest).await.unwrap();
        assert_eq!(content, "fake plugin");
    }

    #[tokio::test]
    async fn test_install_from_file_mod() {
        let tmpdir = tempfile::tempdir().unwrap();
        let game_dir = tmpdir.path().join("server");

        let source = tmpdir.path().join("TestMod.jar");
        std::fs::write(&source, b"fake mod").unwrap();

        let installer = AddonInstaller::new();
        let dest = installer
            .install_from_file(AddonType::Mod, &source, &game_dir)
            .await
            .unwrap();

        assert_eq!(dest.file_name().unwrap(), "TestMod.jar");
        assert!(dest.to_string_lossy().contains("mods"));
    }

    #[tokio::test]
    async fn test_install_from_file_rejects_non_jar() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = tmpdir.path().join("bad.txt");
        std::fs::write(&source, b"not a jar").unwrap();

        let installer = AddonInstaller::new();
        let result = installer
            .install_from_file(AddonType::Plugin, &source, tmpdir.path())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".jar"));
    }

    #[tokio::test]
    async fn test_list_installed_plugins() {
        let tmpdir = tempfile::tempdir().unwrap();
        let plugins_dir = tmpdir.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        std::fs::write(plugins_dir.join("A.jar"), b"").unwrap();
        std::fs::write(plugins_dir.join("B.jar"), b"").unwrap();
        std::fs::write(plugins_dir.join("readme.txt"), b"").unwrap();

        let installer = AddonInstaller::new();
        let list = installer
            .list_installed(AddonType::Plugin, tmpdir.path())
            .await
            .unwrap();

        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|p| p.extension().unwrap() == "jar"));
    }

    #[tokio::test]
    async fn test_list_installed_empty() {
        let tmpdir = tempfile::tempdir().unwrap();
        let installer = AddonInstaller::new();
        let list = installer
            .list_installed(AddonType::Mod, tmpdir.path())
            .await
            .unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_remove_plugin() {
        let tmpdir = tempfile::tempdir().unwrap();
        let plugins_dir = tmpdir.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        let jar = plugins_dir.join("OldPlugin.jar");
        std::fs::write(&jar, b"").unwrap();

        let installer = AddonInstaller::new();
        installer
            .remove(AddonType::Plugin, tmpdir.path(), "OldPlugin.jar")
            .await
            .unwrap();

        assert!(!jar.exists());
    }

    #[tokio::test]
    async fn test_install_from_url_rejects_non_jar() {
        let tmpdir = tempfile::tempdir().unwrap();
        let installer = AddonInstaller::new();
        let result = installer
            .install_from_url(
                AddonType::Plugin,
                "https://example.com/plugin.txt",
                tmpdir.path(),
                None,
            )
            .await;
        assert!(result.is_err());
    }
}
