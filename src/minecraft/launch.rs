use crate::instance::path::InstanceSubdir;
use crate::minecraft::arguments::{ArgumentEngine, build_replacement_map};
use crate::minecraft::version::VersionInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default maximum memory in MB when auto-memory is enabled.
pub const DEFAULT_AUTO_MEMORY_MB: u32 = 2048;
/// Default Minecraft client main class.
pub const DEFAULT_MAIN_CLASS: &str = "net.minecraft.client.main.Main";

/// Launch arguments for the Minecraft client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LaunchArguments {
    /// JVM arguments (memory, GC, system properties).
    pub jvm_args: Vec<String>,
    /// Classpath entries.
    pub class_paths: Vec<String>,
    /// The main class to launch.
    pub main_class: String,
    /// Game arguments (username, version, etc.).
    pub game_args: Vec<String>,
    /// Environment variables to set.
    pub env: HashMap<String, String>,
}

/// Result of generating launch arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    /// The Java executable path.
    pub java_exec: String,
    /// Combined arguments for the command line.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Working directory.
    pub work_dir: PathBuf,
}

/// Window/performance parameters for launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WindowParams {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub max_memory_mb: u32,
    pub auto_join_server: Option<String>,
}

/// Errors during launch argument generation.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum LaunchArgsError {
    #[error("missing required field: {0}")]
    MissingField(String),
    #[error("invalid version info: {0}")]
    InvalidVersionInfo(String),
}

/// Generates a full launch command for a Minecraft client.
///
/// Uses the argument engine for rules + substitution, builds the classpath
/// from libraries, and handles both legacy and modern argument formats.
///
/// `username` is used to derive an offline-mode UUID for authentication
/// placeholders in launch arguments.
pub fn generate_launch_command(
    version_info: &VersionInfo,
    java_exec: &str,
    game_dir: &Path,
    username: &str,
    window: &WindowParams,
) -> Result<LaunchCommand, LaunchArgsError> {
    let mut jvm_args = Vec::new();
    let mut game_args = Vec::new();

    // Memory
    jvm_args.push(format!("-Xmx{}m", window.max_memory_mb));

    // Classpath
    let libraries_dir = InstanceSubdir::Libraries.resolve(game_dir);
    let class_paths = ArgumentEngine::build_classpath(version_info, &libraries_dir, game_dir);
    let class_paths_str: Vec<String> = class_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    // Main class
    let main_class = version_info
        .main_class
        .clone()
        .unwrap_or_else(|| DEFAULT_MAIN_CLASS.to_string());

    // Build replacement map using offline-mode auth
    let auth = AuthParams::new(username);
    let replacements = build_replacement_map(version_info, game_dir, &auth, window);

    // Resolve arguments
    jvm_args.extend(ArgumentEngine::resolve_jvm_args(
        version_info,
        &replacements,
    ));
    game_args.extend(ArgumentEngine::resolve_game_args(
        version_info,
        &replacements,
    ));

    // Window settings
    if window.fullscreen {
        game_args.push("--fullscreen".to_string());
    } else {
        game_args.push("--width".to_string());
        game_args.push(window.width.to_string());
        game_args.push("--height".to_string());
        game_args.push(window.height.to_string());
    }

    // Quick play / auto-join
    if let Some(server) = &window.auto_join_server {
        game_args.push("--server".to_string());
        game_args.push(server.clone());
    }

    // Build the full command
    let mut all_args = jvm_args.clone();
    all_args.push("-cp".to_string());
    all_args.push(class_paths_str.join(if cfg!(windows) { ";" } else { ":" }));
    all_args.push(main_class);
    all_args.extend(game_args.clone());

    Ok(LaunchCommand {
        java_exec: java_exec.to_string(),
        args: all_args,
        env: HashMap::new(),
        work_dir: game_dir.to_path_buf(),
    })
}

/// Minimal offline-mode auth parameters for launch argument substitution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AuthParams {
    pub username: String,
    pub uuid: String,
}

impl AuthParams {
    /// Creates auth params for the given username, deriving an offline UUID.
    #[must_use]
    pub fn new(username: &str) -> Self {
        Self {
            username: username.to_string(),
            uuid: username_to_offline_uuid(username),
        }
    }
}

