//! Daemon that manages running Minecraft instances and serves CLI requests
//! over a local socket (cross-platform via `interprocess`).

pub mod handlers;
pub mod ipc;

use crate::service::instance::InstanceService;
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::traits::tokio::{Listener, Stream as _};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name, ToFsName};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tracing::{error, info};

pub use ipc::{CliRequest, CliResponse, InstanceInfo};

/// Well-known local socket name for daemon IPC.
pub fn socket_name() -> Name<'static> {
    let sock_path = std::env::temp_dir().join("minecraftd.sock");
    sock_path
        .display()
        .to_string()
        .to_fs_name::<GenericFilePath>()
        .expect("hardcoded socket name")
        .into_owned()
}

/// Entry stored in the instance registry.
#[derive(Debug)]
pub struct InstanceEntry {
    pub service: Arc<InstanceService>,
}

pub(crate) type InstanceRegistry = Arc<RwLock<HashMap<String, InstanceEntry>>>;

/// Daemon that manages running Minecraft server instances.
pub struct Daemon {
    instances: InstanceRegistry,
}

impl Daemon {
    #[must_use]
    pub fn new() -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start the daemon: bind local socket, accept connections, serve forever.
    pub async fn run(self) {
        let name = socket_name();

        let listener = match ListenerOptions::new().name(name.borrow()).create_tokio() {
            Ok(l) => l,
            Err(e) => {
                error!("failed to bind {}: {:?}", sock_path_display(), e);
                return;
            }
        };

        info!("daemon listening on {}", sock_path_display());

        let instances = Arc::clone(&self.instances);

        loop {
            match listener.accept().await {
                Ok(stream) => {
                    info!("accepted connection");
                    let registry = Arc::clone(&instances);
                    tokio::spawn(async move {
                        handle_connection(stream, registry).await;
                    });
                }
                Err(e) => {
                    error!("accept error: {}", e);
                }
            }
        }
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

fn sock_path_display() -> String {
    std::env::temp_dir()
        .join("minecraftd.sock")
        .display()
        .to_string()
}

async fn handle_connection(stream: Stream, instances: InstanceRegistry) {
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<CliRequest>(&line) {
            Ok(req) => handlers::handle_request(&req, &instances).await,
            Err(e) => CliResponse::Err {
                error: format!("invalid request: {e}"),
            },
        };

        let mut json = serde_json::to_string(&response)
            .unwrap_or_else(|e| format!(r#"{{"err":{{"error":"{e}"}}}}"#));
        json.push('\n');

        if writer.write_all(json.as_bytes()).await.is_err() {
            break;
        }
    }
}
