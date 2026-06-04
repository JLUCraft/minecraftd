use async_trait::async_trait;
use std::fmt;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    #[error("process is not running")]
    NotRunning,
    #[error("failed to write to process stdin: {0}")]
    WriteFailed(String),
    #[error("failed to kill process: {0}")]
    KillFailed(String),
    #[error("process already exited")]
    AlreadyExited,
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    #[error("command not found: {0}")]
    CommandNotFound(String),
    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("invalid working directory: {0}")]
    InvalidWorkingDirectory(String),
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for SpawnError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_display() {
        assert_eq!(format!("{}", Signal::Term), "TERM");
        assert_eq!(format!("{}", Signal::Kill), "KILL");
        assert_eq!(format!("{}", Signal::Int), "INT");
    }

    #[test]
    fn test_process_spec_default() {
        let spec = ProcessSpec::default();
        assert!(spec.command.is_empty());
        assert!(spec.args.is_empty());
        assert!(spec.cwd.is_none());
        assert!(spec.env.is_empty());
        assert!(!spec.inherit_env);
    }

    #[test]
    fn test_spawn_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let spawn_err: SpawnError = io_err.into();
        assert!(matches!(spawn_err, SpawnError::Io(_)));
    }

    #[test]
    fn test_process_error_display() {
        let e1 = ProcessError::NotRunning;
        assert_eq!(format!("{e1}"), "process is not running");

        let e2 = ProcessError::WriteFailed("broken pipe".into());
        assert!(format!("{e2}").contains("broken pipe"));
    }
}

/// Signals that can be sent to a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    Term,
    Kill,
    Int,
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Term => write!(f, "TERM"),
            Self::Kill => write!(f, "KILL"),
            Self::Int => write!(f, "INT"),
        }
    }
}

/// Specification for spawning a process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: std::collections::HashMap<String, String>,
    pub inherit_env: bool,
}

/// Type aliases for process callbacks.
pub type ProcessOutputCallback = Box<dyn Fn(&[u8]) + Send + Sync>;
pub type ProcessExitCallback = Box<dyn Fn(i32) + Send + Sync>;

/// Trait representing a handle to a running process.
#[async_trait]
pub trait ProcessHandle: Send + Sync {
    fn pid(&self) -> Option<u32>;
    async fn write(&self, data: &[u8]) -> Result<(), ProcessError>;
    async fn kill(&self, signal: Signal) -> Result<(), ProcessError>;
    fn on_output(&self, callback: ProcessOutputCallback);
    fn on_exit(&self, callback: ProcessExitCallback);
    fn is_running(&self) -> bool;
    /// Returns a watch receiver for process exit. None = not exited, Some(code) = exited.
    fn exit_watch(&self) -> Option<tokio::sync::watch::Receiver<Option<i32>>> {
        None
    }
}

/// Trait for spawning processes.
#[async_trait]
pub trait ProcessSpawner: Send + Sync {
    async fn spawn(&self, spec: &ProcessSpec) -> Result<Box<dyn ProcessHandle>, SpawnError>;
}
