use serde::{Deserialize, Serialize};
use std::path::Path;

/// Metadata for a Minecraft mod (Fabric/Forge/NeoForge).
///
/// Parses `fabric.mod.json` and `mods.toml` / `mcmod.info` files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub license: String,
    pub dependencies: Vec<ModDependency>,
    pub mod_loader: String,
    pub mc_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModDependency {
    pub mod_id: String,
    pub version_range: String,
    pub mandatory: bool,
}

/// Parses mod metadata from a mod jar file.
pub async fn parse_mod_metadata(jar_path: &Path) -> Result<Option<ModMetadata>, ModMetaError> {
    let bytes = match tokio::fs::read(jar_path).await {
        Ok(b) => b,
        Err(e) => return Err(ModMetaError::Io(e.to_string())),
    };

    let reader = std::io::Cursor::new(bytes);
    let mut archive = match zip::ZipArchive::new(reader) {
        Ok(a) => a,
        Err(e) => return Err(ModMetaError::Zip(e.to_string())),
    };

    // Try fabric.mod.json first
    if let Ok(mut entry) = archive.by_name("fabric.mod.json") {
        let mut content = String::new();
        use std::io::Read;
        if entry.read_to_string(&mut content).is_ok()
            && let Ok(meta) = parse_fabric_mod_json(&content)
        {
            return Ok(Some(meta));
        }
    }

    // Try Forge mods.toml
    if let Ok(mut entry) = archive.by_name("META-INF/mods.toml") {
        let mut content = String::new();
        use std::io::Read;
        if entry.read_to_string(&mut content).is_ok()
            && let Ok(meta) = parse_forge_mods_toml(&content)
        {
            return Ok(Some(meta));
        }
    }

    // Try legacy mcmod.info
    if let Ok(mut entry) = archive.by_name("mcmod.info") {
        let mut content = String::new();
        use std::io::Read;
        if entry.read_to_string(&mut content).is_ok()
            && let Ok(meta) = parse_mcmod_info(&content)
        {
            return Ok(Some(meta));
        }
    }

    // Try Quilt quilt.mod.json
    if let Ok(mut entry) = archive.by_name("quilt.mod.json") {
        let mut content = String::new();
        use std::io::Read;
        if entry.read_to_string(&mut content).is_ok()
            && let Ok(meta) = parse_quilt_mod_json(&content)
        {
            return Ok(Some(meta));
        }
    }

    Ok(None)
}

fn parse_fabric_mod_json(content: &str) -> Result<ModMetadata, serde_json::Error> {
    let fabric: FabricModJson = serde_json::from_str(content)?;
    Ok(ModMetadata {
        id: fabric.id,
        name: fabric.name.unwrap_or_else(|| "Unknown".into()),
        version: fabric.version,
        description: fabric.description.unwrap_or_default(),
        authors: fabric.authors.unwrap_or_default(),
        license: fabric.license.unwrap_or_default(),
        dependencies: fabric
            .depends
            .into_iter()
            .map(|(mod_id, version_range)| ModDependency {
                mod_id,
                version_range,
                mandatory: true,
            })
            .collect(),
        mod_loader: "fabric".into(),
        mc_version: String::new(),
    })
}

fn parse_forge_mods_toml(content: &str) -> Result<ModMetadata, toml::de::Error> {
    let toml: ForgeModsToml = toml::from_str(content)?;
    let first = toml.mods.into_iter().next().unwrap_or_default();
    Ok(ModMetadata {
        id: first.mod_id,
        name: first.display_name.unwrap_or_else(|| "Unknown".into()),
        version: first.version.unwrap_or_else(|| "1.0".into()),
        description: first.description.unwrap_or_default(),
        authors: first
            .authors
            .map(|s| s.split(',').map(|a| a.trim().to_string()).collect())
            .unwrap_or_default(),
        license: toml.license.unwrap_or_default(),
        dependencies: Vec::new(),
        mod_loader: "forge".into(),
        mc_version: String::new(),
    })
}

