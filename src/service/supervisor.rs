use crate::core::state::InstanceState;
use crate::instance::base::{Instance, InstanceError};
use crate::service::control::ControlMsg;
use crate::service::health::{AlwaysHealthyCheck, CompositeHealthCheck, HealthCheck};
use crate::service::restart::{RestartPolicy, RestartState};
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

/// Supervisor that continuously monitors and controls an instance.
///
/// Runs in a dedicated tokio task. Handles:
/// - Control messages (start, stop, kill, restart, `send_command`)
/// - Process exit detection and auto-restart
/// - Periodic health checks
/// - State transition management
#[derive(Debug)]
pub struct Supervisor<I: Instance> {
    instance: Arc<Mutex<I>>,
    restart_state: RestartState,
    health_check: Arc<dyn HealthCheck>,
    health_interval_secs: u64,
    control_rx: mpsc::Receiver<ControlMsg>,
    /// Shared state that the service can read.
    shared_state: watch::Sender<InstanceState>,
}

impl<I: Instance + Debug> Supervisor<I> {
    pub fn new(
        instance: I,
        restart_policy: RestartPolicy,
        control_rx: mpsc::Receiver<ControlMsg>,
    ) -> (Self, watch::Receiver<InstanceState>) {
        let initial_state = instance.state();
        let (shared_state, state_rx) = watch::channel(initial_state);
        (
            Self {
                instance: Arc::new(Mutex::new(instance)),
                restart_state: RestartState::new(restart_policy),
                health_check: Arc::new(AlwaysHealthyCheck),
                health_interval_secs: 30,
                control_rx,
                shared_state,
            },
            state_rx,
        )
    }

    /// Set a custom health check (replaces the default `AlwaysHealthyCheck`).
    ///
    /// The health check is type-erased into `Arc<dyn HealthCheck>` for runtime
    /// flexibility. Callers should wrap their concrete check in `Arc::new()`.
    #[must_use]
    pub fn with_health_check(mut self, check: Arc<dyn HealthCheck>) -> Self {
        self.health_check = check;
        self
    }

    /// Set the health check interval in seconds.
    #[must_use]
    pub const fn with_health_interval(mut self, secs: u64) -> Self {
        self.health_interval_secs = secs;
        self
    }

    /// Set a composite health check built from multiple checks.
    #[must_use]
    pub fn with_composite_health(mut self, checks: Vec<Box<dyn HealthCheck>>) -> Self {
        self.health_check = Arc::new(CompositeHealthCheck::new(checks));
        self
    }

    /// Run the supervisor loop. This consumes the supervisor and runs until Shutdown is received.
    pub async fn run(mut self) {
        info!("supervisor started");

        let mut health_tick = interval(Duration::from_secs(self.health_interval_secs));
        health_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the first immediate tick so health check doesn't run before instance starts
        health_tick.tick().await;

        // Track whether we have an active process exit watcher
        let mut exit_watch: Option<watch::Receiver<Option<i32>>> = None;

        loop {
            tokio::select! {
                biased;

                // Process control messages
                msg = self.control_rx.recv() => {
                    let Some(msg) = msg else {
                        if let Err(error) = self.handle_shutdown().await {
                            warn!("stop after owner release failed: {}", error);
                        }
                        break;
                    };
                    let (msg, reply) = match msg {
                        ControlMsg::Acknowledged { command, reply } => (*command, Some(reply)),
                        msg => (msg, None),
                    };
                    let shutdown = matches!(msg, ControlMsg::Shutdown);
                    let result = match msg {
                        ControlMsg::Start => self.handle_start(&mut exit_watch).await,
                        ControlMsg::Stop => {
                            let result = self.handle_stop().await;
                            if result.is_ok() { exit_watch = None; }
                            result
                        }
                        ControlMsg::Kill => {
                            let result = self.handle_kill().await;
                            if result.is_ok() { exit_watch = None; }
                            result
                        }
                        ControlMsg::Restart => self.handle_restart(&mut exit_watch).await,
                        ControlMsg::SendCommand(cmd) => self.handle_send_command(&cmd).await,
                        ControlMsg::GetPlayTime(tx) => {
                            let _ = tx.send(self.instance.lock().await.play_time_secs());
                            Ok(())
                        }
                        ControlMsg::GetProcessId(tx) => {
                            let _ = tx.send(self.instance.lock().await.process_handle().and_then(|process| process.pid()));
                            Ok(())
                        }
                        ControlMsg::GetRecentOutput { n, tx } => {
                            let _ = tx.send(self.instance.lock().await.recent_output(n));
                            Ok(())
                        }
                        ControlMsg::GetRecentOutputWithOffset { n, tx } => {
                            let _ = tx.send(self.instance.lock().await.recent_output_with_offset(n));
                            Ok(())
                        }
                        ControlMsg::Shutdown => self.handle_shutdown().await,
                        ControlMsg::Acknowledged { .. } => Err(InstanceError::Config("nested acknowledgement".into())),
                    };
                    // Publish before acknowledging so callers observe the completed operation.
                    let _ = self.shared_state.send(self.instance.lock().await.state());
                    let completed = result.is_ok();
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    } else if let Err(error) = result {
                        warn!("instance control failed: {}", error);
                    }
                    if shutdown && completed { break; }
                }

                // Watch for process exit
                Ok(()) = async {
                    if let Some(ref mut watch) = exit_watch {
                        // Check if already exited (process died before we started watching)
                        if watch.borrow().is_some() {
                            return Ok(());
                        }
                        watch.changed().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    let code = exit_watch.as_ref().and_then(|w| *w.borrow());
                    if let Some(code) = code {
                        info!("process exited with code {}", code);
                        exit_watch = None;

                        // Clean up instance state (transition to Stopped)
                        if let Err(e) = self.handle_process_exit(code).await {
                            warn!("process exit cleanup failed: {}", e);
                        }

                        // Auto-restart logic
                        if self.restart_state.should_restart(code) {
                            let backoff = self.restart_state.record_attempt();
                            info!(
                                "auto-restarting in {:?} (attempt {})",
                                backoff,
                                self.restart_state.attempt_count()
                            );
                            tokio::time::sleep(backoff).await;
                            if let Err(e) = self.handle_start(&mut exit_watch).await {
                                warn!("auto-restart failed: {}", e);
                            }
                        } else {
                            debug!("auto-restart skipped or max retries reached");
                        }
                    }
                }

                // Health check + play time tracking
                _ = health_tick.tick() => {
                    let mut inst = self.instance.lock().await;
                    if inst.is_running() {
                        inst.increment_play_time(self.health_interval_secs);
                        drop(inst);
                        if !self.health_check.check().await {
                            warn!("health check '{}' failed, killing instance", self.health_check.name());
                            if let Err(e) = self.handle_kill().await {
                                warn!("kill after health check failed: {}", e);
                            }
                            exit_watch = None;
                        }
                    }
                }
            }

            // Publish state update
            let state = self.instance.lock().await.state();
            let _ = self.shared_state.send(state);
        }

        info!("supervisor stopped");
    }

