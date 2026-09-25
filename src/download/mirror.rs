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

    /// Rewrites supported resource URLs using the shared endpoint catalog.
    #[must_use]
    pub fn rewrite_url(&self, original: &str) -> String {
        let Ok(url) = Url::parse(original) else {
            return original.to_owned();
        };
        match self {
            Self::Official => original.to_owned(),
            Self::Bmclapi => convert_url_to_target_source(&url, &[], &SourceType::Bmclapi)
                .unwrap_or(url)
                .to_string(),
            Self::Custom(base) => {
                let known = crate::resource::ResourceType::iter()
                    .filter_map(|resource| {
                        crate::resource::get_download_api(SourceType::Official, resource).ok()
                    })
                    .any(|endpoint| endpoint.origin() == url.origin());
                if known
                    || matches!(
                        url.host_str(),
                        Some("piston-meta.mojang.com" | "piston-data.mojang.com")
                    )
                {
                    let Ok(mut target) = Url::parse(base) else {
                        return original.to_owned();
                    };
                    target.set_path(&format!(
                        "{}{}",
                        target.path().trim_end_matches('/'),
                        url.path()
                    ));
                    target.set_query(url.query());
                    target.set_fragment(url.fragment());
                    target.to_string()
                } else {
                    original.to_owned()
                }
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
                    convert_url_to_target_source(&url, &[], &SourceType::Bmclapi)
                        .unwrap_or(url)
                        .to_string()
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

    #[test]
    fn rejects_lookalike_origins() {
        for source in [
            DownloadSource::Bmclapi,
            DownloadSource::Custom("https://mirror.example".into()),
        ] {
            let malicious = "https://libraries.minecraft.net.evil.example/library.jar";
            assert_eq!(source.rewrite_url(malicious), malicious);
            let indirect = "https://example.org/?url=https://libraries.minecraft.net/library.jar";
            assert_eq!(source.rewrite_url(indirect), indirect);
        }
    }
}
