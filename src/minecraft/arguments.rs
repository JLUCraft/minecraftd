use crate::instance::path::InstanceSubdir;
use crate::minecraft::library::evaluate_rules;
use crate::minecraft::version::LibraryInfo;
use crate::minecraft::version::{ArgumentObjectValue, ArgumentValue, VersionInfo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resolves JVM and game arguments from a version info, applying rules and substitutions.
pub struct ArgumentEngine;

impl ArgumentEngine {
    /// Resolves the full classpath for a Minecraft client launch.
    ///
    /// Includes: libraries (filtered by rules) + client jar.
    #[must_use]
    pub fn build_classpath(
        version_info: &VersionInfo,
        libraries_dir: &Path,
        game_dir: &Path,
    ) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        for lib in &version_info.libraries {
            if !lib.is_allowed() {
                continue;
            }
            if lib.is_native() {
                continue;
            }
            if lib.artifact_download().is_some() {
                paths.push(libraries_dir.join(crate::minecraft::library::artifact_path(&lib.name)));
            }
        }

        // Client jar
        let client_jar = game_dir
            .join("versions")
            .join(&version_info.id)
            .join(format!("{}.jar", version_info.id));
        paths.push(client_jar);

        paths
    }

    /// Resolves JVM arguments from version info, applying rules and substitutions.
    #[must_use]
    pub fn resolve_jvm_args(
        version_info: &VersionInfo,
        replacements: &HashMap<String, String>,
    ) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(arguments) = &version_info.arguments {
            args.extend(resolve_arguments(&arguments.jvm, replacements));
        }

        args
    }

    /// Resolves game arguments from version info, applying rules and substitutions.
    #[must_use]
    pub fn resolve_game_args(
        version_info: &VersionInfo,
        replacements: &HashMap<String, String>,
    ) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(mc_args) = &version_info.minecraft_arguments {
            // Legacy format (pre-1.13): simple placeholder replacement
            args.extend(replace_legacy_placeholders(mc_args, replacements));
        } else if let Some(arguments) = &version_info.arguments {
            // Modern format (1.13+)
            args.extend(resolve_arguments(&arguments.game, replacements));
        } else {
            // No arguments section in version info
        }

        args
    }

    /// Builds the natives directory path for a version.
    #[must_use]
    pub fn natives_dir(game_dir: &Path, version_id: &str) -> PathBuf {
        game_dir.join("versions").join(version_id).join("natives")
    }

    /// Collects all native libraries that need extraction.
    #[must_use]
    pub fn collect_native_libraries<'info>(
        version_info: &'info VersionInfo,
        libraries_dir: &Path,
    ) -> Vec<(PathBuf, &'info LibraryInfo)> {
        let mut natives = Vec::new();
        for lib in &version_info.libraries {
            if !lib.is_allowed() || !lib.is_native() {
                continue;
            }
            if let Some(path) = lib.native_path(libraries_dir) {
                natives.push((path, lib));
            }
        }
        natives
    }
}