    async fn handle_start(
        &self,
        exit_watch: &mut Option<watch::Receiver<Option<i32>>>,
    ) -> Result<(), InstanceError> {
        let mut inst = self.instance.lock().await;
        if inst.is_running() {
            debug!("instance already running, skipping start");
            return Ok(());
        }

        inst.start().await?;

        // Get the exit watch from the instance
        *exit_watch = inst.exit_watch();
        debug!(
            "instance started, exit watch acquired: {}",
            exit_watch.is_some()
        );
        drop(inst);
        Ok(())
    }

    async fn handle_stop(&self) -> Result<(), InstanceError> {
        let mut inst = self.instance.lock().await;
        if !inst.is_stoppable() {
            if inst.process_handle().is_some() {
                return Err(InstanceError::NotStoppable(inst.state()));
            }
            debug!("instance not stoppable in state {:?}", inst.state());
            return Ok(());
        }
        inst.stop().await
    }

    async fn handle_kill(&self) -> Result<(), InstanceError> {
        let mut inst = self.instance.lock().await;
        if !inst.is_stoppable() {
            if inst.process_handle().is_some() {
                return Err(InstanceError::NotStoppable(inst.state()));
            }
            debug!("instance not killable in state {:?}", inst.state());
            return Ok(());
        }
        inst.kill().await
    }

    async fn handle_restart(
        &mut self,
        exit_watch: &mut Option<watch::Receiver<Option<i32>>>,
    ) -> Result<(), InstanceError> {
        self.handle_stop().await?;
        // Wait a brief moment for clean shutdown
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.restart_state.reset();
        self.handle_start(exit_watch).await
    }

    async fn handle_shutdown(&self) -> Result<(), InstanceError> {
        if let Err(error) = self.handle_stop().await {
            warn!(%error, "graceful shutdown failed");
        }
        let mut instance = self.instance.lock().await;
        if let Some(process) = instance.process_handle() {
            if let Err(error) = process.kill(crate::core::process::Signal::Kill).await
                && !matches!(error, crate::core::process::ProcessError::AlreadyExited)
            {
                return Err(error.into());
            }
            instance.clear_process(-1);
        }
        drop(instance);
        Ok(())
    }

    async fn handle_send_command(&self, cmd: &str) -> Result<(), InstanceError> {
        let inst = self.instance.lock().await;
        inst.send_command(cmd).await
    }

