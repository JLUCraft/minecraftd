use crate::core::process::{
    ProcessError, ProcessExitCallback, ProcessHandle, ProcessOutputCallback, ProcessSpec, Signal,
    SpawnError,
};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, trace};

pub struct LocalProcessHandle {
    pid: u32,
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    running: Arc<AtomicBool>,
    output_callbacks: Arc<Mutex<Vec<ProcessOutputCallback>>>,
    exit_callbacks: Arc<Mutex<Vec<ProcessExitCallback>>>,
    /// Watch channel for process exit code. None = not exited yet, Some(code) = exited.
    exit_watch: watch::Receiver<Option<i32>>,
}

impl std::fmt::Debug for LocalProcessHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProcessHandle")
            .field("pid", &self.pid)
            .field("running", &self.running)
            .field("exited", &self.exit_watch.borrow().is_some())
            .finish()
    }
}

impl LocalProcessHandle {
    fn new(
        pid: u32,
        stdin: tokio::process::ChildStdin,
        output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        exit_rx: mpsc::UnboundedReceiver<i32>,
        output_callbacks: Arc<Mutex<Vec<ProcessOutputCallback>>>,
        exit_callbacks: Arc<Mutex<Vec<ProcessExitCallback>>>,
        exit_watch: watch::Receiver<Option<i32>>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));

        let output_cbs = output_callbacks.clone();
        let out_running = running.clone();
        tokio::spawn(async move {
            let mut rx = output_rx;
            while out_running.load(Ordering::SeqCst) {
                match rx.recv().await {
                    Some(data) => {
                        if let Ok(cbs) = output_cbs.lock() {
                            for cb in cbs.iter() {
                                cb(&data);
                            }
                        }
                    }
                    None => break,
                }
            }
        });

        let exit_cbs = exit_callbacks.clone();
        let exit_running = running.clone();
        tokio::spawn(async move {
            let mut rx = exit_rx;
            while exit_running.load(Ordering::SeqCst) {
                match rx.recv().await {
                    Some(code) => {
                        if let Ok(cbs) = exit_cbs.lock() {
                            for cb in cbs.iter() {
                                cb(code);
                            }
                        }
                    }
                    None => break,
                }
            }
        });

        Self {
            pid,
            stdin: tokio::sync::Mutex::new(stdin),
            running,
            output_callbacks,
            exit_callbacks,
            exit_watch,
        }
    }

    /// Returns a receiver for watching process exit. None = still running, Some(code) = exited.
    pub fn exit_watch(&self) -> watch::Receiver<Option<i32>> {
        self.exit_watch.clone()
    }

    /// Async wait for the process to exit and return its exit code.
    pub async fn wait_for_exit(&mut self) -> i32 {
        loop {
            let code = *self.exit_watch.borrow();
            if let Some(code) = code {
                return code;
            }
            if self.exit_watch.changed().await.is_err() {
                return -1;
            }
            let code = *self.exit_watch.borrow();
            if let Some(code) = code {
                return code;
            }
        }
    }

    async fn spawn_readers(
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
        output_tx: mpsc::UnboundedSender<Vec<u8>>,
    ) {
        let stdout_tx = output_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let _ = stdout_tx.send(line.clone());
                    }
                    Err(e) => {
                        trace!("stdout read error: {}", e);
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let _ = output_tx.send(line.clone());
                    }
                    Err(e) => {
                        trace!("stderr read error: {}", e);
                        break;
                    }
                }
            }
        });
    }
}