/// Resolves a list of argument values, applying rules and substitutions.
fn resolve_arguments(
    args: &[ArgumentValue],
    replacements: &HashMap<String, String>,
) -> Vec<String> {
    let mut result = Vec::new();

    for arg in args {
        match arg {
            ArgumentValue::String(s) => {
                result.push(substitute(s, replacements));
            }
            ArgumentValue::Object(obj) => {
                if evaluate_rules(&obj.rules) {
                    match &obj.value {
                        ArgumentObjectValue::String(s) => {
                            result.push(substitute(s, replacements));
                        }
                        ArgumentObjectValue::Array(arr) => {
                            for s in arr {
                                result.push(substitute(s, replacements));
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

/// Substitutes placeholders in a string.
fn substitute(input: &str, replacements: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in replacements {
        result = result.replace(key, value);
    }
    result
}

/// Replaces legacy placeholders (pre-1.13 format) by splitting on whitespace.
fn replace_legacy_placeholders(input: &str, replacements: &HashMap<String, String>) -> Vec<String> {
    input
        .split_whitespace()
        .map(|part| substitute(part, replacements))
        .collect()
}

/// Builds the standard replacement map for launch arguments.
#[must_use]
pub fn build_replacement_map(
    version_info: &VersionInfo,
    game_dir: &Path,
    auth: &crate::minecraft::launch::AuthParams,
    window: &crate::minecraft::launch::WindowParams,
) -> HashMap<String, String> {
    let mut map = HashMap::new();

    map.insert("${auth_player_name}".to_string(), auth.username.clone());
    map.insert("${auth_uuid}".to_string(), auth.uuid.clone());
    // Offline mode always uses a zero token and "legacy" user type.
    map.insert("${auth_access_token}".to_string(), "0".repeat(32));
    map.insert("${user_type}".to_string(), "legacy".to_string());
    map.insert("${version_name}".to_string(), version_info.id.clone());
    map.insert(
        "${assets_index_name}".to_string(),
        version_info.asset_index.id.clone(),
    );
    map.insert(
        "${game_directory}".to_string(),
        game_dir.to_string_lossy().to_string(),
    );
    map.insert(
        "${assets_root}".to_string(),
        InstanceSubdir::Assets
            .resolve(game_dir)
            .to_string_lossy()
            .to_string(),
    );
    map.insert(
        "${version_type}".to_string(),
        version_info.version_type.clone(),
    );

    // Native path
    let natives_dir = ArgumentEngine::natives_dir(game_dir, &version_info.id);
    map.insert(
        "${natives_directory}".to_string(),
        natives_dir.to_string_lossy().to_string(),
    );

    // Launcher name/version (for compatibility)
    map.insert("${launcher_name}".to_string(), "minecraftd".to_string());
    map.insert(
        "${launcher_version}".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    // Window params
    map.insert("${resolution_width}".to_string(), window.width.to_string());
    map.insert(
        "${resolution_height}".to_string(),
        window.height.to_string(),
    );

    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::launch::{AuthParams, WindowParams};
    use crate::minecraft::version::{ArgumentObject, DownloadInfo, LibraryDownloads, Rule};

    #[test]
    fn test_build_classpath_filters_natives() {
        let version_info = crate::minecraft::version::VersionInfo {
            id: "1.20.4".into(),
            libraries: vec![
                LibraryInfo {
                    name: "org.lwjgl:lwjgl:3.3.2".into(),
                    downloads: Some(LibraryDownloads {
                        artifact: Some(DownloadInfo {
                            sha1: "abc".into(),
                            size: 100,
                            url: "http://example.com/lwjgl.jar".into(),
                        }),
                        classifiers: None,
                    }),
                    rules: None,
                    ..Default::default()
                },
                LibraryInfo {
                    name: "org.lwjgl:lwjgl:3.3.2".into(),
                    downloads: Some(LibraryDownloads {
                        artifact: Some(DownloadInfo {
                            sha1: "abc".into(),
                            size: 100,
                            url: "http://example.com/lwjgl.jar".into(),
                        }),
                        classifiers: Some({
                            let mut m = std::collections::HashMap::new();
                            m.insert(
                                crate::platform::native_classifier(),
                                DownloadInfo {
                                    sha1: "def".into(),
                                    size: 50,
                                    url: "http://example.com/lwjgl-natives.jar".into(),
                                },
                            );
                            m
                        }),
                    }),
                    natives: Some({
                        let mut m = std::collections::HashMap::new();
                        m.insert(
                            crate::platform::current_os_name().into(),
                            "natives-macos".into(),
                        );
                        m
                    }),
                    rules: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let cp =
            ArgumentEngine::build_classpath(&version_info, Path::new("/libs"), Path::new("/game"));
        // Should have 2 entries: 1 non-native library + client jar
        assert_eq!(cp.len(), 2);
        assert!(cp[1].to_string_lossy().contains("1.20.4.jar"));
    }

    #[test]
    fn test_resolve_arguments_with_rules() {
        let args = vec![
            ArgumentValue::String("-Xmx2G".into()),
            ArgumentValue::Object(ArgumentObject {
                rules: vec![Rule {
                    action: "allow".into(),
                    os: Some(crate::minecraft::version::OsCondition {
                        name: Some("windows".into()),
                        ..Default::default()
                    }),
                    features: None,
                }],
                value: ArgumentObjectValue::String("-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump".into()),
            }),
        ];

        let replacements = HashMap::new();
        let resolved = resolve_arguments(&args, &replacements);

        assert!(resolved.contains(&"-Xmx2G".to_string()));
        // The windows-only arg should only appear on windows
        #[cfg(target_os = "windows")]
        assert_eq!(resolved.len(), 2);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn test_substitute_placeholders() {
        let mut map = HashMap::new();
        map.insert("${name}".to_string(), "Steve".to_string());

        assert_eq!(substitute("hello ${name}", &map), "hello Steve");
        assert_eq!(substitute("no placeholders", &map), "no placeholders");
    }

    #[test]
    fn test_natives_dir() {
        let dir = ArgumentEngine::natives_dir(Path::new("/game"), "1.20.4");
        let lossy = dir.to_string_lossy();
        assert!(
            lossy.contains("versions") && lossy.contains("1.20.4") && lossy.contains("natives")
        );
    }

    #[test]
    fn test_build_replacement_map() {
        let version_info = crate::minecraft::version::VersionInfo {
            id: "1.20.4".into(),
            version_type: "release".into(),
            asset_index: crate::minecraft::version::AssetIndex {
                id: "12".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let auth = AuthParams {
            username: "Steve".into(),
            uuid: "uuid".into(),
        };

        let window = WindowParams {
            width: 800,
            height: 600,
            ..Default::default()
        };

        let map = build_replacement_map(&version_info, Path::new("/game"), &auth, &window);
        assert_eq!(map.get("${auth_player_name}").unwrap(), "Steve");
        assert_eq!(map.get("${version_name}").unwrap(), "1.20.4");
        assert_eq!(map.get("${resolution_width}").unwrap(), "800");
    }
}
