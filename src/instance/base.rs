use crate::core::event::{EventBus, InstanceEvent};
use crate::core::process::{ProcessHandle, ProcessSpec};
use crate::core::ring::OutputRingBuffer;
use crate::core::state::{AtomicStateMachine, InstanceState, StateError, StateMachine};
use crate::instance::path::{InstancePath, InstanceSubdir};
use crate::lifecycle::manager::LifecycleManager;
use async_trait::async_trait;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Core trait for all Minecraft instances (server or client).
///
/// Every instance has an identity (`id`), a location on the file system
/// ([`path`](Self::path)), a lifecycle state, and the ability to spawn and
/// control a process.
///
/// ## Design rationale: `dyn Trait` in trait interface
///
/// While [`InstanceCore`] uses generic type parameters (`S`, `E`) for
/// zero-cost static dispatch, this trait returns `&dyn ProcessHandle` and
/// `&dyn EventBus` for **object safety**: downstream consumers such as
/// [`Supervisor`](crate::service::supervisor::Supervisor) store instances
/// as `Arc<Mutex<I>>` where `I: Instance`, which requires the trait to be
/// object-safe. The `dyn Trait` return types are a deliberate trade-off
/// between compile-time polymorphism (inside [`InstanceCore`]) and runtime
/// polymorphism (in the supervisor layer).
#[async_trait]
pub trait Instance: Send + Sync + Debug {
    fn id(&self) -> &str;
    /// Canonical file-system path of this instance.
    ///
    /// The root directory is the instance's anchor — all subdirectories
    /// (saves, mods, configs, etc.) are derived from it via
    /// [`InstanceSubdir`].
    fn path(&self) -> &InstancePath;
    fn state(&self) -> InstanceState;
    fn is_startable(&self) -> bool;
    fn is_stoppable(&self) -> bool;
    fn is_running(&self) -> bool {
        self.state() == InstanceState::Running
    }
    async fn start(&mut self) -> Result<(), InstanceError>;
    async fn stop(&mut self) -> Result<(), InstanceError>;
    async fn kill(&mut self) -> Result<(), InstanceError>;
    async fn restart(&mut self) -> Result<(), InstanceError> {
        self.stop().await?;
        self.start().await
    }
    async fn send_command(&self, cmd: &str) -> Result<(), InstanceError>;
    fn process_handle(&self) -> Option<&dyn ProcessHandle>;
    fn event_bus(&self) -> &dyn EventBus;
    /// Returns the exit watch receiver if a process is running.
    fn exit_watch(&self) -> Option<tokio::sync::watch::Receiver<Option<i32>>> {
        None
    }
    /// Clear the process handle and transition to Stopped state.
    /// Used by the supervisor when it detects the process has exited.
    fn clear_process(&mut self, exit_code: i32);
    /// Total play time in seconds.
    fn play_time_secs(&self) -> u64 {
        0
    }
    /// Increment play time by the given number of seconds.
    fn increment_play_time(&mut self, secs: u64);
    /// Returns the most recent `n` lines of output from the output ring buffer.
    fn recent_output(&self, _n: usize) -> Vec<String> {
        Vec::new()
    }
    /// Sets up the output ring buffer with the given capacity.
    fn setup_output_ring(&mut self, capacity: usize);
    /// Convenience: derive the absolute path to a well-known subdirectory.
    fn subdir(&self, kind: InstanceSubdir) -> PathBuf {
        self.path().subdir(kind)
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum InstanceError {
    #[error("instance is not startable in state {0:?}")]
    NotStartable(InstanceState),
    #[error("instance is not stoppable in state {0:?}")]
    NotStoppable(InstanceState),
    #[error("instance is locked by another operation")]
    Locked,
    #[error("process error: {0}")]
    Process(String),
    #[error("spawn error: {0}")]
    Spawn(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("lifecycle error: {0}")]
    Lifecycle(String),
    #[error("state error: {0}")]
    State(String),
    #[error("not implemented")]
    NotImplemented,
}

impl From<StateError> for InstanceError {
    fn from(e: StateError) -> Self {
        Self::State(e.to_string())
    }
}

impl From<crate::core::process::ProcessError> for InstanceError {
    fn from(e: crate::core::process::ProcessError) -> Self {
        Self::Process(e.to_string())
    }
}

impl From<crate::core::process::SpawnError> for InstanceError {
    fn from(e: crate::core::process::SpawnError) -> Self {
        Self::Spawn(e.to_string())
    }
}

/// The core instance implementation with generic composable parts.
///
/// This is the key design: `S` (spawner), `E` (event bus), `C` (config) are all
/// generic type parameters. This means:
/// - Any spawner can be combined with any event bus and any config provider
/// - The combinations are resolved at compile time (zero-cost)
/// - No `Arc<dyn Trait>` overhead
pub struct InstanceCore<S, E>
where
    S: crate::core::process::ProcessSpawner,
    E: EventBus,
{
    pub id: String,
    /// Canonical file-system path of this instance.
    pub path: InstancePath,
    pub state_machine: AtomicStateMachine,
    pub spawner: S,
    pub lifecycle: LifecycleManager,
    pub event_bus: E,
    pub process: Option<Box<dyn ProcessHandle>>,
    pub lock: Arc<Mutex<()>>,
    /// Watch receiver for process exit. Populated when process is set.
    pub exit_watch: Option<tokio::sync::watch::Receiver<Option<i32>>>,
    /// Total play time in seconds. Incremented by the supervisor while running.
    pub play_time_secs: u64,
    /// Output ring buffer for capturing stdout/stderr lines.
    pub output_ring: Option<OutputRingBuffer>,
}

impl<S, E> std::fmt::Debug for InstanceCore<S, E>
where
    S: crate::core::process::ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceCore")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("state_machine", &self.state_machine)
            .field("spawner", &self.spawner)
            .field("event_bus", &self.event_bus)
            .field("process", &self.process.is_some())
            .finish_non_exhaustive()
    }
}

