//! Filesystem-based Minecraft instance discovery (cross-platform).
//!
//! Scans the official Minecraft launcher data directory:
//!
//! | Platform | Path                                      |
//! |----------|-------------------------------------------|
//! | Windows  | `%APPDATA%\.minecraft`                    |
//! | macOS    | `~/Library/Application Support/minecraft` |
//! | Linux    | `~/.minecraft`                            |
//!
//! Minecraft instances are classified into three kinds:
//!
//! | Kind             | Detected by                                             |
//! |------------------|---------------------------------------------------------|
//! | `Client`         | client jar, options.txt, saves/, resourcepacks/, etc.  |
//! | `Server`         | server.properties, eula.txt, plugins/                   |
//! | `ClientAndServer`| mix of both client and server indicators                |
//!
//! This mirrors minecraftd's philosophy: client and server are siblings,
//! not master/slave. Both are instances managed by the same lifecycle.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::instance::path::InstanceSubdir;

/// Classification of a discovered Minecraft instance.
///
/// An instance may be a pure client, pure server, or a hybrid that
/// can serve both roles (e.g., a modded creative server the host also connects to).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceKind {
    /// Pure client-side instance (singleplayer worlds, resource packs, etc.).
    Client,
    /// Pure server-side instance (dedicated server, Bukkit/Paper, etc.).
    Server,
    /// Hybrid instance that has both client and server indicators.
    ClientAndServer,
}

impl InstanceKind {
    /// Human-readable label for display.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "Client",
            Self::Server => "Server",
            Self::ClientAndServer => "Client+Server",
        }
    }
}

/// A discovered local Minecraft instance.
#[derive(Debug, Clone)]
pub struct DiscoveredInstance {
    /// Directory name (also used as instance ID).
    pub id: String,
    /// Full path to the instance directory.
    pub path: PathBuf,
    /// Whether this is client, server, or both.
    pub kind: InstanceKind,
    /// World directory names found (e.g. `["world"]`).
    pub worlds: Vec<String>,
}

/// Discover all local Minecraft instances under the standard data directories.
pub fn discover() -> Vec<DiscoveredInstance> {
    scan_dirs(&minecraft_base_dirs())
}

/// Discover version-isolated client instances under the given base directories.
///
/// Unlike [`discover`], this only scans `versions/<id>/` subdirectories and
/// applies the strict launcher-style validation (`<id>.jar` and `<id>.json`
/// both present), so worlds and loose directories are never reported.
///
/// Base directories without a `versions/` subdirectory are silently skipped.
#[must_use]
pub fn scan_version_instances(bases: &[PathBuf]) -> Vec<DiscoveredInstance> {
    let mut instances: Vec<DiscoveredInstance> = Vec::new();

    for base in bases {
        let versions_dir = base.join("versions");
        if versions_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&versions_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let id = file_name(&path);

                // Strict client validation: both `<id>.jar` and `<id>.json`.
                if !path.join(format!("{id}.jar")).is_file()
                    || !path.join(format!("{id}.json")).is_file()
                {
                    continue;
                }

                let instance_saves = path.join(InstanceSubdir::Saves.dir_name());
                let mut worlds: Vec<String> = Vec::new();
                if let Ok(save_entries) = std::fs::read_dir(&instance_saves) {
                    for save_entry in save_entries.flatten() {
                        let save_path = save_entry.path();
                        if save_path.is_dir() && save_path.join("level.dat").is_file() {
                            worlds.push(file_name(&save_path));
                        }
                    }
                }

                instances.push(DiscoveredInstance {
                    id,
                    path,
                    kind: InstanceKind::Client,
                    worlds,
                });
            }
        }
    }

    instances.sort_by(|a, b| a.id.cmp(&b.id));
    instances
}

