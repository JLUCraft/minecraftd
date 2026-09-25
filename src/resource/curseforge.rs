use crate::resource::http::ResourceTransport;
use crate::resource::models::{
    OtherResourceFileInfo, OtherResourceSearchQuery, OtherResourceSearchRes,
    OtherResourceVersionPack, OtherResourceVersionPackQuery,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

use crate::resource::ResourceError;

const BASE_URL: &str = "https://api.curseforge.com/v1";
const GAME_ID: &str = "432";
const TRANSLATE_BASE: &str = "https://mod.mcimirror.top/translate/curseforge";

/// A CurseForge API client.
#[derive(Debug, Clone)]
pub struct CurseForgeClient<C = Client> {
    client: C,
    api_key: String,
}

impl CurseForgeClient {
    /// Creates a new client with the given API key.
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    /// Creates a new client from the `MINECRAFTD_CURSEFORGE_API_KEY` environment variable.
    ///
    /// Returns `None` if the environment variable is not set.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        std::env::var("MINECRAFTD_CURSEFORGE_API_KEY")
            .ok()
            .map(Self::new)
    }
}

impl<C: ResourceTransport> CurseForgeClient<C> {
    pub const fn with_client(client: C, api_key: String) -> Self {
        Self { client, api_key }
    }

    pub async fn search_resources(
        &self,
        query: &OtherResourceSearchQuery,
    ) -> Result<OtherResourceSearchRes, ResourceError> {
        use crate::resource::presentation::{
            cvt_category_to_id, cvt_sort_by_to_id, cvt_type_to_class_id,
        };
        let class = cvt_type_to_class_id(&query.resource_type);
        let sort = cvt_sort_by_to_id(&query.sort_by);
        Ok(self
            .search_mods(&SearchModsQuery {
                class_id: Some(class),
                search_filter: query.search_query.clone(),
                game_version: filter_value(&query.game_version),
                category_id: filter_value(&query.selected_tag)
                    .map(|tag| cvt_category_to_id(&tag, class)),
                sort_field: sort,
                sort_order: if sort == 4 {
                    "asc".into()
                } else {
                    "desc".into()
                },
                index: query
                    .page
                    .checked_mul(query.page_size)
                    .ok_or_else(|| ResourceError::Parse("page overflow".into()))?,
                page_size: query.page_size,
                ..Default::default()
            })
            .await?
            .into())
    }
    pub async fn version_packs(
        &self,
        query: &OtherResourceVersionPackQuery,
    ) -> Result<Vec<OtherResourceVersionPack>, ResourceError> {
        const PAGE_SIZE: u32 = 50;
        let id: u64 = query
            .resource_id
            .parse()
            .map_err(|error: std::num::ParseIntError| ResourceError::Parse(error.to_string()))?;
        let mut files = Vec::new();
        let mut index = 0u32;
        loop {
            let response = self
                .get_mod_files_page(
                    id,
                    &FilesQuery {
                        mod_loader_type: filter_value(&query.mod_loader).map(|loader| {
                            crate::resource::presentation::cvt_mod_loader_to_id(&loader)
                        }),
                        game_version: query
                            .game_versions
                            .first()
                            .and_then(|value| filter_value(value)),
                        index,
                        page_size: PAGE_SIZE,
                        ..Default::default()
                    },
                )
                .await?;
            let empty = response.data.is_empty();
            files.extend(response.data);
            index = index
                .checked_add(PAGE_SIZE)
                .ok_or_else(|| ResourceError::Parse("pagination overflow".into()))?;
            if empty || u64::from(index) >= response.pagination.total_count {
                break;
            }
        }
        Ok(crate::resource::presentation::map_curseforge_file_to_version_pack(files))
    }
    pub async fn identify_file(
        &self,
        path: &std::path::Path,
    ) -> Result<OtherResourceFileInfo, ResourceError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| ResourceError::Parse(error.to_string()))?;
        let sha1 = crate::util::hash::sha1_bytes(&bytes);
        let filtered: Vec<u8> = bytes
            .into_iter()
            .filter(|byte| !matches!(byte, 0x09 | 0x0a | 0x0d | 0x20))
            .collect();
        let fingerprint = u64::from(murmur2::murmur2(&filtered, 1));
        let result = self.get_mods_by_fingerprints(&[fingerprint]).await?;
        let file = result
            .exact_matches
            .iter()
            .map(|entry| &entry.file)
            .find(|file| {
                file.hashes
                    .iter()
                    .any(|hash| hash.algo == 1 && hash.value.eq_ignore_ascii_case(&sha1))
            })
            .ok_or_else(|| ResourceError::Parse("no verified fingerprint match".into()))?;
        Ok((file, None).into())
    }
    /// Searches for mods on CurseForge.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn search_mods(
        &self,
        query: &SearchModsQuery,
    ) -> Result<SearchResult, ResourceError> {
        let url = format!("{BASE_URL}/mods/search");
        let mut params = HashMap::new();
        params.insert("gameId".to_string(), GAME_ID.to_string());
        if let Some(class_id) = query.class_id {
            params.insert("classId".to_string(), class_id.to_string());
        }
        params.insert("searchFilter".to_string(), query.search_filter.clone());
        if let Some(game_version) = &query.game_version {
            params.insert("gameVersion".to_string(), game_version.clone());
        }
        if let Some(category_id) = query.category_id {
            params.insert("categoryId".to_string(), category_id.to_string());
        }
        if let Some(loader) = query.mod_loader_type {
            params.insert("modLoaderType".to_string(), loader.to_string());
        }
        params.insert("sortField".to_string(), query.sort_field.to_string());
        params.insert("sortOrder".to_string(), query.sort_order.clone());
        params.insert("index".to_string(), query.index.to_string());
        params.insert("pageSize".to_string(), query.page_size.to_string());

        let response = self
            .client
            .request_json(&url, &params, None, Some(&self.api_key))
            .await?;

        let data: SearchResponse = serde_json::from_value(response)?;
        Ok(data.into())
    }

    /// Gets a single mod/project by ID.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_mod(&self, mod_id: u64) -> Result<CurseForgeProject, ResourceError> {
        let url = format!("{BASE_URL}/mods/{mod_id}");
        let response = self
            .client
            .request_json(&url, &HashMap::new(), None, Some(&self.api_key))
            .await?;

        let data: GetProjectResponse = serde_json::from_value(response)?;
        Ok(data.data)
    }

    /// Gets all files for a mod.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_mod_files(
        &self,
        mod_id: u64,
        query: &FilesQuery,
    ) -> Result<Vec<CurseForgeFile>, ResourceError> {
        Ok(self.get_mod_files_page(mod_id, query).await?.data)
    }
    pub async fn get_mod_files_page(
        &self,
        mod_id: u64,
        query: &FilesQuery,
    ) -> Result<FilesResponse, ResourceError> {
        let url = format!("{BASE_URL}/mods/{mod_id}/files");
        let mut params = HashMap::new();
        if let Some(loader) = query.mod_loader_type {
            params.insert("modLoaderType".to_string(), loader.to_string());
        }
        if let Some(version) = &query.game_version_type_id {
            params.insert("gameVersionTypeId".to_string(), version.to_string());
        }
        if let Some(version) = &query.game_version {
            params.insert("gameVersion".into(), version.clone());
        }
        params.insert("index".to_string(), query.index.to_string());
        params.insert("pageSize".to_string(), query.page_size.to_string());

        let response = self
            .client
            .request_json(&url, &params, None, Some(&self.api_key))
            .await?;

        let data: FilesResponse = serde_json::from_value(response)?;
        Ok(data)
    }

    /// Gets information about a single file.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_file_info(
        &self,
        mod_id: u64,
        file_id: u64,
    ) -> Result<CurseForgeFile, ResourceError> {
        let url = format!("{BASE_URL}/mods/{mod_id}/files/{file_id}");
        let response = self
            .client
            .request_json(&url, &HashMap::new(), None, Some(&self.api_key))
            .await?;

        let data: FileResponse = serde_json::from_value(response)?;
        Ok(data.data)
    }

    /// Looks up mods by Murmur2 fingerprints (used for local file identification).
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network or parse failures.
    pub async fn get_mods_by_fingerprints(
        &self,
        fingerprints: &[u64],
    ) -> Result<FingerprintResult, ResourceError> {
        let url = format!("{BASE_URL}/fingerprints/{GAME_ID}");
        let payload = json!({ "fingerprints": fingerprints });

        let response = self
            .client
            .request_json(&url, &HashMap::new(), Some(&payload), Some(&self.api_key))
            .await?;

        let data: FingerprintResponse = serde_json::from_value(response)?;
        Ok(data.data)
    }

    /// Fetches a Chinese translation for a mod description.
    ///
    /// Returns `Ok(None)` if translation fails, to avoid blocking major functionality.
    ///
    /// # Errors
    ///
    /// Returns `ResourceError` on network failures.
    pub async fn translate_description(
        &self,
        mod_id: u64,
    ) -> Result<Option<String>, ResourceError> {
        let url = format!("{TRANSLATE_BASE}/{mod_id}");
        let response = self
            .client
            .request_json(&url, &HashMap::new(), None, Some(&self.api_key))
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

/// Parameters for `search_mods`.
#[derive(Debug, Clone, Default)]
pub struct SearchModsQuery {
    pub search_filter: String,
    pub game_version: Option<String>,
    pub class_id: Option<u32>,
    pub category_id: Option<u32>,
    pub mod_loader_type: Option<u32>,
    pub sort_field: u32,
    pub sort_order: String,
    pub index: u32,
    pub page_size: u32,
}

impl SearchModsQuery {
    #[must_use]
    pub fn new(search_filter: impl Into<String>) -> Self {
        Self {
            search_filter: search_filter.into(),
            sort_field: 2,
            sort_order: "desc".to_string(),
            index: 0,
            page_size: 20,
            ..Default::default()
        }
    }
}

/// Parameters for `get_mod_files`.
#[derive(Debug, Clone, Default)]
pub struct FilesQuery {
    pub game_version: Option<String>,
    pub mod_loader_type: Option<u32>,
    pub game_version_type_id: Option<u32>,
    pub index: u32,
    pub page_size: u32,
}

// ============================================================================
// Response data models
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeProject {
    pub id: i32,
    pub class_id: Option<i32>,
    pub links: CurseForgeLinks,
    pub name: String,
    pub slug: String,
    pub summary: String,
    pub categories: Vec<CurseForgeCategory>,
    pub download_count: u64,
    pub logo: Option<CurseForgeLogo>,
    pub date_modified: String,
    pub authors: Vec<CurseForgeAuthor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeLinks {
    pub website_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeCategory {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeLogo {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeAuthor {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFile {
    pub id: i32,
    pub mod_id: i32,
    pub display_name: String,
    pub file_name: String,
    pub release_type: u32,
    pub hashes: Vec<CurseForgeHash>,
    pub file_date: String,
    pub download_url: Option<String>,
    pub download_count: u64,
    pub game_versions: Vec<String>,
    pub dependencies: Vec<CurseForgeDependency>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeHash {
    pub value: String,
    pub algo: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeDependency {
    pub mod_id: i32,
    pub relation_type: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgePagination {
    pub index: u32,
    pub page_size: u32,
    pub total_count: u64,
}

/// Result of a `search_mods` call.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub data: Vec<CurseForgeProject>,
    pub pagination: CurseForgePagination,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    pub data: Vec<CurseForgeProject>,
    pub pagination: CurseForgePagination,
}

impl From<SearchResponse> for SearchResult {
    fn from(resp: SearchResponse) -> Self {
        Self {
            data: resp.data,
            pagination: resp.pagination,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProjectResponse {
    pub data: CurseForgeProject,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesResponse {
    pub data: Vec<CurseForgeFile>,
    pub pagination: CurseForgePagination,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileResponse {
    pub data: CurseForgeFile,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintResponse {
    pub data: FingerprintResult,
}

/// Fingerprint lookup result.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintResult {
    pub exact_matches: Vec<ExactMatch>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactMatch {
    pub file: CurseForgeFile,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslationResponse {
    translated: String,
}

// ============================================================================
// Helper conversions
// ============================================================================

/// Converts a CurseForge `release_type` integer to a human-readable string.
#[must_use]
pub const fn release_type_name(release_type: u32) -> &'static str {
    match release_type {
        1 => "release",
        2 => "beta",
        _ => "alpha",
    }
}

/// Converts a CurseForge `relation_type` integer to a human-readable string.
#[must_use]
pub const fn dependency_type_name(relation_type: u32) -> &'static str {
    match relation_type {
        1 => "embedded",
        2 => "optional",
        3 => "required",
        4 => "tool",
        5 => "incompatible",
        _ => "include",
    }
}

/// Computes the fallback download URL for a CurseForge file when `download_url` is `None`.
///
/// CurseForge files may not have a direct download URL for third-party clients;
/// this generates the `edge.forgecdn.net` URL used as a fallback.
#[must_use]
pub fn fallback_download_url(file_id: i32, file_name: &str) -> String {
    format!(
        "https://edge.forgecdn.net/files/{}/{}/{}",
        file_id / 1000,
        file_id % 1000,
        urlencoding::encode(file_name)
    )
}

pub fn is_curseforge_authenticated_url(url: &url::Url) -> bool {
    matches!(
        url.host_str(),
        Some("api.curseforge.com" | "edge.forgecdn.net" | "mediafilez.forgecdn.net")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_release_type_name() {
        assert_eq!(release_type_name(1), "release");
        assert_eq!(release_type_name(2), "beta");
        assert_eq!(release_type_name(3), "alpha");
    }

    #[test]
    fn test_dependency_type_name() {
        assert_eq!(dependency_type_name(3), "required");
        assert_eq!(dependency_type_name(5), "incompatible");
    }

    #[test]
    fn test_fallback_download_url() {
        let url = fallback_download_url(1234, "mod.jar");
        assert_eq!(url, "https://edge.forgecdn.net/files/1/234/mod.jar");
    }

    #[test]
    fn test_search_query_default() {
        let q = SearchModsQuery::new("jei");
        assert_eq!(q.search_filter, "jei");
        assert_eq!(q.sort_field, 2);
        assert_eq!(q.sort_order, "desc");
        assert_eq!(q.page_size, 20);
    }
}
