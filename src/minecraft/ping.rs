use std::net::SocketAddr;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, trace};

/// Errors that can occur during server ping operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum PingError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("timeout")]
    Timeout,
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for PingError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Server status response from a Minecraft Java server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerStatus {
    pub online: bool,
    pub version: String,
    pub protocol: i32,
    pub current_players: i32,
    pub max_players: i32,
    pub description: String,
    pub favicon: Option<String>,
    pub latency_ms: u64,
}

/// Pings a Minecraft Java server and returns its status.
///
/// Implements the Minecraft Server List Ping protocol:
/// 1. TCP connect
/// 2. Send handshake packet (0x00) with state=1 (status)
/// 3. Send status request packet (0x00)
/// 4. Read `VarInt` length + JSON response
/// 5. Measure latency
///
/// # Errors
///
/// Returns `PingError` if the connection fails, times out, or the response is invalid.
pub async fn ping_java_server(addr: SocketAddr, to: Duration) -> Result<ServerStatus, PingError> {
    debug!("pinging java server at {:?} with timeout {:?}", addr, to);

    let start = std::time::Instant::now();
    let mut stream = timeout(to, TcpStream::connect(addr))
        .await
        .map_err(|_| PingError::Timeout)?
        .map_err(|e| PingError::Connection(e.to_string()))?;

    // Send handshake + status request
    let handshake = build_handshake_packet(addr);
    timeout(to, stream.write_all(&handshake))
        .await
        .map_err(|_| PingError::Timeout)??;

    let status_request = vec![0x01, 0x00]; // length=1, packet_id=0x00
    timeout(to, stream.write_all(&status_request))
        .await
        .map_err(|_| PingError::Timeout)??;

    // Read and parse response
    let json_str = read_ping_response(&mut stream, to).await?;
    let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(0);

    trace!("ping response: {}", json_str);
    parse_status_json(&json_str, latency_ms)
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

// --- Handshake packet building ---

/// Builds the framed Minecraft handshake packet (protocol version -1, next state = status).
fn build_handshake_packet(addr: SocketAddr) -> Vec<u8> {
    let mut handshake = Vec::new();
    write_varint(&mut handshake, 0x00); // packet ID
    write_varint(&mut handshake, -1); // protocol version (unknown)
    write_varint_prefixed_bytes(&mut handshake, addr.ip().to_string().as_bytes());
    handshake.extend_from_slice(&addr.port().to_be_bytes());
    write_varint(&mut handshake, 1); // next state = status

    let mut frame = Vec::new();
    write_varint(&mut frame, i32::try_from(handshake.len()).unwrap_or(0));
    frame.extend_from_slice(&handshake);
    frame
}

/// Reads the status response from the stream: length → `packet_id` → `json_len` → json.
async fn read_ping_response(stream: &mut TcpStream, to: Duration) -> Result<String, PingError> {
    let len = timeout(to, read_varint(stream))
        .await
        .map_err(|_| PingError::Timeout)??;

    if !(0..=0x0001_0000).contains(&len) {
        return Err(PingError::InvalidResponse(format!(
            "invalid response length: {len}"
        )));
    }

    let packet_id = timeout(to, read_varint(stream))
        .await
        .map_err(|_| PingError::Timeout)??;
    if packet_id != 0x00 {
        return Err(PingError::InvalidResponse(format!(
            "unexpected packet id: {packet_id}"
        )));
    }

    let json_len = timeout(to, read_varint(stream))
        .await
        .map_err(|_| PingError::Timeout)??;
    if !(0..=0x0001_0000).contains(&json_len) {
        return Err(PingError::InvalidResponse(format!(
            "invalid json length: {json_len}"
        )));
    }

    let json_len_usize = usize::try_from(json_len).unwrap_or(0);
    let mut json_buf = vec![0_u8; json_len_usize];
    timeout(to, stream.read_exact(&mut json_buf))
        .await
        .map_err(|_| PingError::Timeout)??;

    Ok(String::from_utf8_lossy(&json_buf).to_string())
}

// --- VarInt encoding/decoding ---

fn write_varint(buf: &mut Vec<u8>, value: i32) {
    let mut value = value as u32;
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn write_varint_prefixed_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    write_varint(buf, i32::try_from(bytes.len()).unwrap_or(0));
    buf.extend_from_slice(bytes);
}

async fn read_varint(stream: &mut TcpStream) -> Result<i32, PingError> {
    let mut result = 0_i32;
    let mut shift = 0;
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await?;
        let val = i32::from(byte[0] & 0x7F);
        result |= val << shift;
        if (byte[0] & 0x80) == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 32 {
            return Err(PingError::InvalidResponse("VarInt too long".into()));
        }
    }
}

