//! Local mod metadata and loader interoperability; rendering belongs to the caller.
use crate::minecraft::loaders::ModLoaderType;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error("invalid mod metadata: {0}")]
    Invalid(String),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}
type Result<T> = std::result::Result<T, MetadataError>;
#[derive(Debug, Clone, Default)]
pub struct ModMetadata {
    pub mod_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub loader_type: ModLoaderType,
    pub file_name: String,
    pub file_path: PathBuf,
    pub enabled: bool,
    pub potential_incompatibility: bool,
    pub icon: Vec<u8>,
    pub authors: Vec<String>,
    pub license: String,
    pub dependencies: Vec<ModDependency>,
    pub mc_version: String,
}
#[derive(Debug, Clone, Default)]
pub struct ModDependency {
    pub mod_id: String,
    pub version_range: String,
    pub mandatory: bool,
}
fn metadata_names(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values
            .iter()
            .filter_map(|value| {
                value
                    .as_str()
                    .or_else(|| value.get("name").and_then(Value::as_str))
            })
            .map(str::to_owned)
            .collect(),
        Value::Object(values) => values.keys().cloned().collect(),
        _ => Vec::new(),
    }
}
fn requirements(value: &Value) -> Vec<ModDependency> {
    match value {
        Value::Object(entries) => entries
            .iter()
            .map(|(id, versions)| ModDependency {
                mod_id: id.clone(),
                version_range: versions
                    .as_str()
                    .map_or_else(|| versions.to_string(), str::to_owned),
                mandatory: true,
            })
            .collect(),
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                let id = entry.get("id")?.as_str()?;
                Some(ModDependency {
                    mod_id: id.into(),
                    version_range: entry.get("versions").map_or_else(String::new, |value| {
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned)
                    }),
                    mandatory: !entry
                        .get("optional")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}
fn is_mod_filename(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let name = name.trim_end_matches(".disabled");
    [".jar", ".zip", ".litemod"]
        .iter()
        .any(|ext| name.ends_with(ext))
}
fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
/// Read the main manifest section, including continuation lines.
pub fn manifest_attributes(input: &str) -> HashMap<String, String> {
    let mut values = HashMap::<String, String>::new();
    let mut key = String::new();
    for line in input.lines() {
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix(' ') {
            if let Some(value) = values.get_mut(&key) {
                value.push_str(rest);
            }
        } else if let Some((name, value)) = line.split_once(": ") {
            key = name.to_owned();
            values.insert(key.clone(), value.to_owned());
        }
    }
    values
}
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn decode_json(bytes: &[u8]) -> Result<Value> {
    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(_) => {
            let input = String::from_utf8_lossy(bytes);
            let cleaned: String = input
                .chars()
                .filter(|c| !matches!(c, '\u{0000}'..='\u{001F}'))
                .collect();
            Ok(serde_json::from_str(&cleaned)?)
        }
    }
}
fn parse_metadata<F>(loader: ModLoaderType, read: &mut F) -> Result<Option<ModMetadata>>
where
    F: FnMut(&str) -> Result<Option<Vec<u8>>>,
{
    let names: &[&str] = match loader {
        ModLoaderType::Fabric => &["fabric.mod.json"],
        ModLoaderType::Forge => &["META-INF/mods.toml", "META-INF/neoforge.mods.toml"],
        ModLoaderType::LegacyForge => &["mcmod.info"],
        ModLoaderType::LiteLoader => &["litemod.json"],
        ModLoaderType::Quilt => &["quilt.mod.json"],
        _ => &["META-INF/MANIFEST.MF"],
    };
    let mut source = None;
    for name in names {
        if let Some(bytes) = read(name)? {
            source = Some((*name, bytes));
            break;
        }
    }
    let Some((name, bytes)) = source else {
        return Ok(None);
    };
    let mut info = ModMetadata::default();
    let mut icons = Vec::new();
    if name == "META-INF/MANIFEST.MF" {
        let attrs = manifest_attributes(&String::from_utf8_lossy(&bytes));
        let pick = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| attrs.get(*key))
                .cloned()
                .unwrap_or_default()
        };
        info.name = pick(&["Implementation-Title", "Specification-Title", "Bundle-Name"]);
        info.version = pick(&["Implementation-Version", "Specification-Version"]);
        info.description = pick(&["Implementation-Vendor", "Specification-Vendor"]);
    } else if loader == ModLoaderType::Forge {
        let contents = std::str::from_utf8(&bytes)
            .map_err(|error| MetadataError::Invalid(error.to_string()))?;
        let meta: toml::Value = toml::from_str(contents)?;
        let get = |value: &toml::Value, key: &str| {
            value
                .get(key)
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let mods = meta
            .get("mods")
            .and_then(toml::Value::as_array)
            .filter(|mods| !mods.is_empty())
            .ok_or_else(|| MetadataError::Invalid("empty Forge mods list".into()))?;
        let first = &mods[0];
        info.mod_id = get(first, "modId");
        info.name = get(first, "displayName");
        info.version = get(first, "version");
        info.description = get(first, "description");
        info.license = get(&meta, "license");
        info.authors = get(first, "authors")
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        info.loader_type = if name == "META-INF/mods.toml" {
            ModLoaderType::Forge
        } else {
            ModLoaderType::NeoForge
        };
        icons.push(get(&meta, "logoFile"));
        icons.extend(mods.iter().map(|item| get(item, "logoFile")));
        if info.version == "${file.jarVersion}"
            && let Some(bytes) = read("META-INF/MANIFEST.MF")?
        {
            let attrs = manifest_attributes(&String::from_utf8_lossy(&bytes));
            if let Some(version) = attrs.get("Implementation-Version") {
                info.version.clone_from(version);
            }
        }
    } else {
        let value = decode_json(&bytes)?;
        let value = match loader {
            ModLoaderType::Quilt => value
                .get("quilt_loader")
                .ok_or_else(|| MetadataError::Invalid("missing quilt_loader".into()))?,
            ModLoaderType::LegacyForge => value
                .as_array()
                .and_then(|items| items.first())
                .ok_or_else(|| MetadataError::Invalid("empty mcmod.info".into()))?,
            _ => &value,
        };
        info.mod_id = text(
            value,
            if loader == ModLoaderType::LegacyForge {
                "modid"
            } else {
                "id"
            },
        );
        info.version = text(value, "version");
        let details = if loader == ModLoaderType::Quilt {
            value.get("metadata").unwrap_or(value)
        } else {
            value
        };
        info.name = text(details, "name");
        info.description = text(details, "description");
        info.authors = metadata_names(
            &details[match loader {
                ModLoaderType::Quilt => "contributors",
                ModLoaderType::LegacyForge => "authorList",
                _ => "authors",
            }],
        );
        info.license = metadata_names(&details["license"]).join(", ");
        info.mc_version = text(value, "mcversion");
        info.dependencies = requirements(&value["depends"]);
        icons.push(text(
            details,
            if loader == ModLoaderType::LegacyForge {
                "logoFile"
            } else {
                "icon"
            },
        ));
        info.loader_type = if loader == ModLoaderType::LegacyForge {
            ModLoaderType::Forge
        } else {
            loader
        };
    }
    for icon in icons {
        if safe_relative(&icon)
            && let Some(bytes) = read(&icon)?
        {
            info.icon = bytes;
            break;
        }
    }
    Ok(Some(info))
}
fn read_metadata<F>(
    path: &Path,
    preferred: Option<ModLoaderType>,
    mut read: F,
) -> Result<ModMetadata>
where
    F: FnMut(&str) -> Result<Option<Vec<u8>>>,
{
    let mut last_error = None;
    for loader in mod_metadata_parser_priority(preferred) {
        match parse_metadata(loader, &mut read) {
            Ok(Some(mut info)) => {
                let filename = path
                    .file_name()
                    .ok_or_else(|| MetadataError::Invalid(path.display().to_string()))?
                    .to_string_lossy();
                info.enabled = !filename.ends_with(".disabled");
                let filename = filename.strip_suffix(".disabled").unwrap_or(&filename);
                info.file_name = if path.is_dir() {
                    filename.to_owned()
                } else {
                    Path::new(filename)
                        .file_stem()
                        .map_or_else(|| filename.to_owned(), |s| s.to_string_lossy().into_owned())
                };
                info.file_path = path.to_owned();
                return Ok(info);
            }
            Ok(None) => (),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| MetadataError::Invalid(format!("unrecognized mod: {}", path.display()))))
}
pub async fn get_mod_info_from_jar(
    path: &Path,
    preferred: Option<ModLoaderType>,
) -> Result<ModMetadata> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut jar = zip::ZipArchive::new(std::fs::File::open(&path)?)?;
        read_metadata(&path, preferred, |name| match jar.by_name(name) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                Ok(Some(bytes))
            }
            Err(zip::result::ZipError::FileNotFound) => Ok(None),
            Err(error) => Err(error.into()),
        })
    })
    .await?
}
pub async fn get_mod_info_from_dir(
    path: &Path,
    preferred: Option<ModLoaderType>,
) -> Result<ModMetadata> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let root = std::fs::canonicalize(&path)?;
        read_metadata(&path, preferred, |name| {
            if !safe_relative(name) {
                return Err(MetadataError::Invalid(name.into()));
            }
            let file = match std::fs::canonicalize(root.join(name)) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if !file.starts_with(&root) {
                return Err(MetadataError::Invalid(name.into()));
            }
            Ok(Some(std::fs::read(file)?))
        })
    })
    .await?
}
pub fn discover_local_mod_paths(
    mods_dir: &Path,
    installed_loader_type: Option<ModLoaderType>,
) -> Vec<PathBuf> {
    let supports_mod_subdirectories = matches!(
        installed_loader_type,
        Some(
            ModLoaderType::Forge
                | ModLoaderType::LegacyForge
                | ModLoaderType::Cleanroom
                | ModLoaderType::LiteLoader
                | ModLoaderType::Quilt
        )
    );
    let max_depth = usize::from(supports_mod_subdirectories) + 1;

    WalkDir::new(mods_dir)
        .min_depth(1)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(".connector")
        })
        .filter_map(|entry| match entry {
            Ok(entry)
                if (entry.file_type().is_file()
                    || (entry.file_type().is_symlink() && entry.path().is_file()))
                    && entry.file_name().to_str().is_some_and(is_mod_filename) =>
            {
                Some(entry.into_path())
            }
            Ok(_) => None,
            Err(error) => {
                tracing::warn!("Skipping unreadable mod entry: {}", error);
                None
            }
        })
        .collect()
}

