use crate::download::task::DownloadTask;
use crate::minecraft::loaders::ModLoaderType;
use crate::modpack::{ModpackError, ModpackManifest};
use crate::resource::modrinth::ModrinthClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Modrinth modpack manifest (`modrinth.index.json`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthManifest {
    pub format_version: u64,
    pub game: String,
    pub version_id: String,
    pub name: String,
    pub summary: Option<String>,
    pub files: Vec<ModrinthManifestFile>,
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthManifestFile {
    pub path: String,
    pub hashes: ModrinthFileHashes,
    pub env: Option<ModrinthFileEnv>,
    pub downloads: Vec<String>,
    pub file_size: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthFileHashes {
    pub sha1: String,
    pub sha512: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthFileEnv {
    pub client: String,
    pub server: String,
}

/// Options for exporting a Modrinth modpack.
#[derive(Debug, Clone, Default)]
pub struct ModrinthExportOptions {
    pub version_id: String,
    pub name: String,
    pub summary: Option<String>,
}

#[async_trait::async_trait]
impl ModpackManifest for ModrinthManifest {
    fn from_archive(path: &Path) -> Result<Self, ModpackError>
    where
        Self: Sized,
    {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| ModpackError::Parse(format!("invalid zip: {e}")))?;
        let mut manifest_file = archive
            .by_name("modrinth.index.json")
            .map_err(|_| ModpackError::Parse("modrinth.index.json not found".to_string()))?;
        let mut content = String::new();
        std::io::Read::read_to_string(&mut manifest_file, &mut content)
            .map_err(|e| ModpackError::Io(e.to_string()))?;
        let manifest: Self = serde_json::from_str(&content)?;
        Ok(manifest)
    }

    fn client_version(&self) -> Result<String, ModpackError> {
        self.dependencies
            .get("minecraft")
            .cloned()
            .ok_or_else(|| ModpackError::Parse("missing minecraft dependency".to_string()))
    }

    fn mod_loader(&self) -> Result<Option<(ModLoaderType, String)>, ModpackError> {
        for (key, val) in &self.dependencies {
            match key.as_str() {
                "minecraft" => continue,
                "forge" => return Ok(Some((ModLoaderType::Forge, val.clone()))),
                "fabric-loader" => return Ok(Some((ModLoaderType::Fabric, val.clone()))),
                "neoforge" => return Ok(Some((ModLoaderType::NeoForge, val.clone()))),
                "quilt-loader" => return Ok(Some((ModLoaderType::Quilt, val.clone()))),
                _ => {}
            }
        }
        Ok(None)
    }

    async fn download_tasks(
        &self,
        instance_path: &Path,
    ) -> Result<Vec<DownloadTask>, ModpackError> {
        let mut tasks = Vec::new();

        for file in &self.files {
            let download_url = file
                .downloads
                .first()
                .ok_or_else(|| ModpackError::Parse("missing download url".to_string()))?;
            let dest = instance_path.join(&file.path);
            let mut task = DownloadTask::new(download_url.clone(), dest);
            task = task.with_sha1(file.hashes.sha1.clone());
            tasks.push(task);
        }

        Ok(tasks)
    }

    fn overrides_path(&self) -> &str {
        "overrides"
    }
}

/// Generates a Modrinth manifest from an instance for export.
///
/// `selected_files` is a list of `(relative_path, absolute_path)` pairs.
/// Files under `mods/`, `resourcepacks/`, and `shaderpacks/` that can be
/// matched via the Modrinth API are included as manifest entries;
/// everything else becomes overrides.
pub async fn generate_manifest(
    options: &ModrinthExportOptions,
    _instance_path: &Path,
    mc_version: &str,
    mod_loader: Option<(ModLoaderType, String)>,
    selected_files: &[(String, PathBuf)],
) -> Result<ModrinthManifest, ModpackError> {
    let mut dependencies = HashMap::new();
    dependencies.insert("minecraft".to_string(), mc_version.to_string());

    if let Some((loader_type, version)) = mod_loader {
        let key = match loader_type {
            ModLoaderType::Forge | ModLoaderType::LegacyForge => "forge",
            ModLoaderType::Fabric => "fabric-loader",
            ModLoaderType::NeoForge => "neoforge",
            ModLoaderType::Quilt => "quilt-loader",
            _ => "",
        };
        if !key.is_empty() {
            dependencies.insert(key.to_string(), version);
        }
    }

    let mut files = Vec::new();
    let client = ModrinthClient::new();

    for (rel, full) in selected_files {
        let is_remote_candidate = rel.starts_with("mods/")
            || rel.starts_with("resourcepacks/")
            || rel.starts_with("shaderpacks/");

        if is_remote_candidate
            && let Ok(hash) = crate::util::hash::sha1_file(full).await
            && let Ok(version_info) = client.get_version_file(&hash, "sha1").await
            && let Some(file_info) = version_info.files.iter().find(|f| f.hashes.sha1 == hash)
        {
            files.push(ModrinthManifestFile {
                path: rel.clone(),
                hashes: ModrinthFileHashes {
                    sha1: hash,
                    sha512: file_info.hashes.sha512.clone(),
                },
                env: None,
                downloads: vec![file_info.url.clone()],
                file_size: tokio::fs::metadata(full).await.map_or(0, |m| m.len()),
            });
        }
    }

    Ok(ModrinthManifest {
        format_version: 1,
        game: "minecraft".to_string(),
        version_id: options.version_id.clone(),
        name: options.name.clone(),
        summary: options.summary.clone(),
        files,
        dependencies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mod_loader_parsing() {
        let mut deps = HashMap::new();
        deps.insert("minecraft".to_string(), "1.20.1".to_string());
        deps.insert("fabric-loader".to_string(), "0.15.6".to_string());

        let manifest = ModrinthManifest {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "1.0".into(),
            name: "Test".into(),
            summary: None,
            files: vec![],
            dependencies: deps,
        };

        let loader = manifest.mod_loader().unwrap();
        assert_eq!(loader, Some((ModLoaderType::Fabric, "0.15.6".into())));
    }

    #[test]
    fn test_client_version_missing() {
        let manifest = ModrinthManifest {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "1.0".into(),
            name: "Test".into(),
            summary: None,
            files: vec![],
            dependencies: HashMap::new(),
        };

        assert!(manifest.client_version().is_err());
    }
}