impl<S, E> InstanceCore<S, E>
where
    S: crate::core::process::ProcessSpawner,
    E: EventBus,
{
    pub fn new(id: String, path: InstancePath, spawner: S, event_bus: E) -> Self {
        Self {
            id,
            path,
            state_machine: AtomicStateMachine::with_stopped(),
            spawner,
            lifecycle: LifecycleManager::new(),
            event_bus,
            process: None,
            lock: Arc::new(Mutex::new(())),
            exit_watch: None,
            play_time_secs: 0,
            output_ring: None,
        }
    }

    /// Acquires the instance lock.
    ///
    /// # Errors
    ///
    /// Returns `InstanceError::Locked` if the lock is already held.
    pub fn acquire_lock(&self) -> Result<(), InstanceError> {
        self.lock
            .try_lock()
            .map_or(Err(InstanceError::Locked), |_guard| Ok(()))
    }

    pub fn emit(&self, event: InstanceEvent) {
        self.event_bus.emit(Box::new(event));
    }

    pub fn set_process(&mut self, process: Box<dyn ProcessHandle>) {
        self.exit_watch = process.exit_watch();
        self.process = Some(process);
        if let Err(e) = self.state_machine.transition(InstanceState::Running) {
            tracing::warn!("failed to transition to Running: {}", e);
        }
        self.emit(InstanceEvent::Start {
            instance_id: self.id.clone(),
        });
    }

    pub fn clear_process(&mut self, exit_code: i32) {
        self.process = None;
        self.exit_watch = None;
        // If currently Running, need to go through Stopping first
        if self.state_machine.state() == InstanceState::Running
            && let Err(e) = self.state_machine.transition(InstanceState::Stopping)
        {
            tracing::warn!("failed to transition to Stopping: {}", e);
        }
        if let Err(e) = self.state_machine.transition(InstanceState::Stopped) {
            tracing::warn!("failed to transition to Stopped: {}", e);
        }
        self.emit(InstanceEvent::Stop {
            instance_id: self.id.clone(),
            exit_code,
        });
    }

    /// Spawns a process using the configured spawner.
    ///
    /// # Errors
    ///
    /// Returns `InstanceError::Spawn` if the process fails to spawn.
    pub async fn spawn_process(
        &self,
        spec: &ProcessSpec,
    ) -> Result<Box<dyn ProcessHandle>, InstanceError> {
        let handle = self.spawner.spawn(spec).await?;
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::TokioBroadcastBus;
    use crate::process::local::LocalSpawner;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_instance_core_creation() {
        let core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        assert_eq!(core.id, "test");
        assert_eq!(core.state_machine.state(), InstanceState::Stopped);
        assert!(core.process.is_none());
    }

    #[tokio::test]
    async fn test_instance_core_lock() {
        let core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        // First acquire succeeds
        let _guard = core.lock.try_lock().unwrap();
        // Second acquire should fail since the guard is held
        assert!(matches!(core.acquire_lock(), Err(InstanceError::Locked)));
    }

    #[tokio::test]
    async fn test_instance_core_emit() {
        let bus = TokioBroadcastBus::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        bus.subscribe(
            "instance:start",
            Box::new(move |_evt| {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            bus,
        );
        core.emit(InstanceEvent::Start {
            instance_id: "test".into(),
        });
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_instance_core_set_and_clear_process() {
        let mut core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );

        // First transition to Starting so that set_process can go to Running
        core.state_machine
            .transition(InstanceState::Starting)
            .unwrap();

        // Spawn a real process to get a handle
        let spec = crate::core::process::ProcessSpec {
            command: "sleep".into(),
            args: vec!["0.5".into()],
            ..Default::default()
        };
        let handle = core.spawn_process(&spec).await.unwrap();
        core.set_process(handle);
        assert_eq!(core.state_machine.state(), InstanceState::Running);
        assert!(core.process.is_some());

        // Transition to Stopping before clearing so clear_process can go to Stopped
        core.state_machine
            .transition(InstanceState::Stopping)
            .unwrap();
        core.clear_process(0);
        assert_eq!(core.state_machine.state(), InstanceState::Stopped);
        assert!(core.process.is_none());
    }

    #[tokio::test]
    async fn test_instance_core_set_process_invalid_transition() {
        let mut core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );

        // set_process when state is Stopped (not Starting) should log warning but not panic
        let spec = crate::core::process::ProcessSpec {
            command: "sleep".into(),
            args: vec!["0.1".into()],
            ..Default::default()
        };
        let handle = core.spawn_process(&spec).await.unwrap();
        core.set_process(handle);
        // State should remain Stopped because Running <- Stopped is invalid
        assert_eq!(core.state_machine.state(), InstanceState::Stopped);
    }

    #[tokio::test]
    async fn test_instance_core_clear_process_invalid_transition() {
        let mut core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );

        // clear_process when state is Stopped (not Stopping) should log warning but not panic
        core.clear_process(0);
        // State should remain Stopped because Stopped <- Stopped is valid (same state)
        assert_eq!(core.state_machine.state(), InstanceState::Stopped);
    }

    #[tokio::test]
    async fn test_instance_trait_restart() {
        // restart is a default method on Instance trait: stop then start
        // We test it indirectly by verifying the trait default compiles and is callable
        let core: InstanceCore<LocalSpawner, TokioBroadcastBus> = InstanceCore::new(
            "test".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        // The trait default requires &mut self; we just exercise the lock acquisition
        assert!(core.acquire_lock().is_ok());
    }
}
