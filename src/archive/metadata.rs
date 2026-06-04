use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::state::InstanceState;

/// Metadata persisted alongside each archive.
///
/// Each archive consists of two files:
/// - `{archive_id}.{ext}` — the compressed archive file
/// - `{archive_id}.meta.json` — this metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveMetadata {
    /// Unique archive identifier (UUID v7, time-ordered).
    pub id: String,
    /// Human-readable label (e.g. "打凋零前", "weekly-backup").
    pub label: String,
    /// The instance this archive belongs to.
    pub instance_id: String,
    /// When the archive was created.
    pub created_at: DateTime<Utc>,
    /// Minecraft version at time of archive (if known).
    pub game_version: Option<String>,
    /// What was included in the archive.
    pub scope: ArchiveScope,
    /// Compression algorithm used.
    pub algorithm: CompressionAlgorithm,
    /// Instance state at the time of archiving.
    pub instance_state: InstanceState,
    /// Number of files in the archive.
    pub file_count: u64,
    /// Total original size in bytes.
    pub original_size: u64,
    /// Compressed size in bytes.
    pub compressed_size: u64,
    /// SHA-256 checksum of the archive file.
    pub sha256: String,
    /// World directory names included (e.g. `["world", "world_nether"]`).
    pub world_names: Vec<String>,
}

/// Describes what subset of the instance directory was archived.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchiveScope {
    /// The entire instance directory.
    Full,
    /// Only world directories (e.g. `world/`, `world_nether/`, `world_the_end/`).
    Worlds,
    /// User-specified list of subdirectories.
    Custom { dirs: Vec<String> },
}

/// Supported compression algorithms.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum CompressionAlgorithm {
    /// Standard zip (compatible everywhere, moderate compression).
    #[default]
    Zip,
    /// tar + gzip (good balance of speed and size).
    TarGz,
}

impl CompressionAlgorithm {
    /// Returns the file extension for this algorithm (without leading dot).
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }
}
