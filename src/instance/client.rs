use crate::core::config::{ConfigProvider, MemoryConfig};
use crate::core::event::EventBus;
use crate::core::process::{ProcessSpawner, ProcessSpec, Signal};
use crate::core::state::{InstanceState, StateMachine};
use crate::instance::base::{Instance, InstanceCore, InstanceError};
use crate::instance::path::{InstancePath, InstanceSubdir};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;
use tracing::{debug, info};

/// Configuration for a Minecraft client instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ClientConfig {
    pub name: String,
    pub version: String,
    pub version_path: PathBuf,
    pub version_isolation: bool,
    pub window: WindowConfig,
    pub performance: PerformanceConfig,
    pub java: JavaConfig,
    pub mod_loader: ModLoaderConfig,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    pub auto_join_server: Option<String>,
    pub display_game_log: bool,
    pub launcher_visibility: LauncherVisibility,
    /// Instance metadata (name, tags, icon, etc.)
    #[serde(default)]
    pub metadata: crate::instance::metadata::InstanceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub custom_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PerformanceConfig {
    pub auto_memory: bool,
    pub max_memory_mb: u32,
    pub process_priority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JavaConfig {
    pub auto_select: bool,
    pub exec_path: String,
    pub major_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModLoaderConfig {
    pub loader_type: String,
    pub version: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum LauncherVisibility {
    #[default]
    Always,
    StartHidden,
    RunningHidden,
}

/// A Minecraft client instance.
///
/// Generic over spawner `S` and event bus `E`, enabling compile-time composition.
#[derive(Debug)]
pub struct ClientInstance<S, E>
where
    S: ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    pub core: InstanceCore<S, E>,
    pub config: MemoryConfig<ClientConfig>,
    pub play_time_secs: u64,
}

impl<S, E> ClientInstance<S, E>
where
    S: ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    pub const fn with_core(core: InstanceCore<S, E>, config: ClientConfig) -> Self {
        Self {
            core,
            config: MemoryConfig::new(config),
            play_time_secs: 0,
        }
    }

    pub fn config(&self) -> &ClientConfig {
        self.config.get()
    }

    pub fn config_mut(&mut self) -> &mut ClientConfig {
        self.config.get_mut()
    }

    fn build_launch_spec(&self) -> ProcessSpec {
        let cfg = self.config.get();
        let java = if cfg.java.exec_path.is_empty() {
            "java".to_string()
        } else {
            cfg.java.exec_path.clone()
        };

        let mut args = Vec::new();

        // Memory
        let max_mem = if cfg.performance.auto_memory {
            crate::minecraft::launch::DEFAULT_AUTO_MEMORY_MB
        } else {
            cfg.performance.max_memory_mb
        };
        args.push(format!("-Xmx{max_mem}m"));

        // JVM isolation: dedicated tmpdir per instance
        let work_dir = if cfg.version_isolation {
            self.core.path.root.clone()
        } else {
            self.core
                .path
                .root
                .parent()
                .unwrap_or(&self.core.path.root)
                .to_path_buf()
        };
        let tmpdir = self.core.path.subdir(InstanceSubdir::Tmp);
        args.push(format!("-Djava.io.tmpdir={}", tmpdir.display()));

        // JVM args
        args.extend(cfg.jvm_args.clone());

        // Main class
        args.push(crate::minecraft::launch::DEFAULT_MAIN_CLASS.to_string());

        // Game args
        if cfg.window.fullscreen {
            args.push("--fullscreen".to_string());
        }
        if let Some(server) = &cfg.auto_join_server {
            args.push("--server".to_string());
            args.push(server.clone());
        }
        args.extend(cfg.game_args.clone());

        let mut env = std::collections::HashMap::new();
        env.insert("TMPDIR".into(), tmpdir.to_string_lossy().into());
        env.insert("TEMP".into(), tmpdir.to_string_lossy().into());
        env.insert("TMP".into(), tmpdir.to_string_lossy().into());

        ProcessSpec {
            command: java,
            args,
            cwd: Some(work_dir),
            env,
            inherit_env: true,
        }
    }
}

#[async_trait]
impl<S, E> Instance for ClientInstance<S, E>
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

        self.core
            .state_machine
            .transition(InstanceState::Starting)?;

        let id = self.core.id.clone();
        self.core.lifecycle.run_start_hooks(&id).await;

        let spec = self.build_launch_spec();
        debug!("launching client with spec: {:?}", spec);

        let handle = self.core.spawn_process(&spec).await?;

        // Wire up output to ring buffer if configured
        let ring = self.core.output_ring.clone();
        handle.on_output(Box::new(move |data| {
            if let Some(r) = &ring {
                r.push_bytes(data);
            }
        }));

        self.core.set_process(handle);

        info!("client instance {} launched", self.core.id);
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

        if let Some(process) = &self.core.process {
            process.kill(Signal::Term).await?;
        }

        let id = self.core.id.clone();
        self.core.lifecycle.run_stop_hooks(&id).await;
        self.core.clear_process(0);

        info!("client instance {} stopped", self.core.id);
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

        info!("client instance {} killed", self.core.id);
        Ok(())
    }

    async fn send_command(&self, _cmd: &str) -> Result<(), InstanceError> {
        Err(InstanceError::NotImplemented)
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
    fn test_client_config_default() {
        let cfg = ClientConfig::default();
        assert!(cfg.name.is_empty());
        assert!(!cfg.version_isolation);
        assert!(cfg.jvm_args.is_empty());
    }

    #[test]
    fn test_client_instance_build_launch_spec() {
        let core = InstanceCore::new(
            "cl".to_string(),
            InstancePath::new("/game/versions/1.20.4"),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        let client = ClientInstance::with_core(
            core,
            ClientConfig {
                java: JavaConfig {
                    exec_path: "/usr/bin/java".into(),
                    ..Default::default()
                },
                version_path: std::path::PathBuf::from("/game/versions/1.20.4"),
                version_isolation: true,
                performance: PerformanceConfig {
                    auto_memory: false,
                    max_memory_mb: 4096,
                    ..Default::default()
                },
                window: WindowConfig {
                    fullscreen: true,
                    ..Default::default()
                },
                auto_join_server: Some("mc.example.com".into()),
                jvm_args: vec!["-XX:+UseG1GC".into()],
                ..Default::default()
            },
        );

        let spec = client.build_launch_spec();
        assert_eq!(spec.command, "/usr/bin/java");
        assert!(spec.args.contains(&"-Xmx4096m".to_string()));
        assert!(spec.args.contains(&"-XX:+UseG1GC".to_string()));
        assert!(spec.args.contains(&"--fullscreen".to_string()));
        assert!(spec.args.contains(&"--server".to_string()));
        assert!(spec.args.contains(&"mc.example.com".to_string()));
        assert!(spec.env.contains_key("TMPDIR"));
        assert_eq!(
            spec.cwd,
            Some(std::path::PathBuf::from("/game/versions/1.20.4"))
        );
    }

    #[test]
    fn test_client_instance_build_launch_spec_auto_memory() {
        let core = InstanceCore::new(
            "cl".to_string(),
            InstancePath::new("."),
            LocalSpawner::new(),
            TokioBroadcastBus::new(),
        );
        let client = ClientInstance::with_core(
            core,
            ClientConfig {
                performance: PerformanceConfig {
                    auto_memory: true,
                    ..Default::default()
                },
                version_path: std::path::PathBuf::from("/game"),
                ..Default::default()
            },
        );

        let spec = client.build_launch_spec();
        assert!(spec.args.contains(&"-Xmx2048m".to_string()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_client_instance_start_stop() {
        let tmpdir = tempfile::tempdir().unwrap();
        let version_path = tmpdir.path().join("versions").join("1.20.4");
        std::fs::create_dir_all(&version_path).unwrap();

        let mut client = InstanceBuilder::new("cl".to_string(), InstancePath::new(&version_path))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig {
                version_path,
                java: JavaConfig {
                    exec_path: "echo".into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        assert!(client.is_startable());
        client.start().await.unwrap();
        assert!(client.is_running());

        client.stop().await.unwrap();
        assert!(!client.is_running());
    }

    #[tokio::test]
    async fn test_client_instance_send_command() {
        let client = InstanceBuilder::new("cl".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig::default());

        let result = client.send_command("help").await;
        assert!(matches!(result, Err(InstanceError::NotImplemented)));
    }

    #[tokio::test]
    async fn test_client_instance_stop_when_not_running() {
        let mut client = InstanceBuilder::new("cl".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig::default());

        let result = client.stop().await;
        assert!(matches!(result, Err(InstanceError::NotStoppable(_))));
    }

    #[tokio::test]
    async fn test_client_instance_kill_when_not_running() {
        let mut client = InstanceBuilder::new("cl".to_string(), InstancePath::new("."))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig::default());

        let result = client.kill().await;
        assert!(matches!(result, Err(InstanceError::NotStoppable(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_client_instance_start_when_already_running() {
        let tmpdir = tempfile::tempdir().unwrap();
        let version_path = tmpdir.path().join("versions").join("1.20.4");
        std::fs::create_dir_all(&version_path).unwrap();

        let mut client = InstanceBuilder::new("cl".to_string(), InstancePath::new(&version_path))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig {
                version_path,
                java: JavaConfig {
                    exec_path: "sleep".into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        client.start().await.unwrap();
        let result = client.start().await;
        assert!(matches!(result, Err(InstanceError::NotStartable(_))));
        client.kill().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_client_instance_kill_while_running() {
        let tmpdir = tempfile::tempdir().unwrap();
        let version_path = tmpdir.path().join("versions").join("1.20.4");
        std::fs::create_dir_all(&version_path).unwrap();

        let mut client = InstanceBuilder::new("cl".to_string(), InstancePath::new(&version_path))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig {
                version_path,
                java: JavaConfig {
                    exec_path: "sleep".into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        client.start().await.unwrap();
        assert!(client.is_running());
        client.kill().await.unwrap();
        assert!(!client.is_running());
    }
}
