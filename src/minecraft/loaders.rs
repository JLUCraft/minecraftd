use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Type of mod loader.
///
/// Mirrors SJMCL's `ModLoaderType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ModLoaderType {
    #[default]
    Unknown,
    Fabric,
    Forge,
    LegacyForge,
    NeoForge,
    LiteLoader,
    Quilt,
    Paper,
}

impl ModLoaderType {
    /// Returns true if this loader requires a separate installer.
    #[must_use]
    pub const fn requires_installer(&self) -> bool {
        matches!(self, Self::Forge | Self::LegacyForge | Self::NeoForge)
    }

    /// Returns true if this loader is installed via library injection.
    #[must_use]
    pub const fn is_library_based(&self) -> bool {
        matches!(self, Self::Fabric | Self::Quilt | Self::Unknown)
    }

    /// Returns true if this loader is a server-only platform.
    #[must_use]
    pub const fn is_server_platform(&self) -> bool {
        matches!(self, Self::Paper)
    }

    /// Returns a human-readable name.
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Unknown => "Vanilla",
            Self::Fabric => "Fabric",
            Self::Forge => "Forge",
            Self::LegacyForge => "Legacy Forge",
            Self::NeoForge => "NeoForge",
            Self::LiteLoader => "LiteLoader",
            Self::Quilt => "Quilt",
            Self::Paper => "Paper",
        }
    }
}

impl FromStr for ModLoaderType {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.to_lowercase().as_str() {
            "unknown" | "vanilla" => Ok(Self::Unknown),
            "fabric" => Ok(Self::Fabric),
            "forge" => Ok(Self::Forge),
            "legacyforge" | "legacy_forge" => Ok(Self::LegacyForge),
            "neoforge" | "neo_forge" => Ok(Self::NeoForge),
            "liteloader" | "lite_loader" => Ok(Self::LiteLoader),
            "quilt" => Ok(Self::Quilt),
            "paper" => Ok(Self::Paper),
            _ => Err(format!("unsupported mod loader type: {input}")),
        }
    }
}

/// Status of mod loader installation.
///
/// Mirrors SJMCL's `ModLoaderStatus` enum with its state machine semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ModLoaderStatus {
    /// Mod loader libraries have not been downloaded.
    #[default]
    NotDownloaded,
    /// Download or installation failed.
    DownloadFailed,
    /// Download is in progress.
    Downloading,
    /// Installation processors are running.
    Installing,
    /// Fully installed and ready.
    Installed,
}

impl ModLoaderStatus {
    /// Returns true if the mod loader is in a terminal state.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Installed | Self::DownloadFailed)
    }

    /// Returns true if the mod loader is in progress.
    #[must_use]
    pub const fn is_in_progress(&self) -> bool {
        matches!(self, Self::Downloading | Self::Installing)
    }

    /// Returns true if the mod loader is ready to use.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        *self == Self::Installed
    }
}

/// Information about an installed mod loader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModLoaderInfo {
    pub loader_type: ModLoaderType,
    pub version: String,
    pub status: ModLoaderStatus,
    pub branch: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mod_loader_type_from_str() {
        assert_eq!(
            ModLoaderType::from_str("fabric").unwrap(),
            ModLoaderType::Fabric
        );
        assert_eq!(
            ModLoaderType::from_str("Forge").unwrap(),
            ModLoaderType::Forge
        );
        assert_eq!(
            ModLoaderType::from_str("NEOFORGE").unwrap(),
            ModLoaderType::NeoForge
        );
        assert_eq!(
            ModLoaderType::from_str("Paper").unwrap(),
            ModLoaderType::Paper
        );
        assert!(ModLoaderType::from_str("unknown_loader").is_err());
    }

    #[test]
    fn test_mod_loader_status() {
        assert!(ModLoaderStatus::Installed.is_ready());
        assert!(!ModLoaderStatus::NotDownloaded.is_ready());
        assert!(ModLoaderStatus::Downloading.is_in_progress());
        assert!(!ModLoaderStatus::DownloadFailed.is_in_progress());
    }

    #[test]
    fn test_mod_loader_requires_installer() {
        assert!(ModLoaderType::Forge.requires_installer());
        assert!(ModLoaderType::NeoForge.requires_installer());
        assert!(!ModLoaderType::Fabric.requires_installer());
        assert!(!ModLoaderType::Quilt.requires_installer());
    }

    #[test]
    fn test_mod_loader_is_library_based() {
        assert!(ModLoaderType::Fabric.is_library_based());
        assert!(ModLoaderType::Quilt.is_library_based());
        assert!(ModLoaderType::Unknown.is_library_based());
        assert!(!ModLoaderType::Forge.is_library_based());
        assert!(!ModLoaderType::NeoForge.is_library_based());
    }

    #[test]
    fn test_mod_loader_display_name() {
        assert_eq!(ModLoaderType::Unknown.display_name(), "Vanilla");
        assert_eq!(ModLoaderType::Fabric.display_name(), "Fabric");
        assert_eq!(ModLoaderType::Forge.display_name(), "Forge");
        assert_eq!(ModLoaderType::LegacyForge.display_name(), "Legacy Forge");
        assert_eq!(ModLoaderType::NeoForge.display_name(), "NeoForge");
        assert_eq!(ModLoaderType::LiteLoader.display_name(), "LiteLoader");
        assert_eq!(ModLoaderType::Quilt.display_name(), "Quilt");
        assert_eq!(ModLoaderType::Paper.display_name(), "Paper");
    }

    #[test]
    fn test_mod_loader_status_terminal() {
        assert!(ModLoaderStatus::Installed.is_terminal());
        assert!(ModLoaderStatus::DownloadFailed.is_terminal());
        assert!(!ModLoaderStatus::NotDownloaded.is_terminal());
    }

    #[test]
    fn test_mod_loader_info_default() {
        let info = ModLoaderInfo::default();
        assert_eq!(info.loader_type, ModLoaderType::Unknown);
        assert_eq!(info.status, ModLoaderStatus::NotDownloaded);
    }
}