const DEFAULT_MOD_LOADER_PRIORITY_LIST: [ModLoaderType; 6] = [
    ModLoaderType::Fabric,
    ModLoaderType::Forge,
    ModLoaderType::LegacyForge,
    ModLoaderType::LiteLoader,
    ModLoaderType::Quilt,
    ModLoaderType::Unknown,
];
fn mod_metadata_parser_priority(
    prior_loader_type: Option<ModLoaderType>,
) -> impl Iterator<Item = ModLoaderType> {
    // Normalize loaders that share a parser before filtering the fallback list.
    let preferred_loader = prior_loader_type.map(|loader| match loader {
        ModLoaderType::NeoForge => ModLoaderType::Forge,
        ModLoaderType::Cleanroom => ModLoaderType::LegacyForge,
        loader => loader,
    });
    preferred_loader.into_iter().chain(
        DEFAULT_MOD_LOADER_PRIORITY_LIST
            .into_iter()
            .filter(move |loader| Some(*loader) != preferred_loader),
    )
}

// update the potential_incompatibility field of ModMetadata.
pub fn check_potential_incompatibility(
    mod_infos: &mut [ModMetadata],
    installed_loader_type: Option<ModLoaderType>,
    game_version: &str,
) {
    let Some(installed_loader_type) = installed_loader_type else {
        return;
    };

    let mut supported_loaders = vec![installed_loader_type];

    if installed_loader_type == ModLoaderType::Quilt {
        supported_loaders.push(ModLoaderType::Fabric);
    }
    if installed_loader_type == ModLoaderType::Cleanroom {
        supported_loaders.push(ModLoaderType::Forge);
    }
    if installed_loader_type == ModLoaderType::NeoForge && game_version == "1.20.1" {
        supported_loaders.push(ModLoaderType::Forge);
    }

    let contains_mod = |mod_id: &str, loader_type: ModLoaderType| -> bool {
        mod_infos
            .iter()
            .any(|m| m.loader_type == loader_type && m.mod_id == mod_id)
    };

    // Sinytra Connector
    if installed_loader_type == ModLoaderType::NeoForge
        && (contains_mod("connector", ModLoaderType::NeoForge)
            || contains_mod("connectormod", ModLoaderType::NeoForge))
    {
        supported_loaders.push(ModLoaderType::Fabric);
    }
    if installed_loader_type == ModLoaderType::Forge
        && game_version == "1.20.1"
        && contains_mod("connectormod", ModLoaderType::Forge)
    {
        supported_loaders.push(ModLoaderType::Fabric);
    }

    // Kilt
    if installed_loader_type == ModLoaderType::Fabric && contains_mod("kilt", ModLoaderType::Fabric)
    {
        supported_loaders.push(ModLoaderType::Forge);
        supported_loaders.push(ModLoaderType::NeoForge);
    }

    mod_infos.iter_mut().for_each(|mod_info| {
        mod_info.potential_incompatibility = !supported_loaders.contains(&mod_info.loader_type);
    });
}

