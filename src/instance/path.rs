//! File-system anchor for Minecraft instances.
//!
//! Every instance lives at a directory on disk. `InstancePath` makes this
//! explicit, replacing ad-hoc `cwd: String` patterns. Well-known subdirectories
//! are derived deterministically via [`InstanceSubdir`] — no path string
//! concatenation scattered across the codebase.
//!
//! Design mirrors SJMCL's approach: the instance model carries its own
//! `version_path`, and all subdirectory paths flow from that single anchor.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Canonical file-system anchor for a Minecraft instance.
///
/// Every instance owns a directory on disk — this is that directory.
/// For servers it's the server root; for clients following the Mojang
/// layout it's `versions/<name>/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstancePath {
    /// The root directory of this instance on the file system.
    pub root: PathBuf,
}

/// Well-known subdirectory types within a Minecraft instance.
///
/// Each variant maps to a conventional subdirectory name. Call
/// [`InstancePath::subdir`] or [`InstanceSubdir::resolve`] to get the
/// absolute [`PathBuf`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceSubdir {
    /// The instance root directory itself.
    Root,
    /// `saves/` — single-player world save data.
    Saves,
    /// `mods/` — JAR-based mod files (Fabric, Forge, etc.).
    Mods,
    /// `resourcepacks/` — client-side resource packs.
    ResourcePacks,
    /// `shaderpacks/` — OptiFine / Iris shader packs.
    ShaderPacks,
    /// `screenshots/` — in-game screenshots.
    Screenshots,
    /// `schematics/` — Litematica / WorldEdit schematics.
    Schematics,
    /// `config/` — per-mod configuration files.
    Config,
    /// `logs/` — log output files.
    Logs,
    /// `backups/` — local world backups.
    Backups,
    /// `.tmp/` — JVM-isolated temporary directory.
    Tmp,
    /// `libraries/` — Mojang library files (shared at game-dir level).
    Libraries,
    /// `assets/` — game assets (shared at game-dir level).
    Assets,
    /// `server-resource-packs/` — server-provided resource packs.
    ServerResourcePacks,
}

impl InstancePath {
    /// Wrap an existing path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Derive the absolute path to a well-known subdirectory.
    ///
    /// This is a pure computation — no disk I/O.
    #[must_use]
    pub fn subdir(&self, kind: InstanceSubdir) -> PathBuf {
        kind.resolve(&self.root)
    }
}

impl InstanceSubdir {
    /// Resolve this subdirectory kind against a root path.
    ///
    /// Pure computation, no I/O.
    #[must_use]
    pub fn resolve(self, root: &Path) -> PathBuf {
        match self {
            Self::Root => root.to_path_buf(),
            Self::Saves => root.join("saves"),
            Self::Mods => root.join("mods"),
            Self::ResourcePacks => root.join("resourcepacks"),
            Self::ShaderPacks => root.join("shaderpacks"),
            Self::Screenshots => root.join("screenshots"),
            Self::Schematics => root.join("schematics"),
            Self::Config => root.join("config"),
            Self::Logs => root.join("logs"),
            Self::Backups => root.join("backups"),
            Self::Tmp => root.join(".tmp"),
            Self::Libraries => root.join("libraries"),
            Self::Assets => root.join("assets"),
            Self::ServerResourcePacks => root.join("server-resource-packs"),
        }
    }

    /// Human-readable directory name for this subdirectory kind.
    #[must_use]
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Root => ".",
            Self::Saves => "saves",
            Self::Mods => "mods",
            Self::ResourcePacks => "resourcepacks",
            Self::ShaderPacks => "shaderpacks",
            Self::Screenshots => "screenshots",
            Self::Schematics => "schematics",
            Self::Config => "config",
            Self::Logs => "logs",
            Self::Backups => "backups",
            Self::Tmp => ".tmp",
            Self::Libraries => "libraries",
            Self::Assets => "assets",
            Self::ServerResourcePacks => "server-resource-packs",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subdir_resolve() {
        let ip = InstancePath::new("/srv/minecraft/survival");
        assert_eq!(
            ip.subdir(InstanceSubdir::Root),
            PathBuf::from("/srv/minecraft/survival")
        );
        assert_eq!(
            ip.subdir(InstanceSubdir::Saves),
            PathBuf::from("/srv/minecraft/survival/saves")
        );
        assert_eq!(
            ip.subdir(InstanceSubdir::Mods),
            PathBuf::from("/srv/minecraft/survival/mods")
        );
        assert_eq!(
            ip.subdir(InstanceSubdir::Tmp),
            PathBuf::from("/srv/minecraft/survival/.tmp")
        );
    }

    #[test]
    fn test_subdir_resolve_trait() {
        let root = Path::new("/srv/mc");
        assert_eq!(
            InstanceSubdir::Backups.resolve(root),
            PathBuf::from("/srv/mc/backups")
        );
        assert_eq!(
            InstanceSubdir::Logs.resolve(root),
            PathBuf::from("/srv/mc/logs")
        );
    }

    #[test]
    fn test_dir_name() {
        assert_eq!(InstanceSubdir::Saves.dir_name(), "saves");
        assert_eq!(InstanceSubdir::Tmp.dir_name(), ".tmp");
        assert_eq!(InstanceSubdir::Root.dir_name(), ".");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let ip = InstancePath::new("/tmp/test-instance");
        let json = serde_json::to_string(&ip).unwrap();
        let ip2: InstancePath = serde_json::from_str(&json).unwrap();
        assert_eq!(ip, ip2);
    }
}
