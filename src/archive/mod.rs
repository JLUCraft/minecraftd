//! Archive management for Minecraft instances.
//!
//! Provides backup, restore, list, verify, and cleanup of instance data
//! (worlds, configs, mods, plugins) using zip or tar.gz compression.
//!
//! ## Architecture
//!
//! - `ArchiveManager` — Core API for creating, listing, restoring, and deleting archives
//! - `ArchiveStrategy` — Pluggable strategies (full, incremental, timed)
//! - `ArchiveScheduleTask` — `LifecycleTask` for automatic scheduled backups

pub mod compress;
pub mod manager;
pub mod metadata;
pub mod schedule;
pub mod strategy;

use crate::core::state::InstanceState;
use thiserror::Error;

/// Errors that can occur during archive operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    #[error("archive not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("instance must be stopped before archiving, current state: {0:?}")]
    InstanceNotStopped(InstanceState),
    #[error("compression error: {0}")]
    Compression(String),
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("metadata parse error: {0}")]
    MetadataParse(String),
    #[error("restore path exists but is not empty: {0}")]
    RestorePathNotEmpty(String),
    #[error("archive id invalid: {0}")]
    InvalidId(String),
}

impl From<std::io::Error> for ArchiveError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Result type for archive operations.
pub type ArchiveResult<T> = std::result::Result<T, ArchiveError>;
