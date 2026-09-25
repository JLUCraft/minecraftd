use crate::download::mirror::DownloadSource;
use crate::download::request::DownloadParam;
use crate::download::task::DownloadTask;
use crate::instance::path::InstanceSubdir;
use crate::minecraft::dependencies::convert_library_name_to_path;
use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loader::processors::add_library_entry;
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::profile::{
    ArgumentsItem, LaunchArgumentTemplate, LibrariesValue, McClientInfo,
};
use crate::minecraft::version::VersionInfo;
use crate::resource::{ResourceType, SourceType, convert_url_to_target_source, get_download_api};
use std::fs;
use std::io;
use zip::{ZipArchive, ZipWriter, write::FileOptions};

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
                let rel = convert_library_name_to_path(&coord, None)?;
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
            let rel = convert_library_name_to_path(&coord, None)?;
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

        let mut profile = VersionInfo {
            main_class: Some("net.minecraft.client.main.Main".into()),
            arguments: Some(Default::default()),
            ..Default::default()
        };
        let loader = if self.is_forge_combo {
            ModLoaderType::Forge
        } else {
            ModLoaderType::Unknown
        };
        let downloads = prepare_libraries(
            &[SourceType::Bmclapi],
            &filename,
            loader_version,
            loader,
            &libraries_dir,
            &mut profile,
        )?;
        for request in downloads {
            DownloadTask::new(request.src.to_string(), request.dest)
                .execute()
                .await
                .map_err(|error| LoaderInstallError::Network(error.to_string()))?;
        }
        let java = if requires_patcher(&libraries_dir, &filename)? {
            Some(crate::minecraft::loader::installer::find_java_executable().await?)
        } else {
            None
        };
        let base_jar = game_dir
            .join("versions")
            .join(mc_version)
            .join(format!("{mc_version}.jar"));
        finish_install(java.as_deref(), &libraries_dir, &filename, &base_jar).await?;
        Ok(profile)
    }
}

