use crate::minecraft::version::{VersionInfo, VersionManifest, VersionMeta};
use std::time::Duration;
use tracing::{debug, info};

const VERSION_MANIFEST_URL: &str = "https://launchermeta.mojang.com/mc/game/version_manifest.json";
const CACHE_TTL: Duration = Duration::from_hours(1);

/// Client for fetching and caching Mojang's version manifest.
#[derive(Debug, Clone)]
pub struct VersionManifestClient {
    cache_path: Option<std::path::PathBuf>,
}

impl VersionManifestClient {
    #[must_use]
    pub const fn new() -> Self {
        Self { cache_path: None }
    }

    #[must_use]
    pub const fn with_cache(cache_path: std::path::PathBuf) -> Self {
        Self {
            cache_path: Some(cache_path),
        }
    }

    /// Fetches the version manifest, using cache if available and fresh.
    pub async fn fetch(&self) -> Result<VersionManifest, VersionManifestError> {
        // Try cache first
        if let Some(cache) = &self.cache_path
            && let Ok(metadata) = tokio::fs::metadata(cache).await
            && let Ok(modified) = metadata.modified()
            && modified.elapsed().unwrap_or(Duration::MAX) < CACHE_TTL
        {
            debug!("using cached version manifest");
            let content = tokio::fs::read_to_string(cache).await?;
            let manifest: VersionManifest = serde_json::from_str(&content)?;
            return Ok(manifest);
        }

        // Fetch from network
        info!("fetching version manifest from {}", VERSION_MANIFEST_URL);
        let response = reqwest::get(VERSION_MANIFEST_URL)
            .await
            .map_err(|e| VersionManifestError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(VersionManifestError::Http(
                response.status().as_u16(),
                response.status().to_string(),
            ));
        }

        let text = response
            .text()
            .await
            .map_err(|e| VersionManifestError::Parse(format!("failed to read body: {e}")))?;
        let manifest: VersionManifest = serde_json::from_str(&text)
            .map_err(|e| VersionManifestError::Parse(format!("json parse: {e}")))?;

        // Save to cache
        if let Some(cache) = &self.cache_path {
            if let Some(parent) = cache.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let json = serde_json::to_string_pretty(&manifest)?;
            let _ = tokio::fs::write(cache, json).await;
        }

        Ok(manifest)
    }

    /// Finds a version by ID in the manifest.
    pub async fn find_version(
        &self,
        version_id: &str,
    ) -> Result<Option<VersionMeta>, VersionManifestError> {
        let manifest = self.fetch().await?;
        Ok(manifest.versions.into_iter().find(|v| v.id == version_id))
    }

    /// Downloads the version JSON for a specific version.
    pub async fn download_version_json(
        &self,
        version_meta: &VersionMeta,
    ) -> Result<VersionInfo, VersionManifestError> {
        debug!("downloading version JSON for {}", version_meta.id);
        let response = reqwest::get(&version_meta.url)
            .await
            .map_err(|e| VersionManifestError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(VersionManifestError::Http(
                response.status().as_u16(),
                response.status().to_string(),
            ));
        }

        let text = response
            .text()
            .await
            .map_err(|e| VersionManifestError::Parse(format!("failed to read body: {e}")))?;
        let info: VersionInfo = serde_json::from_str(&text)
            .map_err(|e| VersionManifestError::Parse(format!("json parse: {e}")))?;

        Ok(info)
    }
}

impl Default for VersionManifestClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur when fetching the version manifest.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VersionManifestError {
    #[error("network error: {0}")]
    Network(String),
    #[error("http error {0}: {1}")]
    Http(u16, String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for VersionManifestError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for VersionManifestError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_manifest_error_display() {
        let e = VersionManifestError::Network("timeout".into());
        assert!(format!("{e}").contains("timeout"));

        let e2 = VersionManifestError::Http(404, "not found".into());
        assert!(format!("{e2}").contains("404"));
    }

    #[tokio::test]
    async fn test_version_manifest_client_fetch() {
        let client = VersionManifestClient::new();
        let result = client.fetch().await;
        assert!(
            result.is_ok(),
            "fetch should succeed with network: {result:?}"
        );
        let manifest = result.unwrap();
        assert!(!manifest.versions.is_empty());
        assert!(!manifest.latest.release.is_empty());
    }

    #[tokio::test]
    async fn test_version_manifest_client_find_version() {
        let client = VersionManifestClient::new();
        let result = client.find_version("1.20.4").await;
        assert!(
            result.is_ok(),
            "find_version should succeed with network: {result:?}"
        );
        let meta = result.unwrap();
        assert!(meta.is_some(), "1.20.4 should exist in manifest");
        assert_eq!(meta.unwrap().id, "1.20.4");
    }

    #[tokio::test]
    async fn test_version_manifest_client_find_nonexistent_version() {
        let client = VersionManifestClient::new();
        let result = client.find_version("0.0.0-does-not-exist").await;
        assert!(
            result.is_ok(),
            "find_version should not error for missing version"
        );
        assert!(
            result.unwrap().is_none(),
            "nonexistent version should return None"
        );
    }
}
