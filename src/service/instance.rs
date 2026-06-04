use crate::core::state::InstanceState;
use crate::instance::base::{Instance, InstanceError};
use crate::service::control::ControlMsg;
use crate::service::health::HealthCheck;
use crate::service::restart::RestartPolicy;
use crate::service::supervisor::Supervisor;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// A container-like service that wraps an instance and provides
/// continuous monitoring, auto-restart, and external control.
///
/// All operations are async and non-blocking. The actual work is done
/// by a background supervisor task.
#[derive(Debug)]
pub struct InstanceService {
    control_tx: mpsc::Sender<ControlMsg>,
    state_rx: watch::Receiver<InstanceState>,
    id: String,
}

impl InstanceService {
    /// Create a new service from an instance and restart policy.
    ///
    /// This spawns the supervisor background task immediately.
    pub fn new<I>(instance: I, restart_policy: RestartPolicy) -> Self
    where
        I: Instance + Debug + 'static,
    {
        let id = instance.id().to_string();
        let (control_tx, control_rx) = mpsc::channel(32);
        let (supervisor, state_rx) = Supervisor::new(instance, restart_policy, control_rx);

        tokio::spawn(async move {
            supervisor.run().await;
        });

        Self {
            control_tx,
            state_rx,
            id,
        }
    }

