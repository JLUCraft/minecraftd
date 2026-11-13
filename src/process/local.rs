use crate::core::process::{
    OUTPUT_SEGMENT_BYTES, ProcessError, ProcessExitCallback, ProcessHandle, ProcessOutputCallback,
    ProcessSpec, Signal, SpawnError,
};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, trace};

const OUTPUT_QUEUE_CAPACITY: usize = 64;

pub struct LocalProcessHandle {
    pid: u32,
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    running: Arc<AtomicBool>,
    cleanup_group: Arc<AtomicBool>,
    observe_groups: bool,
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
        output_rx: mpsc::Receiver<Vec<u8>>,
        exit_rx: mpsc::UnboundedReceiver<i32>,
        output_callbacks: Arc<Mutex<Vec<ProcessOutputCallback>>>,
        exit_callbacks: Arc<Mutex<Vec<ProcessExitCallback>>>,
        exit_watch: watch::Receiver<Option<i32>>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let running = Arc::new(AtomicBool::new(true));

        let output_cbs = output_callbacks.clone();
        let output_task = tokio::spawn(async move {
            let mut rx = output_rx;
            while let Some(data) = rx.recv().await {
                if let Ok(cbs) = output_cbs.lock() {
                    for cb in cbs.iter() {
                        cb(&data);
                    }
                }
            }
        });

        let exit_cbs = exit_callbacks.clone();
        tokio::spawn(async move {
            let mut rx = exit_rx;
            while let Some(code) = rx.recv().await {
                if let Ok(cbs) = exit_cbs.lock() {
                    for cb in cbs.iter() {
                        cb(code);
                    }
                }
            }
        });

        (
            Self {
                pid,
                stdin: tokio::sync::Mutex::new(stdin),
                running,
                cleanup_group: Arc::new(AtomicBool::new(true)),
                observe_groups: false,
                output_callbacks,
                exit_callbacks,
                exit_watch,
            },
            output_task,
        )
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

    fn utf8_suffix(data: &[u8], keep_last_complete: bool) -> usize {
        let end = if keep_last_complete && data.last() == Some(&b'\r') {
            data.len() - 1
        } else {
            data.len()
        };
        let start = data[..end]
            .iter()
            .rposition(|byte| byte & 0b1100_0000 != 0b1000_0000)
            .unwrap_or(0);
        match std::str::from_utf8(&data[start..]) {
            Err(error) if error.error_len().is_none() => data.len() - start,
            Ok(_) if keep_last_complete => data.len() - start,
            _ => 0,
        }
    }

    async fn read_output<R: AsyncRead + Unpin>(source: R, output: mpsc::Sender<Vec<u8>>) {
        let mut reader = BufReader::new(source);
        let mut pending = Vec::with_capacity(OUTPUT_SEGMENT_BYTES);
        loop {
            let remaining = OUTPUT_SEGMENT_BYTES - pending.len();
            match (&mut reader)
                .take(remaining as u64)
                .read_until(b'\n', &mut pending)
                .await
            {
                Ok(0) => {
                    if !pending.is_empty() {
                        let _ = output.send(pending).await;
                    }
                    break;
                }
                Ok(_) => {
                    let keep_last =
                        pending.len() == OUTPUT_SEGMENT_BYTES && pending.last() != Some(&b'\n');
                    let split = pending.len() - Self::utf8_suffix(&pending, keep_last);
                    if split == 0 {
                        continue;
                    }
                    let suffix = pending.split_off(split);
                    let segment = std::mem::replace(&mut pending, suffix);
                    if output.send(segment).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    trace!(%error,"process output read failed");
                    break;
                }
            }
        }
    }