pub async fn remove_fabric_api_mods(mods_dir: &Path) -> Result<()> {
    let mut entries = match tokio::fs::read_dir(mods_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if entry.file_type().await?.is_file()
            && name.ends_with(".jar")
            && (name.starts_with("fabric-api-") || name.starts_with("quilted-fabric-api-"))
        {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

/// Toggle a mod without accumulating disabled suffixes or replacing another file.
pub fn set_enabled(path: &Path, enabled: bool) -> Result<PathBuf> {
    static RENAME_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _guard = RENAME_LOCK.lock();
    if !path.is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MetadataError::Invalid("invalid mod filename".into()))?;
    let bare = name.trim_end_matches(".disabled");
    let target = path.with_file_name(if enabled {
        bare.to_owned()
    } else {
        format!("{bare}.disabled")
    });
    if target != path {
        if target.exists() {
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
        }
        std::fs::rename(path, &target)?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn jar(path: &Path, entries: &[(&str, &str)]) {
        let mut jar = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, content) in entries {
            jar.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            jar.write_all(content.as_bytes()).unwrap();
        }
        jar.finish().unwrap();
    }
    #[tokio::test]
    async fn reads_fabric_and_quilt_from_archives_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fabric.jar.disabled");
        jar(
            &path,
            &[
                (
                    "fabric.mod.json",
                    r#"{"id":"test","version":"2","name":"Fabric test","icon":"icon.png"}"#,
                ),
                ("icon.png", "icon"),
            ],
        );
        let info = get_mod_info_from_jar(&path, Some(ModLoaderType::Quilt))
            .await
            .unwrap();
        assert_eq!(info.loader_type, ModLoaderType::Fabric);
        assert_eq!(info.mod_id, "test");
        assert_eq!(info.icon, b"icon");
        assert!(!info.enabled);
        assert_eq!(info.file_name, "fabric");
        std::fs::write(dir.path().join("quilt.mod.json"), r#"{"schema_version":1,"quilt_loader":{"id":"quilt-test","version":"3","metadata":{"name":"Quilt test"}}}"#).unwrap();
        let info = get_mod_info_from_dir(dir.path(), None).await.unwrap();
        assert_eq!(info.loader_type, ModLoaderType::Quilt);
        assert_eq!(info.name, "Quilt test");
        assert_eq!(info.mod_id, "quilt-test");
    }
    #[tokio::test]
    async fn forge_reads_manifest_version_and_continuations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forge.jar");
        jar(
            &path,
            &[
                (
                    "META-INF/neoforge.mods.toml",
                    "[[mods]]\nmodId = 'example'\ndisplayName = 'Example'\nversion = '${file.jarVersion}'\n",
                ),
                (
                    "META-INF/MANIFEST.MF",
                    "Manifest-Version: 1.0\r\nImplementation-Version: 1.2.\r\n 3\r\n\r\nName: other\r\nImplementation-Version: ignored\r\n",
                ),
            ],
        );
        let info = get_mod_info_from_jar(&path, None).await.unwrap();
        assert_eq!(info.loader_type, ModLoaderType::NeoForge);
        assert_eq!(info.version, "1.2.3");
    }
    #[tokio::test]
    async fn malformed_metadata_is_reported_and_icon_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fabric.mod.json"), "invalid").unwrap();
        assert!(get_mod_info_from_dir(dir.path(), None).await.is_err());
        std::fs::write(
            dir.path().join("fabric.mod.json"),
            r#"{"id":"test","version":"1","icon":"../outside"}"#,
        )
        .unwrap();
        assert!(
            get_mod_info_from_dir(dir.path(), None)
                .await
                .unwrap()
                .icon
                .is_empty()
        );
    }
    #[test]
    fn connector_and_cleanroom_interoperability() {
        let mut mods = vec![
            ModMetadata {
                mod_id: "connectormod".into(),
                loader_type: ModLoaderType::Forge,
                ..Default::default()
            },
            ModMetadata {
                loader_type: ModLoaderType::Fabric,
                ..Default::default()
            },
        ];
        check_potential_incompatibility(&mut mods, Some(ModLoaderType::Forge), "1.20.1");
        assert!(!mods[1].potential_incompatibility);
        check_potential_incompatibility(&mut mods, Some(ModLoaderType::Forge), "1.20.2");
        assert!(mods[1].potential_incompatibility);
        check_potential_incompatibility(&mut mods, Some(ModLoaderType::Cleanroom), "1.12.2");
        assert!(!mods[0].potential_incompatibility);
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn test_parse_fabric_mod_json() {
        let json = r#"{
            "id": "fabric-api",
            "version": "0.91.3+1.20.4",
            "name": "Fabric API",
            "description": "Core API module providing key hooks and inter-compatibility features.",
            "authors": ["FabricMC"],
            "license": "Apache-2.0",
            "depends": {
                "minecraft": ">=1.20.4",
                "fabricloader": ">=0.15.0"
            }
        }"#;

        let meta = parse_metadata(ModLoaderType::Fabric, &mut |path| {
            Ok((path == "fabric.mod.json").then(|| json.as_bytes().to_vec()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(meta.mod_id, "fabric-api");
        assert_eq!(meta.name, "Fabric API");
        assert_eq!(meta.loader_type, ModLoaderType::Fabric);
        assert_eq!(meta.dependencies.len(), 2);
    }

    #[test]
    fn test_parse_forge_mods_toml() {
        let toml = r#"
license = "MIT"
[[mods]]
modId = "examplemod"
displayName = "Example Mod"
version = "1.0.0"
description = "An example mod"
authors = "Author1, Author2"
"#;

        let meta = parse_metadata(ModLoaderType::Forge, &mut |path| {
            Ok((path == "META-INF/mods.toml").then(|| toml.as_bytes().to_vec()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(meta.mod_id, "examplemod");
        assert_eq!(meta.name, "Example Mod");
        assert_eq!(meta.loader_type, ModLoaderType::Forge);
    }

    #[test]
    fn test_parse_mcmod_info() {
        let json = r#"[{
            "modid": "jei",
            "name": "Just Enough Items",
            "description": "JEI is an item and recipe viewing mod.",
            "version": "15.2.0.27",
            "mcversion": "1.20.1",
            "authorList": ["mezz"]
        }]"#;

        let meta = parse_metadata(ModLoaderType::LegacyForge, &mut |path| {
            Ok((path == "mcmod.info").then(|| json.as_bytes().to_vec()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(meta.mod_id, "jei");
        assert_eq!(meta.name, "Just Enough Items");
        assert_eq!(meta.loader_type, ModLoaderType::Forge);
    }

    #[test]
    fn test_parse_quilt_mod_json() {
        let json = r#"{
            "schema_version": 1,
            "quilt_loader": {
                "id": "quilted-fabric-api",
                "version": "7.5.0+0.91.3-1.20.4",
                "metadata": {
                    "name": "Quilted Fabric API",
                    "description": "Quilt's replacement for Fabric API.",
                    "contributors": ["QuiltMC"],
                    "license": "Apache-2.0"
                },
                "depends": {
                    "quilt_loader": ">=0.23.0",
                    "minecraft": ">=1.20.4"
                }
            }
        }"#;

        let meta = parse_metadata(ModLoaderType::Quilt, &mut |path| {
            Ok((path == "quilt.mod.json").then(|| json.as_bytes().to_vec()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(meta.mod_id, "quilted-fabric-api");
        assert_eq!(meta.name, "Quilted Fabric API");
        assert_eq!(meta.loader_type, ModLoaderType::Quilt);
        assert_eq!(meta.dependencies.len(), 2);
    }

    #[test]
    fn test_mod_metadata_default() {
        let meta = ModMetadata::default();
        assert!(meta.mod_id.is_empty());
        assert!(meta.dependencies.is_empty());
    }
}
