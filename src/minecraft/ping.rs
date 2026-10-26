use lite_mc_ping::{PingOptions, ServerAddress};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PingError {
    #[error(transparent)]
    Query(#[from] lite_mc_ping::Error),
    #[error("invalid server address: {0}")]
    Address(String),
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("invalid Bedrock status packet: {0}")]
    BedrockPacket(String),
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

const RAKNET_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];
const UNCONNECTED_PING: u8 = 0x01;
const UNCONNECTED_PONG: u8 = 0x1c;
const RAKNET_PONG_HEADER_LEN: usize = 35;
const MAX_UDP_PACKET_LEN: usize = 65_535;

/// Queries a Bedrock server using a RakNet unconnected ping/pong exchange.
///
/// This synchronous call blocks for at most the configured socket timeouts.
/// Async callers should use a blocking task.
///
/// # Errors
/// Returns a connection error for socket failures or a zero timeout, or a
/// packet error for malformed or unrelated pong data.
pub fn ping_bedrock_server(addr: SocketAddr, to: Duration) -> Result<ServerStatus, PingError> {
    if to.is_zero() {
        return Err(PingError::Connection("timeout must be positive".into()));
    }
    let bind_address = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let query = || -> std::io::Result<(Vec<u8>, [u8; 8], Duration)> {
        let socket = UdpSocket::bind(bind_address)?;
        socket.connect(addr)?;
        socket.set_read_timeout(Some(to))?;
        socket.set_write_timeout(Some(to))?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)?;
        let timestamp = u64::try_from(timestamp.as_millis())
            .map_err(std::io::Error::other)?
            .to_be_bytes();
        let mut request = Vec::with_capacity(RAKNET_PONG_HEADER_LEN - 2);
        request.push(UNCONNECTED_PING);
        request.extend_from_slice(&timestamp);
        request.extend_from_slice(&RAKNET_MAGIC);
        request.extend_from_slice(&uuid::Uuid::now_v7().as_bytes()[..8]);
        let started = Instant::now();
        socket.send(&request)?;
        let mut response = vec![0; MAX_UDP_PACKET_LEN];
        let received = socket.recv(&mut response)?;
        let elapsed = started.elapsed();
        response.truncate(received);
        Ok((response, timestamp, elapsed))
    };
    let (response, timestamp, elapsed) =
        query().map_err(|error| PingError::Connection(error.to_string()))?;
    parse_bedrock_pong(&response, timestamp, elapsed)
}

fn parse_bedrock_pong(
    response: &[u8],
    timestamp: [u8; 8],
    elapsed: Duration,
) -> Result<ServerStatus, PingError> {
    let invalid = |message: &str| PingError::BedrockPacket(message.to_owned());
    if response.len() < RAKNET_PONG_HEADER_LEN {
        return Err(invalid("truncated pong header"));
    }
    if response[0] != UNCONNECTED_PONG {
        return Err(invalid("unexpected packet ID"));
    }
    if response[1..9] != timestamp {
        return Err(invalid("pong timestamp does not match ping"));
    }
    if response[17..33] != RAKNET_MAGIC {
        return Err(invalid("invalid RakNet magic"));
    }
    let text_len = usize::from(u16::from_be_bytes([response[33], response[34]]));
    if response.len() != RAKNET_PONG_HEADER_LEN + text_len {
        return Err(invalid("advertisement length does not match packet"));
    }
    let text = std::str::from_utf8(&response[RAKNET_PONG_HEADER_LEN..])
        .map_err(|_| invalid("advertisement is not UTF-8"))?;
    let fields: Vec<&str> = text.split(';').collect();
    if fields.len() < 6 || fields[0] != "MCPE" {
        return Err(invalid("missing Bedrock advertisement fields"));
    }
    let protocol = fields[2]
        .parse::<i32>()
        .map_err(|_| invalid("invalid protocol number"))?;
    if protocol < 0 || fields[3].is_empty() {
        return Err(invalid("invalid protocol or version"));
    }
    let current_players = fields[4]
        .parse::<u32>()
        .map_err(|_| invalid("invalid online player count"))?;
    let max_players = fields[5]
        .parse::<u32>()
        .map_err(|_| invalid("invalid maximum player count"))?;
    let mut description = fields[1].to_owned();
    if let Some(subtitle) = fields.get(7).filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push('\n');
        }
        description.push_str(subtitle);
    }
    Ok(ServerStatus {
        online: true,
        version: fields[3].to_owned(),
        protocol,
        current_players,
        max_players,
        description,
        favicon: None,
        latency_ms: u64::try_from(elapsed.as_millis()).ok(),
    })
}
