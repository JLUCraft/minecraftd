use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::resource::ResourceError;

const BASE_URL: &str = "https://api.modrinth.com/v2";
const TRANSLATE_BASE: &str = "https://mod.mcimirror.top/translate/modrinth";

/// A Modrinth API client.
#[derive(Debug, Clone)]
pub struct ModrinthClient {
    client: Client,
}

impl ModrinthClient {
    /// Creates a new client.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Searches for projects on Modrinth.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn search_projects(
        &self,
        query: &SearchQuery,
    ) -> Result<SearchResult, ResourceError> {
        let url = format!("{BASE_URL}/search");
        let mut params = HashMap::new();
        params.insert("query".to_string(), query.query.clone());

        let mut facets = vec![vec![format!("project_type:{}", query.project_type)]];
        if let Some(version) = &query.game_version {
            facets.push(vec![format!("versions:{version}")]);
        }
        if let Some(categories) = &query.categories {
            for cat in categories {
                facets.push(vec![format!("categories:{cat}")]);
            }
        }
        params.insert(
            "facets".to_string(),
            serde_json::to_string(&facets).unwrap_or_default(),
        );
        params.insert("offset".to_string(), query.offset.to_string());
        params.insert("limit".to_string(), query.limit.to_string());
        params.insert("index".to_string(), query.index.clone());

        let resp = self
            .client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| ResourceError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ResourceError::Api(format!(
                "HTTP {} from Modrinth search",
                resp.status()
            )));
        }

        let data: SearchResponse = resp.json().await?;
        Ok(data.into())
    }

    /// Gets a single project by ID or slug.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_project(&self, project_id: &str) -> Result<ModrinthProject, ResourceError> {
        let url = format!("{BASE_URL}/project/{project_id}");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ResourceError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ResourceError::Api(format!(
                "HTTP {} from Modrinth get_project",
                resp.status()
            )));
        }

        Ok(resp.json().await?)
    }

    /// Gets all versions for a project, optionally filtered.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_project_versions(
        &self,
        project_id: &str,
        loaders: Option<&[&str]>,
        game_versions: Option<&[&str]>,
    ) -> Result<Vec<ModrinthVersion>, ResourceError> {
        let url = format!("{BASE_URL}/project/{project_id}/version");
        let mut params = HashMap::new();
        if let Some(loaders) = loaders {
            params.insert(
                "loaders".to_string(),
                format!(
                    "[{}]",
                    loaders
                        .iter()
                        .map(|s| format!("\"{s}\""))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            );
        }
        if let Some(versions) = game_versions {
            params.insert(
                "game_versions".to_string(),
                format!(
                    "[{}]",
                    versions
                        .iter()
                        .map(|s| format!("\"{s}\""))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            );
        }

        let resp = self
            .client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| ResourceError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ResourceError::Api(format!(
                "HTTP {} from Modrinth get_project_versions",
                resp.status()
            )));
        }

        Ok(resp.json().await?)
    }

    /// Gets a version file by its hash.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_version_file(
        &self,
        hash: &str,
        algorithm: &str,
    ) -> Result<ModrinthVersion, ResourceError> {
        let url = format!("{BASE_URL}/version_file/{hash}");
        let mut params = HashMap::new();
        params.insert("algorithm".to_string(), algorithm.to_string());

        let resp = self
            .client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| ResourceError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ResourceError::Api(format!(
                "HTTP {} from Modrinth get_version_file",
                resp.status()
            )));
        }

        Ok(resp.json().await?)
    }

    /// Finds the latest version of a project matching the given criteria.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` if no matching version is found or on API errors.
    pub async fn get_latest_for_game(
        &self,
        project_id: &str,
        loaders: Option<&[&str]>,
        game_versions: Option<&[&str]>,
    ) -> Result<ModrinthVersion, ResourceError> {
        let versions = self
            .get_project_versions(project_id, loaders, game_versions)
            .await?;
        versions
            .into_iter()
            .next()
            .ok_or_else(|| ResourceError::Api("no matching version found for project".to_string()))
    }

    /// Fetches a Chinese translation for a project description.
    ///
    /// Returns `Ok(None)` if translation fails, to avoid blocking major functionality.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network failures.
    pub async fn translate_description(
        &self,
        project_id: &str,
    ) -> Result<Option<String>, ResourceError> {
        let url = format!("{TRANSLATE_BASE}/{project_id}");
        let result = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ResourceError::Network(e.to_string()))?;

        if !result.status().is_success() {
            return Ok(None);
        }

        let translation: TranslationResponse = match result.json().await {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        Ok(Some(translation.translated))
    }
}