const OPTIFINE_PATCH_PRIORITY: i64 = 10_000;
pub fn installer_download(
    game_version: &str,
    optifine: &crate::minecraft::profile::OptiFineResourceInfo,
    library_dir: &Path,
) -> Result<DownloadParam, LoaderInstallError> {
    let root = get_download_api(SourceType::Bmclapi, ResourceType::OptiFine)?;
    let src = root.join(&format!(
        "{}/{}/{}",
        game_version, optifine.r#type, optifine.patch
    ))?;
    let coordinate = format!(
        "net.minecraftforge:optifine:{}-installer",
        optifine.filename
    );
    Ok(DownloadParam {
        src,
        dest: library_dir.join(convert_library_name_to_path(&coordinate, None)?),
        filename: None,
        sha1: None,
    })
}
pub fn prepare_libraries(
    priority: &[SourceType],
    filename: &str,
    version: &str,
    loader: ModLoaderType,
    lib_dir: &Path,
    client_info: &mut McClientInfo,
) -> Result<Vec<DownloadParam>, LoaderInstallError> {
    let source = *priority
        .first()
        .ok_or_else(|| LoaderInstallError::Network("no sources configured".into()))?;
    let mut task_params: Vec<DownloadParam> = vec![];
    let installer_coord = format!("net.minecraftforge:optifine:{}-installer", filename);
    let installer_rel = convert_library_name_to_path(&installer_coord, None)?;
    let installer_path = lib_dir.join(&installer_rel);

    if !installer_path.exists() {
        return Err(LoaderInstallError::InstallFailed(
            "OptiFine installer missing".into(),
        ));
    }
    let extracted = OptiFineInstaller::extract_launchwrapper(&installer_path, lib_dir)?;
    let has_launchwrapper = extracted.is_some();
    let mut lw_coord = extracted.unwrap_or_default();
    if has_launchwrapper {
        add_library_entry(&mut client_info.libraries, &lw_coord, None)?;
    }
    let temp_lw_coord = "net.minecraft:launchwrapper:1.12".to_string();
    let lw_rel = convert_library_name_to_path(&temp_lw_coord, None)?;
    let lw_dest = lib_dir.join(&lw_rel);

    let base = get_download_api(source, ResourceType::Libraries)?;
    let src =
        convert_url_to_target_source(&base.join(&lw_rel)?, &[ResourceType::Libraries], &source)?;

    task_params.push(DownloadParam {
        src,
        dest: lw_dest,
        filename: None,
        sha1: None,
    });

    if !has_launchwrapper {
        lw_coord = "net.minecraft:launchwrapper:1.12".to_string();
        add_library_entry(&mut client_info.libraries, &temp_lw_coord, None)?;
    }

    let optifine_runtime_coord = format!("net.minecraftforge:optifine:{}", filename);
    add_library_entry(&mut client_info.libraries, &optifine_runtime_coord, None)?;
    let lw_main = "net.minecraft.launchwrapper.Launch".to_string();

    if let Some(v_args) = client_info.arguments.clone() {
        let mut g: Vec<ArgumentsItem> = v_args.game.clone();
        let flag = ArgumentsItem {
            value: vec!["--tweakClass".to_string()],
            rules: vec![],
        };
        let val = if matches!(
            loader,
            ModLoaderType::Forge | ModLoaderType::LegacyForge | ModLoaderType::Cleanroom
        ) {
            ArgumentsItem {
                value: vec!["optifine.OptiFineForgeTweaker".to_string()],
                rules: vec![],
            }
        } else {
            ArgumentsItem {
                value: vec!["optifine.OptiFineTweaker".to_string()],
                rules: vec![],
            }
        };

        if let Some(pos) = g.iter().position(|item| {
            item.value
                .first()
                .map(|v| v == "--launchTarget")
                .unwrap_or(false)
        }) {
            g.insert(pos, val);
            g.insert(pos, flag);
        } else {
            g.insert(0, val);
            g.insert(0, flag);
        }

        let new_args = LaunchArgumentTemplate {
            game: g,
            jvm: v_args.jvm,
        };
        client_info.arguments = Some(new_args);
    } else {
        let mut s = client_info.minecraft_arguments.clone().unwrap_or_default();
        if !s.is_empty() && !s.ends_with(' ') {
            s.push(' ');
        }
        if matches!(
            loader,
            ModLoaderType::Forge | ModLoaderType::LegacyForge | ModLoaderType::Cleanroom
        ) {
            s.push_str("--tweakClass optifine.OptiFineForgeTweaker");
        } else {
            s.push_str("--tweakClass optifine.OptiFineTweaker");
        }
        client_info.minecraft_arguments = Some(s);
    };

    let (patch_arguments, patch_minecraft_arguments) = if client_info.arguments.is_some() {
        let patch_args = LaunchArgumentTemplate {
            game: vec![
                ArgumentsItem {
                    value: vec!["--tweakClass".to_string()],
                    rules: vec![],
                },
                ArgumentsItem {
                    value: vec!["optifine.OptiFineTweaker".to_string()],
                    rules: vec![],
                },
            ],
            jvm: vec![],
        };
        (Some(patch_args), None)
    } else {
        (
            None,
            Some("--tweakClass optifine.OptiFineTweaker".to_string()),
        )
    };

    client_info.patches.push(McClientInfo {
        id: "optifine".to_string(),
        version: Some(version.to_owned()),
        priority: Some(OPTIFINE_PATCH_PRIORITY),
        main_class: Some(lw_main.clone()),
        arguments: patch_arguments,
        minecraft_arguments: patch_minecraft_arguments,
        libraries: vec![
            LibrariesValue {
                name: optifine_runtime_coord.clone(),
                ..Default::default()
            },
            LibrariesValue {
                name: lw_coord.clone(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    if client_info.main_class == Some("net.minecraft.client.main.Main".to_string()) {
        client_info.main_class = Some(lw_main);
    }

    Ok(task_params)
}
fn installer_path(
    libraries: &Path,
    filename: &str,
) -> Result<std::path::PathBuf, LoaderInstallError> {
    Ok(libraries.join(convert_library_name_to_path(
        &format!("net.minecraftforge:optifine:{filename}-installer"),
        None,
    )?))
}
pub fn requires_patcher(libraries: &Path, filename: &str) -> Result<bool, LoaderInstallError> {
    let mut archive = ZipArchive::new(fs::File::open(installer_path(libraries, filename)?)?)?;
    Ok(archive.by_name("optifine/Patcher.class").is_ok())
}
pub async fn finish_install(
    java: Option<&Path>,
    libraries: &Path,
    filename: &str,
    base_client_jar: &Path,
) -> Result<(), LoaderInstallError> {
    let installer = installer_path(libraries, filename)?;
    let output_path = libraries.join(convert_library_name_to_path(
        &format!("net.minecraftforge:optifine:{filename}"),
        None,
    )?);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if requires_patcher(libraries, filename)? {
        let java = java.ok_or_else(|| {
            LoaderInstallError::InstallFailed("OptiFine patcher requires Java".into())
        })?;
        let mut command = tokio::process::Command::new(java);
        command
            .kill_on_drop(true)
            .arg("-cp")
            .arg(&installer)
            .arg("optifine.Patcher")
            .arg(base_client_jar)
            .arg(&installer)
            .arg(&output_path);
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000);
        let output = command.output().await?;
        if !output.status.success() {
            return Err(LoaderInstallError::InstallFailed(format!(
                "OptiFine patcher failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    } else {
        fs::copy(&installer, &output_path)?;
    }
    if !output_path.is_file() {
        return Err(LoaderInstallError::InstallFailed(
            "OptiFine patcher did not produce the runtime jar".into(),
        ));
    }
    remove_entry_from_zip(&output_path, "META-INF/mods.toml")?;
    Ok(())
}
fn remove_entry_from_zip(zip_path: &Path, entry_name: &str) -> Result<(), LoaderInstallError> {
    if !zip_path.exists() {
        return Ok(());
    }

    let tmp_path = {
        let mut p = zip_path.to_path_buf();
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "jar".to_string());
        p.set_extension(format!("{}.tmp", ext));
        p
    };

    let src = fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(src)?;

    let dst = fs::File::create(&tmp_path)?;
    let mut writer = ZipWriter::new(dst);

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();

        if name == entry_name {
            continue;
        }
        if name.ends_with('/') {
            writer.add_directory(name, FileOptions::<()>::default())?;
            continue;
        }

        let options = FileOptions::<()>::default().compression_method(file.compression());
        writer.start_file(name, options)?;
        io::copy(&mut file, &mut writer)?;
    }

    writer.finish()?;
    fs::rename(&tmp_path, zip_path)?;

    Ok(())
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

    fn installer_fixture(libraries: &Path, patcher: bool) -> std::path::PathBuf {
        use std::io::Write;
        let path = installer_path(libraries, "example").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut archive = ZipWriter::new(fs::File::create(&path).unwrap());
        for name in ["META-INF/mods.toml", "optifine/Example.class"] {
            archive
                .start_file(name, FileOptions::<()>::default())
                .unwrap();
            archive.write_all(b"fixture").unwrap();
        }
        if patcher {
            archive
                .start_file("optifine/Patcher.class", FileOptions::<()>::default())
                .unwrap();
            archive.write_all(b"patcher").unwrap();
        }
        archive.finish().unwrap();
        path
    }

    #[tokio::test]
    async fn finishing_without_patcher_produces_jar_and_removes_forge_descriptor() {
        let root = tempfile::tempdir().unwrap();
        installer_fixture(root.path(), false);
        finish_install(None, root.path(), "example", Path::new("unused.jar"))
            .await
            .unwrap();
        let path = root.path().join(
            convert_library_name_to_path("net.minecraftforge:optifine:example", None).unwrap(),
        );
        let mut archive = ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        assert!(archive.by_name("META-INF/mods.toml").is_err());
        assert!(archive.by_name("optifine/Example.class").is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_patcher_without_output_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        installer_fixture(root.path(), true);
        assert!(
            finish_install(
                Some(Path::new("/bin/true")),
                root.path(),
                "example",
                Path::new("client.jar")
            )
            .await
            .is_err()
        );
    }
}
