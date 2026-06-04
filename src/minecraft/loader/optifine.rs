use crate::download::mirror::DownloadSource;
use crate::download::task::DownloadTask;
use crate::instance::path::InstanceSubdir;
use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::{
    ArgumentValue, Arguments, DownloadInfo, LibraryDownloads, LibraryInfo, VersionInfo,
};
use std::path::Path;
use tracing::{debug, info};

const BMCLAPI_OPTIFINE_ROOT: &str = "https://bmclapi2.bangbang93.com/optifine";

/// Parsed OptiFine version components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptiFineVersion {
    pub optifine_type: String,
    pub patch: String,
}

impl OptiFineVersion {
    /// Parses an OptiFine version string such as `"HD_U_I7"`.
    #[must_use]
    pub fn parse(version: &str) -> Option<Self> {
        let parts: Vec<&str> = version.split('_').collect();
        if parts.len() >= 2 {
            let patch = parts.last()?.to_string();
            let optifine_type = parts[..parts.len() - 1].join("_");
            Some(Self {
                optifine_type,
                patch,
            })
        } else {
            None
        }
    }
}

/// Installer for OptiFine.
///
/// OptiFine installation downloads the installer JAR from BMCLAPI, extracts the
/// launchwrapper, and produces a `VersionInfo` patch that sets the
/// LaunchWrapper main class and required tweak class arguments.
#[derive(Debug, Clone, Default)]
pub struct OptiFineInstaller {
    pub is_forge_combo: bool,
}

impl OptiFineInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            is_forge_combo: false,
        }
    }

    #[must_use]
    pub const fn with_forge_combo(mut self) -> Self {
        self.is_forge_combo = true;
        self
    }

    fn build_filename(mc_version: &str, version: &OptiFineVersion) -> String {
        format!(
            "OptiFine_{mc_version}_{}_{}",
            version.optifine_type, version.patch
        )
    }

    fn installer_url(mc_version: &str, version: &OptiFineVersion) -> String {
        format!(
            "{BMCLAPI_OPTIFINE_ROOT}/{mc_version}/{}/{}",
            version.optifine_type, version.patch
        )
    }

    /// Extracts the launchwrapper from an OptiFine installer JAR.
    ///
    /// Returns the Maven coordinate of the extracted launchwrapper, if any.
    fn extract_launchwrapper(
        installer_path: &Path,
        libraries_dir: &Path,
    ) -> Result<Option<String>, LoaderInstallError> {
        let file = std::fs::File::open(installer_path)
            .map_err(|e| LoaderInstallError::Io(format!("failed to open installer jar: {e}")))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| LoaderInstallError::Parse(format!("failed to read installer jar: {e}")))?;

        // Try modern launchwrapper-of
        let ver_opt = match archive.by_name("launchwrapper-of.txt") {
            Ok(mut txt) => {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut txt, &mut s)
                    .map_err(|e| LoaderInstallError::Io(e.to_string()))?;
                let v = s.trim().to_string();
                if v.is_empty() { None } else { Some(v) }
            }
            Err(_) => None,
        };

        if let Some(ver) = ver_opt {
            let jar_name = format!("launchwrapper-of-{ver}.jar");
            if let Ok(mut entry) = archive.by_name(&jar_name) {
                let coord = format!("optifine:launchwrapper-of:{ver}");
                let rel = crate::minecraft::library::artifact_path(&coord);
                let dest = libraries_dir.join(&rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| LoaderInstallError::Io(e.to_string()))?;
                }
                let mut out = std::fs::File::create(&dest)
                    .map_err(|e| LoaderInstallError::Io(e.to_string()))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| LoaderInstallError::Io(e.to_string()))?;
                return Ok(Some(coord));
            }
        }

        // Fallback to old launchwrapper-2.0
        if let Ok(mut entry) = archive.by_name("launchwrapper-2.0.jar") {
            let coord = "optifine:launchwrapper:2.0".to_string();
            let rel = crate::minecraft::library::artifact_path(&coord);
            let dest = libraries_dir.join(&rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| LoaderInstallError::Io(e.to_string()))?;
            }
            let mut out =
                std::fs::File::create(&dest).map_err(|e| LoaderInstallError::Io(e.to_string()))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| LoaderInstallError::Io(e.to_string()))?;
            return Ok(Some(coord));
        }

        Ok(None)
    }
}