    /// Create a service with a custom health check.
    pub fn with_health_check<I>(
        instance: I,
        restart_policy: RestartPolicy,
        health_check: Arc<dyn HealthCheck>,
    ) -> Self
    where
        I: Instance + Debug + 'static,
    {
        let id = instance.id().to_string();
        let (control_tx, control_rx) = mpsc::channel(32);
        let (supervisor, state_rx) = Supervisor::new(instance, restart_policy, control_rx);
        let supervisor = supervisor.with_health_check(health_check);

        tokio::spawn(async move {
            supervisor.run().await;
        });

        Self {
            control_tx,
            state_rx,
            id,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Current state of the instance.
    #[must_use]
    pub fn state(&self) -> InstanceState {
        self.state_rx.borrow().clone()
    }

    /// Subscribe to state changes.
    #[must_use]
    pub fn state_watch(&self) -> watch::Receiver<InstanceState> {
        self.state_rx.clone()
    }

    /// Total play time in seconds.
    pub async fn play_time_secs(&self) -> u64 {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .control_tx
            .send(ControlMsg::GetPlayTime(tx))
            .await
            .is_err()
        {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    /// Start the instance.
    pub async fn start(&self) -> Result<(), InstanceServiceError> {
        self.send_control(ControlMsg::Start).await
    }

    /// Stop the instance gracefully.
    pub async fn stop(&self) -> Result<(), InstanceServiceError> {
        self.send_control(ControlMsg::Stop).await
    }

    /// Kill the instance immediately.
    pub async fn kill(&self) -> Result<(), InstanceServiceError> {
        self.send_control(ControlMsg::Kill).await
    }

    /// Restart the instance.
    pub async fn restart(&self) -> Result<(), InstanceServiceError> {
        self.send_control(ControlMsg::Restart).await
    }

    /// Send a command to the instance's stdin.
    pub async fn send_command(&self, cmd: String) -> Result<(), InstanceServiceError> {
        self.send_control(ControlMsg::SendCommand(cmd)).await
    }

    /// Shutdown the service (stops the supervisor loop).
    pub async fn shutdown(&self) -> Result<(), InstanceServiceError> {
        self.send_control(ControlMsg::Shutdown).await
    }

    /// Retrieve the most recent `n` lines of stdout/stderr output.
    pub async fn recent_output(&self, n: usize) -> Result<Vec<String>, InstanceServiceError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.control_tx
            .send(ControlMsg::GetRecentOutput { n, tx })
            .await
            .map_err(|_| InstanceServiceError::SupervisorShutdown)?;
        rx.await
            .map_err(|_| InstanceServiceError::SupervisorShutdown)
    }

    async fn send_control(&self, msg: ControlMsg) -> Result<(), InstanceServiceError> {
        self.control_tx
            .send(msg)
            .await
            .map_err(|_| InstanceServiceError::SupervisorShutdown)
    }
}

/// Errors from the instance service.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum InstanceServiceError {
    #[error("supervisor has shut down")]
    SupervisorShutdown,
    #[error("instance error: {0}")]
    Instance(String),
}

impl From<InstanceError> for InstanceServiceError {
    fn from(e: InstanceError) -> Self {
        Self::Instance(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::TokioBroadcastBus;
    use crate::instance::builder::InstanceBuilder;
    use crate::instance::path::InstancePath;
    use crate::instance::server::ServerConfig;
    use crate::process::local::LocalSpawner;

    #[tokio::test]
    async fn test_service_lifecycle() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-service".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    stop_command: "^C".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        assert_eq!(service.state(), InstanceState::Stopped);

        service.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(service.state(), InstanceState::Running);

        // Use kill instead of stop to avoid 10s graceful shutdown timeout
        service.kill().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(service.state(), InstanceState::Stopped);

        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_state_watch() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-watch".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        let mut watch = service.state_watch();
        assert_eq!(*watch.borrow(), InstanceState::Stopped);

        service.start().await.unwrap();
        watch.changed().await.unwrap();
        assert_eq!(*watch.borrow(), InstanceState::Running);

        service.kill().await.unwrap();
        watch.changed().await.unwrap();
        assert_eq!(*watch.borrow(), InstanceState::Stopped);

        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_send_command() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-cmd".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "cat".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        service.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        service.send_command("hello".to_string()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        service.kill().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_restart() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-restart".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    stop_command: "^C".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        service.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(service.state(), InstanceState::Running);

        service.restart().await.unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(service.state(), InstanceState::Running);

        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_auto_restart() {
        // Process exits quickly, service should auto-restart
        let service = InstanceService::new(
            InstanceBuilder::new("test-auto-restart".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 0.1".into(),
                    ..Default::default()
                }),
            RestartPolicy {
                enabled: true,
                max_retries: 2,
                backoff_base_ms: 50,
                backoff_max_ms: 200,
                reset_window_secs: 60,
                exit_code_allowlist: vec![],
            },
        );

        service.start().await.unwrap();

        // Wait for process to exit and auto-restart
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(service.state(), InstanceState::Running);

        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_max_restart_limit() {
        // Process always crashes immediately (exits with code 1)
        let service = InstanceService::new(
            InstanceBuilder::new("test-max-restart".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "false".into(),
                    ..Default::default()
                }),
            RestartPolicy {
                enabled: true,
                max_retries: 1,
                backoff_base_ms: 10,
                backoff_max_ms: 50,
                reset_window_secs: 60,
                exit_code_allowlist: vec![],
            },
        );

        service.start().await.unwrap();

        // Wait for: initial start -> exit -> backoff -> restart -> exit -> max retries reached
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(service.state(), InstanceState::Stopped);

        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_play_time_zero_when_stopped() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-playtime-zero".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        // Play time should be zero when instance has never started
        assert_eq!(service.play_time_secs().await, 0);

        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_recent_output_after_shutdown() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-output".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        service.shutdown().await.unwrap();

        // Recent output should fail after supervisor shuts down
        let result = service.recent_output(10).await;
        assert!(
            matches!(result, Err(InstanceServiceError::SupervisorShutdown)),
            "expected SupervisorShutdown, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_service_start_when_already_running() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-double-start".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        service.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(service.state(), InstanceState::Running);

        // Starting an already-running instance is a no-op (supervisor silently ignores)
        // but must not panic or corrupt state.
        service.start().await.unwrap();
        assert_eq!(service.state(), InstanceState::Running);

        service.kill().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_service_stop_when_not_running() {
        let service = InstanceService::new(
            InstanceBuilder::new("test-stop-idle".to_string(), InstancePath::new("."))
                .spawner(LocalSpawner::new())
                .event_bus(TokioBroadcastBus::new())
                .build_server(ServerConfig {
                    start_command: "sleep 10".into(),
                    ..Default::default()
                }),
            RestartPolicy::never(),
        );

        // Stopping an idle instance is a no-op but must not panic
        service.stop().await.unwrap();
        assert_eq!(service.state(), InstanceState::Stopped);

        service.shutdown().await.unwrap();
    }

    use std::time::Duration;
}