/// Converts a username to an offline-mode UUID (version 3 MD5 hash).
///
/// This matches the algorithm used by the official Minecraft launcher
/// for offline mode: `OfflinePlayer:<username>` → MD5 → format as UUID.
#[must_use]
pub fn username_to_offline_uuid(username: &str) -> String {
    let input = format!("OfflinePlayer:{username}");
    let hash = md5::compute(input.as_bytes());
    let mut b = hash.0;
    b[6] = (b[6] & 0x0f) | 0x30; // version 3
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_launch_command_legacy() {
        let version_info = VersionInfo {
            id: "1.12.2".into(),
            version_type: "release".into(),
            asset_index: crate::minecraft::version::AssetIndex {
                id: "legacy".into(),
                ..Default::default()
            },
            minecraft_arguments: Some(
                "${auth_player_name} ${version_name} ${game_directory} ${assets_root} ${assets_index_name} ${user_type} ${version_type}".into(),
            ),
            main_class: Some("net.minecraft.client.main.Main".into()),
            ..Default::default()
        };

        let window = WindowParams {
            width: 800,
            height: 600,
            fullscreen: false,
            max_memory_mb: 2048,
            auto_join_server: Some("mc.example.com".into()),
        };

        let cmd = generate_launch_command(
            &version_info,
            "/usr/bin/java",
            std::path::Path::new("/game"),
            "Steve",
            &window,
        )
        .unwrap();

        assert_eq!(cmd.java_exec, "/usr/bin/java");
        assert!(cmd.args.contains(&"-Xmx2048m".to_string()));
        assert!(cmd.args.contains(&"--server".to_string()));
        assert!(cmd.args.contains(&"mc.example.com".to_string()));
        assert!(cmd.args.contains(&"--width".to_string()));
        assert!(cmd.args.contains(&"800".to_string()));
        assert!(cmd.args.contains(&"--height".to_string()));
        assert!(cmd.args.contains(&"600".to_string()));
        assert!(cmd.args.contains(&"Steve".to_string()));
    }

    #[test]
    fn test_generate_launch_command_modern() {
        let version_info = VersionInfo {
            id: "1.20.4".into(),
            version_type: "release".into(),
            asset_index: crate::minecraft::version::AssetIndex {
                id: "12".into(),
                ..Default::default()
            },
            arguments: Some(crate::minecraft::version::Arguments {
                game: vec![
                    crate::minecraft::version::ArgumentValue::String("--username".into()),
                    crate::minecraft::version::ArgumentValue::String("Steve".into()),
                ],
                jvm: vec![crate::minecraft::version::ArgumentValue::String(
                    "-XX:+UseG1GC".into(),
                )],
            }),
            main_class: Some("net.minecraft.client.main.Main".into()),
            ..Default::default()
        };

        let window = WindowParams {
            fullscreen: true,
            max_memory_mb: 4096,
            ..Default::default()
        };

        let cmd = generate_launch_command(
            &version_info,
            "java",
            std::path::Path::new("/game"),
            "",
            &window,
        )
        .unwrap();

        assert!(cmd.args.contains(&"-Xmx4096m".to_string()));
        assert!(cmd.args.contains(&"--fullscreen".to_string()));
        assert!(!cmd.args.contains(&"--width".to_string()));
    }

    #[test]
    fn test_launch_args_error_display() {
        let e1 = LaunchArgsError::MissingField("main_class".into());
        assert!(format!("{e1}").contains("main_class"));

        let e2 = LaunchArgsError::InvalidVersionInfo("bad json".into());
        assert!(format!("{e2}").contains("bad json"));
    }

    #[test]
    fn test_generate_launch_command_no_main_class() {
        let version_info = VersionInfo {
            id: "1.20.4".into(),
            version_type: "release".into(),
            asset_index: crate::minecraft::version::AssetIndex {
                id: "12".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let window = WindowParams::default();

        let cmd = generate_launch_command(
            &version_info,
            "java",
            std::path::Path::new("/game"),
            "",
            &window,
        )
        .unwrap();

        assert!(
            cmd.args
                .contains(&"net.minecraft.client.main.Main".to_string())
        );
    }

    #[test]
    fn test_generate_launch_command_no_auto_join() {
        let version_info = VersionInfo {
            id: "1.20.4".into(),
            version_type: "release".into(),
            asset_index: crate::minecraft::version::AssetIndex {
                id: "12".into(),
                ..Default::default()
            },
            arguments: Some(crate::minecraft::version::Arguments {
                game: vec![],
                jvm: vec![],
            }),
            main_class: Some("net.minecraft.client.main.Main".into()),
            ..Default::default()
        };

        let window = WindowParams {
            width: 800,
            height: 600,
            fullscreen: false,
            max_memory_mb: 1024,
            auto_join_server: None,
        };

        let cmd = generate_launch_command(
            &version_info,
            "java",
            std::path::Path::new("/game"),
            "",
            &window,
        )
        .unwrap();

        assert!(!cmd.args.contains(&"--server".to_string()));
        assert!(cmd.args.contains(&"--width".to_string()));
    }
}
