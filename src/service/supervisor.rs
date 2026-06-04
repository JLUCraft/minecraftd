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
                Some(msg) = self.control_rx.recv() => {
                    match msg {
                        ControlMsg::Start => {
                            if let Err(e) = self.handle_start(&mut exit_watch).await {
                                warn!("start failed: {}", e);
                            }
                        }
                        ControlMsg::Stop => {
                            if let Err(e) = self.handle_stop().await {
                                warn!("stop failed: {}", e);
                            }
                            exit_watch = None;
                        }
                        ControlMsg::Kill => {
                            if let Err(e) = self.handle_kill().await {
                                warn!("kill failed: {}", e);
                            }
                            exit_watch = None;
                        }
                        ControlMsg::Restart => {
                            if let Err(e) = self.handle_restart(&mut exit_watch).await {
                                warn!("restart failed: {}", e);
                            }
                        }
                        ControlMsg::SendCommand(cmd) => {
                            if let Err(e) = self.handle_send_command(&cmd).await {
                                warn!("send_command failed: {}", e);
                            }
                        }
                        ControlMsg::GetPlayTime(tx) => {
                            let secs = self.instance.lock().await.play_time_secs();
                            let _ = tx.send(secs);
                        }
                        ControlMsg::GetRecentOutput { n, tx } => {
                            let lines = self.instance.lock().await.recent_output(n);
                            let _ = tx.send(lines);
                        }
                        ControlMsg::Shutdown => {
                            info!("supervisor shutting down");
                            let _ = self.handle_stop().await;
                            break;
                        }
                    }
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
            debug!("instance not stoppable in state {:?}", inst.state());
            return Ok(());
        }
        inst.stop().await
    }

    async fn handle_kill(&self) -> Result<(), InstanceError> {
        let mut inst = self.instance.lock().await;
        if !inst.is_stoppable() {
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

    async fn handle_send_command(&self, cmd: &str) -> Result<(), InstanceError> {
        let inst = self.instance.lock().await;
        inst.send_command(cmd).await
    }

    /// Clean up after process exit: transition state to Stopped, clear process handle.
    async fn handle_process_exit(&self, exit_code: i32) -> Result<(), InstanceError> {
        let mut inst = self.instance.lock().await;
        if inst.is_running() {
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

    #[tokio::test]
    async fn test_supervisor_start_stop() {
        let (tx, rx) = mpsc::channel(16);
        let server = InstanceBuilder::new("test-supervisor".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sleep 10".into(),
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
                start_command: "sleep 10".into(),
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