fn parse_mcmod_info(content: &str) -> Result<ModMetadata, serde_json::Error> {
    let infos: Vec<McModInfo> = serde_json::from_str(content)?;
    let first = infos.into_iter().next().unwrap_or_default();
    Ok(ModMetadata {
        id: first.modid,
        name: first.name.unwrap_or_else(|| "Unknown".into()),
        version: first.version.unwrap_or_else(|| "1.0".into()),
        description: first.description.unwrap_or_default(),
        authors: first.author_list.unwrap_or_default(),
        license: first.mcversion.unwrap_or_default(),
        dependencies: Vec::new(),
        mod_loader: "forge".into(),
        mc_version: String::new(),
    })
}

fn parse_quilt_mod_json(content: &str) -> Result<ModMetadata, serde_json::Error> {
    let quilt: QuiltModJson = serde_json::from_str(content)?;
    let quilt_meta = quilt.quilt_loader;
    let metadata = quilt_meta.metadata;
    Ok(ModMetadata {
        id: quilt_meta.id,
        name: metadata.name.unwrap_or_else(|| "Unknown".into()),
        version: quilt_meta.version,
        description: metadata.description.unwrap_or_default(),
        authors: metadata.contributors.unwrap_or_default(),
        license: metadata.license.unwrap_or_default(),
        dependencies: quilt_meta
            .depends
            .into_iter()
            .map(|(mod_id, version_range)| ModDependency {
                mod_id,
                version_range,
                mandatory: true,
            })
            .collect(),
        mod_loader: "quilt".into(),
        mc_version: String::new(),
    })
}

#[derive(Debug, Clone, Deserialize)]
struct FabricModJson {
    id: String,
    version: String,
    name: Option<String>,
    description: Option<String>,
    authors: Option<Vec<String>>,
    license: Option<String>,
    #[serde(default)]
    depends: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ForgeModsToml {
    #[serde(default)]
    mods: Vec<ForgeModEntry>,
    license: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ForgeModEntry {
    #[serde(rename = "modId")]
    mod_id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    authors: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct McModInfo {
    modid: String,
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    mcversion: Option<String>,
    #[serde(rename = "authorList")]
    author_list: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuiltModJson {
    #[serde(rename = "schema_version")]
    _schema_version: i32,
    #[serde(rename = "quilt_loader")]
    quilt_loader: QuiltLoader,
}

#[derive(Debug, Clone, Deserialize)]
struct QuiltLoader {
    id: String,
    version: String,
    metadata: QuiltMetadata,
    #[serde(default)]
    depends: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct QuiltMetadata {
    name: Option<String>,
    description: Option<String>,
    contributors: Option<Vec<String>>,
    license: Option<String>,
}

use std::collections::HashMap;

/// Errors that can occur during mod metadata parsing.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ModMetaError {
    #[error("io error: {0}")]
    Io(String),
    #[error("zip error: {0}")]
    Zip(String),
    #[error("parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
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

        let meta = parse_fabric_mod_json(json).unwrap();
        assert_eq!(meta.id, "fabric-api");
        assert_eq!(meta.name, "Fabric API");
        assert_eq!(meta.mod_loader, "fabric");
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

        let meta = parse_forge_mods_toml(toml).unwrap();
        assert_eq!(meta.id, "examplemod");
        assert_eq!(meta.name, "Example Mod");
        assert_eq!(meta.mod_loader, "forge");
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

        let meta = parse_mcmod_info(json).unwrap();
        assert_eq!(meta.id, "jei");
        assert_eq!(meta.name, "Just Enough Items");
        assert_eq!(meta.mod_loader, "forge");
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

        let meta = parse_quilt_mod_json(json).unwrap();
        assert_eq!(meta.id, "quilted-fabric-api");
        assert_eq!(meta.name, "Quilted Fabric API");
        assert_eq!(meta.mod_loader, "quilt");
        assert_eq!(meta.dependencies.len(), 2);
    }

    #[test]
    fn test_mod_metadata_default() {
        let meta = ModMetadata::default();
        assert!(meta.id.is_empty());
        assert!(meta.dependencies.is_empty());
    }
}
