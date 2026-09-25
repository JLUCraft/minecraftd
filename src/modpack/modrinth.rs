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
        crate::modpack::read_manifest(&std::fs::File::open(path)?, "modrinth.index.json")
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
        Ok(self
            .download_requests(instance_path)?
            .into_iter()
            .map(|request| {
                let mut task = DownloadTask::new(request.src.to_string(), request.dest);
                if let Some(sha1) = request.sha1 {
                    task = task.with_sha1(sha1);
                }
                task
            })
            .collect())
    }

    fn overrides_path(&self) -> String {
        "overrides".into()
    }
}

impl Default for ModrinthManifest {
    fn default() -> Self {
        Self {
            format_version: 1,
            game: "minecraft".into(),
            version_id: String::new(),
            name: String::new(),
            summary: None,
            files: Vec::new(),
            dependencies: HashMap::new(),
        }
    }
}
impl ModrinthManifest {
    pub fn download_requests(
        &self,
        instance_path: &Path,
    ) -> Result<Vec<crate::download::request::DownloadParam>, ModpackError> {
        let mut requests = Vec::new();
        for file in &self.files {
            if file
                .env
                .as_ref()
                .is_some_and(|env| env.client == "unsupported")
            {
                continue;
            }
            if file.path.is_empty()
                || file.path.contains('\\')
                || !Path::new(&file.path)
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err(ModpackError::Parse("invalid modpack file path".into()));
            }
            let url = file
                .downloads
                .first()
                .ok_or_else(|| ModpackError::Parse("missing download URL".into()))?;
            requests.push(crate::download::request::DownloadParam {
                src: url
                    .parse()
                    .map_err(|error: url::ParseError| ModpackError::Parse(error.to_string()))?,
                dest: instance_path.join(&file.path),
                filename: None,
                sha1: Some(file.hashes.sha1.clone()),
            });
        }
        Ok(requests)
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
    let options = ExportModpackOptions {
        format: crate::modpack::export::ExportFormat::Modrinth,
        name: options.name.clone(),
        version: options.version_id.clone(),
        description: options.summary.clone(),
        author: None,
        min_memory: None,
        no_create_remote_files: Some(false),
        skip_curseforge_remote_files: Some(true),
    };
    let (loader, version) = mod_loader.unwrap_or((ModLoaderType::Unknown, String::new()));
    let mut manifest = generate_modrinth_manifest(mc_version, loader, &version, &options);
    manifest.files =
        collect_modrinth_files(&reqwest::Client::new(), None, selected_files, false, true)
            .await?
            .0;
    Ok(manifest)
}

use crate::modpack::export::{
    ExportModpackOptions, ModpackExportBundle, normalize_mod_loader_version,
};
use crate::resource::{curseforge::CurseForgeClient, http::ResourceTransport};
use sha1::Digest;
pub async fn build_modrinth_export_bundle<C: ResourceTransport>(
    client: &C,
    curseforge_key: Option<&str>,
    game_version: &str,
    loader: ModLoaderType,
    loader_version: &str,
    options: &ExportModpackOptions,
    selected_files: &[(String, PathBuf)],
) -> Result<ModpackExportBundle, ModpackError> {
    let mut manifest = generate_modrinth_manifest(game_version, loader, loader_version, options);
    let no_create_remote_files = options.no_create_remote_files.unwrap_or(false);
    let skip_curseforge = options.skip_curseforge_remote_files.unwrap_or(false);
    let (modrinth_files, override_files) = collect_modrinth_files(
        client,
        curseforge_key,
        selected_files,
        no_create_remote_files,
        skip_curseforge,
    )
    .await?;

    manifest.files = modrinth_files;
    let json = serde_json::to_string_pretty(&manifest)?;

    Ok(ModpackExportBundle {
        overrides_prefix: "overrides".to_string(),
        overrides_files: override_files,
        extra_files: vec![("modrinth.index.json".to_string(), json)],
    })
}

fn generate_modrinth_manifest(
    game_version: &str,
    loader: ModLoaderType,
    loader_version: &str,
    options: &ExportModpackOptions,
) -> ModrinthManifest {
    let mut dependencies = HashMap::new();
    dependencies.insert("minecraft".to_string(), game_version.to_owned());

    if loader != ModLoaderType::Unknown {
        let version = normalize_mod_loader_version(loader_version);
        let loader_key = match loader {
            ModLoaderType::Forge | ModLoaderType::LegacyForge => "forge",
            ModLoaderType::Fabric => "fabric-loader",
            ModLoaderType::NeoForge => "neoforge",
            ModLoaderType::Quilt => "quilt-loader",
            _ => "",
        };

        if !loader_key.is_empty() {
            dependencies.insert(loader_key.to_string(), version);
        }
    }

    ModrinthManifest {
        version_id: options.version.clone(),
        name: options.name.clone(),
        summary: options.description.clone(),
        files: Vec::new(),
        dependencies,
        ..Default::default()
    }
}

async fn build_modrinth_remote_file<C: ResourceTransport>(
    client: &C,
    curseforge_key: Option<&str>,
    rel: &str,
    full: &Path,
    skip_curseforge: bool,
) -> Result<Option<ModrinthManifestFile>, ModpackError> {
    let is_disabled = rel.ends_with(".disabled");
    let manifest_path = if is_disabled {
        rel.trim_end_matches(".disabled").to_string()
    } else {
        rel.to_string()
    };

    let mut downloads = Vec::new();

    if let Ok(remote) = ModrinthClient::with_client(client)
        .identify_file(full)
        .await
    {
        downloads.push(remote.download_url);
    }

    if !skip_curseforge
        && let Some(key) = curseforge_key
        && let Ok(remote) = CurseForgeClient::with_client(client, key.to_owned())
            .identify_file(full)
            .await
    {
        downloads.push(remote.download_url);
    }

    if downloads.is_empty() {
        return Ok(None);
    }

    let file_content = tokio::fs::read(full).await?;
    let mut sha1_hasher = sha1::Sha1::new();
    let mut sha512_hasher = sha2::Sha512::new();
    sha1_hasher.update(&file_content);
    sha512_hasher.update(&file_content);
    let sha1 = hex::encode(sha1_hasher.finalize());
    let sha512 = hex::encode(sha512_hasher.finalize());
    let file_size = file_content.len() as u64;
    let env = if is_disabled {
        Some(ModrinthFileEnv {
            client: "optional".to_string(),
            server: "unsupported".to_string(),
        })
    } else {
        None
    };

    Ok(Some(ModrinthManifestFile {
        path: manifest_path,
        hashes: ModrinthFileHashes { sha1, sha512 },
        env,
        downloads,
        file_size,
    }))
}

async fn collect_modrinth_files<C: ResourceTransport>(
    client: &C,
    curseforge_key: Option<&str>,
    selected_files: &[(String, PathBuf)],
    no_create_remote_files: bool,
    skip_curseforge: bool,
) -> Result<(Vec<ModrinthManifestFile>, Vec<(String, PathBuf)>), ModpackError> {
    let mut remote = Vec::new();
    let mut overrides = Vec::new();
    for (relative, full) in selected_files {
        let candidate = ["mods/", "resourcepacks/", "shaderpacks/"]
            .iter()
            .any(|prefix| relative.starts_with(prefix));
        let file = if candidate && !no_create_remote_files {
            build_modrinth_remote_file(client, curseforge_key, relative, full, skip_curseforge)
                .await?
        } else {
            None
        };
        if let Some(file) = file {
            remote.push(file);
        } else {
            overrides.push((relative.clone(), full.clone()));
        }
    }
    Ok((remote, overrides))
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