    fn spawn_readers(
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
        output: mpsc::Sender<Vec<u8>>,
    ) {
        tokio::spawn(Self::read_output(stdout, output.clone()));
        tokio::spawn(Self::read_output(stderr, output));
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
            match signal::killpg(Pid::from_raw(self.pid as i32), nix_sig) {
                Ok(()) => {}
                // The process group can exit between the running check and the
                // signal; stopping an already dead process is a success.
                Err(nix::errno::Errno::ESRCH) => {}
                Err(error) => return Err(ProcessError::KillFailed(error.to_string())),
            }
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
                // taskkill reports a pid that already exited as a failure; a
                // stop racing with process exit is still a successful stop.
                let mut exit = self.exit_watch.clone();
                let exited = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if exit.borrow().is_some() {
                            return;
                        }
                        if exit.changed().await.is_err() {
                            return;
                        }
                    }
                })
                .await
                .is_ok();
                if exited {
                    self.running.store(false, Ordering::SeqCst);
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ProcessError::KillFailed(stderr.to_string()));
            }
        }

        if matches!(signal, Signal::Kill) || cfg!(windows) {
            let mut exit = self.exit_watch.clone();
            loop {
                if exit.borrow().is_some() {
                    break;
                }
                exit.changed()
                    .await
                    .map_err(|error| ProcessError::KillFailed(error.to_string()))?;
            }
        }
        Ok(())
    }

    fn require_graceful_exit(&self) -> Result<(), ProcessError> {
        if !self.observe_groups {
            return Err(ProcessError::GracefulExitUnsupported);
        }
        self.cleanup_group.store(false, Ordering::SeqCst);
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
    /// Initialize Linux descendant observation before spawning local services.
    pub fn with_group_observation() -> Result<Self, SpawnError> {
        #[cfg(target_os = "linux")]
        {
            nix::sys::prctl::set_child_subreaper(true)
                .map_err(|error| SpawnError::SpawnFailed(error.to_string()))?;
            Ok(Self)
        }
        #[cfg(not(target_os = "linux"))]
        Err(SpawnError::SpawnFailed(
            "descendant exit observation is unavailable".into(),
        ))
    }
}

#[async_trait]
impl crate::core::process::ProcessSpawner for LocalSpawner {
    async fn spawn(
        &self,
        spec: &ProcessSpec,
        output: Option<ProcessOutputCallback>,
    ) -> Result<Box<dyn ProcessHandle>, SpawnError> {
        #[cfg(target_os = "linux")]
        let observe_groups = nix::sys::prctl::get_child_subreaper()
            .map_err(|error| SpawnError::SpawnFailed(error.to_string()))?;
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .kill_on_drop(true)
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

        // Keep wrappers and their JVM descendants in an owned group.
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
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

        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_QUEUE_CAPACITY);
        let (exit_tx, exit_rx) = mpsc::unbounded_channel::<i32>();

        let output_callbacks: Arc<Mutex<Vec<ProcessOutputCallback>>> =
            Arc::new(Mutex::new(output.into_iter().collect()));
        let exit_callbacks: Arc<Mutex<Vec<ProcessExitCallback>>> = Arc::new(Mutex::new(Vec::new()));

        let (exit_watch_tx, exit_watch_rx) = watch::channel(None);

        let (handle, output_task) = LocalProcessHandle::new(
            pid,
            stdin,
            output_rx,
            exit_rx,
            output_callbacks.clone(),
            exit_callbacks.clone(),
            exit_watch_rx,
        );

        #[cfg(target_os = "linux")]
        let handle = LocalProcessHandle {
            observe_groups,
            ..handle
        };
        LocalProcessHandle::spawn_readers(stdout, stderr, output_tx);

        let running = handle.running.clone();
        #[cfg(unix)]
        let cleanup_group = handle.cleanup_group.clone();
        tokio::spawn(async move {
            let exit_code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(-1),
                Err(e) => {
                    error!("process wait error: {}", e);
                    -1
                }
            };
            // A wrapper can exit before its JVM; release the whole owned group.
            #[cfg(unix)]
            if cleanup_group.load(Ordering::SeqCst)
                && let Err(error) = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                )
                && error != nix::errno::Errno::ESRCH
            {
                tracing::warn!(%error, "process group cleanup failed");
            }
            #[cfg(target_os = "linux")]
            let exit_code = if observe_groups {
                match tokio::task::spawn_blocking(move || wait_group(pid, exit_code)).await {
                    Ok(Ok(code)) => code,
                    Ok(Err(error)) => {
                        tracing::error!(%error,"process group exit observation failed");
                        return;
                    }
                    Err(error) => {
                        tracing::error!(%error,"process group wait task failed");
                        return;
                    }
                }
            } else {
                exit_code
            };
            let _ = output_task.await;
            running.store(false, Ordering::SeqCst);
            let _ = exit_tx.send(exit_code);
            let _ = exit_watch_tx.send(Some(exit_code));
        });

        debug!("spawned local process pid={}", pid);
        Ok(Box::new(handle))
    }
}