impl Default for ModrinthClient {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Query structs
// ============================================================================

/// Parameters for `search_projects`.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub query: String,
    pub project_type: String,
    pub game_version: Option<String>,
    pub categories: Option<Vec<String>>,
    pub offset: u32,
    pub limit: u32,
    pub index: String,
}

impl SearchQuery {
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            project_type: "mod".to_string(),
            offset: 0,
            limit: 20,
            index: "relevance".to_string(),
            ..Default::default()
        }
    }
}

// ============================================================================
// Response data models
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthProject {
    #[serde(alias = "id")]
    pub project_id: String,
    pub project_type: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub categories: Vec<String>,
    pub downloads: u64,
    pub icon_url: Option<String>,
    #[serde(alias = "updated")]
    pub date_modified: String,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthSearchRes {
    pub hits: Vec<ModrinthProject>,
    pub total_hits: u64,
    pub offset: u32,
    pub limit: u32,
}

/// Result of a `search_projects` call.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub hits: Vec<ModrinthProject>,
    pub total_hits: u64,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SearchResponse {
    hits: Vec<ModrinthProject>,
    pub total_hits: u64,
    pub offset: u32,
    pub limit: u32,
}

impl From<SearchResponse> for SearchResult {
    fn from(resp: SearchResponse) -> Self {
        Self {
            hits: resp.hits,
            total_hits: resp.total_hits,
            offset: resp.offset,
            limit: resp.limit,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthVersion {
    pub project_id: String,
    pub dependencies: Vec<ModrinthDependency>,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub name: String,
    pub date_published: String,
    pub downloads: u64,
    pub version_type: String,
    pub files: Vec<ModrinthFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthDependency {
    pub project_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthFile {
    pub url: String,
    pub filename: String,
    pub hashes: ModrinthFileHashes,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthFileHashes {
    pub sha1: String,
    pub sha512: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TranslationResponse {
    translated: String,
}

// ============================================================================
// Helpers
// ============================================================================

/// Normalizes a Modrinth loader string to a canonical display name.
#[must_use]
pub fn normalize_loader(loader: &str) -> Option<String> {
    if loader.is_empty() || loader == "minecraft" {
        None
    } else {
        match loader.to_lowercase().as_str() {
            "forge" => Some("Forge".to_string()),
            "fabric" => Some("Fabric".to_string()),
            "quilt" => Some("Quilt".to_string()),
            "neoforge" => Some("NeoForge".to_string()),
            "vanilla" => Some("Vanilla".to_string()),
            "iris" => Some("Iris".to_string()),
            "canvas" => Some("Canvas".to_string()),
            "optifine" => Some("OptiFine".to_string()),
            _ => Some(loader.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_loader() {
        assert_eq!(normalize_loader("fabric"), Some("Fabric".to_string()));
        assert_eq!(normalize_loader("FORGE"), Some("Forge".to_string()));
        assert_eq!(normalize_loader("minecraft"), None);
        assert_eq!(normalize_loader(""), None);
    }

    #[test]
    fn test_search_query_default() {
        let q = SearchQuery::new("sodium");
        assert_eq!(q.query, "sodium");
        assert_eq!(q.project_type, "mod");
        assert_eq!(q.limit, 20);
        assert_eq!(q.index, "relevance");
    }
}