#[async_trait::async_trait]
impl ModLoaderInstaller for OptiFineInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Unknown // OptiFine is not a traditional mod loader
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        let version = OptiFineVersion::parse(loader_version).ok_or_else(|| {
            LoaderInstallError::Unsupported(format!(
                "invalid OptiFine version format: {loader_version}"
            ))
        })?;

        info!(
            "installing OptiFine {}_{} for Minecraft {}",
            version.optifine_type, version.patch, mc_version
        );

        let filename = Self::build_filename(mc_version, &version);
        let libraries_dir = InstanceSubdir::Libraries.resolve(game_dir);

        // 1. Download installer JAR
        let installer_url = Self::installer_url(mc_version, &version);
        let installer_coord = format!("net.minecraftforge:optifine:{filename}-installer");
        let installer_rel = crate::minecraft::library::artifact_path(&installer_coord);
        let installer_path = libraries_dir.join(&installer_rel);

        if let Some(parent) = installer_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        debug!("downloading OptiFine installer from {}", installer_url);
        let task = DownloadTask::new(installer_url, installer_path.clone())
            .with_sources(vec![DownloadSource::Bmclapi]);
        task.execute().await.map_err(|e| {
            LoaderInstallError::Network(format!("failed to download installer: {e}"))
        })?;

        // 2. Extract launchwrapper from installer
        let lw_coord = tokio::task::spawn_blocking({
            let installer_path = installer_path.clone();
            let libraries_dir = libraries_dir.clone();
            move || Self::extract_launchwrapper(&installer_path, &libraries_dir)
        })
        .await
        .map_err(|e| LoaderInstallError::Io(format!("spawn_blocking failed: {e}")))??;

        let mut libraries = Vec::new();

        // 3. Always inject net.minecraft:launchwrapper:1.12
        let lw12_coord = "net.minecraft:launchwrapper:1.12".to_string();
        let lw12_rel = crate::minecraft::library::artifact_path(&lw12_coord);
        let lw12_path = libraries_dir.join(&lw12_rel);
        if !lw12_path.exists() {
            let base = crate::resource::get_download_api(
                crate::resource::SourceType::Bmclapi,
                crate::resource::ResourceType::Libraries,
            )
            .map_err(|e| LoaderInstallError::Network(e.to_string()))?;
            let lw12_url = base
                .join(lw12_rel.to_string_lossy().as_ref())
                .map_err(|e| LoaderInstallError::Network(e.to_string()))?;
            let task = DownloadTask::new(lw12_url.to_string(), lw12_path)
                .with_sources(vec![DownloadSource::Bmclapi, DownloadSource::Official]);
            task.execute().await.map_err(|e| {
                LoaderInstallError::Network(format!("failed to download launchwrapper: {e}"))
            })?;
        }
        libraries.push(library_info_from_coord(&lw12_coord));

        // 4. Add extracted launchwrapper if present
        if let Some(coord) = lw_coord {
            libraries.push(library_info_from_coord(&coord));
        }

        // 5. Add OptiFine runtime library
        let runtime_coord = format!("net.minecraftforge:optifine:{filename}");
        libraries.push(library_info_from_coord(&runtime_coord));

        // 6. Build VersionInfo patch
        let tweak_class = if self.is_forge_combo {
            "optifine.OptiFineForgeTweaker"
        } else {
            "optifine.OptiFineTweaker"
        };

        let version_info = VersionInfo {
            id: format!("optifine-{filename}"),
            version_type: "release".into(),
            main_class: Some("net.minecraft.launchwrapper.Launch".to_string()),
            libraries,
            arguments: Some(Arguments {
                game: vec![
                    ArgumentValue::String("--tweakClass".to_string()),
                    ArgumentValue::String(tweak_class.to_string()),
                ],
                jvm: vec![],
            }),
            ..Default::default()
        };

        Ok(version_info)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optifine_version_parse() {
        let v = OptiFineVersion::parse("HD_U_I7").unwrap();
        assert_eq!(v.optifine_type, "HD_U");
        assert_eq!(v.patch, "I7");

        let v2 = OptiFineVersion::parse("HD_U_G9").unwrap();
        assert_eq!(v2.optifine_type, "HD_U");
        assert_eq!(v2.patch, "G9");
    }

    #[test]
    fn test_build_filename() {
        let v = OptiFineVersion::parse("HD_U_I7").unwrap();
        let name = OptiFineInstaller::build_filename("1.20.4", &v);
        assert_eq!(name, "OptiFine_1.20.4_HD_U_I7");
    }

    #[test]
    fn test_installer_url() {
        let v = OptiFineVersion::parse("HD_U_I7").unwrap();
        let url = OptiFineInstaller::installer_url("1.20.4", &v);
        assert!(url.contains("bmclapi2.bangbang93.com/optifine/1.20.4/HD_U/I7"));
    }

    #[test]
    fn test_optifine_installer_type() {
        let installer = OptiFineInstaller::new();
        // OptiFine maps to Unknown because it is not a traditional mod loader
        assert_eq!(installer.loader_type(), ModLoaderType::Unknown);
    }
}
