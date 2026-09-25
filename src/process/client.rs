//! Owned desktop client processes, output capture, readiness and play-time tracking.
use crate::core::process::{ProcessHandle, ProcessSpec, Signal};
use crate::process::local::LocalSpawner;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

const READY_MARKERS: &[&str] = &["render thread", "lwjgl version", "lwjgl openal"];
#[derive(Debug, thiserror::Error)]
pub enum ClientProcessError {
    #[error(transparent)]
    Spawn(#[from] crate::core::process::SpawnError),
    #[error(transparent)]
    Process(#[from] crate::core::process::ProcessError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("client output failed: {0}")]
    Output(String),
    #[error("owned client process is unavailable")]
    Unavailable,
}
type Handles = Arc<Mutex<HashMap<u32, Arc<dyn ProcessHandle>>>>;
#[derive(Default, Clone)]
pub struct ClientProcesses {
    handles: Handles,
}
struct Capture {
    file: File,
    started: Option<Instant>,
    error: Option<String>,
}
pub struct ClientSession {
    pub pid: u32,
    ready: watch::Receiver<bool>,
    exited: watch::Receiver<Option<i32>>,
    capture: Arc<Mutex<Capture>>,
    handles: Handles,
}
#[derive(Debug, Clone, Copy)]
pub struct ClientExit {
    pub code: i32,
    pub reached_ready: bool,
    pub play_time_secs: u64,
}
impl ClientProcesses {
    pub async fn spawn(
        &self,
        spec: &ProcessSpec,
        log: &Path,
        on_line: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<ClientSession, ClientProcessError> {
        if let Some(parent) = log.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let capture = Arc::new(Mutex::new(Capture {
            file: OpenOptions::new().create_new(true).write(true).open(log)?,
            started: None,
            error: None,
        }));
        let (ready_tx, ready) = watch::channel(false);
        let output = capture.clone();
        let callback = Box::new(move |bytes: &[u8]| {
            let line = String::from_utf8_lossy(bytes);
            let mut capture = output.lock();
            if let Err(error) = capture.file.write_all(bytes) {
                capture.error = Some(error.to_string());
            }
            if capture.started.is_none()
                && READY_MARKERS
                    .iter()
                    .any(|marker| line.to_lowercase().contains(marker))
            {
                capture.started = Some(Instant::now());
                ready_tx.send_replace(true);
            }
            drop(capture);
            on_line(line.trim_end_matches(['\r', '\n']));
        });
        let handle: Arc<dyn ProcessHandle> = LocalSpawner::new()
            .spawn_with_output(spec, Some(callback))
            .await?
            .into();
        let pid = handle.pid().ok_or(ClientProcessError::Unavailable)?;
        let exited = handle.exit_watch().ok_or(ClientProcessError::Unavailable)?;
        self.handles.lock().insert(pid, handle);
        let handles = self.handles.clone();
        let mut completion = exited.clone();
        tokio::spawn(async move {
            while completion.borrow().is_none() {
                if completion.changed().await.is_err() {
                    break;
                }
            }
            handles.lock().remove(&pid);
        });
        Ok(ClientSession {
            pid,
            ready,
            exited,
            capture,
            handles: self.handles.clone(),
        })
    }
    pub async fn kill(&self, pid: u32) -> Result<(), ClientProcessError> {
        let handle = self
            .handles
            .lock()
            .get(&pid)
            .cloned()
            .ok_or(ClientProcessError::Unavailable)?;
        handle.kill(Signal::Kill).await?;
        Ok(())
    }
}
impl ClientSession {
    /// False means the child exited before readiness; never wait forever on an early crash.
    pub async fn wait_ready(&mut self) -> bool {
        loop {
            if *self.ready.borrow() {
                return true;
            }
            if self.exited.borrow().is_some() {
                return false;
            }
            tokio::select! {
                changed = self.ready.changed() => { if changed.is_err() { return false; } }
                changed = self.exited.changed() => { if changed.is_err() { return false; } }
            }
        }
    }
    pub async fn wait(mut self) -> Result<ClientExit, ClientProcessError> {
        loop {
            if self.exited.borrow().is_some() {
                break;
            }
            if self.exited.changed().await.is_err() {
                break;
            }
        }
        self.handles.lock().remove(&self.pid);
        let mut capture = self.capture.lock();
        capture.file.flush()?;
        if let Some(error) = capture.error.take() {
            return Err(ClientProcessError::Output(error));
        }
        Ok(ClientExit {
            code: self.exited.borrow().unwrap_or(-1),
            reached_ready: capture.started.is_some(),
            play_time_secs: capture.started.map_or(0, |start| start.elapsed().as_secs()),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn captures_short_lived_process_and_drains_output_before_exit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("game.log");
        let spec = ProcessSpec {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'Render thread ready\n'; printf 'last error\n' >&2".into(),
            ],
            ..Default::default()
        };
        let processes = ClientProcesses::default();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let output = lines.clone();
        let mut session = processes
            .spawn(&spec, &path, move |line| {
                output.lock().push(line.to_owned())
            })
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), session.wait_ready())
                .await
                .unwrap()
        );
        let exit = session.wait().await.unwrap();
        assert_eq!(exit.code, 0);
        assert!(exit.reached_ready);
        assert!(processes.handles.lock().is_empty());
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.contains("Render thread ready"));
        assert!(log.contains("last error"));
        assert_eq!(lines.lock().len(), 2);
    }
    #[tokio::test]
    async fn early_crash_unblocks_readiness_and_only_owned_processes_can_be_killed() {
        let directory = tempfile::tempdir().unwrap();
        let processes = ClientProcesses::default();
        let spec = ProcessSpec {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "exit 7".into()],
            ..Default::default()
        };
        let mut session = processes
            .spawn(&spec, &directory.path().join("game.log"), |_| {})
            .await
            .unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_secs(3), session.wait_ready())
                .await
                .unwrap()
        );
        assert_eq!(session.wait().await.unwrap().code, 7);
        assert!(processes.kill(std::process::id()).await.is_err());
    }
}
