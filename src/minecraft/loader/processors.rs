//! Shared client-side Forge, NeoForge and Cleanroom installer processing.
use crate::download::request::DownloadParam;
use crate::minecraft::dependencies::{convert_library_name_to_path, merge_library_lists};
use crate::minecraft::loader::installer::LoaderInstallError;
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::profile::{
    LibrariesValue, McClientInfo, RulePlatform, reset_fields_from_patches,
};
use crate::resource::{ResourceType, SourceType, convert_url_to_target_source, get_download_api};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use url::Url;
use zip::ZipArchive;
type Result<T> = std::result::Result<T, LoaderInstallError>;
const LOADER_PATCH_PRIORITY: i64 = 30_000;
pub struct InstallTarget<'a> {
    pub loader: ModLoaderType,
    pub loader_version: &'a str,
    pub name: &'a str,
    pub version_dir: &'a Path,
    pub root_dir: &'a Path,
    pub library_dir: &'a Path,
}
fn family_coordinate(loader: ModLoaderType, version: &str) -> Result<&'static str> {
    match loader {
        ModLoaderType::Forge | ModLoaderType::LegacyForge => Ok("net.minecraftforge:forge"),
        ModLoaderType::NeoForge if version.starts_with("1.20.1-") => Ok("net.neoforged:forge"),
        ModLoaderType::NeoForge => Ok("net.neoforged:neoforge"),
        ModLoaderType::Cleanroom => Ok("com.cleanroommc:cleanroom"),
        _ => Err(LoaderInstallError::Unsupported(loader.to_string())),
    }
}
pub fn installer_download(
    priority: &[SourceType],
    loader: ModLoaderType,
    game_version: &str,
    version: &str,
    branch: Option<&str>,
    library_dir: &Path,
) -> Result<DownloadParam> {
    let source = *priority
        .first()
        .ok_or_else(|| LoaderInstallError::Network("no sources configured".into()))?;
    let coordinate = family_coordinate(loader, version)?;
    let src = match loader {
        ModLoaderType::Forge | ModLoaderType::LegacyForge => {
            let root = get_download_api(source, ResourceType::ForgeInstall)?;
            if source == SourceType::Official {
                let version = [game_version, version, branch.unwrap_or_default()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-");
                root.join(&format!("{version}/forge-{version}-installer.jar"))?
            } else {
                let mut url = root;
                url.query_pairs_mut().extend_pairs([
                    ("mcversion", game_version),
                    ("version", version),
                    ("branch", branch.unwrap_or_default()),
                    ("category", "installer"),
                    ("format", "jar"),
                ]);
                url
            }
        }
        ModLoaderType::Cleanroom => get_download_api(source, ResourceType::CleanroomInstall)?
            .join(&format!("cleanroom-{version}-installer.jar"))?,
        ModLoaderType::NeoForge => {
            if version.starts_with("1.20.1-") {
                get_download_api(SourceType::Official, ResourceType::NeoforgeInstall)?.join(
                    &format!("net/neoforged/forge/{version}/forge-{version}-installer.jar"),
                )?
            } else if source == SourceType::Official {
                get_download_api(source, ResourceType::NeoforgeInstall)?.join(&format!(
                    "net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar"
                ))?
            } else {
                get_download_api(source, ResourceType::NeoforgeInstall)?
                    .join(&format!("{version}/download/installer"))?
            }
        }
        _ => return Err(LoaderInstallError::Unsupported(loader.to_string())),
    };
    let path = convert_library_name_to_path(&format!("{coordinate}:{version}-installer"), None)?;
    Ok(DownloadParam {
        src,
        dest: library_dir.join(path),
        filename: None,
        sha1: None,
    })
}
pub fn add_library_entry(
    libraries: &mut Vec<LibrariesValue>,
    name: &str,
    params: Option<LibrariesValue>,
) -> Result<()> {
    convert_library_name_to_path(name, None)?;
    let library = LibrariesValue {
        name: name.to_owned(),
        ..params.unwrap_or_default()
    };
    *libraries = merge_library_lists(libraries, &[library]);
    Ok(())
}
pub async fn prepare_libraries(
    priority: &[SourceType],
    target: &InstallTarget<'_>,
    client_info: &mut McClientInfo,
    platform: &RulePlatform,
) -> Result<Vec<DownloadParam>> {
    let source = *priority
        .first()
        .ok_or_else(|| LoaderInstallError::Network("no sources configured".into()))?;
    let root_dir = target.root_dir;
    let lib_dir = target.library_dir;
    let mut task_params = Vec::new();
    let coordinate = family_coordinate(target.loader, target.loader_version)?;
    let installer_coord = format!("{coordinate}:{}-installer", target.loader_version);
    let installer_rel = convert_library_name_to_path(&installer_coord, None)?;
    let installer_path = lib_dir.join(&installer_rel);
    let bin_patch = lib_dir.join(convert_library_name_to_path(
        &format!("{coordinate}:{}:clientdata@lzma", target.loader_version),
        None,
    )?);
    if !installer_path.exists() {
        return Err(LoaderInstallError::InstallFailed(
            "loader installer not found".into(),
        ));
    }
    let file = File::open(&installer_path)?;
    let mut archive = ZipArchive::new(file)?;

    // Extract maven folder contents to lib_dir
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let path = file
            .enclosed_name()
            .ok_or_else(|| LoaderInstallError::Parse("unsafe installer entry path".into()))?;
        let outpath = if path.starts_with("maven/") {
            // Remove "maven/" prefix and join with lib_dir
            let relative_path = path
                .strip_prefix("maven/")
                .map_err(|error| LoaderInstallError::Parse(error.to_string()))?;
            lib_dir.join(relative_path)
        } else if path == *"data/client.lzma" {
            bin_patch.clone()
        } else {
            continue;
        };

        if file.is_file() {
            // Create parent directories if they don't exist
            if let Some(p) = outpath.parent()
                && !p.exists()
            {
                fs::create_dir_all(p)?;
            }

            // Extract file
            let mut outfile = File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }
    }

    let (install_profile, version) = {
        let mut s = String::new();
        if let Ok(mut install_profile) = archive.by_name("install_profile.json") {
            install_profile.read_to_string(&mut s)?;
        }

        let mut t = String::new();
        if let Ok(mut version_file) = archive.by_name("version.json") {
            version_file.read_to_string(&mut t)?;
        }

        (s, t)
    };

    if install_profile.is_empty() {
        return Err(LoaderInstallError::Parse("missing install profile".into()));
    }

    if !version.is_empty() {
        // It's modern version Forge installer

        let mut profile: InstallProfile = serde_json::from_str(&install_profile)?;

        let mut args_map = HashMap::<String, String>::new();
        args_map.insert(
            "{MINECRAFT_JAR}".into(),
            target
                .version_dir
                .join(format!("{}.jar", target.name))
                .to_string_lossy()
                .to_string(),
        );
        args_map.insert("{BINPATCH}".into(), bin_patch.to_string_lossy().to_string());
        args_map.insert(
            "{INSTALLER}".into(),
            installer_path.to_string_lossy().to_string(),
        );
        args_map.insert("{SIDE}".into(), "client".to_string());
        args_map.insert("{ROOT}".into(), root_dir.to_string_lossy().to_string());
        for (key, value) in profile.data.iter() {
            if args_map.contains_key(&format!("{{{key}}}")) {
                continue;
            }
            let value_client = if let Some(library) = value.client.strip_circumfix('[', ']') {
                lib_dir
                    .join(convert_library_name_to_path(library, None)?)
                    .to_string_lossy()
                    .into_owned()
            } else {
                value.client.clone()
            };
            args_map.insert(format!("{{{key}}}"), value_client);
        }

        for processor in profile.processors.iter_mut() {
            if processor.args.contains(&"DOWNLOAD_MOJMAPS".to_string()) {
                if let Some(mojmaps) = args_map.get("{MOJMAPS}")
                    && let Some(client_mappings) = client_info.downloads.get("client_mappings")
                {
                    task_params.push(DownloadParam {
                        src: client_mappings.url.parse()?,
                        dest: lib_dir.join(mojmaps),
                        filename: None,
                        sha1: Some(client_mappings.sha1.clone()),
                    });
                }
                processor.args.clear();
                continue;
            }

            processor.jar = lib_dir
                .join(convert_library_name_to_path(&processor.jar, None)?)
                .to_string_lossy()
                .to_string();

            for class in processor.classpath.iter_mut() {
                *class = lib_dir
                    .join(convert_library_name_to_path(class, None)?)
                    .to_string_lossy()
                    .to_string();
            }

            for arg in processor.args.iter_mut() {
                if let Some(library) = arg.strip_circumfix('[', ']') {
                    *arg = lib_dir
                        .join(convert_library_name_to_path(library, None)?)
                        .to_string_lossy()
                        .into_owned();
                }
                for (key, value) in &args_map {
                    *arg = arg.replace(key, value);
                }
            }
        }

        profile.processors.retain(|processor| {
            !processor.args.is_empty()
                && processor
                    .sides
                    .as_ref()
                    .is_none_or(|sides| sides.iter().any(|side| side == "client"))
        });

        fs::write(
            target.version_dir.join("install_profile.json"),
            &serde_json::to_vec_pretty(&profile)?,
        )?;

        let forge_info: McClientInfo = serde_json::from_str(&version)?;
        client_info.main_class = forge_info.main_class.clone();

        let mut loader_libraries = vec![];
        for lib in forge_info.libraries.iter() {
            let name = &lib.name;
            add_library_entry(&mut client_info.libraries, name, Some(lib.clone()))?;
            add_library_entry(&mut loader_libraries, name, Some(lib.clone()))?;
        }

        let arguments = forge_info.arguments.clone();
        let minecraft_arguments = if arguments.is_some() {
            None
        } else {
            forge_info.minecraft_arguments.clone()
        };

        client_info.patches.push(McClientInfo {
            id: target.loader.to_string().to_lowercase(),
            version: Some(target.loader_version.to_owned()),
            priority: Some(LOADER_PATCH_PRIORITY),
            inherits_from: forge_info.inherits_from.clone(),
            main_class: forge_info.main_class.clone(),
            arguments,
            minecraft_arguments,
            libraries: loader_libraries,
            ..Default::default()
        });

        for lib in profile.libraries.iter() {
            let name = &lib.name;
            let url = lib
                .downloads
                .as_ref()
                .and_then(|d| d.artifact.as_ref())
                .map(|a| a.url.as_str())
                .unwrap_or_default();

            if url.is_empty() {
                continue;
            }

            let rel = convert_library_name_to_path(&name.to_string(), None)?;
            task_params.push(DownloadParam {
                src: convert_url_to_target_source(
                    &Url::parse(url)?,
                    &[
                        ResourceType::ForgeMaven,
                        ResourceType::ForgeMavenNew,
                        ResourceType::NeoforgeMaven,
                        ResourceType::CleanroomMaven,
                        ResourceType::Libraries,
                    ],
                    &source,
                )?,
                dest: lib_dir.join(&rel),
                filename: None,
                sha1: None,
            });
        }
    } else {
        // It's legacy version Forge installer

        let profile: LegacyInstallProfile = serde_json::from_str(&install_profile)?;

        let main_class = profile.version_info.main_class;
        let libraries = profile.version_info.libraries;

        client_info.main_class = Some(main_class.clone());

        let mut new_patch = McClientInfo {
            id: target.loader.to_string().to_lowercase(),
            version: Some(target.loader_version.to_owned()),
            priority: Some(LOADER_PATCH_PRIORITY),
            main_class: Some(main_class),
            inherits_from: Some(profile.version_info.inherits_from),
            arguments: None,
            minecraft_arguments: Some(profile.version_info.minecraft_arguments.clone()),
            release_time: profile.version_info.release_time,
            time: profile.version_info.time,
            type_: profile.version_info.type_,
            assets: profile.version_info.assets,

            ..Default::default()
        };

        client_info.minecraft_arguments = Some(profile.version_info.minecraft_arguments.clone());

        let mut file = archive.by_name(&profile.install.file_path)?;
        let dest_path = lib_dir.join(convert_library_name_to_path(&profile.install.path, None)?);
        if let Some(parent) = dest_path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&dest_path)?;
        std::io::copy(&mut file, &mut output)?;

        for lib in libraries.iter() {
            let name = lib.name.clone();

            add_library_entry(&mut client_info.libraries, &name, None)?;
            add_library_entry(&mut new_patch.libraries, &name, None)?;

            if name == profile.install.path {
                continue;
            }

            let url = match &lib.url {
                Some(url) => Url::parse(url)?,
                None => get_download_api(source, ResourceType::Libraries)?,
            };

            let rel = convert_library_name_to_path(&name, None)?;
            let src = convert_url_to_target_source(
                &url.join(&rel)?,
                &[
                    ResourceType::ForgeMaven,
                    ResourceType::ForgeMavenNew,
                    ResourceType::NeoforgeMaven,
                    ResourceType::CleanroomMaven,
                    ResourceType::Libraries,
                ],
                &source,
            )?;
            task_params.push(DownloadParam {
                src,
                dest: lib_dir.join(&rel),
                filename: None,
                sha1: None,
            });
        }
        client_info.patches.push(new_patch);
    }

    reset_fields_from_patches(client_info);

    task_params.extend(
        crate::minecraft::dependencies::download_plan(source, lib_dir, client_info, platform, true)
            .await?,
    );

    let mut missing = Vec::new();
    for request in task_params {
        let cached = match fs::metadata(&request.dest) {
            Ok(metadata) if metadata.is_file() => {
                if let Some(expected) = &request.sha1 {
                    crate::util::hash::sha1_file(&request.dest)
                        .await
                        .map_err(|error| LoaderInstallError::Io(error.to_string()))?
                        == *expected
                } else {
                    false
                }
            }
            Ok(_) => {
                return Err(LoaderInstallError::Io(format!(
                    "library destination is not a file: {}",
                    request.dest.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !cached {
            missing.push(request);
        }
    }

    let mut seen = std::collections::HashSet::new();
    missing.retain(|param| seen.insert(param.dest.clone()));

    Ok(missing)
}
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallProfileData {
    pub client: String,
    pub server: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorsValue {
    pub sides: Option<Vec<String>>,
    pub jar: String,
    pub classpath: Vec<String>,
    pub args: Vec<String>,
    pub outputs: Option<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallProfile {
    pub spec: u64,
    pub profile: String,
    pub version: String,
    pub path: Option<String>,
    pub minecraft: String,
    pub data: HashMap<String, InstallProfileData>,
    pub processors: Vec<ProcessorsValue>,
    pub libraries: Vec<LibrariesValue>,
    pub json: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LegacyInstallProfile {
    install: LegacyInstall,
    version_info: LegacyVersion,
}
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LegacyInstall {
    path: String,
    version: String,
    file_path: String,
}
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LegacyVersion {
    id: String,
    time: String,
    release_time: String,
    #[serde(rename = "type")]
    type_: String,
    minecraft_arguments: String,
    main_class: String,
    minimum_launcher_version: i32,
    assets: String,
    inherits_from: String,
    jar: String,
    libraries: Vec<LegacyLibrariesValue>,
}
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyLibrariesValue {
    pub name: String,
    pub url: Option<String>,
}

pub async fn execute_processors(
    java: &Path,
    version_dir: &Path,
    profile: &InstallProfile,
) -> Result<()> {
    for processor in &profile.processors {
        let path = version_dir.join(&processor.jar);
        let mut archive = ZipArchive::new(File::open(&path)?)?;
        let mut manifest = String::new();
        archive
            .by_name("META-INF/MANIFEST.MF")?
            .read_to_string(&mut manifest)?;
        let attributes = crate::minecraft::metadata::manifest_attributes(&manifest);
        let main = attributes
            .get("Main-Class")
            .ok_or_else(|| LoaderInstallError::Parse("processor Main-Class missing".into()))?;
        let mut classpath = processor.classpath.clone();
        classpath.push(path.to_string_lossy().into_owned());
        let mut command = tokio::process::Command::new(java);
        command
            .kill_on_drop(true)
            .arg("-cp")
            .arg(classpath.join(crate::minecraft::plan::get_separator()))
            .arg(main)
            .args(&processor.args)
            .current_dir(version_dir);
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000);
        let output = command.output().await?;
        if !output.status.success() {
            return Err(LoaderInstallError::InstallFailed(format!(
                "processor {} exited with {:?}: {}",
                path.display(),
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn archive(path: &Path, entries: &[(&str, String)]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut jar = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, contents) in entries {
            jar.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            jar.write_all(contents.as_bytes()).unwrap();
        }
        jar.finish().unwrap();
    }
    #[tokio::test]
    async fn installer_keeps_native_rows_and_downloads_only_missing_current_platform_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let libraries = dir.path().join("libraries");
        let version_dir = dir.path().join("versions/example");
        fs::create_dir_all(&version_dir).unwrap();
        let target = InstallTarget {
            loader: ModLoaderType::Forge,
            loader_version: "1",
            name: "example",
            version_dir: &version_dir,
            root_dir: dir.path(),
            library_dir: &libraries,
        };
        let request = installer_download(
            &[SourceType::Official],
            target.loader,
            "1.20.4",
            "1",
            None,
            &libraries,
        )
        .unwrap();
        let java_bytes = b"existing complete artifact";
        let native_bytes = b"downloaded native artifact";
        let java_hash = crate::util::hash::sha1_bytes(java_bytes);
        let native_hash = crate::util::hash::sha1_bytes(native_bytes);
        let rows: Vec<LibrariesValue> = serde_json::from_value(serde_json::json!([
          {"name":"test:bridge:1","downloads":{"artifact":{"path":"test/bridge/1/bridge-1.jar","url":"https://example.test/java.jar","sha1":java_hash,"size":java_bytes.len()}}},
          {"name":"test:bridge:1","natives":{"linux":"natives-linux"},"rules":[{"action":"allow","os":{"name":"linux","arch":"x64"}}],"downloads":{"classifiers":{"natives-linux":{"url":"https://example.test/linux.jar","sha1":native_hash,"size":native_bytes.len()}}}},
          {"name":"test:bridge:1","natives":{"windows":"natives-windows"},"rules":[{"action":"allow","os":{"name":"windows"}}],"downloads":{"classifiers":{"natives-windows":{"path":"test/bridge/1/bridge-1-natives-windows.jar","url":"https://example.test/windows.jar","sha1":"windows","size":1}}}}
        ])).unwrap();
        archive(
            &request.dest,
            &[
                (
                    "install_profile.json",
                    serde_json::to_string(&InstallProfile::default()).unwrap(),
                ),
                (
                    "version.json",
                    serde_json::to_string(&McClientInfo {
                        libraries: rows,
                        ..Default::default()
                    })
                    .unwrap(),
                ),
            ],
        );
        let cached = libraries.join("test/bridge/1/bridge-1.jar");
        fs::create_dir_all(cached.parent().unwrap()).unwrap();
        fs::write(&cached, java_bytes).unwrap();
        let platform = RulePlatform {
            os: "linux".into(),
            arch: "x64".into(),
            version: String::new(),
        };
        let mut client = McClientInfo {
            patches: vec![McClientInfo {
                id: "game".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let downloads = prepare_libraries(&[SourceType::Official], &target, &mut client, &platform)
            .await
            .unwrap();
        assert_eq!(client.libraries.len(), 3);
        assert_eq!(downloads.len(), 1);
        assert!(downloads[0].dest.ends_with("bridge-1-natives-linux.jar"));
        assert_eq!(downloads[0].sha1.as_deref(), Some(native_hash.as_str()));
        fs::write(&downloads[0].dest, native_bytes).unwrap();
        let downloads = prepare_libraries(&[SourceType::Official], &target, &mut client, &platform)
            .await
            .unwrap();
        assert!(downloads.is_empty());
        assert_eq!(
            fs::read_to_string(cached).unwrap(),
            "existing complete artifact"
        );
        let cached = libraries.join("test/bridge/1/bridge-1.jar");
        fs::write(&cached, "corrupt cached bytes").unwrap();
        let downloads = prepare_libraries(&[SourceType::Official], &target, &mut client, &platform)
            .await
            .unwrap();
        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].dest, cached);
        assert_eq!(downloads[0].sha1.as_deref(), Some(java_hash.as_str()));
    }
    #[tokio::test]
    async fn all_families_resolve_processors_and_extract_embedded_libraries() {
        for loader in [
            ModLoaderType::Forge,
            ModLoaderType::NeoForge,
            ModLoaderType::Cleanroom,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let libraries = dir.path().join("libraries");
            let version_dir = dir.path().join("versions/example");
            fs::create_dir_all(&version_dir).unwrap();
            let target = InstallTarget {
                loader,
                loader_version: "1.0",
                name: "example",
                version_dir: &version_dir,
                root_dir: dir.path(),
                library_dir: &libraries,
            };
            let request = installer_download(
                &[SourceType::Official],
                loader,
                "1.20.4",
                "1.0",
                None,
                &libraries,
            )
            .unwrap();
            let profile = InstallProfile {
                data: HashMap::from([
                    (
                        "INPUT".into(),
                        InstallProfileData {
                            client: "[test:input:1]".into(),
                            ..Default::default()
                        },
                    ),
                    (
                        "MOJMAPS".into(),
                        InstallProfileData {
                            client: "[test:mappings:1]".into(),
                            ..Default::default()
                        },
                    ),
                ]),
                processors: vec![
                    ProcessorsValue {
                        jar: "test:processor:1".into(),
                        classpath: vec!["test:helper:1".into()],
                        args: vec!["{INPUT}".into(), "{SIDE}".into(), "{MINECRAFT_JAR}".into()],
                        ..Default::default()
                    },
                    ProcessorsValue {
                        jar: "test:processor:1".into(),
                        args: vec!["server-only".into()],
                        sides: Some(vec!["server".into()]),
                        ..Default::default()
                    },
                    ProcessorsValue {
                        jar: "test:processor:1".into(),
                        args: vec!["DOWNLOAD_MOJMAPS".into()],
                        sides: Some(vec!["client".into()]),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            let version = McClientInfo {
                main_class: Some("loader.Main".into()),
                ..Default::default()
            };
            archive(
                &request.dest,
                &[
                    (
                        "install_profile.json",
                        serde_json::to_string(&profile).unwrap(),
                    ),
                    ("version.json", serde_json::to_string(&version).unwrap()),
                    ("maven/test/embedded/1/embedded-1.jar", "embedded".into()),
                    ("data/client.lzma", "patch".into()),
                ],
            );
            let mut client = McClientInfo {
                downloads: HashMap::from([(
                    "client_mappings".into(),
                    crate::minecraft::profile::DownloadsArtifact {
                        url: "https://example.test/mappings".into(),
                        sha1: "sha1".into(),
                        ..Default::default()
                    },
                )]),
                patches: vec![McClientInfo {
                    id: "game".into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let downloads = prepare_libraries(
                &[SourceType::Official],
                &target,
                &mut client,
                &RulePlatform::default(),
            )
            .await
            .unwrap();
            assert_eq!(downloads.len(), 1);
            assert!(
                downloads[0]
                    .dest
                    .ends_with("test/mappings/1/mappings-1.jar")
            );
            let processed: InstallProfile = serde_json::from_slice(
                &fs::read(version_dir.join("install_profile.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(processed.processors.len(), 1);
            assert_eq!(processed.processors[0].args[1], "client");
            assert!(processed.processors[0].args[0].ends_with("test/input/1/input-1.jar"));
            assert_eq!(client.main_class.as_deref(), Some("loader.Main"));
            assert_eq!(
                fs::read(libraries.join("test/embedded/1/embedded-1.jar")).unwrap(),
                b"embedded"
            );
        }
    }
    #[tokio::test]
    async fn rejects_empty_sources_and_unsafe_embedded_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            installer_download(&[], ModLoaderType::Forge, "1.20.4", "1.0", None, dir.path())
                .is_err()
        );
        let request = installer_download(
            &[SourceType::Official],
            ModLoaderType::Forge,
            "1.20.4",
            "1.0",
            None,
            dir.path(),
        )
        .unwrap();
        archive(&request.dest, &[("../../outside", "unexpected".into())]);
        let target = InstallTarget {
            loader: ModLoaderType::Forge,
            loader_version: "1.0",
            name: "example",
            version_dir: dir.path(),
            root_dir: dir.path(),
            library_dir: dir.path(),
        };
        assert!(
            prepare_libraries(
                &[SourceType::Official],
                &target,
                &mut McClientInfo::default(),
                &RulePlatform::default()
            )
            .await
            .is_err()
        );
    }
}
