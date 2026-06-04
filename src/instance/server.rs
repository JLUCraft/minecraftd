use crate::core::config::{ConfigProvider, MemoryConfig};
use crate::core::event::EventBus;
use crate::core::process::{ProcessSpawner, ProcessSpec, Signal};
use crate::core::state::{InstanceState, StateMachine};
use crate::instance::base::{Instance, InstanceCore, InstanceError};
use crate::instance::path::{InstancePath, InstanceSubdir};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use tracing::{debug, info, warn};

/// Default timeout in seconds for graceful server stop before force kill.
pub const DEFAULT_GRACEFUL_STOP_TIMEOUT_SECS: u64 = 10;

/// Configuration for a Minecraft server instance.
///
/// The server's working directory is derived from its [`InstancePath`],
/// not stored as a raw string here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServerConfig {
    pub nickname: String,
    /// Full command to start the server, e.g. "java `-Xmx2G` -jar server.jar nogui"
    pub start_command: String,
    /// Command sent to stdin for graceful stop, e.g. "stop"
    pub stop_command: String,
    /// Input encoding
    pub input_encoding: String,
    /// Output encoding
    pub output_encoding: String,
    /// Whether to enable auto-restart on crash
    pub auto_restart: bool,
    /// Maximum auto-restart attempts (-1 = unlimited)
    pub auto_restart_max_times: i32,
    /// Server type: "minecraft/java", "minecraft/bedrock", "universal"
    pub server_type: String,
    /// RCON configuration
    pub rcon: Option<RconConfig>,
    /// Java runtime path (overrides PATH lookup)
    pub java_path: String,
    /// JVM isolation: dedicated tmpdir per instance
    pub isolated_tmpdir: bool,
    /// JVM isolation: custom properties file
    pub jvm_properties: Vec<(String, String)>,
    /// Instance metadata (name, tags, icon, etc.)
    #[serde(default)]
    pub metadata: crate::instance::metadata::InstanceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RconConfig {
    pub enabled: bool,
    pub password: String,
    pub port: u16,
    pub host: String,
}

/// A Minecraft server instance.
///
/// Generic over spawner `S` and event bus `E`, enabling compile-time composition
/// of any spawner with any event bus.
#[derive(Debug)]
pub struct ServerInstance<S, E>
where
    S: ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    pub core: InstanceCore<S, E>,
    pub config: MemoryConfig<ServerConfig>,
    pub auto_restart_count: u32,
}

impl<S, E> ServerInstance<S, E>
where
    S: ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    pub const fn with_core(core: InstanceCore<S, E>, config: ServerConfig) -> Self {
        Self {
            core,
            config: MemoryConfig::new(config),
            auto_restart_count: 0,
        }
    }

    pub fn config(&self) -> &ServerConfig {
        self.config.get()
    }

    pub fn config_mut(&mut self) -> &mut ServerConfig {
        self.config.get_mut()
    }

    /// Builds the process specification with JVM isolation.
    ///
    /// The working directory is derived from the instance's [`InstancePath`],
    /// not from a raw string in `ServerConfig`.
    ///
    /// Isolation is achieved through:
    /// - Dedicated working directory (`self.core.path.root`)
    /// - Dedicated java.io.tmpdir (if `isolated_tmpdir`)
    /// - Custom JVM properties
    fn build_process_spec(&self) -> Result<ProcessSpec, InstanceError> {
        let cfg = self.config.get();
        let parts = shlex::split(&cfg.start_command).ok_or_else(|| {
            InstanceError::Config("invalid start_command: shell quoting error".into())
        })?;

        if parts.is_empty() {
            return Err(InstanceError::Config("start_command is empty".into()));
        }

        let command = parts[0].clone();
        let mut args = parts[1..].to_vec();

        // Server daemon always runs headless — suppress the Minecraft GUI.
        args.push("--nogui".to_string());

        // Apply JVM isolation properties
        if cfg.isolated_tmpdir {
            let tmpdir = self.core.path.subdir(InstanceSubdir::Tmp);
            args.insert(0, format!("-Djava.io.tmpdir={}", tmpdir.display()));
        }
        for (key, value) in &cfg.jvm_properties {
            args.insert(0, format!("-D{key}={value}"));
        }

        let mut env = std::collections::HashMap::new();
        if cfg.isolated_tmpdir {
            let tmpdir = self.core.path.subdir(InstanceSubdir::Tmp);
            env.insert("TMPDIR".into(), tmpdir.to_string_lossy().into());
            env.insert("TEMP".into(), tmpdir.to_string_lossy().into());
            env.insert("TMP".into(), tmpdir.to_string_lossy().into());
        }

        Ok(ProcessSpec {
            command,
            args,
            cwd: Some(self.core.path.root.clone()),
            env,
            inherit_env: true,
        })
    }
}