/// List of well-known directories that are scanned by `discover()`.
pub fn known_dirs() -> Vec<PathBuf> {
    minecraft_base_dirs()
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

fn scan_dirs(bases: &[PathBuf]) -> Vec<DiscoveredInstance> {
    let mut instances: Vec<DiscoveredInstance> = Vec::new();

    for base in bases {
        if !base.is_dir() {
            continue;
        }

        // ── Pass 1: `saves/` — each subdirectory is a distinct world ──
        //
        // Worlds are inherently client-side; they represent single-player
        // gameplay data. A world directory alone does not run a server.
        let saves_dir = base.join(InstanceSubdir::Saves.dir_name());
        if saves_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&saves_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() || !path.join("level.dat").is_file() {
                    continue;
                }
                let id = file_name(&path);
                let worlds = detect_worlds_in_dir(&path);
                let worlds = if worlds.is_empty() {
                    vec![id.clone()]
                } else {
                    worlds
                };
                instances.push(DiscoveredInstance {
                    id,
                    path,
                    kind: InstanceKind::Client,
                    worlds,
                });
            }
        }

        // ── Pass 2: `versions/{id}/` — version-isolated profiles ──
        //
        // Each subdirectory of `versions/` is a potential instance.
        // We check for both client and server indicators independently.
        let versions_dir = base.join("versions");
        if versions_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&versions_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let id = file_name(&path);
                let Some(kind) = classify_instance_dir(&path, Some(&id)) else {
                    continue;
                };

                // Collect worlds from saves/
                let instance_saves = path.join(InstanceSubdir::Saves.dir_name());
                let mut worlds: Vec<String> = Vec::new();
                if let Ok(save_entries) = std::fs::read_dir(&instance_saves) {
                    for save_entry in save_entries.flatten() {
                        let save_path = save_entry.path();
                        if save_path.is_dir() && save_path.join("level.dat").is_file() {
                            worlds.push(file_name(&save_path));
                        }
                    }
                }

                instances.push(DiscoveredInstance {
                    id,
                    path,
                    kind,
                    worlds,
                });
            }
        }

        // ── Pass 3: Top-level — server dirs or loose client dirs ──
        //
        // Scan the base directory for entries that aren't standard
        // Mojang subdirectories. These may be dedicated servers or
        // non-standard client installations.
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let id = file_name(&path);

                // Skip known Mojang standard subdirectories
                if is_standard_dir(&id) {
                    continue;
                }

                let Some(kind) = classify_instance_dir(&path, None) else {
                    continue;
                };

                let worlds = detect_worlds_in_dir(&path);
                instances.push(DiscoveredInstance {
                    id,
                    path,
                    kind,
                    worlds,
                });
            }
        }
    }

    instances.sort_by(|a, b| a.id.cmp(&b.id));
    instances.dedup_by(|a, b| a.path == b.path);
    instances
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Inspects a directory and returns its `InstanceKind` based on file-system
/// indicators.
///
/// Checks for client and server indicators **independently**, so a directory
/// that has both `{id}.jar` and `server.properties` is classified as
/// `ClientAndServer`.
///
/// `version_id` is only needed for `versions/{id}/` directories where the
/// client jar is named `{id}.jar`. Pass `None` for top-level scans.
fn classify_instance_dir(path: &Path, version_id: Option<&str>) -> Option<InstanceKind> {
    let is_client = has_client_indicators(path, version_id);
    let is_server = has_server_indicators(path);

    match (is_client, is_server) {
        (true, true) => Some(InstanceKind::ClientAndServer),
        (true, false) => Some(InstanceKind::Client),
        (false, true) => Some(InstanceKind::Server),
        (false, false) => None,
    }
}

/// Checks for signs that this directory is a Minecraft **client** instance.
fn has_client_indicators(path: &Path, version_id: Option<&str>) -> bool {
    // 1. Client jar: versions/<id>/<id>.jar
    if let Some(vid) = version_id
        && path.join(format!("{vid}.jar")).is_file()
    {
        return true;
    }

    // 2. Client options file
    if path.join("options.txt").is_file() {
        return true;
    }

    // 3. Server list (connects to multiplayer)
    if path.join("servers.dat").is_file() {
        return true;
    }

    // 4. Saves with level.dat inside (single-player worlds)
    let saves = path.join(InstanceSubdir::Saves.dir_name());
    if saves.is_dir()
        && let Ok(entries) = std::fs::read_dir(&saves)
        && entries
            .flatten()
            .any(|e| e.path().is_dir() && e.path().join("level.dat").is_file())
    {
        return true;
    }

    // 5. Resource packs or shader packs directory with content
    for sub in [InstanceSubdir::ResourcePacks, InstanceSubdir::ShaderPacks] {
        let p = path.join(sub.dir_name());
        if p.is_dir()
            && let Ok(entries) = std::fs::read_dir(&p)
            && entries.flatten().any(|e| e.path().is_file())
        {
            return true;
        }
    }

    false
}

