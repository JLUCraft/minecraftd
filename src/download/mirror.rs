use crate::resource::{SourceType, convert_url_to_target_source};
use url::Url;

/// A download source for Minecraft assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadSource {
    /// Official Mojang servers.
    Official,
    /// BMCLAPI mirror (China).
    Bmclapi,
    /// Custom base URL.
    Custom(String),
}

impl DownloadSource {
    /// Converts to the canonical `SourceType` if possible.
    #[must_use]
    pub const fn to_source_type(&self) -> Option<SourceType> {
        match self {
            Self::Official => Some(SourceType::Official),
            Self::Bmclapi => Some(SourceType::Bmclapi),
            Self::Custom(_) => None,
        }
    }

    /// Rewrites a Mojang URL to use this source.
    ///
    /// Backward-compatible string-replacement approach.
    #[must_use]
    pub fn rewrite_url(&self, original: &str) -> String {
        match self {
            Self::Official => original.to_string(),
            Self::Bmclapi => {
                // BMCLAPI mirrors Mojang's resources
                original
                    .replace(
                        "https://launchermeta.mojang.com",
                        "https://bmclapi2.bangbang93.com",
                    )
                    .replace(
                        "https://launcher.mojang.com",
                        "https://bmclapi2.bangbang93.com",
                    )
                    .replace(
                        "https://piston-meta.mojang.com",
                        "https://bmclapi2.bangbang93.com",
                    )
                    .replace(
                        "https://piston-data.mojang.com",
                        "https://bmclapi2.bangbang93.com",
                    )
                    .replace(
                        "https://libraries.minecraft.net",
                        "https://bmclapi2.bangbang93.com/maven",
                    )
                    .replace(
                        "https://resources.download.minecraft.net",
                        "https://bmclapi2.bangbang93.com/assets",
                    )
            }
            Self::Custom(base) => {
                // For custom mirrors, only rewrite known Mojang domains
                if original.starts_with("https://") {
                    let domains = [
                        "launchermeta.mojang.com",
                        "launcher.mojang.com",
                        "piston-meta.mojang.com",
                        "piston-data.mojang.com",
                        "libraries.minecraft.net",
                        "resources.download.minecraft.net",
                    ];
                    for domain in &domains {
                        if original.contains(domain) {
                            return original
                                .replace(&format!("https://{domain}"), base.trim_end_matches('/'));
                        }
                    }
                }
                original.to_string()
            }
        }
    }

    /// Resource-type-aware URL rewrite using the canonical API mapping table.
    ///
    /// Falls back to [`rewrite_url`](Self::rewrite_url) for custom sources or
    /// when parsing fails.
    #[must_use]
    pub fn rewrite_url_for(
        &self,
        original: &str,
        resource_type: crate::resource::ResourceType,
    ) -> String {
        match self {
            Self::Official => original.to_string(),
            Self::Bmclapi => {
                if let Ok(url) = Url::parse(original) {
                    // Try precise mapping first
                    if let Ok(converted) = crate::resource::convert_url_source_type(
                        &url,
                        resource_type,
                        SourceType::Official,
                        SourceType::Bmclapi,
                    ) {
                        return converted.to_string();
                    }
                    // Fallback to generic conversion
                    convert_url_to_target_source(&url, SourceType::Bmclapi).to_string()
                } else {
                    original.to_string()
                }
            }
            Self::Custom(_) => self.rewrite_url(original),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ResourceType;

    #[test]
    fn test_official_rewrite() {
        let source = DownloadSource::Official;
        let url = "https://launchermeta.mojang.com/v1/packages/...";
        assert_eq!(source.rewrite_url(url), url);
    }

    #[test]
    fn test_bmclapi_rewrite_launchermeta() {
        let source = DownloadSource::Bmclapi;
        let url = "https://launchermeta.mojang.com/v1/packages/abc/version.json";
        assert_eq!(
            source.rewrite_url(url),
            "https://bmclapi2.bangbang93.com/v1/packages/abc/version.json"
        );
    }

    #[test]
    fn test_bmclapi_rewrite_libraries() {
        let source = DownloadSource::Bmclapi;
        let url = "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar";
        assert_eq!(
            source.rewrite_url(url),
            "https://bmclapi2.bangbang93.com/maven/org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar"
        );
    }

    #[test]
    fn test_custom_rewrite() {
        let source = DownloadSource::Custom("https://mirror.example.com".to_string());
        let url = "https://launchermeta.mojang.com/v1/packages/abc.json";
        assert_eq!(
            source.rewrite_url(url),
            "https://mirror.example.com/v1/packages/abc.json"
        );
    }

    #[test]
    fn test_rewrite_url_for_libraries() {
        let source = DownloadSource::Bmclapi;
        let url = "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar";
        let rewritten = source.rewrite_url_for(url, ResourceType::Libraries);
        assert_eq!(
            rewritten,
            "https://bmclapi2.bangbang93.com/maven/org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar"
        );
    }

    #[test]
    fn test_to_source_type() {
        assert_eq!(
            DownloadSource::Official.to_source_type(),
            Some(SourceType::Official)
        );
        assert_eq!(
            DownloadSource::Bmclapi.to_source_type(),
            Some(SourceType::Bmclapi)
        );
        assert_eq!(
            DownloadSource::Custom("https://x.com".to_string()).to_source_type(),
            None
        );
    }
}
