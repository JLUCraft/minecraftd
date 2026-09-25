//! Daemon that manages running Minecraft instances and serves CLI requests
//! over a local socket (cross-platform via `interprocess`).

pub mod handlers;
pub mod ipc;

use crate::service::instance::InstanceService;
use interprocess::local_socket::tokio::Stream;
#[cfg(unix)]
use interprocess::local_socket::traits::StreamCommon as _;
use interprocess::local_socket::traits::tokio::{Listener, Stream as _};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name, ToFsName};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub use ipc::{CliRequest, CliResponse, InstanceInfo};

/// Well-known local socket name for daemon IPC.
pub fn socket_name() -> crate::Result<Name<'static>> {
    Ok(socket_path().to_fs_name::<GenericFilePath>()?.into_owned())
}

fn socket_path() -> std::path::PathBuf {
    std::env::temp_dir().join("minecraftd.sock")
}

/// Authenticate both accepted clients and the server before sending privileged requests.
pub fn authorize_peer(stream: &Stream) -> crate::Result<()> {
    #[cfg(unix)]
    {
        require_uid(stream.peer_creds()?.euid(), nix::unistd::geteuid().as_raw())
    }
    #[cfg(not(unix))]
    {
        let _ = stream;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "authenticated daemon IPC requires Unix peer credentials",
        )
        .into())
    }
}

#[cfg(unix)]
fn require_uid(actual: Option<u32>, expected: u32) -> crate::Result<()> {
    if actual != Some(expected) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "daemon peer belongs to another user",
        )
        .into());
    }
    Ok(())
}

fn bind_socket(
    path: &std::path::Path,
) -> crate::Result<interprocess::local_socket::tokio::Listener> {
    let name = path.to_fs_name::<GenericFilePath>()?;

    let options = ListenerOptions::new().name(name.borrow());
    // Set the mode before bind on platforms supporting fchmod on socket FDs.
    #[cfg(target_os = "linux")]
    let options = {
        use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
        options.mode(0o600)
    };
    let listener = options.create_tokio()?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;

    Ok(listener)
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
    pub async fn run(self) -> crate::Result<()> {
        if !cfg!(unix) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "authenticated daemon IPC is not implemented on this platform",
            )
            .into());
        }
        let listener = bind_socket(&socket_path())?;

        info!("daemon listening on {}", sock_path_display());

        let instances = Arc::clone(&self.instances);

        loop {
            match listener.accept().await {
                Ok(stream) => {
                    if let Err(error) = authorize_peer(&stream) {
                        warn!(%error, "rejected daemon connection");
                        continue;
                    }
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
    socket_path().display().to_string()
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn local_socket_restricts_mode_and_checks_both_peers()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon.sock");
        let listener = bind_socket(&path)?;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        let client = Stream::connect(path.to_fs_name::<GenericFilePath>()?).await?;
        let server = listener.accept().await?;
        authorize_peer(&client)?;
        authorize_peer(&server)?;
        let uid = nix::unistd::geteuid().as_raw();
        assert!(require_uid(Some(uid.wrapping_add(1)), uid).is_err());
        assert!(require_uid(None, uid).is_err());
        Ok(())
    }
}
