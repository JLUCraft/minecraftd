//! Fabric and Quilt installation plans, independent of task/UI runtimes.
use crate::download::request::DownloadParam;
use crate::minecraft::dependencies::convert_library_name_to_path;
use crate::minecraft::loader::installer::LoaderInstallError;
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::profile::{
    DownloadsArtifact, LibrariesDownloads, LibrariesValue, McClientInfo,
};
use crate::resource::{ResourceType, SourceType, convert_url_to_target_source, get_download_api};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use url::Url;

const LOADER_PATCH_PRIORITY: i64 = 30_000;
pub struct LibraryInstallPlan {
    pub patch: McClientInfo,
    pub downloads: Vec<DownloadParam>,
}
impl LibraryInstallPlan {
    pub fn apply(&self, profile: &mut McClientInfo) {
        profile.main_class.clone_from(&self.patch.main_class);
        profile.libraries = crate::minecraft::dependencies::merge_library_lists(
            &profile.libraries,
            &self.patch.libraries,
        );
        profile.patches.retain(|patch| patch.id != self.patch.id);
        profile.patches.push(self.patch.clone());
    }
}
fn parse_error(error: impl std::fmt::Display) -> LoaderInstallError {
    LoaderInstallError::Parse(error.to_string())
}
fn endpoints(
    loader: ModLoaderType,
) -> Result<(ResourceType, ResourceType, &'static str), LoaderInstallError> {
    match loader {
        ModLoaderType::Fabric => Ok((ResourceType::FabricMeta, ResourceType::FabricMaven, "v2")),
        ModLoaderType::Quilt => Ok((ResourceType::QuiltMeta, ResourceType::QuiltMaven, "v3")),
        _ => Err(LoaderInstallError::Unsupported(loader.to_string())),
    }
}
/// HTTP boundary for hosts that already own proxy/TLS-configured clients.
#[async_trait::async_trait]
pub trait MetadataClient: Sync {
    async fn get_json(&self, url: Url) -> Result<Value, LoaderInstallError>;
}
#[async_trait::async_trait]
impl MetadataClient for reqwest::Client {
    async fn get_json(&self, url: Url) -> Result<Value, LoaderInstallError> {
        Ok(self
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
pub async fn plan(
    client: &impl MetadataClient,
    priority: &[SourceType],
    loader: ModLoaderType,
    game_version: &str,
    loader_version: &str,
    library_dir: &Path,
) -> Result<LibraryInstallPlan, LoaderInstallError> {
    let (meta_type, _, api_version) = endpoints(loader)?;
    let mut last_error = LoaderInstallError::Network("no download sources configured".into());
    for source in priority {
        let root = get_download_api(*source, meta_type).map_err(parse_error)?;
        let url = root
            .join(&format!(
                "{api_version}/versions/loader/{game_version}/{loader_version}"
            ))
            .map_err(parse_error)?;
        let response = client.get_json(url).await;
        match response {
            Ok(meta) => match from_metadata(&meta, loader, loader_version, library_dir, *source) {
                Ok(plan) => return Ok(plan),
                Err(error) => last_error = error,
            },
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}
pub fn from_metadata(
    meta: &Value,
    loader: ModLoaderType,
    loader_version: &str,
    library_dir: &Path,
    source: SourceType,
) -> Result<LibraryInstallPlan, LoaderInstallError> {
    let (_, maven_type, _) = endpoints(loader)?;
    let root = get_download_api(SourceType::Official, maven_type).map_err(parse_error)?;
    let main_class = meta
        .pointer("/launcherMeta/mainClass/client")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| parse_error("missing mainClass.client"))?;
    let mut coordinates = Vec::new();
    for key in ["loader", "intermediary", "hashed"] {
        if let Some(name) = meta
            .get(key)
            .and_then(|value| value.get("maven"))
            .and_then(Value::as_str)
        {
            coordinates.push((name, None, None, 0));
        } else if key == "loader" || (loader == ModLoaderType::Fabric && key == "intermediary") {
            return Err(parse_error(format!("missing {key} Maven coordinate")));
        }
    }
    // Only common/client dependencies belong in a game client classpath.
    for side in ["common", "client"] {
        if let Some(libraries) = meta
            .pointer(&format!("/launcherMeta/libraries/{side}"))
            .and_then(Value::as_array)
        {
            for library in libraries {
                let name = library
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| parse_error("missing library name"))?;
                coordinates.push((
                    name,
                    library.get("url").and_then(Value::as_str),
                    library.get("sha1").and_then(Value::as_str),
                    library
                        .get("size")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                ));
            }
        }
    }
    let mut plan = LibraryInstallPlan {
        patch: McClientInfo {
            id: loader.to_string().to_lowercase(),
            version: Some(loader_version.into()),
            priority: Some(LOADER_PATCH_PRIORITY),
            main_class: Some(main_class.into()),
            ..Default::default()
        },
        downloads: Vec::new(),
    };
    let mut seen = HashSet::new();
    for (name, base, sha1, size) in coordinates {
        if !seen.insert(name) {
            continue;
        }
        let path = convert_library_name_to_path(name, None).map_err(parse_error)?;
        let fallback = if loader == ModLoaderType::Quilt && name.starts_with("net.fabricmc:") {
            get_download_api(SourceType::Official, ResourceType::FabricMaven)
                .map_err(parse_error)?
        } else {
            root.clone()
        };
        let root = base
            .map(Url::parse)
            .transpose()
            .map_err(parse_error)?
            .unwrap_or(fallback);
        let url = root.join(&path).map_err(parse_error)?;
        let src = convert_url_to_target_source(
            &url,
            &[
                ResourceType::QuiltMaven,
                ResourceType::FabricMaven,
                ResourceType::Libraries,
            ],
            &source,
        )
        .map_err(parse_error)?;
        let artifact = DownloadsArtifact {
            path: path.clone(),
            url: src.to_string(),
            sha1: sha1.unwrap_or_default().into(),
            size,
        };
        plan.patch.libraries.push(LibrariesValue {
            name: name.into(),
            downloads: Some(LibrariesDownloads {
                artifact: Some(artifact),
                ..Default::default()
            }),
            ..Default::default()
        });
        plan.downloads.push(DownloadParam {
            src,
            dest: library_dir.join(path),
            filename: None,
            sha1: sha1.map(str::to_owned),
        });
    }
    Ok(plan)
}
pub async fn install(
    loader: ModLoaderType,
    game_version: &str,
    loader_version: &str,
    game_dir: &Path,
) -> Result<McClientInfo, LoaderInstallError> {
    let plan = plan(
        &reqwest::Client::new(),
        &[SourceType::Official],
        loader,
        game_version,
        loader_version,
        &game_dir.join("libraries"),
    )
    .await?;
    let tasks = plan
        .downloads
        .into_iter()
        .map(|request| {
            let mut task =
                crate::download::task::DownloadTask::new(request.src.to_string(), request.dest);
            if let Some(sha1) = request.sha1 {
                task = task.with_sha1(sha1);
            }
            task
        })
        .collect();
    crate::download::queue::DownloadQueue::new(8)
        .download_all(tasks)
        .await
        .map_err(|error| LoaderInstallError::Network(error.to_string()))?;
    Ok(plan.patch)
}
/// Resolve the optional API mod through the same configured transport as other resources.
pub async fn companion<C: crate::resource::http::ResourceTransport>(
    client: &C,
    loader: ModLoaderType,
    version: &str,
    directory: &Path,
) -> Result<Option<DownloadParam>, crate::resource::ResourceError> {
    let project = match loader {
        ModLoaderType::Fabric => "P7dR8mSH",
        ModLoaderType::Quilt => "qvIfYCYJ",
        _ => return Ok(None),
    };
    crate::resource::modrinth::ModrinthClient::with_client(client)
        .latest_download(project, loader, version, directory)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quilt_plan_preserves_main_class_mirror_and_side_boundaries() {
        let meta = serde_json::json!({"loader":{"maven":"org.quiltmc:quilt-loader:0.1"}, "intermediary":{"maven":"net.fabricmc:intermediary:1.20"}, "launcherMeta":{"mainClass":{"client":"client.Main"}, "libraries":{"client":[{"name":"x:client:1"}], "server":[{"name":"x:server:1"}], "development":[{"name":"x:dev:1"}]}}});
        let plan = from_metadata(
            &meta,
            ModLoaderType::Quilt,
            "0.1",
            Path::new("libraries"),
            SourceType::Bmclapi,
        )
        .unwrap();
        assert_eq!(plan.patch.main_class.as_deref(), Some("client.Main"));
        assert_eq!(plan.downloads.len(), 3);
        assert!(
            plan.downloads
                .iter()
                .all(|request| request.src.host_str() == Some("bmclapi2.bangbang93.com"))
        );
        let mut profile = McClientInfo::default();
        plan.apply(&mut profile);
        plan.apply(&mut profile);
        assert_eq!(profile.patches.len(), 1);
        assert_eq!(profile.libraries.len(), 3);
    }
}
