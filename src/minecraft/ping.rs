use lite_mc_ping::{PingOptions, ServerAddress};
use std::net::SocketAddr;
use std::time::Duration;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug)]
pub enum PingError {
    #[error(transparent)]
    Query(#[from] lite_mc_ping::Error),
    #[error("invalid server address: {0}")]
    Address(String),
    #[error("connection failed: {0}")]
    Connection(String),
}

/// Server status and measured ping/pong latency.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerStatus {
    pub online: bool,
    pub version: String,
    pub protocol: i32,
    pub current_players: u32,
    pub max_players: u32,
    pub description: String,
    pub favicon: Option<String>,
    pub latency_ms: Option<u64>,
}

/// Queries a Java server, resolving SRV records for hostnames without an explicit port.
///
/// # Errors
/// Returns a query error for invalid addresses, network failures, or malformed status packets.
pub async fn ping_java_server(address: &str, timeout: Duration) -> Result<ServerStatus, PingError> {
    let address: ServerAddress = address.parse().map_err(PingError::Address)?;
    let options = PingOptions {
        measure_latency: true,
        use_srv: true,
        timeout,
        ..PingOptions::default()
    };
    let result = lite_mc_ping::ping(&address, &options).await?;
    Ok(ServerStatus {
        online: true,
        version: result.status.version.name,
        protocol: result.status.version.protocol,
        current_players: result.status.players.online,
        max_players: result.status.players.max,
        description: result.status.description,
        favicon: result.status.favicon,
        latency_ms: result
            .latency
            .and_then(|latency| u64::try_from(latency.as_millis()).ok()),
    })
}

/// Pings a Minecraft Bedrock server.
///
/// **Not yet implemented**: `RakNet` protocol for Bedrock ping.
///
/// # Errors
///
/// Always returns `PingError::Connection` since the protocol is not implemented.
pub fn ping_bedrock_server(addr: SocketAddr, to: Duration) -> Result<ServerStatus, PingError> {
    debug!("pinging bedrock server at {:?} with timeout {:?}", addr, to);
    Err(PingError::Connection(
        "Minecraft Bedrock server ping protocol (RakNet) is not yet implemented".to_string(),
    ))
}