#[cfg(target_os = "linux")]
fn wait_group(group: u32, mut code: i32) -> Result<i32, ProcessError> {
    use nix::{
        errno::Errno,
        sys::wait::{WaitStatus, waitpid},
        unistd::Pid,
    };
    loop {
        match waitpid(Pid::from_raw(-(group as i32)), None) {
            Ok(WaitStatus::Exited(_, status)) => {
                if code == 0 {
                    code = status;
                }
            }
            Ok(WaitStatus::Signaled(..)) => {
                if code == 0 {
                    code = -1;
                }
            }
            Ok(_) => {}
            Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return Ok(code),
            Err(error) => return Err(ProcessError::WaitFailed(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::process::ProcessSpawner;

    #[tokio::test]
    async fn prewired_output_drains_short_lived_children_in_bounded_segments()
    -> Result<(), Box<dyn std::error::Error>> {
        const FIRST: &str = "initial\n";
        const LAST: &str = "界🙂final";
        let flood = OUTPUT_SEGMENT_BYTES * 4 - 4;
        let terminated_line = OUTPUT_SEGMENT_BYTES - 1;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&captured);
        let ring = crate::core::ring::OutputRingBuffer::new(2);
        let output_ring = ring.clone();
        let spec = ProcessSpec {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "printf '{FIRST}'; head -c {flood} /dev/zero; printf '{LAST}\r\n'; head -c {terminated_line} /dev/zero; printf '\r\n'"
                ),
            ],
            ..Default::default()
        };
        let handle = LocalSpawner::new()
            .spawn(
                &spec,
                Some(Box::new(move |data| {
                    assert!(data.len() <= OUTPUT_SEGMENT_BYTES);
                    assert!(std::str::from_utf8(data).is_ok());
                    if let Ok(mut bytes) = collected.lock() {
                        bytes.extend_from_slice(data);
                    }
                    output_ring.push_bytes(data);
                })),
            )
            .await?;
        let mut exited = handle.exit_watch().ok_or("process exit watch missing")?;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while exited.borrow().is_none() {
                exited.changed().await?;
            }
            Ok::<_, tokio::sync::watch::error::RecvError>(())
        })
        .await??;
        let bytes = captured.lock().map_err(|_| "output lock poisoned")?;
        assert!(bytes.starts_with(FIRST.as_bytes()));
        assert_eq!(
            &bytes[FIRST.len() + flood..FIRST.len() + flood + LAST.len()],
            LAST.as_bytes()
        );
        assert_eq!(
            bytes.len(),
            FIRST.len() + flood + LAST.len() + terminated_line + 4
        );
        drop(bytes);
        assert_eq!(ring.len(), 2);
        assert!(
            ring.all_lines()
                .iter()
                .all(|line| !line.is_empty() && line.len() <= OUTPUT_SEGMENT_BYTES)
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_local_spawner_echo() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "echo".into(),
            args: vec!["hello".into()],
            ..Default::default()
        };
        let handle = spawner.spawn(&spec, None).await.unwrap();
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
        let handle = spawner.spawn(&spec, None).await.unwrap();
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
        let handle = spawner.spawn(&spec, None).await.unwrap();

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
        let handle = spawner.spawn(&spec, None).await.unwrap();

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
        let handle = spawner.spawn(&spec, None).await.unwrap();
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
        let handle = spawner.spawn(&spec, None).await.unwrap();
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
        let handle = spawner.spawn(&spec, None).await.unwrap();
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
        let _handle = spawner.spawn(&spec, None).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_local_spawner_invalid_command() {
        let spawner = LocalSpawner::new();
        let spec = ProcessSpec {
            command: "/nonexistent/command_xyz".into(),
            ..Default::default()
        };
        let result = spawner.spawn(&spec, None).await;
        assert!(result.is_err());
    }

    #[cfg(target_os = "linux")]
    async fn assert_wrapper_descendant_stops(kill_wrapper: bool) {
        use std::time::Duration;

        let directory = tempfile::tempdir().unwrap();
        let spec = ProcessSpec {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "sleep 60 & echo $! > child.pid; read line".into(),
            ],
            cwd: Some(directory.path().to_path_buf()),
            ..Default::default()
        };
        let handle = LocalSpawner::new().spawn(&spec, None).await.unwrap();
        let child_pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(contents) =
                    tokio::fs::read_to_string(directory.path().join("child.pid")).await
                    && let Ok(pid) = contents.trim().parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if kill_wrapper {
            handle.kill(Signal::Kill).await.unwrap();
        } else {
            handle.write(b"exit\n").await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = tokio::fs::read_to_string(format!("/proc/{child_pid}/stat")).await;
                // Orphaned zombies are already dead and await reaping by the host init.
                let stopped = match status {
                    Ok(status) => status
                        .rsplit_once(") ")
                        .is_some_and(|(_, rest)| rest.starts_with('Z')),
                    Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                };
                if stopped && !handle.is_running() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn killing_wrapper_stops_descendants() {
        assert_wrapper_descendant_stops(true).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn exiting_wrapper_stops_descendants() {
        assert_wrapper_descendant_stops(false).await;
    }
}
