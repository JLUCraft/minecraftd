use crate::resource::http::ResourceTransport;
use crate::resource::models::{
    OtherResourceFileInfo, OtherResourceSearchQuery, OtherResourceSearchRes,
    OtherResourceVersionPack, OtherResourceVersionPackQuery,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::resource::ResourceError;

const BASE_URL: &str = "https://api.modrinth.com/v2";
const TRANSLATE_BASE: &str = "https://mod.mcimirror.top/translate/modrinth";

/// A Modrinth API client.
#[derive(Debug, Clone)]
pub struct ModrinthClient<C = Client> {
    client: C,
}

impl ModrinthClient {
    /// Creates a new client.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl<C: ResourceTransport> ModrinthClient<C> {
    pub const fn with_client(client: C) -> Self {
        Self { client }
    }

    pub async fn search_resources(
        &self,
        query: &OtherResourceSearchQuery,
    ) -> Result<OtherResourceSearchRes, ResourceError> {
        let result = self
            .search_projects(&SearchQuery {
                query: query.search_query.clone(),
                project_type: query.resource_type.clone(),
                game_version: filter_value(&query.game_version),
                categories: filter_value(&query.selected_tag).map(|tag| vec![tag]),
                offset: query
                    .page
                    .checked_mul(query.page_size)
                    .ok_or_else(|| ResourceError::Parse("page overflow".into()))?,
                limit: query.page_size,
                index: query.sort_by.clone(),
            })
            .await?;
        Ok(result.into())
    }
    pub async fn version_packs(
        &self,
        query: &OtherResourceVersionPackQuery,
    ) -> Result<Vec<OtherResourceVersionPack>, ResourceError> {
        let loader = filter_value(&query.mod_loader).map(|value| value.to_lowercase());
        let loaders = loader.as_deref().map(|loader| vec![loader]);
        let versions = query
            .game_versions
            .iter()
            .filter(|value| !value.is_empty() && value.as_str() != "All")
            .map(String::as_str)
            .collect::<Vec<_>>();
        let versions = if versions.is_empty() {
            None
        } else {
            Some(versions.as_slice())
        };
        Ok(
            crate::resource::presentation::map_modrinth_file_to_version_pack(
                self.get_project_versions(&query.resource_id, loaders.as_deref(), versions)
                    .await?,
                &query.resource_type,
            ),
        )
    }
    pub async fn identify_file(
        &self,
        path: &std::path::Path,
    ) -> Result<OtherResourceFileInfo, ResourceError> {
        let hash = crate::util::hash::sha1_file(path)
            .await
            .map_err(|error| ResourceError::Parse(error.to_string()))?;
        let version = self.get_version_file(&hash, "sha1").await?;
        let file = version
            .files
            .iter()
            .find(|file| file.hashes.sha1.eq_ignore_ascii_case(&hash))
            .ok_or_else(|| ResourceError::Parse("remote file SHA1 mismatch".into()))?;
        Ok((&version, file, version.loaders.first().cloned()).into())
    }
    pub async fn latest_download(
        &self,
        project: &str,
        loader: crate::minecraft::loaders::ModLoaderType,
        game_version: &str,
        directory: &std::path::Path,
    ) -> Result<Option<crate::download::request::DownloadParam>, ResourceError> {
        let packs = self
            .version_packs(&OtherResourceVersionPackQuery {
                resource_id: project.into(),
                mod_loader: loader.to_string(),
                game_versions: vec![game_version.into()],
                resource_type: String::new(),
            })
            .await?;
        let Some(pack) = packs.first() else {
            return Ok(None);
        };
        let Some(file) = pack
            .items
            .iter()
            .filter(|file| matches!(file.release_type.as_str(), "release" | "beta"))
            .max_by(|a, b| a.file_date.cmp(&b.file_date))
        else {
            return Ok(None);
        };
        if std::path::Path::new(&file.file_name).components().count() != 1
            || file.file_name.contains(['/', '\\'])
            || matches!(file.file_name.as_str(), "." | ".." | "")
        {
            return Err(ResourceError::Parse("invalid remote file name".into()));
        }
        Ok(Some(crate::download::request::DownloadParam {
            src: file.download_url.parse()?,
            dest: directory.join(&file.file_name),
            filename: Some(file.file_name.clone()),
            sha1: Some(file.sha1.clone()),
        }))
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

        let response = self.client.request_json(&url, &params, None, None).await?;

        Ok(serde_json::from_value(response)?)
    }

    /// Gets a single project by ID or slug.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_project(&self, project_id: &str) -> Result<ModrinthProject, ResourceError> {
        let url = format!("{BASE_URL}/project/{project_id}");
        let response = self
            .client
            .request_json(&url, &HashMap::new(), None, None)
            .await?;

        Ok(serde_json::from_value(response)?)
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
            params.insert("loaders".to_owned(), serde_json::to_string(loaders)?);
        }
        if let Some(versions) = game_versions {
            params.insert("game_versions".to_owned(), serde_json::to_string(versions)?);
        }

        let response = self.client.request_json(&url, &params, None, None).await?;

        Ok(serde_json::from_value(response)?)
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

        let response = self.client.request_json(&url, &params, None, None).await?;

        Ok(serde_json::from_value(response)?)
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
        let response = self
            .client
            .request_json(&url, &HashMap::new(), None, None)
            .await;
        let translation: TranslationResponse = match response
            .and_then(|value| serde_json::from_value(value).map_err(ResourceError::from))
        {
            Ok(translation) => translation,
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

fn filter_value(value: &str) -> Option<String> {
    if value.is_empty() || value == "All" {
        None
    } else {
        Some(value.to_owned())
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

pub type SearchResult = ModrinthSearchRes;

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
    #[serde(default)]
    pub primary: bool,
    pub url: String,
    pub filename: String,
    pub hashes: ModrinthFileHashes,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModrinthFileHashes {
    pub sha1: String,
    #[serde(default)]
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
