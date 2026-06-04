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
        let objects_dir = assets_dir.join("objects");
        let base_url = "https://resources.download.minecraft.net";

        let mut tasks = Vec::new();

        for entry in index.objects.values() {
            let prefix = &entry.hash[..2];
            let dest = objects_dir.join(prefix).join(&entry.hash);

            if dest.exists() {
                continue;
            }

            let url = format!("{}/{}/{}", base_url, prefix, entry.hash);
            let task = DownloadTask::new(url, dest).with_sha1(entry.hash.clone());
            tasks.push(task);
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_index_error_display() {
        let e = AssetIndexError::Download("timeout".into());
        assert!(format!("{e}").contains("timeout"));
    }
}
