pub mod curseforge;
pub mod modrinth;

use thiserror::Error;
use url::Url;

/// Type of Minecraft-related resource for URL mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    VersionManifest,
    VersionManifestV2,
    LauncherMeta,
    Launcher,
    Assets,
    Libraries,
    MojangJava,
    ForgeMaven,
    ForgeMeta,
    ForgeMavenNew,
    ForgeInstall,
    Liteloader,
    OptiFine,
    AuthlibInjector,
    FabricMeta,
    FabricMaven,
    NeoforgeMetaForge,
    NeoforgeMetaNeoforge,
    NeoforgeInstall,
    NeoforgeMaven,
    QuiltMaven,
    QuiltMeta,
}

/// A download source for Minecraft assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceType {
    /// Official Mojang servers.
    Official,
    /// BMCLAPI mirror (China).
    Bmclapi,
}

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

/// Returns the base API URL for a given source and resource type.
///
/// Mirrors SJMCL's `get_download_api` mapping (~80 lines).
///
/// # Errors
///
/// Returns `ResourceError::NoDownloadApi` if the resource type has no known
/// API endpoint for the selected source (e.g. OptiFine on Official).
pub fn get_download_api(
    source: SourceType,
    resource_type: ResourceType,
) -> Result<Url, ResourceError> {
    match source {
        SourceType::Official => match resource_type {
            ResourceType::VersionManifest => Ok(Url::parse(
                "https://launchermeta.mojang.com/mc/game/version_manifest.json",
            )?),
            ResourceType::VersionManifestV2 => Ok(Url::parse(
                "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
            )?),
            ResourceType::LauncherMeta => Ok(Url::parse("https://launchermeta.mojang.com/")?),
            ResourceType::Launcher => Ok(Url::parse("https://launcher.mojang.com/")?),
            ResourceType::Assets => Ok(Url::parse("https://resources.download.minecraft.net/")?),
            ResourceType::Libraries => Ok(Url::parse("https://libraries.minecraft.net/")?),
            ResourceType::MojangJava => Ok(Url::parse(
                "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json",
            )?),
            ResourceType::ForgeMaven => Ok(Url::parse("https://files.minecraftforge.net/maven/")?),
            ResourceType::ForgeMavenNew => Ok(Url::parse("https://maven.minecraftforge.net")?),
            ResourceType::ForgeInstall => Ok(Url::parse(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/",
            )?),
            ResourceType::ForgeMeta => Err(ResourceError::NoDownloadApi),
            ResourceType::Liteloader => Ok(Url::parse(
                "https://dl.liteloader.com/versions/versions.json",
            )?),
            ResourceType::OptiFine => Err(ResourceError::NoDownloadApi),
            ResourceType::AuthlibInjector => Ok(Url::parse("https://authlib-injector.yushi.moe/")?),
            ResourceType::FabricMeta => Ok(Url::parse("https://meta.fabricmc.net/")?),
            ResourceType::FabricMaven => Ok(Url::parse("https://maven.fabricmc.net/")?),
            ResourceType::NeoforgeMetaForge => Ok(Url::parse(
                "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge/",
            )?),
            ResourceType::NeoforgeMetaNeoforge => Ok(Url::parse(
                "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge/",
            )?),
            ResourceType::NeoforgeMaven | ResourceType::NeoforgeInstall => {
                Ok(Url::parse("https://maven.neoforged.net/releases/")?)
            }
            ResourceType::QuiltMaven => {
                Ok(Url::parse("https://maven.quiltmc.org/repository/release/")?)
            }
            ResourceType::QuiltMeta => Ok(Url::parse("https://meta.quiltmc.org/")?),
        },
        SourceType::Bmclapi => match resource_type {
            ResourceType::VersionManifest => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/mc/game/version_manifest.json",
            )?),
            ResourceType::VersionManifestV2 => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json",
            )?),
            ResourceType::LauncherMeta => Ok(Url::parse("https://bmclapi2.bangbang93.com/")?),
            ResourceType::Launcher => Ok(Url::parse("https://bmclapi2.bangbang93.com/")?),
            ResourceType::Assets => Ok(Url::parse("https://bmclapi2.bangbang93.com/assets/")?),
            ResourceType::Libraries => Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?),
            ResourceType::MojangJava => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json",
            )?),
            ResourceType::ForgeMaven
            | ResourceType::ForgeMavenNew
            | ResourceType::NeoforgeMaven => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?)
            }
            ResourceType::ForgeInstall => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/forge/download/",
            )?),
            ResourceType::ForgeMeta => Ok(Url::parse("https://bmclapi2.bangbang93.com/forge/")?),
            ResourceType::Liteloader => Ok(Url::parse(
                "https://bmclapi.bangbang93.com/maven/com/mumfrey/liteloader/versions.json",
            )?),
            ResourceType::AuthlibInjector => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/",
            )?),
            ResourceType::FabricMeta => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/fabric-meta/")?)
            }
            ResourceType::FabricMaven => Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?),
            ResourceType::NeoforgeMetaForge | ResourceType::NeoforgeMetaNeoforge => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/neoforge/")?)
            }
            ResourceType::NeoforgeInstall => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/neoforge/version/",
            )?),
            ResourceType::OptiFine => Ok(Url::parse("https://bmclapi2.bangbang93.com/optifine/")?),
            ResourceType::QuiltMaven => Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?),
            ResourceType::QuiltMeta => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/quilt-meta/")?)
            }
        },
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

