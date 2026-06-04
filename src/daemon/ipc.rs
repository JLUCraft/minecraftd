//! IPC message types between CLI and daemon over a local socket.

use serde::{Deserialize, Serialize};

/// Request sent from CLI to daemon.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum CliRequest {
    /// Create and start a new server instance from a TOML config file.
    Run {
        config_path: std::path::PathBuf,
        #[serde(default)]
        restart: bool,
    },
    /// Query which instances the daemon currently has running.
    Running,
    /// Stop an instance gracefully.
    Stop { id: String },
    /// Start a stopped instance.
    Start { id: String },
    /// Restart an instance.
    Restart { id: String },
    /// Force kill an instance.
    Kill { id: String },
    /// Show recent output lines.
    Logs {
        id: String,
        #[serde(default = "default_tail")]
        tail: usize,
    },
    /// Send a command to instance stdin.
    Exec { id: String, command: String },
    /// Shutdown the daemon.
    Shutdown,
}

const fn default_tail() -> usize {
    50
}

/// Response sent from daemon to CLI.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliResponse {
    /// Success with optional message.
    Ok { message: Option<String> },
    /// Instance list response (running instances).
    Instances { instances: Vec<InstanceInfo> },
    /// Logs response.
    Logs { lines: Vec<String> },
    /// Running instance IDs.
    RunningIds { ids: Vec<String> },
    /// Error response.
    Err { error: String },
}

/// Lightweight running-instance info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub id: String,
    pub state: String,
    pub play_time_secs: u64,
}
