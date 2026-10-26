//! Maven coordinates, classpath selection and native extraction.
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::profile::{
    DownloadsArtifact, FeaturesInfo, InstructionRule, IsAllowed, LibrariesValue, McClientInfo,
    ProfileError, RulePlatform,
};
use semver::Version;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tokio::fs;
#[derive(Debug, thiserror::Error)]
pub enum DependencyError {
    #[error(transparent)]
    Hash(#[from] crate::util::hash::HashError),
    #[error(transparent)]
    Resource(#[from] crate::resource::ResourceError),
    #[error("missing library download: {0}")]
    MissingDownload(String),
    #[error("invalid Maven coordinate: {0}")]
    Coordinate(String),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}
pub type DependencyResult<T> = Result<T, DependencyError>;
#[must_use]
pub fn native_classifier(
    natives: &HashMap<String, String>,
    platform: &RulePlatform,
) -> Option<String> {
    let os = if platform.os == "macos" {
        "osx"
    } else {
        &platform.os
    };
    let arch = match platform.arch.as_str() {
        "x86" | "i386" | "i686" => "32",
        _ => "64",
    };
    natives
        .get(os)
        .map(|value| value.replace(concat!("${", "arch}"), arch))
}
pub fn get_nonnative_library_artifacts(
    client_info: &McClientInfo,
    platform: &RulePlatform,
) -> DependencyResult<Vec<DownloadsArtifact>> {
    let mut artifacts = HashSet::new();
    let feature = FeaturesInfo {
        platform: platform.clone(),
        ..Default::default()
    };
    for library in &client_info.libraries {
        if !library.is_allowed(&feature)? {
            continue;
        }
        if library.natives.is_some() {
            continue;
        }
        if let Some(downloads) = &library.downloads
            && let Some(artifact) = &downloads.artifact
        {
            artifacts.insert(DownloadsArtifact {
                path: convert_library_name_to_path(&library.name, None)?,
                ..artifact.clone()
            });
        }
    }
    Ok(artifacts.into_iter().collect())
}

pub fn get_native_library_artifacts(
    client_info: &McClientInfo,
    platform: &RulePlatform,
) -> DependencyResult<Vec<DownloadsArtifact>> {
    let mut artifacts = HashSet::new();
    let feature = FeaturesInfo {
        platform: platform.clone(),
        ..Default::default()
    };

    for library in &client_info.libraries {
        if !library.is_allowed(&feature)? {
            continue;
        }
        if let Some(natives) = &library.natives
            && let Some(native) = native_classifier(natives, platform)
            && let Some(downloads) = &library.downloads
            && let Some(classifiers) = &downloads.classifiers
            && let Some(artifact) = classifiers.get(&native)
        {
            artifacts.insert(DownloadsArtifact {
                path: convert_library_name_to_path(&library.name, Some(native))?,
                ..artifact.clone()
            });
        }
    }
    Ok(artifacts.into_iter().collect())
}

pub struct LibraryParts {
    pub path: String,
    pub pack_name: String,
    pub pack_version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

pub fn parse_library_name(name: &str, native: Option<String>) -> DependencyResult<LibraryParts> {
    let (coordinate, extension) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<&str> = coordinate.split(':').collect();
    let safe = |part: &str| {
        !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\\', ':', '@'])
    };
    if !(3..=4).contains(&parts.len())
        || !parts.iter().all(|part| safe(part))
        || !safe(extension)
        || parts[0].split('.').any(|part| !safe(part))
        || native.as_deref().is_some_and(|part| !safe(part))
    {
        return Err(DependencyError::Coordinate(name.to_owned()));
    }
    Ok(LibraryParts {
        path: parts[0].replace('.', "/"),
        pack_name: parts[1].into(),
        pack_version: parts[2].into(),
        classifier: native.or_else(|| parts.get(3).map(|part| (*part).to_owned())),
        extension: extension.into(),
    })
}

#[derive(Debug, Hash, Eq, PartialEq)]
struct LibraryKey {
    path: String,
    pack_name: String,
    classifier: Option<String>,
    extension: String,
    natives: Option<Vec<(String, String)>>,
    rules: Vec<InstructionRule>,
}

// Resolve versions within each artifact/platform selection, preserving native and rule variants.
pub fn merge_library_lists(
    libraries_a: &[LibrariesValue],
    libraries_b: &[LibrariesValue],
) -> Vec<LibrariesValue> {
    let mut positions: HashMap<LibraryKey, usize> = HashMap::new();
    let mut libraries: Vec<LibrariesValue> = Vec::new();
    for library in libraries_a.iter().chain(libraries_b) {
        if let Ok(parts) = parse_library_name(&library.name, None) {
            let key = LibraryKey {
                path: parts.path,
                pack_name: parts.pack_name,
                classifier: parts.classifier,
                extension: parts.extension,
                natives: library.natives.as_ref().map(|natives| {
                    let mut entries = natives
                        .iter()
                        .map(|(os, classifier)| (os.clone(), classifier.clone()))
                        .collect::<Vec<_>>();
                    entries.sort();
                    entries
                }),
                rules: library.rules.clone(),
            };
            if let Some(index) = positions.get(&key).copied() {
                let existing = parse_library_name(&libraries[index].name, None)
                    .map(|parts| parts.pack_version)
                    .unwrap_or_default();
                if parse_sem_version(&parts.pack_version) > parse_sem_version(&existing) {
                    libraries[index] = library.clone();
                }
            } else {
                positions.insert(key, libraries.len());
                libraries.push(library.clone());
            }
        }
    }
    libraries
}

fn parse_sem_version(version: &str) -> Version {
    Version::parse(version).unwrap_or({
        let mut parts = version.split('.').collect::<Vec<_>>();
        while parts.len() < 3 {
            parts.push("0");
        }
        Version::parse(&parts[..3].join(".")).unwrap_or(Version::new(0, 1, 0))
    })
}

pub fn convert_library_name_to_path(
    name: &str,
    native: Option<String>,
) -> DependencyResult<String> {
    let LibraryParts {
        path,
        pack_name,
        pack_version,
        classifier,
        extension: file_ext,
    } = parse_library_name(name, native)?;

    let file_name = [
        pack_name.clone(),
        pack_version.clone(),
        classifier.unwrap_or_default(),
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .map(|s| s.as_str())
    .collect::<Vec<&str>>()
    .join("-")
        + "."
        + &file_ext;
    Ok(format!("{path}/{pack_name}/{pack_version}/{file_name}"))
}

pub fn get_nonnative_library_paths(
    client_info: &McClientInfo,
    library_path: &Path,
    mod_loader_type: ModLoaderType,
    platform: &RulePlatform,
) -> DependencyResult<Vec<PathBuf>> {
    let mut libraries = Vec::new();
    let feature = FeaturesInfo {
        platform: platform.clone(),
        ..Default::default()
    };
    for library in &client_info.libraries {
        if !library.is_allowed(&feature)? {
            continue;
        }
        if library.natives.is_some() {
            continue;
        }

        if mod_loader_type == ModLoaderType::Cleanroom {
            // Cleanroom replaces legacy LWJGL 2 libraries, whose versions vary by platform.
            if library.name.starts_with("org.lwjgl.lwjgl:lwjgl:")
                || library.name.starts_with("org.lwjgl.lwjgl:lwjgl_util:")
                || matches!(
                    library.name.as_str(),
                    "net.java.dev.jna:platform:3.4.0" | "com.ibm.icu:icu4j-core-mojang:51.2"
                )
            {
                continue;
            }
        }

        let mut selected = library.clone();
        selected.rules.clear(); // Rules have already been evaluated for this platform.
        libraries.push(selected);
    }

    libraries = merge_library_lists(&libraries, &[]); // remove duplicates to prevent launch errors
    let mut result = Vec::new();
    for library in libraries {
        result.push(library_path.join(convert_library_name_to_path(&library.name, None)?));
    }
    Ok(result)
}

pub fn get_native_library_paths(
    client_info: &McClientInfo,
    library_path: &Path,
    use_native_glfw: bool,
    use_native_openal: bool,
    platform: &RulePlatform,
) -> DependencyResult<Vec<PathBuf>> {
    let mut result = Vec::new();
    let feature = FeaturesInfo {
        platform: platform.clone(),
        ..Default::default()
    };
    for library in &client_info.libraries {
        if !library.is_allowed(&feature)? {
            continue;
        }
        if let Some(natives) = &library.natives
            && let Some(native) = native_classifier(natives, platform)
        {
            let path = convert_library_name_to_path(&library.name, Some(native))?;
            if platform.os == "linux" {
                if use_native_glfw && library.name.to_lowercase().contains("glfw") {
                    continue;
                }
                if use_native_openal && library.name.to_lowercase().contains("openal") {
                    continue;
                }
            }
            result.push(library_path.join(path));
        }
    }
    Ok(result)
}

fn get_native_library_extract_dir(client_info: &McClientInfo, natives_dir: &Path) -> PathBuf {
    const JAVA_LIBRARY_PATH_PREFIX: &str = "-Djava.library.path=";
    const NATIVES_SUBDIR_PREFIX: &str = "${natives_directory}/";

    let Some(arguments) = &client_info.arguments else {
        return natives_dir.to_path_buf();
    };

    for argument in &arguments.jvm {
        if !argument.rules.is_empty() {
            continue;
        }

        for value in &argument.value {
            let Some(path) = value.strip_prefix(JAVA_LIBRARY_PATH_PREFIX) else {
                continue;
            };

            // Minecraft versions before 26.2-snapshot-1 use ${natives_directory} directly.
            if path == "${natives_directory}" {
                return natives_dir.to_path_buf();
            }

            // Minecraft 26.2-snapshot-1+ may use a subdirectory such as ${natives_directory}/java.
            let Some(relative_path) = path.strip_prefix(NATIVES_SUBDIR_PREFIX) else {
                return natives_dir.to_path_buf();
            };
            let relative_path = Path::new(relative_path);

            if relative_path.as_os_str().is_empty()
                || !relative_path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                return natives_dir.to_path_buf();
            }

            return natives_dir.join(relative_path);
        }
    }

    natives_dir.to_path_buf()
}

pub async fn extract_native_libraries(
    client_info: &McClientInfo,
    library_path: &Path,
    natives_dir: &Path,
    use_native_glfw: bool,
    use_native_openal: bool,
    platform: &RulePlatform,
) -> DependencyResult<()> {
    if platform.os == "linux" && (use_native_glfw || use_native_openal) && natives_dir.exists() {
        fs::remove_dir_all(natives_dir).await?;
    }
    let extract_dir = get_native_library_extract_dir(client_info, natives_dir);
    fs::create_dir_all(&extract_dir).await?;
    let libraries = get_native_library_paths(
        client_info,
        library_path,
        use_native_glfw,
        use_native_openal,
        platform,
    )?;
    tokio::task::spawn_blocking(move || {
        for path in libraries {
            let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
            archive.extract(&extract_dir)?;
        }
        Ok(())
    })
    .await?
}

/// Validate library files and plan downloads using the same artifact/rule evaluation as launch.
pub async fn download_plan(
    source: crate::resource::SourceType,
    directory: &Path,
    profile: &McClientInfo,
    platform: &RulePlatform,
    check_hash: bool,
) -> DependencyResult<Vec<crate::download::request::DownloadParam>> {
    let mut artifacts = get_native_library_artifacts(profile, platform)?;
    artifacts.extend(get_nonnative_library_artifacts(profile, platform)?);
    let mut downloads = Vec::new();
    for artifact in artifacts {
        let relative = Path::new(&artifact.path);
        if artifact.path.is_empty()
            || artifact.path.contains('\\')
            || artifact.path.contains(':')
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(DependencyError::Coordinate(artifact.path));
        }
        let destination = directory.join(relative);
        if fs::try_exists(&destination).await?
            && (!check_hash || crate::util::hash::sha1_file(&destination).await? == artifact.sha1)
        {
            continue;
        }
        if artifact.url.is_empty() {
            return Err(DependencyError::MissingDownload(artifact.path));
        }
        let url = url::Url::parse(&artifact.url).map_err(crate::resource::ResourceError::from)?;
        let src = crate::resource::convert_url_to_target_source(&url, &[], &source)?;
        downloads.push(crate::download::request::DownloadParam {
            src,
            dest: destination,
            filename: None,
            sha1: Some(artifact.sha1),
        });
    }
    Ok(downloads)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_artifact_native_and_platform_rows_through_patch_composition() {
        let rows: Vec<LibrariesValue> = serde_json::from_value(serde_json::json!([
          {"name":"test:bridge:1","downloads":{"artifact":{"path":"test/bridge/1/bridge-1.jar","url":"https://example.test/java.jar","sha1":"java","size":1}}},
          {"name":"test:bridge:1","natives":{"linux":"natives-linux"},"rules":[{"action":"allow","os":{"name":"linux","arch":"x64"}}],"downloads":{"classifiers":{"natives-linux":{"path":"test/bridge/1/bridge-1-natives-linux.jar","url":"https://example.test/linux.jar","sha1":"linux","size":1}}}},
          {"name":"test:bridge:1","natives":{"windows":"natives-windows"},"rules":[{"action":"allow","os":{"name":"windows"}}],"downloads":{"classifiers":{"natives-windows":{"path":"test/bridge/1/bridge-1-natives-windows.jar","url":"https://example.test/windows.jar","sha1":"windows","size":1}}}}
        ])).unwrap();
        let merged = merge_library_lists(&rows, &rows);
        assert_eq!(merged.len(), 3);
        let mut profile = McClientInfo {
            patches: vec![McClientInfo {
                libraries: rows,
                ..Default::default()
            }],
            ..Default::default()
        };
        crate::minecraft::profile::reset_fields_from_patches(&mut profile);
        assert_eq!(profile.libraries, merged);
        let platform = RulePlatform {
            os: "linux".into(),
            arch: "x64".into(),
            version: String::new(),
        };
        assert_eq!(
            get_nonnative_library_artifacts(&profile, &platform)
                .unwrap()
                .len(),
            1
        );
        let natives = get_native_library_artifacts(&profile, &platform).unwrap();
        assert_eq!(natives.len(), 1);
        assert!(natives[0].path.ends_with("natives-linux.jar"));
        let mut higher = profile.libraries[0].clone();
        higher.name = "test:bridge:2".into();
        let updated = merge_library_lists(&profile.libraries, &[higher]);
        assert_eq!(updated.len(), 3);
        assert_eq!(updated[0].name, "test:bridge:2");
        assert_eq!(
            updated.iter().filter(|row| row.natives.is_some()).count(),
            2
        );
    }
    #[test]
    fn maven_classifier_extension_and_traversal() {
        assert_eq!(
            convert_library_name_to_path("net.minecraftforge:forge:1.0:clientdata@lzma", None)
                .unwrap(),
            "net/minecraftforge/forge/1.0/forge-1.0-clientdata.lzma"
        );
        for invalid in [
            "../test:lib:1",
            "test:../lib:1",
            "test:lib:..",
            "test:lib:1@../zip",
            "test:lib:1:extra:bad",
        ] {
            assert!(
                convert_library_name_to_path(invalid, None).is_err(),
                "{invalid}"
            );
        }
        let natives = HashMap::from([("windows".into(), "natives-windows-${arch}".into())]);
        assert_eq!(
            native_classifier(
                &natives,
                &RulePlatform {
                    os: "windows".into(),
                    arch: "x86".into(),
                    version: String::new()
                }
            ),
            Some("natives-windows-32".into())
        );
    }
    #[tokio::test]
    async fn damaged_native_archive_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test/native/1/native-1-natives-linux.jar");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"not a zip").unwrap();
        let profile = McClientInfo {
            libraries: vec![LibrariesValue {
                name: "test:native:1".into(),
                natives: Some(HashMap::from([("linux".into(), "natives-linux".into())])),
                ..Default::default()
            }],
            ..Default::default()
        };
        let platform = RulePlatform {
            os: "linux".into(),
            arch: "x64".into(),
            version: String::new(),
        };
        assert!(
            extract_native_libraries(
                &profile,
                dir.path(),
                &dir.path().join("natives"),
                false,
                false,
                &platform
            )
            .await
            .is_err()
        );
    }
}
