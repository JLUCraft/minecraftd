use crate::download::task::DownloadTask;
use crate::minecraft::validator::AssetEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info};

/// Parsed asset index from Mojang's asset index JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetIndexData {
    pub objects: HashMap<String, AssetEntry>,
}

/// Client for downloading and managing Minecraft assets.
pub struct AssetIndexClient;

impl AssetIndexClient {
    /// Downloads an asset index JSON and parses it.
    pub async fn download_and_parse(
        url: &str,
        destination: &Path,
    ) -> Result<AssetIndexData, AssetIndexError> {
        debug!("downloading asset index from {}", url);

        let task = DownloadTask::new(url.to_string(), destination.to_path_buf());
        task.execute()
            .await
            .map_err(|e| AssetIndexError::Download(e.to_string()))?;

        let content = tokio::fs::read_to_string(destination).await?;
        let index: AssetIndexData = serde_json::from_str(&content)?;

        info!("parsed asset index with {} objects", index.objects.len());
        Ok(index)
    }

    /// Downloads all assets from an index.
    pub async fn download_assets(
        index: &AssetIndexData,
        assets_dir: &Path,
    ) -> Result<(), AssetIndexError> {
        let requests = plan_assets(
            index,
            assets_dir,
            crate::resource::SourceType::Official,
            false,
        )
        .await?;
        let tasks: Vec<DownloadTask> = requests
            .into_iter()
            .map(|request| {
                let mut task = DownloadTask::new(request.src.to_string(), request.dest);
                if let Some(sha1) = request.sha1 {
                    task = task.with_sha1(sha1);
                }
                task
            })
            .collect();
        if tasks.is_empty() {
            info!("all assets already present");
            return Ok(());
        }

        info!("downloading {} assets", tasks.len());
        let queue = crate::download::queue::DownloadQueue::new(8);
        queue
            .download_all(tasks)
            .await
            .map_err(|e| AssetIndexError::Download(e.to_string()))?;

        Ok(())
    }
}

/// Errors that can occur during asset index operations.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AssetIndexError {
    #[error("download error: {0}")]
    Download(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
}

impl From<std::io::Error> for AssetIndexError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AssetIndexError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

fn asset_relative(hash: &str) -> Result<String, AssetIndexError> {
    if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AssetIndexError::Parse("invalid asset SHA1".into()));
    }
    Ok(format!("{}/{}", &hash[..2], hash))
}
pub async fn plan_assets(
    index: &AssetIndexData,
    assets_dir: &Path,
    source: crate::resource::SourceType,
    check_hash: bool,
) -> Result<Vec<crate::download::request::DownloadParam>, AssetIndexError> {
    let root = crate::resource::get_download_api(source, crate::resource::ResourceType::Assets)
        .map_err(|error| AssetIndexError::Parse(error.to_string()))?;
    let mut requests = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in index.objects.values() {
        if !seen.insert(&entry.hash) {
            continue;
        }
        let relative = asset_relative(&entry.hash)?;
        let dest = assets_dir.join("objects").join(&relative);
        if tokio::fs::try_exists(&dest).await?
            && (!check_hash
                || crate::util::hash::verify_sha1(&dest, &entry.hash)
                    .await
                    .is_ok())
        {
            continue;
        }
        requests.push(crate::download::request::DownloadParam {
            src: root
                .join(&relative)
                .map_err(|error| AssetIndexError::Parse(error.to_string()))?,
            dest,
            filename: None,
            sha1: Some(entry.hash.clone()),
        });
    }
    Ok(requests)
}
pub async fn prepare_legacy_assets(
    root_dir: &Path,
    assets_dir: &Path,
    index_name: &str,
) -> Result<(), AssetIndexError> {
    let roots = match index_name {
        "legacy" => vec![assets_dir.join("virtual/legacy")],
        "pre-1.6" => vec![
            assets_dir.join("virtual/legacy"),
            root_dir.join("resources"),
        ],
        _ => return Ok(()),
    };
    let bytes = tokio::fs::read(
        assets_dir
            .join("indexes")
            .join(format!("{index_name}.json")),
    )
    .await?;
    let index: AssetIndexData = serde_json::from_slice(&bytes)?;
    for (name, entry) in index.objects {
        if name.is_empty()
            || name.contains('\\')
            || !Path::new(&name)
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            return Err(AssetIndexError::Parse("invalid legacy asset path".into()));
        }
        let origin = assets_dir
            .join("objects")
            .join(asset_relative(&entry.hash)?);
        if !tokio::fs::try_exists(&origin).await? {
            continue;
        }
        for root in &roots {
            let destination = root.join(&name);
            if tokio::fs::try_exists(&destination).await? {
                continue;
            }
            if let Some(parent) = destination.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::copy(&origin, &destination).await?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_index_error_display() {
        let e = AssetIndexError::Download("timeout".into());
        assert!(format!("{e}").contains("timeout"));
    }
}
