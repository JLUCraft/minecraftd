use crate::minecraft::loaders::ModLoaderType;
use crate::modpack::{ModpackError, ModpackManifest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MultiMcCacheRequires {
    pub uid: String,
    pub equals: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MultiMcComponent {
    pub cached_name: Option<String>,
    pub cached_requires: Option<Vec<MultiMcCacheRequires>>,
    pub cached_version: Option<String>,
    pub cached_volatile: Option<bool>,
    pub important: Option<bool>,
    pub dependency_only: Option<bool>,
    pub uid: String,
    pub version: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MultiMcManifest {
    pub components: Vec<MultiMcComponent>,
    pub format_version: u64,
    #[serde(skip)]
    pub cfg: HashMap<String, String>,
    #[serde(skip)]
    pub base_path: String,
}
impl MultiMcManifest {
    pub fn read(file: &File) -> Result<Self, ModpackError> {
        let mut archive =
            ZipArchive::new(file).map_err(|error| ModpackError::Parse(error.to_string()))?;

        let base_path = if archive.by_name("mmc-pack.json").is_ok() {
            String::new()
        } else {
            let mut found_path = None;
            for i in 0..archive.len() {
                let file = archive
                    .by_index(i)
                    .map_err(|error| ModpackError::Parse(error.to_string()))?;
                let name = file.name();

                if name.ends_with("mmc-pack.json")
                    && let Some(last_slash) = name.rfind('/')
                {
                    let dir_path = &name[..=last_slash];
                    let depth = name[..last_slash].matches('/').count();
                    if depth <= 1 {
                        found_path = Some(dir_path.to_string());
                        break;
                    }
                }
            }
            found_path.ok_or(ModpackError::UnknownFormat)?
        };

        let mut manifest: Self;
        {
            let manifest_path = format!("{}mmc-pack.json", base_path);
            let mut manifest_file = archive
                .by_name(&manifest_path)
                .map_err(|error| ModpackError::Parse(error.to_string()))?;
            let mut manifest_content = String::new();
            manifest_file.read_to_string(&mut manifest_content)?;
            manifest = serde_json::from_str(&manifest_content)?;
        }

        let cfg_path = format!("{}instance.cfg", base_path);
        let mut cfg_file = archive
            .by_name(&cfg_path)
            .map_err(|error| ModpackError::Parse(error.to_string()))?;
        let mut cfg_str = String::new();
        cfg_file.read_to_string(&mut cfg_str)?;

        manifest.base_path = base_path;
        manifest.cfg = cfg_str
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.starts_with(['#', ';', '[']) {
                    return None;
                }
                let (key, value) = line.split_once('=')?;
                Some((key.trim().to_owned(), value.trim().to_owned()))
            })
            .collect();

        Ok(manifest)
    }
}
#[async_trait::async_trait]
impl ModpackManifest for MultiMcManifest {
    fn from_archive(path: &Path) -> Result<Self, ModpackError> {
        Self::read(&File::open(path)?)
    }
    fn client_version(&self) -> Result<String, ModpackError> {
        let component = self
            .components
            .iter()
            .find(|component| component.uid == "net.minecraft")
            .ok_or(ModpackError::UnknownFormat)?;

        get_version(component)
    }

    fn mod_loader(&self) -> Result<Option<(ModLoaderType, String)>, ModpackError> {
        for component in &self.components {
            match component.uid.as_str() {
                "net.minecraft" => continue,
                "net.minecraftforge" => {
                    return Ok(Some((ModLoaderType::Forge, get_version(component)?)));
                }
                "net.fabricmc.fabric-loader" => {
                    return Ok(Some((ModLoaderType::Fabric, get_version(component)?)));
                }
                "net.neoforged" => {
                    return Ok(Some((ModLoaderType::NeoForge, get_version(component)?)));
                }
                "org.quiltmc.quilt-loader" => {
                    return Ok(Some((ModLoaderType::Quilt, get_version(component)?)));
                }
                _ => continue,
            }
        }
        Ok(None)
    }

    async fn download_tasks(
        &self,
        _instance_path: &Path,
    ) -> Result<Vec<crate::download::task::DownloadTask>, ModpackError> {
        Ok(Vec::new())
    }
    fn overrides_path(&self) -> String {
        self.overrides_prefix()
    }
}
fn get_version(component: &MultiMcComponent) -> Result<String, ModpackError> {
    component
        .version
        .as_ref()
        .or(component.cached_version.as_ref())
        .cloned()
        .ok_or(ModpackError::UnknownFormat)
}

impl MultiMcManifest {
    pub fn overrides_prefix(&self) -> String {
        format!("{}.minecraft", self.base_path)
    }
}

use crate::modpack::export::{
    ExportModpackOptions, ModpackExportBundle, normalize_mod_loader_version,
};
use std::path::PathBuf;
pub fn build_multimc_export_bundle(
    game_version: &str,
    loader: ModLoaderType,
    loader_version: &str,
    options: &ExportModpackOptions,
    selected_files: &[(String, PathBuf)],
) -> Result<ModpackExportBundle, ModpackError> {
    let manifest = generate_multimc_manifest(game_version, loader, loader_version);
    let json = serde_json::to_string_pretty(&manifest)?;

    Ok(ModpackExportBundle {
        overrides_prefix: ".minecraft".to_string(),
        overrides_files: selected_files.to_vec(),
        extra_files: vec![
            ("mmc-pack.json".to_string(), json),
            (
                "instance.cfg".to_string(),
                generate_multimc_instance_cfg(options),
            ),
            (".packignore".to_string(), String::new()),
        ],
    })
}

fn generate_multimc_manifest(
    game_version: &str,
    loader: ModLoaderType,
    loader_version: &str,
) -> MultiMcManifest {
    let mut components = Vec::new();
    components.push(MultiMcComponent {
        uid: "net.minecraft".to_string(),
        version: Some(game_version.to_string()),
        important: Some(true),
        dependency_only: Some(false),
        cached_name: None,
        cached_requires: None,
        cached_version: None,
        cached_volatile: None,
    });

    if loader != ModLoaderType::Unknown {
        let version = normalize_mod_loader_version(loader_version);
        let uid = match loader {
            ModLoaderType::Forge | ModLoaderType::LegacyForge => "net.minecraftforge",
            ModLoaderType::NeoForge => "net.neoforged",
            ModLoaderType::Fabric => "net.fabricmc.fabric-loader",
            ModLoaderType::Quilt => "org.quiltmc.quilt-loader",
            _ => "",
        };

        if !uid.is_empty() {
            components.push(MultiMcComponent {
                uid: uid.to_string(),
                version: Some(version),
                important: Some(false),
                dependency_only: Some(false),
                cached_name: None,
                cached_requires: None,
                cached_version: None,
                cached_volatile: None,
            });
        }
    }

    MultiMcManifest {
        format_version: 1,
        components,
        cfg: HashMap::new(),
        base_path: String::new(),
    }
}

fn generate_multimc_instance_cfg(options: &ExportModpackOptions) -> String {
    let mut content = format!(
        "# Auto generated by minecraftd\nInstanceType=OneSix\nname={}\n",
        options.name
    );
    if let Some(min_memory) = options.min_memory {
        content.push_str("OverrideMemory=true\n");
        content.push_str(&format!("MinMemAlloc={}\n", min_memory));
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn nested_archive_quilt_and_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for (name, contents) in [
            (
                "Example/mmc-pack.json",
                r#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21"},{"uid":"org.quiltmc.quilt-loader","cachedVersion":"0.2"}]}"#,
            ),
            (
                "Example/instance.cfg",
                "# comment\nInstanceType=OneSix\nname=Example pack\n",
            ),
            ("Example/.minecraft/config/example.txt", "config"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let manifest = MultiMcManifest::from_archive(&path).unwrap();
        assert_eq!(manifest.client_version().unwrap(), "1.21");
        assert_eq!(
            manifest.mod_loader().unwrap(),
            Some((ModLoaderType::Quilt, "0.2".into()))
        );
        assert_eq!(manifest.cfg["name"], "Example pack");
        let destination = dir.path().join("instance");
        crate::modpack::extract_overrides(&path, &destination, &manifest.overrides_path()).unwrap();
        assert_eq!(
            std::fs::read(destination.join("config/example.txt")).unwrap(),
            b"config"
        );
    }
}
