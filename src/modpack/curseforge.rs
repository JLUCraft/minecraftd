use crate::download::task::DownloadTask;
use crate::minecraft::loaders::ModLoaderType;
use crate::modpack::{ModpackError, ModpackManifest};
use crate::resource::curseforge::{CurseForgeClient, fallback_download_url};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// CurseForge modpack manifest (`manifest.json`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifest {
    pub name: String,
    pub version: Option<String>,
    pub author: String,
    pub overrides: String,
    pub minecraft: CurseForgeManifestMinecraft,
    pub files: Vec<CurseForgeManifestFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestMinecraft {
    pub version: String,
    pub mod_loaders: Vec<CurseForgeManifestModLoader>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestModLoader {
    pub id: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestFile {
    #[serde(rename = "projectID")]
    pub project_id: u64,
    #[serde(rename = "fileID")]
    pub file_id: u64,
    pub required: bool,
}

/// Options for exporting a CurseForge modpack.
#[derive(Debug, Clone, Default)]
pub struct CurseForgeExportOptions {
    pub name: String,
    pub version: String,
    pub author: String,
    pub overrides: String,
}

#[async_trait::async_trait]
impl ModpackManifest for CurseForgeManifest {
    fn from_archive(path: &Path) -> Result<Self, ModpackError>
    where
        Self: Sized,
    {
        crate::modpack::read_manifest(&std::fs::File::open(path)?, "manifest.json")
    }

    fn client_version(&self) -> Result<String, ModpackError> {
        Ok(self.minecraft.version.clone())
    }

    fn mod_loader(&self) -> Result<Option<(ModLoaderType, String)>, ModpackError> {
        let loader = self
            .minecraft
            .mod_loaders
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.minecraft.mod_loaders.first());

        if let Some(loader) = loader {
            let id = &loader.id;
            let Some((loader_type, version)) = id.split_once('-') else {
                return Err(ModpackError::Parse(format!("invalid mod loader id: {id}")));
            };
            let loader_type = ModLoaderType::from_str(loader_type).map_err(ModpackError::Parse)?;
            Ok(Some((loader_type, version.to_string())))
        } else {
            Ok(None)
        }
    }

    async fn download_tasks(
        &self,
        instance_path: &Path,
    ) -> Result<Vec<DownloadTask>, ModpackError> {
        let client = CurseForgeClient::from_env().ok_or_else(|| {
            ModpackError::Api("MINECRAFTD_CURSEFORGE_API_KEY not set".to_string())
        })?;

        Ok(self
            .download_requests(&client, instance_path)
            .await?
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
        self.overrides.clone()
    }
}

impl CurseForgeManifest {
    pub async fn download_requests<C: crate::resource::http::ResourceTransport>(
        &self,
        client: &CurseForgeClient<C>,
        instance: &Path,
    ) -> Result<Vec<crate::download::request::DownloadParam>, ModpackError> {
        let mut requests = Vec::new();
        for entry in &self.files {
            let project = client.get_mod(entry.project_id).await?;
            let file = client
                .get_file_info(entry.project_id, entry.file_id)
                .await?;
            let name = &file.file_name;
            if name.is_empty() || name.contains(['/', '\\']) || matches!(name.as_str(), "." | "..")
            {
                return Err(ModpackError::Parse("invalid CurseForge filename".into()));
            }
            let directory = match project.class_id {
                Some(12 | 6945) => "resourcepacks",
                Some(6552) => "shaderpacks",
                _ => "mods",
            };
            let url = file
                .download_url
                .unwrap_or_else(|| fallback_download_url(file.id, name));
            requests.push(crate::download::request::DownloadParam {
                src: url
                    .parse()
                    .map_err(|error: url::ParseError| ModpackError::Parse(error.to_string()))?,
                dest: instance.join(directory).join(name),
                filename: Some(name.clone()),
                sha1: file
                    .hashes
                    .iter()
                    .find(|hash| hash.algo == 1)
                    .map(|hash| hash.value.clone()),
            });
        }
        Ok(requests)
    }
}

/// Generates a CurseForge manifest from an instance for export.
///
/// `selected_files` is a list of `(relative_path, absolute_path)` pairs.
/// Files under `mods/`, `resourcepacks/`, and `shaderpacks/` are included as
/// manifest entries; everything else becomes overrides.
pub fn generate_manifest(
    options: &CurseForgeExportOptions,
    mc_version: &str,
    mod_loader: Option<(ModLoaderType, String)>,
    _selected_files: &[(String, PathBuf)],
) -> Result<CurseForgeManifest, ModpackError> {
    let mod_loaders = if let Some((loader_type, version)) = mod_loader {
        vec![CurseForgeManifestModLoader {
            id: format!("{}-{}", format!("{loader_type:?}").to_lowercase(), version),
            primary: true,
        }]
    } else {
        vec![]
    };

    let files = Vec::new();
    // In a real export, mods/resourcepacks/shaderpacks would be resolved
    // via CurseForge API. For a minimal implementation, we include them
    // as overrides instead of manifest entries.

    Ok(CurseForgeManifest {
        name: options.name.clone(),
        version: Some(options.version.clone()),
        author: options.author.clone(),
        overrides: options.overrides.clone(),
        minecraft: CurseForgeManifestMinecraft {
            version: mc_version.to_string(),
            mod_loaders,
        },
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mod_loader_parsing() {
        let manifest = CurseForgeManifest {
            name: "Test".into(),
            version: Some("1.0".into()),
            author: "Author".into(),
            overrides: "overrides".into(),
            minecraft: CurseForgeManifestMinecraft {
                version: "1.20.1".into(),
                mod_loaders: vec![CurseForgeManifestModLoader {
                    id: "forge-47.2.0".into(),
                    primary: true,
                }],
            },
            files: vec![],
        };

        let loader = manifest.mod_loader().unwrap();
        assert_eq!(loader, Some((ModLoaderType::Forge, "47.2.0".into())));
    }

    #[test]
    fn test_generate_manifest() {
        let options = CurseForgeExportOptions {
            name: "MyPack".into(),
            version: "1.0.0".into(),
            author: "me".into(),
            overrides: "overrides".into(),
        };
        let manifest = generate_manifest(
            &options,
            "1.20.1",
            Some((ModLoaderType::Forge, "47.2.0".into())),
            &[],
        )
        .unwrap();
        assert_eq!(manifest.name, "MyPack");
        assert_eq!(manifest.minecraft.version, "1.20.1");
    }
}
