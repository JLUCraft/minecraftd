use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata about a Minecraft version.
///
/// This corresponds to Mojang's version manifest entries and
/// SJMCL's `McClientInfo` structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VersionMeta {
    /// The version ID (e.g., "1.20.4", "23w14a").
    pub id: String,
    /// The version type ("release", "snapshot", "`old_beta`", "`old_alpha`").
    #[serde(rename = "type")]
    pub version_type: String,
    /// URL to the version JSON.
    pub url: String,
    /// Release time in ISO 8601 format.
    pub time: String,
    /// Release date.
    #[serde(rename = "releaseTime")]
    pub release_time: String,
    /// SHA1 hash of the version JSON.
    #[serde(default)]
    pub sha1: String,
    /// Compliance level (for newer versions).
    pub compliance_level: Option<u32>,
}

/// Mojang's version manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

/// Detailed version information from a version JSON file.
///
/// This is the parsed form of Mojang's `version.json` files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VersionInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    #[serde(rename = "assetIndex", default)]
    pub asset_index: AssetIndex,
    #[serde(default)]
    pub assets: String,
    #[serde(rename = "complianceLevel")]
    pub compliance_level: Option<u32>,
    #[serde(default)]
    pub downloads: HashMap<String, DownloadInfo>,
    pub java_version: Option<JavaVersionRequirement>,
    #[serde(default)]
    pub libraries: Vec<LibraryInfo>,
    #[serde(rename = "mainClass")]
    pub main_class: Option<String>,
    #[serde(rename = "minecraftArguments")]
    pub minecraft_arguments: Option<String>,
    pub arguments: Option<Arguments>,
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
    #[serde(default)]
    pub time: String,
    /// Parent version ID for inheritance chains (e.g., Forge patches Vanilla).
    #[serde(rename = "inheritsFrom", skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    /// Logging configuration (client only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<HashMap<String, LoggingConfig>>,
    /// Minimum launcher version required.
    #[serde(
        rename = "minimumLauncherVersion",
        skip_serializing_if = "Option::is_none"
    )]
    pub minimum_launcher_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AssetIndex {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    #[serde(rename = "totalSize")]
    pub total_size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DownloadInfo {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JavaVersionRequirement {
    pub component: String,
    pub major_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LibraryInfo {
    pub downloads: Option<LibraryDownloads>,
    pub name: String,
    pub rules: Option<Vec<Rule>>,
    pub natives: Option<HashMap<String, String>>,
    pub extract: Option<ExtractRules>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LibraryDownloads {
    pub artifact: Option<DownloadInfo>,
    pub classifiers: Option<HashMap<String, DownloadInfo>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsCondition>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OsCondition {
    pub name: Option<String>,
    pub version: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExtractRules {
    pub exclude: Vec<String>,
}

/// Logging configuration for a version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoggingConfig {
    pub argument: String,
    pub file: LoggingFile,
    #[serde(rename = "type")]
    pub log_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Arguments {
    pub game: Vec<ArgumentValue>,
    pub jvm: Vec<ArgumentValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ArgumentValue {
    String(String),
    Object(ArgumentObject),
}

impl Default for ArgumentValue {
    fn default() -> Self {
        Self::String(String::new())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ArgumentObject {
    pub rules: Vec<Rule>,
    pub value: ArgumentObjectValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ArgumentObjectValue {
    String(String),
    Array(Vec<String>),
}

impl Default for ArgumentObjectValue {
    fn default() -> Self {
        Self::String(String::new())
    }
}

/// Compares two Minecraft version strings.
///
/// Handles release versions ("1.20.4") and snapshots ("23w14a").
/// Snapshots are always considered less than release versions.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_is_snapshot = a.contains('w');
    let b_is_snapshot = b.contains('w');

    if a_is_snapshot && !b_is_snapshot {
        return std::cmp::Ordering::Less;
    }
    if !a_is_snapshot && b_is_snapshot {
        return std::cmp::Ordering::Greater;
    }

    // Parse semver-like components
    let a_parts: Vec<u32> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let b_parts: Vec<u32> = b.split('.').filter_map(|p| p.parse().ok()).collect();

    for (a_p, b_p) in a_parts.iter().zip(b_parts.iter()) {
        match a_p.cmp(b_p) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    a_parts.len().cmp(&b_parts.len())
}

/// Extracts the major version from a version string.
///
/// E.g., "1.20.4" → "1.20"
#[must_use]
pub fn get_major_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        version.to_string()
    }
}

/// Downloads a version JSON from a URL.
pub async fn download_version_json(url: &str) -> Result<VersionInfo, reqwest::Error> {
    let response = reqwest::get(url).await?;
    let info: VersionInfo = response.json().await?;
    Ok(info)
}

/// Resolves the full version info including inheritance chain.
///
/// If `inherits_from` is set, downloads and merges the parent version.
pub async fn resolve_version_info(
    version_id: &str,
    version_json_dir: &std::path::Path,
) -> Result<VersionInfo, Box<dyn std::error::Error + Send + Sync>> {
    resolve_version_info_inner(version_id, version_json_dir).await
}

type ResolveVersionFuture<'info> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<VersionInfo, Box<dyn std::error::Error + Send + Sync>>,
            > + Send
            + 'info,
    >,
>;

fn resolve_version_info_inner<'info>(
    version_id: &'info str,
    version_json_dir: &'info std::path::Path,
) -> ResolveVersionFuture<'info> {
    Box::pin(async move {
        let path = version_json_dir.join(format!("{version_id}.json"));
        let content = tokio::fs::read_to_string(&path).await?;
        let mut info: VersionInfo = serde_json::from_str(&content)?;

        if let Some(parent_id) = &info.inherits_from {
            let parent = resolve_version_info_inner(parent_id, version_json_dir).await?;
            merge_version_info(&mut info, parent);
        }

        Ok(info)
    })
}

/// Merges parent version info into child.
///
/// Child values take precedence over parent values.
fn merge_version_info(child: &mut VersionInfo, parent: VersionInfo) {
    if child.asset_index.id.is_empty() {
        child.asset_index = parent.asset_index;
    }
    if child.assets.is_empty() {
        child.assets = parent.assets;
    }
    if child.downloads.is_empty() {
        child.downloads = parent.downloads;
    }
    if child.java_version.is_none() {
        child.java_version = parent.java_version;
    }
    if child.libraries.is_empty() {
        child.libraries = parent.libraries;
    }
    if child.main_class.is_none() {
        child.main_class = parent.main_class;
    }
    if let (Some(parent_args), Some(child_args)) = (&parent.arguments, child.arguments.take()) {
        let mut merged_game = parent_args.game.clone();
        merged_game.extend(child_args.game);
        let mut merged_jvm = parent_args.jvm.clone();
        merged_jvm.extend(child_args.jvm);
        child.arguments = Some(Arguments {
            game: merged_game,
            jvm: merged_jvm,
        });
    } else if child.arguments.is_none() {
        child.arguments = parent.arguments;
    }
    if child.minecraft_arguments.is_none() {
        child.minecraft_arguments = parent.minecraft_arguments;
    }
    if child.logging.is_none() {
        child.logging = parent.logging;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_meta() {
        let meta = VersionMeta {
            id: "1.20.4".into(),
            version_type: "release".into(),
            url: "https://...".into(),
            time: "2023-12-07T12:...".into(),
            release_time: "2023-12-07T12:...".into(),
            sha1: "abc123".into(),
            compliance_level: Some(1),
        };

        assert_eq!(meta.id, "1.20.4");
        assert_eq!(meta.version_type, "release");
    }

    #[test]
    fn test_get_major_version() {
        assert_eq!(get_major_version("1.20.4"), "1.20");
        assert_eq!(get_major_version("1.21"), "1.21");
        assert_eq!(get_major_version("23w14a"), "23w14a");
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(
            compare_versions("1.20.4", "1.20.4"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.21", "1.20.4"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("1.19", "1.20"), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_argument_value_default() {
        let av = ArgumentValue::default();
        assert!(matches!(av, ArgumentValue::String(s) if s.is_empty()));
    }

    #[test]
    fn test_argument_object_value_default() {
        let av = ArgumentObjectValue::default();
        assert!(matches!(av, ArgumentObjectValue::String(s) if s.is_empty()));
    }
}