/// Converts a URL from one source type to another for a specific resource type.
///
/// # Errors
///
/// Returns `ResourceError::NoDownloadApi` if either source lacks an API for the
/// resource type, or if the URL does not start with the source API.
pub fn convert_url_source_type(
    url: &Url,
    resource_type: ResourceType,
    src_type: SourceType,
    dst_type: SourceType,
) -> Result<Url, ResourceError> {
    let url_str = url.as_str();
    let src_api = get_download_api(src_type, resource_type)?;
    let dst_api = get_download_api(dst_type, resource_type)?;
    if url_str.starts_with(src_api.as_str()) {
        Ok(Url::parse(
            url_str
                .replacen(src_api.as_str(), dst_api.as_str(), 1)
                .as_str(),
        )?)
    } else {
        Err(ResourceError::NoDownloadApi)
    }
}

/// Attempts to convert a URL to a target source by trying all known resource types.
///
/// If no replacement is possible, returns a clone of the original URL.
pub fn convert_url_to_target_source(url: &Url, dst_type: SourceType) -> Url {
    let url_str = url.as_str();

    for resource_type in all_resource_types() {
        let dst_api = match get_download_api(dst_type, *resource_type) {
            Ok(api) => api,
            Err(_) => continue,
        };

        for src_type in [SourceType::Official, SourceType::Bmclapi] {
            if src_type == dst_type {
                continue;
            }
            if let Ok(src_api) = get_download_api(src_type, *resource_type)
                && url_str.starts_with(src_api.as_str())
            {
                let new_url_str = url_str.replacen(src_api.as_str(), dst_api.as_str(), 1);
                if let Ok(new_url) = Url::parse(&new_url_str) {
                    return new_url;
                }
            }
        }
    }

    url.clone()
}

const ALL_RESOURCE_TYPES: &[ResourceType] = &[
    ResourceType::VersionManifest,
    ResourceType::VersionManifestV2,
    ResourceType::LauncherMeta,
    ResourceType::Launcher,
    ResourceType::Assets,
    ResourceType::Libraries,
    ResourceType::MojangJava,
    ResourceType::ForgeMaven,
    ResourceType::ForgeMeta,
    ResourceType::ForgeMavenNew,
    ResourceType::ForgeInstall,
    ResourceType::Liteloader,
    ResourceType::OptiFine,
    ResourceType::AuthlibInjector,
    ResourceType::FabricMeta,
    ResourceType::FabricMaven,
    ResourceType::NeoforgeMetaForge,
    ResourceType::NeoforgeMetaNeoforge,
    ResourceType::NeoforgeInstall,
    ResourceType::NeoforgeMaven,
    ResourceType::QuiltMaven,
    ResourceType::QuiltMeta,
];

#[must_use]
pub const fn all_resource_types() -> &'static [ResourceType] {
    ALL_RESOURCE_TYPES
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
