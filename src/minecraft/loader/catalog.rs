//! Loader version discovery and ordering shared by every client.
use crate::minecraft::loader::installer::LoaderInstallError;
use crate::minecraft::loader::libraries::MetadataClient;
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::profile::OptiFineResourceInfo;
use crate::resource::{ResourceType, SourceType, get_download_api};
use serde::{Deserialize, Serialize};
use serde_json::Value;
type Result<T> = std::result::Result<T, LoaderInstallError>;
fn parse_error(error: impl std::fmt::Display) -> LoaderInstallError {
    LoaderInstallError::Parse(error.to_string())
}
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModLoaderResourceInfo {
    pub loader_type: ModLoaderType,
    pub version: String,
    pub description: String,
    pub stable: Option<bool>,
    pub branch: Option<String>,
}

pub async fn versions(
    client: &impl MetadataClient,
    priority: &[SourceType],
    loader: ModLoaderType,
    game_version: &str,
) -> Result<Vec<ModLoaderResourceInfo>> {
    if loader == ModLoaderType::Cleanroom && game_version != "1.12.2" {
        return Ok(Vec::new());
    }
    let mut last_error = LoaderInstallError::Network("no usable download source".into());
    for source in priority {
        let endpoint = match loader {
            ModLoaderType::Fabric => ResourceType::FabricMeta,
            ModLoaderType::Quilt => ResourceType::QuiltMeta,
            ModLoaderType::Forge | ModLoaderType::LegacyForge => ResourceType::ForgeMeta,
            ModLoaderType::Cleanroom => ResourceType::CleanroomMeta,
            ModLoaderType::NeoForge
                if game_version == "1.20.1" && *source == SourceType::Official =>
            {
                ResourceType::NeoforgeMetaForge
            }
            ModLoaderType::NeoForge => ResourceType::NeoforgeMetaNeoforge,
            _ => return Err(LoaderInstallError::Unsupported(loader.to_string())),
        };
        let Ok(root) = get_download_api(*source, endpoint) else {
            continue;
        };
        let suffix = match loader {
            ModLoaderType::Fabric => format!("v2/versions/loader/{game_version}"),
            ModLoaderType::Quilt => format!("v3/versions/loader/{game_version}"),
            ModLoaderType::Forge | ModLoaderType::LegacyForge => {
                format!("minecraft/{game_version}")
            }
            ModLoaderType::NeoForge if *source == SourceType::Bmclapi => {
                format!("list/{game_version}")
            }
            _ => String::new(),
        };
        let url = if suffix.is_empty() {
            root
        } else {
            root.join(&suffix)?
        };
        let result = match client.get_json(url).await {
            Ok(value) => parse_versions(&value, loader, *source, game_version),
            Err(error) => Err(error),
        };
        match result {
            Ok(versions) => return Ok(versions),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}
pub fn parse_versions(
    value: &Value,
    loader: ModLoaderType,
    source: SourceType,
    game_version: &str,
) -> Result<Vec<ModLoaderResourceInfo>> {
    if loader == ModLoaderType::NeoForge && source == SourceType::Official {
        return neoforge_versions(value, game_version);
    }
    let entries = value
        .as_array()
        .ok_or_else(|| parse_error("expected loader version list"))?;
    let mut entries: Vec<&Value> = entries.iter().collect();
    match loader {
        ModLoaderType::Forge | ModLoaderType::LegacyForge => entries.sort_by_key(|item| {
            std::cmp::Reverse(
                item.get("build")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
        }),
        ModLoaderType::Cleanroom => {
            entries.sort_by_key(|item| std::cmp::Reverse(text(item, "created_at")))
        }
        _ => (),
    }
    let mut result: Vec<ModLoaderResourceInfo> = entries
        .into_iter()
        .map(|item| {
            let details = if matches!(loader, ModLoaderType::Fabric | ModLoaderType::Quilt) {
                item.get("loader").unwrap_or(item)
            } else {
                item
            };
            let version = text(
                details,
                if loader == ModLoaderType::Cleanroom {
                    "name"
                } else {
                    "version"
                },
            );
            ModLoaderResourceInfo {
                loader_type: loader,
                stable: if matches!(loader, ModLoaderType::Quilt | ModLoaderType::NeoForge) {
                    Some(
                        !version.contains("alpha")
                            && !version.contains("beta")
                            && !version.contains("rc"),
                    )
                } else {
                    None
                },
                version,
                description: match loader {
                    ModLoaderType::Cleanroom => text(item, "created_at"),
                    ModLoaderType::Forge | ModLoaderType::LegacyForge => text(item, "modified"),
                    _ => String::new(),
                },
                branch: item
                    .get("branch")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        })
        .collect();
    if result.iter().any(|item| item.version.is_empty()) {
        return Err(parse_error("missing loader version"));
    }
    if matches!(loader, ModLoaderType::Quilt | ModLoaderType::NeoForge) {
        result.sort_by(|a, b| {
            match (
                semver::Version::parse(&a.version),
                semver::Version::parse(&b.version),
            ) {
                (Ok(a), Ok(b)) => b.cmp(&a),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => b.version.cmp(&a.version),
            }
        });
    }
    Ok(result)
}
pub async fn optifine_versions(
    client: &impl MetadataClient,
    priority: &[SourceType],
    game_version: &str,
) -> Result<Vec<OptiFineResourceInfo>> {
    if !priority.contains(&SourceType::Bmclapi) {
        return Err(LoaderInstallError::Network(
            "OptiFine mirror not enabled".into(),
        ));
    }
    let url = get_download_api(SourceType::Bmclapi, ResourceType::OptiFine)?.join(game_version)?;
    let mut entries: Vec<OptiFineResourceInfo> =
        serde_json::from_value(client.get_json(url).await?)?;
    for entry in &mut entries {
        entry.filename = std::path::Path::new(&entry.filename)
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| parse_error("invalid OptiFine filename"))?;
    }
    entries.sort_by(|a, b| {
        get_optifine_sort_key(b)
            .cmp(&get_optifine_sort_key(a))
            .then_with(|| b.filename.cmp(&a.filename))
    });
    Ok(entries)
}
fn neoforge_versions(versions: &Value, game_version: &str) -> Result<Vec<ModLoaderResourceInfo>> {
    let old_version_regex =
        regex::Regex::new(r"^(?:1\.20\.1\-)?(\d+)\.(\d+)\.(\d+)$").map_err(parse_error)?;
    let regular_version_regex = regex::Regex::new(r"^(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?(?:-(alpha|beta|rc)(?:\.(\d+))?)?(?:\+(snapshot|pre|rc)-(\d+))?$").map_err(parse_error)?;
    let april_fools_version_regex =
        regex::Regex::new(r"^0\.(\d+\w+)\.(\d+)(-beta)?$").map_err(parse_error)?;
    if game_version == "1.20.1" {
        let Some(version_list) = versions.get("versions").and_then(|v| v.as_array()) else {
            return Err(parse_error("missing versions"));
        };

        let mut results = Vec::new();
        for version_value in version_list {
            if let Some(version) = version_value.as_str()
                && let Some(cap) = old_version_regex.captures(version)
            {
                let major: i32 = cap[1].parse()?;
                let minor: i32 = cap[2].parse()?;
                let patch: i32 = cap[3].parse()?;

                results.push((
                    (major, minor, patch),
                    ModLoaderResourceInfo {
                        loader_type: ModLoaderType::NeoForge,
                        version: version.to_string(),
                        description: String::new(),
                        stable: Some(
                            versions
                                .get("is_snapshot")
                                .is_none_or(|v| !v.as_bool().unwrap_or(false)),
                        ),
                        branch: None,
                    },
                ));
            }
        }

        results.sort_by_key(|b| std::cmp::Reverse(b.0));
        return Ok(results.into_iter().map(|r| r.1).collect());
    }
    let Some(version_list) = versions.get("versions").and_then(|v| v.as_array()) else {
        return Err(parse_error("missing versions"));
    };

    let mut results: Vec<(Vec<i32>, ModLoaderResourceInfo)> = Vec::new();

    for version_value in version_list {
        if let Some(version) = version_value.as_str() {
            let (is_april_fools, cap_opt) =
                april_fools_version_regex.captures(version).map_or_else(
                    || (false, regular_version_regex.captures(version)),
                    |cap| (true, Some(cap)),
                );

            if let Some(cap) = cap_opt {
                let matches_game_version = if is_april_fools {
                    *game_version == cap[1]
                } else {
                    let major: i32 = cap[1].parse()?;
                    let minor: i32 = cap[2].parse()?;
                    let patch: i32 = cap[3].parse()?;

                    if game_version.starts_with("1.") {
                        game_version == format!("1.{major}.{minor}")
                            || (minor == 0 && game_version == format!("1.{major}"))
                    } else {
                        let mut derived = if patch != 0 {
                            format!("{}.{}.{}", major, minor, patch)
                        } else {
                            format!("{}.{}", major, minor)
                        };

                        // cap[7]=snapshot|pre|rc, cap[8]=N
                        if let (Some(kind), Some(n)) = (cap.get(7), cap.get(8)) {
                            derived.push('-');
                            derived.push_str(kind.as_str());
                            derived.push('-');
                            derived.push_str(n.as_str());
                        }

                        game_version == derived
                    }
                };

                if matches_game_version {
                    let sort_key: Vec<i32> = if is_april_fools {
                        vec![cap[2].parse()?]
                    } else {
                        let patch: i32 = cap[3].parse()?;
                        let build: i32 = cap.get(4).map_or(Ok(0), |m| m.as_str().parse::<i32>())?;
                        let release_rank = match cap.get(5).map(|m| m.as_str()) {
                            None => 3,
                            Some("rc") => 2,
                            Some("beta") => 1,
                            Some("alpha") => 0,
                            Some(_) => 0,
                        };
                        let prerelease: i32 =
                            cap.get(6).map_or(Ok(0), |m| m.as_str().parse::<i32>())?;
                        vec![patch, build, release_rank, prerelease]
                    };

                    let stable = if is_april_fools {
                        cap.get(3).is_none()
                    } else {
                        cap.get(5).is_none()
                    };

                    results.push((
                        sort_key,
                        ModLoaderResourceInfo {
                            loader_type: ModLoaderType::NeoForge,
                            version: version.to_string(),
                            description: String::new(),
                            stable: Some(stable),
                            branch: None,
                        },
                    ));
                }
            }
        }
    }

    results.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(results.into_iter().map(|r| r.1).collect())
}
fn get_optifine_sort_key(info: &OptiFineResourceInfo) -> (u32, u32, u32) {
    let Some((_, suffix)) = info.filename.rsplit_once("_HD_U_") else {
        return (0, 0, 0);
    };
    let (version, pre) = match suffix.split_once("_pre") {
        Some((version, pre)) => (version, pre.parse().unwrap_or(0)),
        None => (suffix, u32::MAX),
    };
    let mut chars = version.chars();
    let prefix = chars
        .next()
        .map(|ch| ch.to_ascii_uppercase() as u32)
        .unwrap_or(0);
    let series = chars.as_str().parse().unwrap_or(0);
    (prefix, series, pre)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neoforge_releases_snapshots_and_april_fools() {
        let versions = serde_json::json!({"versions":["21.0.20", "21.0.10-beta", "26.1.0.2+snapshot-3", "0.25w14craftmine.3-beta", "21.1.7"]});
        let releases = parse_versions(
            &versions,
            ModLoaderType::NeoForge,
            SourceType::Official,
            "1.21",
        )
        .unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "21.0.20");
        assert_eq!(releases[1].stable, Some(false));
        assert_eq!(
            parse_versions(
                &versions,
                ModLoaderType::NeoForge,
                SourceType::Official,
                "26.1-snapshot-3"
            )
            .unwrap()[0]
                .version,
            "26.1.0.2+snapshot-3"
        );
        assert_eq!(
            parse_versions(
                &versions,
                ModLoaderType::NeoForge,
                SourceType::Official,
                "25w14craftmine"
            )
            .unwrap()[0]
                .version,
            "0.25w14craftmine.3-beta"
        );
    }
    #[test]
    fn quilt_without_intermediary_and_forge_branch() {
        let quilt = serde_json::json!([{"loader":{"version":"0.2.0-beta.1"}}, {"loader":{"version":"0.2.0"}}]);
        assert_eq!(
            parse_versions(&quilt, ModLoaderType::Quilt, SourceType::Official, "26.1").unwrap()[0]
                .version,
            "0.2.0"
        );
        let forge = serde_json::json!([{"version":"1.0", "branch":"legacy", "build":1}]);
        assert_eq!(
            parse_versions(&forge, ModLoaderType::Forge, SourceType::Bmclapi, "1.7.10").unwrap()[0]
                .branch
                .as_deref(),
            Some("legacy")
        );
    }
}