#[async_trait]
impl<S, E> Instance for ServerInstance<S, E>
where
    S: ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    fn id(&self) -> &str {
        &self.core.id
    }

    fn path(&self) -> &InstancePath {
        &self.core.path
    }

    fn state(&self) -> InstanceState {
        self.core.state_machine.state()
    }

    fn is_startable(&self) -> bool {
        self.core.state_machine.is_operable() && self.core.state_machine.is_terminal()
    }

    fn is_stoppable(&self) -> bool {
        self.core.state_machine.is_running()
    }

    async fn start(&mut self) -> Result<(), InstanceError> {
        self.core.acquire_lock()?;

        if !self.is_startable() {
            return Err(InstanceError::NotStartable(self.state()));
        }

        let cfg = self.config.get().clone();
        if cfg.start_command.is_empty() {
            return Err(InstanceError::Config("start_command is empty".into()));
        }

        self.core
            .state_machine
            .transition(InstanceState::Starting)?;

        let id = self.core.id.clone();
        self.core.lifecycle.run_start_hooks(&id).await;

        let spec = self.build_process_spec()?;
        debug!("spawning server with spec: {:?}", spec);

        let handle = self.core.spawn_process(&spec).await?;

        // Wire up output forwarding to event bus and output ring
        let bus_id = self.core.id.clone();
        let bus = self.core.event_bus.clone_boxed();
        let ring = self.core.output_ring.clone();
        handle.on_output(Box::new(move |data| {
            bus.emit(Box::new(crate::core::event::InstanceEvent::Output {
                instance_id: bus_id.clone(),
                data: data.to_vec(),
            }));
            if let Some(r) = &ring {
                r.push_bytes(data);
            }
        }));

        self.core.set_process(handle);

        info!("server instance {} started", self.core.id);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), InstanceError> {
        self.core.acquire_lock()?;

        if !self.is_stoppable() {
            return Err(InstanceError::NotStoppable(self.state()));
        }

        self.core
            .state_machine
            .transition(InstanceState::Stopping)?;

        let cfg = self.config.get().clone();

        // Try graceful stop via stdin
        if !cfg.stop_command.is_empty()
            && cfg.stop_command != "^C"
            && let Some(process) = &self.core.process
        {
            let cmd = format!("{}\n", cfg.stop_command);
            if let Err(e) = process.write(cmd.as_bytes()).await {
                warn!("failed to send stop command: {}", e);
            }
        }

        // Wait for graceful shutdown, then force kill
        tokio::time::sleep(tokio::time::Duration::from_secs(
            DEFAULT_GRACEFUL_STOP_TIMEOUT_SECS,
        ))
        .await;

        if let Some(process) = &self.core.process
            && process.is_running()
        {
            warn!("server did not stop gracefully, force killing");
            process.kill(Signal::Kill).await?;
        }

        let id = self.core.id.clone();
        self.core.lifecycle.run_stop_hooks(&id).await;
        self.core.clear_process(0);

        info!("server instance {} stopped", self.core.id);
        Ok(())
    }

    async fn kill(&mut self) -> Result<(), InstanceError> {
        self.core.acquire_lock()?;

        if !self.is_stoppable() {
            return Err(InstanceError::NotStoppable(self.state()));
        }

        self.core
            .state_machine
            .transition(InstanceState::Stopping)?;

        if let Some(process) = &self.core.process {
            process.kill(Signal::Kill).await?;
        }

        let id = self.core.id.clone();
        self.core.lifecycle.run_stop_hooks(&id).await;
        self.core.clear_process(-1);

        info!("server instance {} killed", self.core.id);
        Ok(())
    }

    async fn send_command(&self, cmd: &str) -> Result<(), InstanceError> {
        if let Some(process) = &self.core.process {
            let line = format!("{cmd}\n");
            process.write(line.as_bytes()).await?;
            Ok(())
        } else {
            Err(InstanceError::Process("not running".into()))
        }
    }

    fn process_handle(&self) -> Option<&dyn crate::core::process::ProcessHandle> {
        self.core.process.as_ref().map(std::convert::AsRef::as_ref)
    }

    fn event_bus(&self) -> &dyn EventBus {
        &self.core.event_bus
    }

    fn exit_watch(&self) -> Option<tokio::sync::watch::Receiver<Option<i32>>> {
        self.core.exit_watch.clone()
    }

    fn clear_process(&mut self, exit_code: i32) {
        self.core.clear_process(exit_code);
    }

    fn play_time_secs(&self) -> u64 {
        self.core.play_time_secs
    }

    fn increment_play_time(&mut self, secs: u64) {
        self.core.play_time_secs += secs;
    }

    fn recent_output(&self, n: usize) -> Vec<String> {
        self.core
            .output_ring
            .as_ref()
            .map(|r| r.recent_lines(n))
            .unwrap_or_default()
    }

    fn setup_output_ring(&mut self, capacity: usize) {
        self.core.output_ring = Some(crate::core::ring::OutputRingBuffer::new(capacity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::TokioBroadcastBus;
    use crate::instance::base::Instance;
    use crate::instance::builder::InstanceBuilder;
    use crate::process::local::LocalSpawner;

    #[test]
    fn test_server_config_default() {
        let cfg = ServerConfig::default();
        assert!(cfg.start_command.is_empty());
        assert!(!cfg.auto_restart);
        assert!(!cfg.isolated_tmpdir);
    }

    #[test]
    fn test_server_instance_build_process_spec() {
        let core = InstanceCore::new(
            "srv".to_string(),
            InstancePath::new("/tmp/mc"),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        let server = ServerInstance::with_core(
            core,
            ServerConfig {
                start_command: "java -jar server.jar nogui".into(),
                isolated_tmpdir: true,
                jvm_properties: vec![("foo".into(), "bar".into())],
                ..Default::default()
            },
        );

        let spec = server.build_process_spec().unwrap();
        assert_eq!(spec.command, "java");
        assert!(spec.args.contains(&"-jar".to_string()));
        assert!(spec.args.iter().any(|a| a.starts_with("-Djava.io.tmpdir=")));
        assert!(spec.args.iter().any(|a| a == "-Dfoo=bar"));
        assert!(spec.env.contains_key("TMPDIR"));
        assert_eq!(spec.cwd, Some(std::path::PathBuf::from("/tmp/mc")));
    }

    #[test]
    fn test_server_instance_build_process_spec_no_isolation() {
        let core = InstanceCore::new(
            "srv".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        let server = ServerInstance::with_core(
            core,
            ServerConfig {
                start_command: "java -jar server.jar".into(),
                ..Default::default()
            },
        );

        let spec = server.build_process_spec().unwrap();
        assert!(!spec.args.iter().any(|a| a.starts_with("-Djava.io.tmpdir=")));
        assert!(spec.env.is_empty());
    }

    #[tokio::test]
    async fn test_server_instance_start_stop() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sh -c 'sleep 0.5'".into(),
                stop_command: "^C".into(),
                ..Default::default()
            });

        assert!(server.is_startable());
        server.start().await.unwrap();
        assert!(server.is_running());

        // kill instead of stop to avoid 10s sleep
        server.kill().await.unwrap();
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn test_server_instance_send_command_not_running() {
        let server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "echo hello".into(),
                ..Default::default()
            });

        let result = server.send_command("list").await;
        assert!(matches!(result, Err(InstanceError::Process(_))));
    }

    #[tokio::test]
    async fn test_server_instance_start_empty_command() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: String::new(),
                ..Default::default()
            });

        let result = server.start().await;
        assert!(matches!(result, Err(InstanceError::Config(_))));
    }

    #[test]
    fn test_server_instance_build_process_spec_invalid_shlex() {
        let core = InstanceCore::new(
            "srv".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        let server = ServerInstance::with_core(
            core,
            ServerConfig {
                start_command: "java '\"unclosed".into(),
                ..Default::default()
            },
        );

        let result = server.build_process_spec();
        assert!(matches!(result, Err(InstanceError::Config(_))));
    }

    #[tokio::test]
    async fn test_server_instance_stop_when_not_running() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "echo".into(),
                ..Default::default()
            });

        let result = server.stop().await;
        assert!(matches!(result, Err(InstanceError::NotStoppable(_))));
    }

    #[tokio::test]
    async fn test_server_instance_kill_when_not_running() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "echo".into(),
                ..Default::default()
            });

        let result = server.kill().await;
        assert!(matches!(result, Err(InstanceError::NotStoppable(_))));
    }

    #[tokio::test]
    async fn test_server_instance_start_when_already_running() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sh -c 'sleep 0.5'".into(),
                ..Default::default()
            });

        server.start().await.unwrap();
        let result = server.start().await;
        assert!(matches!(result, Err(InstanceError::NotStartable(_))));

        server.kill().await.unwrap();
    }

    #[tokio::test]
    async fn test_server_instance_send_command_while_running() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sh -c 'cat'".into(),
                ..Default::default()
            });

        server.start().await.unwrap();
        let result = server.send_command("list").await;
        assert!(result.is_ok());
        server.kill().await.unwrap();
    }

    #[tokio::test]
    async fn test_server_instance_restart() {
        let mut server = InstanceBuilder::new("srv".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "sh -c 'sleep 0.3'".into(),
                ..Default::default()
            });

        server.start().await.unwrap();
        assert!(server.is_running());

        server.stop().await.unwrap();
        assert!(!server.is_running());
    }
}