#[async_trait]
impl ProcessHandle for LocalProcessHandle {
    fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }

    async fn write(&self, data: &[u8]) -> Result<(), ProcessError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ProcessError::NotRunning);
        }
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(data)
            .await
            .map_err(|e| ProcessError::WriteFailed(e.to_string()))?;
        stdin
            .flush()
            .await
            .map_err(|e| ProcessError::WriteFailed(e.to_string()))?;
        drop(stdin);
        Ok(())
    }

    async fn kill(&self, signal: Signal) -> Result<(), ProcessError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ProcessError::AlreadyExited);
        }

        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal as NixSignal};
            use nix::unistd::Pid;
            let nix_sig = match signal {
                Signal::Term => NixSignal::SIGTERM,
                Signal::Kill => NixSignal::SIGKILL,
                Signal::Int => NixSignal::SIGINT,
            };
            signal::kill(Pid::from_raw(self.pid as i32), nix_sig)
                .map_err(|e| ProcessError::KillFailed(e.to_string()))?;
        }

        #[cfg(windows)]
        {
            let _ = signal;
            let output = tokio::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.pid.to_string()])
                .creation_flags(0x08000000)
                .output()
                .await
                .map_err(|e| ProcessError::KillFailed(e.to_string()))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ProcessError::KillFailed(stderr.to_string()));
            }
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn on_output(&self, callback: ProcessOutputCallback) {
        if let Ok(mut cbs) = self.output_callbacks.lock() {
            cbs.push(callback);
        }
    }

    fn on_exit(&self, callback: ProcessExitCallback) {
        if let Ok(mut cbs) = self.exit_callbacks.lock() {
            cbs.push(callback);
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn exit_watch(&self) -> Option<tokio::sync::watch::Receiver<Option<i32>>> {
        Some(self.exit_watch.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub struct LocalSpawner;

impl LocalSpawner {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl crate::core::process::ProcessSpawner for LocalSpawner {
    async fn spawn(&self, spec: &ProcessSpec) -> Result<Box<dyn ProcessHandle>, SpawnError> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());

        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        if !spec.inherit_env {
            cmd.env_clear();
        }
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| SpawnError::SpawnFailed("no pid".into()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SpawnError::SpawnFailed("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SpawnError::SpawnFailed("no stderr".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SpawnError::SpawnFailed("no stdin".into()))?;

        let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (exit_tx, exit_rx) = mpsc::unbounded_channel::<i32>();

        let output_callbacks: Arc<Mutex<Vec<ProcessOutputCallback>>> =
            Arc::new(Mutex::new(Vec::new()));
        let exit_callbacks: Arc<Mutex<Vec<ProcessExitCallback>>> = Arc::new(Mutex::new(Vec::new()));

        let (exit_watch_tx, exit_watch_rx) = watch::channel(None);

        let handle = LocalProcessHandle::new(
            pid,
            stdin,
            output_rx,
            exit_rx,
            output_callbacks.clone(),
            exit_callbacks.clone(),
            exit_watch_rx,
        );

        LocalProcessHandle::spawn_readers(stdout, stderr, output_tx).await;

        let running = handle.running.clone();
        let exit_tx2 = exit_tx.clone();
        tokio::spawn(async move {
            let exit_code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(-1),
                Err(e) => {
                    error!("process wait error: {}", e);
                    -1
                }
            };
            running.store(false, Ordering::SeqCst);
            let _ = exit_tx2.send(exit_code);
            let _ = exit_watch_tx.send(Some(exit_code));
        });

        debug!("spawned local process pid={}", pid);
        Ok(Box::new(handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::process::ProcessSpawner;

    #[tokio::test]
    async fn test_local_spawner_echo() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "echo".into(),
            args: vec!["hello".into()],
            ..Default::default()
        };
        let handle = spawner.spawn(&spec).await.unwrap();
        assert!(handle.pid().is_some());
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_local_spawner_cat() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "cat".into(),
            ..Default::default()
        };
        let handle = spawner.spawn(&spec).await.unwrap();
        handle.write(b"hello world\n").await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        handle.kill(Signal::Term).await.unwrap();
    }

    #[tokio::test]
    async fn test_local_process_output_callback() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "echo".into(),
            args: vec!["callback_test".into()],
            ..Default::default()
        };
        let handle = spawner.spawn(&spec).await.unwrap();

        let output = Arc::new(Mutex::new(Vec::new()));
        let out = output.clone();
        handle.on_output(Box::new(move |data| {
            out.lock().unwrap().extend_from_slice(data);
        }));

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        let data = output.lock().unwrap().clone();
        let s = String::from_utf8_lossy(&data);
        assert!(s.contains("callback_test"));
    }

    #[tokio::test]
    async fn test_local_process_exit_callback() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "true".into(),
            ..Default::default()
        };
        let handle = spawner.spawn(&spec).await.unwrap();

        let exited = Arc::new(AtomicBool::new(false));
        let ex = exited.clone();
        handle.on_exit(Box::new(move |_code| {
            ex.store(true, Ordering::SeqCst);
        }));

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        assert!(exited.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_local_process_write_not_running() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "true".into(),
            ..Default::default()
        };
        let handle = spawner.spawn(&spec).await.unwrap();
        // Wait for process to exit
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        let result = handle.write(b"data\n").await;
        assert!(matches!(result, Err(ProcessError::NotRunning)));
    }

    #[tokio::test]
    async fn test_local_process_kill_already_exited() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "true".into(),
            ..Default::default()
        };
        let handle = spawner.spawn(&spec).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        let result = handle.kill(Signal::Kill).await;
        assert!(matches!(result, Err(ProcessError::AlreadyExited)));
    }

    #[tokio::test]
    async fn test_local_spawner_env_and_cwd() {
        let spawner = LocalSpawner::new();
        let tmpdir = tempfile::tempdir().unwrap();
        let spec = ProcessSpec {
            command: "pwd".into(),
            args: vec![],
            cwd: Some(tmpdir.path().to_path_buf()),
            env: std::collections::HashMap::new(),
            inherit_env: true,
        };
        let handle = spawner.spawn(&spec).await.unwrap();
        assert!(handle.is_running());
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_local_spawner_no_inherit_env() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "env".into(),
            args: vec![],
            cwd: None,
            env: std::collections::HashMap::new(),
            inherit_env: false,
        };
        let _handle = spawner.spawn(&spec).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_local_spawner_invalid_command() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "/nonexistent/command_xyz".into(),
            ..Default::default()
        };
        let result = spawner.spawn(&spec).await;
        assert!(result.is_err());
    }
}
