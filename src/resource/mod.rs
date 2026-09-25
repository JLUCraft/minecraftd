pub mod curseforge;
pub mod http;
pub mod mirrors;
pub mod models;
pub mod modrinth;
pub mod presentation;
pub use mirrors::{
    ResourceType, SourceType, convert_url_source_type, convert_url_to_target_source,
    get_download_api,
};
use thiserror::Error;
#[cfg(test)]
use url::Url;
/// Errors from resource operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    #[error("no download api for this resource type")]
    NoDownloadApi,
    #[error("parse error: {0}")]
    Parse(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("api error: {0}")]
    Api(String),
}

impl From<url::ParseError> for ResourceError {
    fn from(e: url::ParseError) -> Self {
        Self::Parse(e.to_string())
    }
}

impl From<reqwest::Error> for ResourceError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<serde_json::Error> for ResourceError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

/// Priority configuration for download sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePriority {
    pub primary: SourceType,
    pub fallbacks: Vec<SourceType>,
}

impl Default for SourcePriority {
    fn default() -> Self {
        Self {
            primary: SourceType::Official,
            fallbacks: vec![SourceType::Bmclapi],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_download_api_official_version_manifest() {
        let url = get_download_api(SourceType::Official, ResourceType::VersionManifest).unwrap();
        assert_eq!(
            url.as_str(),
            "https://launchermeta.mojang.com/mc/game/version_manifest.json"
        );
    }

    #[test]
    fn test_get_download_api_bmclapi_libraries() {
        let url = get_download_api(SourceType::Bmclapi, ResourceType::Libraries).unwrap();
        assert_eq!(url.as_str(), "https://bmclapi2.bangbang93.com/maven/");
    }

    #[test]
    fn test_get_download_api_optifine_official_fails() {
        let result = get_download_api(SourceType::Official, ResourceType::OptiFine);
        assert!(matches!(result, Err(ResourceError::NoDownloadApi)));
    }

    #[test]
    fn test_convert_url_source_type() {
        let url =
            Url::parse("https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar")
                .unwrap();
        let converted = convert_url_source_type(
            &url,
            ResourceType::Libraries,
            SourceType::Official,
            SourceType::Bmclapi,
        )
        .unwrap();
        assert_eq!(
            converted.as_str(),
            "https://bmclapi2.bangbang93.com/maven/org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar"
        );
    }

    #[test]
    fn test_source_priority_default() {
        let priority = SourcePriority::default();
        assert_eq!(priority.primary, SourceType::Official);
        assert_eq!(priority.fallbacks, vec![SourceType::Bmclapi]);
    }
}
