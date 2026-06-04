//! Request handlers for the daemon.

use crate::core::event::TokioBroadcastBus;
use crate::daemon::ipc::{CliRequest, CliResponse};
use crate::instance::builder::InstanceBuilder;
use crate::instance::path::InstancePath;
use crate::instance::server::ServerConfig;
use crate::process::local::LocalSpawner;
use crate::service::instance::InstanceService;
use crate::service::restart::RestartPolicy;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::InstanceEntry;

pub(crate) type InstanceRegistry = Arc<RwLock<HashMap<String, InstanceEntry>>>;

/// Dispatch a single request from the CLI.
pub async fn handle_request(req: &CliRequest, instances: &InstanceRegistry) -> CliResponse {
    match req {
        CliRequest::Run {
            config_path,
            restart,
        } => handle_run(config_path, *restart, instances).await,
        CliRequest::Running => {
            let guard = instances.read().await;
            let ids: Vec<String> = guard.keys().cloned().collect();
            drop(guard);
            CliResponse::RunningIds { ids }
        }
        CliRequest::Stop { id } => {
            with_instance(instances, id, |svc| async move {
                svc.stop().await.map_err(|e| e.to_string())
            })
            .await
        }
        CliRequest::Start { id } => {
            with_instance(instances, id, |svc| async move {
                svc.start().await.map_err(|e| e.to_string())
            })
            .await
        }
        CliRequest::Restart { id } => {
            with_instance(instances, id, |svc| async move {
                svc.restart().await.map_err(|e| e.to_string())
            })
            .await
        }
        CliRequest::Kill { id } => {
            with_instance(instances, id, |svc| async move {
                svc.kill().await.map_err(|e| e.to_string())
            })
            .await
        }
        CliRequest::Logs { id, tail } => {
            with_instance_map(instances, id, |svc| async move {
                svc.recent_output(*tail)
                    .await
                    .map_err(|e| e.to_string())
                    .map(|lines| CliResponse::Logs { lines })
            })
            .await
        }
        CliRequest::Exec { id, command } => {
            with_instance(instances, id, |svc| async move {
                svc.send_command(command.clone())
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
        }
        CliRequest::Shutdown => {
            warn!("shutdown requested");
            CliResponse::Ok {
                message: Some("shutting down".into()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Instance helpers
// ---------------------------------------------------------------------------

async fn with_instance<F, Fut>(instances: &InstanceRegistry, id: &str, f: F) -> CliResponse
where
    F: FnOnce(Arc<InstanceService>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let guard = instances.read().await;
    let svc = guard.get(id).map(|e| Arc::clone(&e.service));
    drop(guard);

    match svc {
        Some(svc) => match f(svc).await {
            Ok(()) => CliResponse::Ok {
                message: Some(format!("command executed on {id}")),
            },
            Err(e) => CliResponse::Err { error: e },
        },
        None => CliResponse::Err {
            error: format!("instance not found: {id}"),
        },
    }
}

async fn with_instance_map<F, Fut>(instances: &InstanceRegistry, id: &str, f: F) -> CliResponse
where
    F: FnOnce(Arc<InstanceService>) -> Fut,
    Fut: std::future::Future<Output = Result<CliResponse, String>>,
{
    let guard = instances.read().await;
    let svc = guard.get(id).map(|e| Arc::clone(&e.service));
    drop(guard);

    match svc {
        Some(svc) => match f(svc).await {
            Ok(resp) => resp,
            Err(e) => CliResponse::Err { error: e },
        },
        None => CliResponse::Err {
            error: format!("instance not found: {id}"),
        },
    }
}

// ---------------------------------------------------------------------------
// Run handler
// ---------------------------------------------------------------------------

async fn handle_run(
    config_path: &Path,
    restart: bool,
    instances: &InstanceRegistry,
) -> CliResponse {
    let config_str = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => {
            return CliResponse::Err {
                error: format!("failed to read config {}: {e}", config_path.display()),
            };
        }
    };

    let server_config: ServerConfig = match toml::from_str(&config_str) {
        Ok(c) => c,
        Err(e) => {
            return CliResponse::Err {
                error: format!("failed to parse config: {e}"),
            };
        }
    };

    let id = if server_config.metadata.name.is_empty() {
        "unnamed".to_string()
    } else {
        server_config.metadata.name.clone()
    };

    // Derive the instance path from the config file location.
    // The config file lives at {instance_root}/server.toml (or similar),
    // so the parent directory IS the instance directory.
    let instance_path = config_path
        .parent()
        .map(|p| InstancePath::new(p.to_path_buf()))
        .unwrap_or_else(|| InstancePath::new("."));

    let restart_policy = if restart {
        RestartPolicy::auto(-1)
    } else {
        RestartPolicy::never()
    };

    let server = InstanceBuilder::new(id.clone(), instance_path)
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(server_config);

    let service = Arc::new(InstanceService::new(server, restart_policy));

    let mut guard = instances.write().await;
    if guard.contains_key(&id) {
        return CliResponse::Err {
            error: format!("instance {id} is already running"),
        };
    }
    guard.insert(
        id.clone(),
        InstanceEntry {
            service: Arc::clone(&service),
        },
    );
    drop(guard);

    if let Err(e) = service.start().await {
        return CliResponse::Err {
            error: format!("instance {id} created but failed to start: {e}"),
        };
    }

    info!("instance {id} started from {}", config_path.display());
    CliResponse::Ok {
        message: Some(format!("instance {id} started")),
    }
}