/// Checks for signs that this directory is a Minecraft **server** instance.
fn has_server_indicators(path: &Path) -> bool {
    // 1. Server properties file (canonical server indicator)
    if path.join("server.properties").is_file() {
        return true;
    }

    // 2. EULA acceptance (required for modern servers)
    if path.join("eula.txt").is_file() {
        return true;
    }

    // 3. Plugins directory (Bukkit/Spigot/Paper)
    let plugins = path.join("plugins");
    if plugins.is_dir()
        && let Ok(entries) = std::fs::read_dir(&plugins)
        && entries.flatten().any(|e| e.path().is_file())
    {
        return true;
    }

    // 4. Server jar directly in directory
    if let Ok(entries) = std::fs::read_dir(path)
        && entries.flatten().any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            e.path().is_file()
                && name.ends_with(".jar")
                && (name.contains("server") || name.contains("paper") || name.contains("spigot"))
        })
    {
        return true;
    }

    false
}

/// Returns true for well-known Mojang subdirectory names that should not
/// be treated as instances themselves.
fn is_standard_dir(name: &str) -> bool {
    matches!(
        name,
        "versions"
            | "runtime"
            | "libraries"
            | "assets"
            | "logs"
            | "launcher"
            | "webcache"
            | "webcache2"
    )
}

/// Returns the official Minecraft launcher directory for the current platform.
fn minecraft_base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // %APPDATA%\.minecraft
        if let Some(data) = dirs::data_dir() {
            dirs.push(data.join(".minecraft"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        // ~/Library/Application Support/minecraft
        if let Some(data) = dirs::data_dir() {
            dirs.push(data.join("minecraft"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(ref home) = dirs::home_dir() {
            dirs.push(home.join(".minecraft"));
        }
    }

    dirs
}

/// Detect world directories in a given directory (not recursive into `saves/`).
fn detect_worlds_in_dir(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return names;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if (path.join("level.dat").is_file() || path.join("session.lock").is_file())
            && let Some(name) = path.file_name().and_then(OsStr::to_str)
        {
            names.push(name.to_string());
        }
    }

    names
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("unknown")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_does_not_panic() {
        let instances = discover();
        for inst in &instances {
            assert!(!inst.id.is_empty());
            assert!(inst.path.is_dir());
        }
    }

    #[test]
    fn test_scan_version_instances_requires_jar_and_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let versions = base.join("versions");
        let valid = versions.join("1.20.4");
        let jar_only = versions.join("jar-only");
        let json_only = versions.join("json-only");
        std::fs::create_dir_all(&valid).expect("create valid dir");
        std::fs::create_dir_all(&jar_only).expect("create jar-only dir");
        std::fs::create_dir_all(&json_only).expect("create json-only dir");

        std::fs::write(valid.join("1.20.4.jar"), b"jar").expect("write jar");
        std::fs::write(valid.join("1.20.4.json"), b"{}").expect("write json");
        std::fs::write(jar_only.join("jar-only.jar"), b"jar").expect("write jar");
        std::fs::write(json_only.join("json-only.json"), b"{}").expect("write json");

        // A valid world inside the valid instance is reported as its world.
        let saves = valid.join("saves").join("myworld");
        std::fs::create_dir_all(&saves).expect("create world dir");
        std::fs::write(saves.join("level.dat"), b"nbt").expect("write level.dat");

        let instances = scan_version_instances(&[base.to_path_buf()]);
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].id, "1.20.4");
        assert_eq!(instances[0].kind, InstanceKind::Client);
        assert_eq!(instances[0].path, valid);
        assert_eq!(instances[0].worlds, vec!["myworld".to_string()]);
    }

    #[test]
    fn test_scan_version_instances_skips_missing_versions_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("no-versions-here");
        std::fs::create_dir_all(&base).expect("create base dir");

        let instances = scan_version_instances(&[base]);
        assert!(instances.is_empty());
    }
}