// --- JSON parsing ---

fn parse_status_json(json: &str, latency_ms: u64) -> Result<ServerStatus, PingError> {
    let parsed: serde_json::Value =
        serde_json::from_str(json).map_err(|e| PingError::InvalidResponse(e.to_string()))?;

    let version = parsed
        .get("version")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let protocol = i32::try_from(
        parsed
            .get("version")
            .and_then(|v| v.get("protocol"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    )
    .unwrap_or(0);

    let players = parsed.get("players");
    let current_players = i32::try_from(
        players
            .and_then(|p| p.get("online"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    )
    .unwrap_or(0);
    let max_players = i32::try_from(
        players
            .and_then(|p| p.get("max"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    )
    .unwrap_or(0);

    let description = parsed
        .get("description")
        .map(|d| {
            d.as_str().map_or_else(
                || {
                    d.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map_or_else(|| d.to_string(), std::string::ToString::to_string)
                },
                std::string::ToString::to_string,
            )
        })
        .unwrap_or_default();

    let favicon = parsed
        .get("favicon")
        .and_then(|f| f.as_str())
        .map(std::string::ToString::to_string);

    Ok(ServerStatus {
        online: true,
        version,
        protocol,
        current_players,
        max_players,
        description,
        favicon,
        latency_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::rcon::RconClient;

    #[test]
    fn test_parse_status_json() {
        let json = r#"{
            "version": {"name": "1.20.4", "protocol": 765},
            "players": {"max": 20, "online": 5},
            "description": "A Minecraft Server"
        }"#;
        let status = parse_status_json(json, 42).unwrap();
        assert!(status.online);
        assert_eq!(status.version, "1.20.4");
        assert_eq!(status.protocol, 765);
        assert_eq!(status.current_players, 5);
        assert_eq!(status.max_players, 20);
        assert_eq!(status.description, "A Minecraft Server");
        assert_eq!(status.latency_ms, 42);
    }

    #[test]
    fn test_parse_status_json_chat_description() {
        let json = r#"{
            "version": {"name": "1.20.4", "protocol": 765},
            "players": {"max": 20, "online": 0},
            "description": {"text": "Hello World"}
        }"#;
        let status = parse_status_json(json, 10).unwrap();
        assert_eq!(status.description, "Hello World");
    }

    #[test]
    fn test_parse_status_json_missing_fields() {
        let json = r"{}";
        let status = parse_status_json(json, 0).unwrap();
        assert_eq!(status.version, "Unknown");
        assert_eq!(status.protocol, 0);
        assert_eq!(status.current_players, 0);
        assert_eq!(status.max_players, 0);
    }

    #[test]
    fn test_varint_roundtrip() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 255);
        assert_eq!(buf, vec![0xFF, 0x01]);

        let mut buf2 = Vec::new();
        write_varint(&mut buf2, -1);
        assert_eq!(buf2, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[tokio::test]
    async fn test_ping_java_server_no_server() {
        // Connect to a port that should have no server
        let addr = "127.0.0.1:1".parse().unwrap();
        let result = ping_java_server(addr, Duration::from_millis(100)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rcon_client_execute_no_server() {
        let client = RconClient::new("127.0.0.1".to_string(), 1, "password".to_string());
        let result = client
            .execute("list", std::time::Duration::from_millis(100))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ping_bedrock_server_not_implemented() {
        let addr = "127.0.0.1:19132".parse().unwrap();
        let result = ping_bedrock_server(addr, Duration::from_secs(5));
        assert!(matches!(result, Err(PingError::Connection(_))));
    }

    #[tokio::test]
    async fn test_ping_java_server_default() {
        let status = ServerStatus::default();
        assert!(!status.online);
        assert_eq!(status.protocol, 0);
    }

    #[test]
    fn test_ping_error_display() {
        let e1 = PingError::Connection("refused".into());
        assert!(format!("{e1}").contains("refused"));

        let e2 = PingError::InvalidResponse("bad json".into());
        assert!(format!("{e2}").contains("bad json"));

        let e3 = PingError::Timeout;
        assert!(format!("{e3}").contains("timeout"));

        let e4 = PingError::Io("broken pipe".into());
        assert!(format!("{e4}").contains("broken pipe"));
    }
}