    /// Clean up after process exit: transition state to Stopped, clear process handle.
    async fn handle_process_exit(&self, exit_code: i32) -> Result<(), InstanceError> {
        let mut inst = self.instance.lock().await;
        if inst.process_handle().is_some() {
            inst.clear_process(exit_code);
            drop(inst);
        }
        debug!("process exit cleanup complete, code={}", exit_code);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::TokioBroadcastBus;
    use crate::core::state::InstanceState;
    use crate::instance::builder::InstanceBuilder;
    use crate::instance::path::InstancePath;
    use crate::instance::server::ServerConfig;
    use crate::process::local::LocalSpawner;

    #[derive(Debug)]
    struct ProcessControl {
        alive: std::sync::atomic::AtomicBool,
        failures: std::sync::atomic::AtomicUsize,
        spawns: std::sync::atomic::AtomicUsize,
        exit: watch::Sender<Option<i32>>,
    }

    #[derive(Debug)]
    struct ControlledProcess(Arc<ProcessControl>);

    #[async_trait::async_trait]
    impl crate::core::process::ProcessHandle for ControlledProcess {
        fn pid(&self) -> Option<u32> {
            self.is_running().then_some(42)
        }
        async fn write(&self, _data: &[u8]) -> Result<(), crate::core::process::ProcessError> {
            Ok(())
        }
        async fn kill(
            &self,
            _signal: crate::core::process::Signal,
        ) -> Result<(), crate::core::process::ProcessError> {
            use std::sync::atomic::Ordering::SeqCst;
            if self
                .0
                .failures
                .try_update(SeqCst, SeqCst, |count| count.checked_sub(1))
                .is_ok()
            {
                return Err(crate::core::process::ProcessError::KillFailed(
                    "injected termination failure".into(),
                ));
            }
            self.0.alive.store(false, SeqCst);
            self.0.exit.send_replace(Some(0));
            Ok(())
        }
        fn on_output(&self, _callback: crate::core::process::ProcessOutputCallback) {}
        fn on_exit(&self, _callback: crate::core::process::ProcessExitCallback) {}
        fn is_running(&self) -> bool {
            self.0.alive.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn exit_watch(&self) -> Option<watch::Receiver<Option<i32>>> {
            Some(self.0.exit.subscribe())
        }
    }

    #[async_trait::async_trait]
    impl crate::core::process::ProcessSpawner for ControlledProcess {
        async fn spawn(
            &self,
            _spec: &crate::core::process::ProcessSpec,
        ) -> Result<Box<dyn crate::core::process::ProcessHandle>, crate::core::process::SpawnError>
        {
            use std::sync::atomic::Ordering::SeqCst;
            self.0.spawns.fetch_add(1, SeqCst);
            self.0.alive.store(true, SeqCst);
            self.0.exit.send_replace(None);
            Ok(Box::new(Self(Arc::clone(&self.0))))
        }
    }

    #[tokio::test]
    async fn termination_errors_preserve_ownership_and_shutdown_retries_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
        let (exit, _) = watch::channel(None);
        let control = Arc::new(ProcessControl {
            alive: AtomicBool::new(false),
            failures: AtomicUsize::new(1),
            spawns: AtomicUsize::new(0),
            exit,
        });
        let server = InstanceBuilder::new("controlled".into(), InstancePath::new("."))
            .spawner(ControlledProcess(Arc::clone(&control)))
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "managed-service".into(),
                stop_command: String::new(),
                server_type: "universal".into(),
                ..Default::default()
            });
        let service =
            crate::service::instance::InstanceService::new(server, RestartPolicy::never());
        service.start().await?;
        assert!(service.stop().await.is_err());
        assert!(matches!(service.state(), InstanceState::Error(_)));
        assert!(control.alive.load(SeqCst));
        assert!(service.start().await.is_err());
        assert_eq!(control.spawns.load(SeqCst), 1);
        service.stop().await?;
        assert_eq!(service.state(), InstanceState::Stopped);
        service.start().await?;
        control.failures.store(3, SeqCst);
        assert!(service.kill().await.is_err());
        assert!(service.shutdown().await.is_err());
        assert!(control.alive.load(SeqCst));
        control.failures.store(1, SeqCst);
        service.shutdown().await?;
        assert!(!control.alive.load(SeqCst));
        assert_eq!(service.state(), InstanceState::Stopped);
        Ok(())
    }

    #[tokio::test]
    async fn test_supervisor_start_stop() {
        let (tx, rx) = mpsc::channel(16);
        let server = InstanceBuilder::new("test-supervisor".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sh -c 'sleep 10'".into(),
                stop_command: "^C".into(),
                ..Default::default()
            });

        let (supervisor, state_rx) = Supervisor::new(server, RestartPolicy::never(), rx);
        let handle = tokio::spawn(supervisor.run());

        // Start
        tx.send(ControlMsg::Start).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(*state_rx.borrow(), InstanceState::Running);

        // Kill (stop would wait 10s for graceful shutdown)
        tx.send(ControlMsg::Kill).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(*state_rx.borrow(), InstanceState::Stopped);

        // Shutdown
        tx.send(ControlMsg::Shutdown).await.unwrap();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_supervisor_get_state() {
        let (tx, rx) = mpsc::channel(16);
        let server = InstanceBuilder::new("test-state".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sh -c 'sleep 10'".into(),
                ..Default::default()
            });

        let (supervisor, state_rx) = Supervisor::new(server, RestartPolicy::never(), rx);
        let handle = tokio::spawn(supervisor.run());

        // State is available directly from the watch receiver
        assert_eq!(*state_rx.borrow(), InstanceState::Stopped);

        tx.send(ControlMsg::Shutdown).await.unwrap();
        handle.await.unwrap();
    }
}
