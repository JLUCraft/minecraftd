use quartz_nbt::io::Flavor;
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Debug, thiserror::Error)]
pub enum ServerListError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("NBT error: {0}")]
    Nbt(String),
}
pub type ServerListResult<T> = Result<T, ServerListError>;
pub const SERVERS_DAT_FILENAME: &str = "servers.dat";

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameServerInfo {
    pub icon_src: String,
    pub ip: String,
    pub name: String,
    pub hidden: bool,
    pub description: String,
    pub is_queried: bool, // if true, this is a complete result from a successful query
    pub players_online: usize,
    pub players_max: usize,
    pub online: bool, // if false, it may be offline in the query result or failed in the query.
    pub latency: Option<u64>, // ping latency in milliseconds
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct NbtServerInfo {
    pub ip: String,
    pub icon: Option<String>,
    pub name: String,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct NbtServersInfo {
    pub servers: Vec<NbtServerInfo>,
}

impl From<NbtServerInfo> for GameServerInfo {
    fn from(nbt: NbtServerInfo) -> Self {
        Self {
            ip: nbt.ip,
            name: nbt.name,
            icon_src: nbt.icon.unwrap_or_default(),
            hidden: nbt.hidden,
            ..Default::default()
        }
    }
}

impl From<GameServerInfo> for NbtServerInfo {
    fn from(server: GameServerInfo) -> Self {
        Self {
            ip: server.ip,
            icon: (!server.icon_src.is_empty()).then_some(server.icon_src),
            name: server.name,
            hidden: server.hidden,
        }
    }
}

impl From<&GameServerInfo> for NbtServerInfo {
    fn from(server: &GameServerInfo) -> Self {
        Self {
            ip: server.ip.clone(),
            icon: (!server.icon_src.is_empty()).then_some(server.icon_src.clone()),
            name: server.name.clone(),
            hidden: server.hidden,
        }
    }
}

pub async fn load_servers_info_from_nbt(path: &Path) -> ServerListResult<Vec<GameServerInfo>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = tokio::fs::read(path).await?;
    let (servers_info, _snbt) =
        quartz_nbt::serde::deserialize::<NbtServersInfo>(&bytes, Flavor::Uncompressed)
            .map_err(|error| ServerListError::Nbt(error.to_string()))?;
    let game_server_list = servers_info
        .servers
        .into_iter()
        .map(|nbt| nbt.into())
        .collect();

    Ok(game_server_list)
}

pub async fn save_servers_to_nbt(path: &Path, servers: &[GameServerInfo]) -> ServerListResult<()> {
    let servers_info = NbtServersInfo {
        servers: servers.iter().map(NbtServerInfo::from).collect(),
    };
    let bytes = quartz_nbt::serde::serialize(&servers_info, None, Flavor::Uncompressed)
        .map_err(|error| ServerListError::Nbt(error.to_string()))?;
    tokio::fs::write(path, bytes).await?;

    Ok(())
}

const PING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const PING_CONCURRENCY: usize = 10;
async fn query_one(server: &mut GameServerInfo) {
    let status = match crate::minecraft::ping::ping_java_server(&server.ip, PING_TIMEOUT).await {
        Ok(status) => status,
        Err(error) => {
            tracing::debug!(address = %server.ip, %error, "server query failed");
            crate::minecraft::ping::ServerStatus::default()
        }
    };
    server.is_queried = true;
    server.online = status.online;
    server.latency = status.latency_ms;
    server.players_online = usize::try_from(status.current_players).unwrap_or_default();
    server.players_max = usize::try_from(status.max_players).unwrap_or_default();
    server.description = status.description;
    if let Some(icon) = status.favicon {
        server.icon_src = icon;
    }
}
pub async fn query_servers_online(mut servers: Vec<GameServerInfo>) -> Vec<GameServerInfo> {
    // Scoped batches retain input order, update duplicate addresses, and bound network fan-out.
    for batch in servers.chunks_mut(PING_CONCURRENCY) {
        let mut tasks = tokio::task::JoinSet::new();
        for (index, server) in batch.iter().cloned().enumerate() {
            tasks.spawn(async move {
                let mut server = server;
                query_one(&mut server).await;
                (index, server)
            });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((index, server)) => batch[index] = server,
                Err(error) => tracing::warn!(%error, "server query task failed"),
            }
        }
    }
    servers
}
