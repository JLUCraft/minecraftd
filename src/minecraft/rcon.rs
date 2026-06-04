use std::io::Cursor;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, trace};

/// Errors from RCON operations.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RconError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("authentication failed")]
    AuthFailed,
    #[error("io error: {0}")]
    Io(String),
    #[error("timeout")]
    Timeout,
    #[error("invalid response")]
    InvalidResponse,
}

impl From<std::io::Error> for RconError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// RCON packet types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum RconPacketType {
    /// Response from server.
    Response = 0,
    /// Command packet from client.
    Command = 2,
    /// Login/authentication packet.
    Login = 3,
}

/// An RCON packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RconPacket {
    pub request_id: i32,
    pub packet_type: i32,
    pub payload: String,
}

impl RconPacket {
    /// Serialize the packet to bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let payload_bytes = self.payload.as_bytes();
        let len = 4 + 4 + payload_bytes.len() + 2; // request_id + type + payload + 2 null bytes
        let mut buf = Vec::with_capacity(4 + len);

        // Length (does not include the length field itself)
        buf.extend_from_slice(&(len as i32).to_le_bytes());
        // Request ID
        buf.extend_from_slice(&self.request_id.to_le_bytes());
        // Type
        buf.extend_from_slice(&self.packet_type.to_le_bytes());
        // Payload
        buf.extend_from_slice(payload_bytes);
        // Two null bytes
        buf.push(0);
        buf.push(0);

        buf
    }

    /// Deserialize a packet from a byte slice.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RconError> {
        if bytes.len() < 10 {
            return Err(RconError::InvalidResponse);
        }

        let mut cursor = Cursor::new(bytes);
        let mut request_id_bytes = [0_u8; 4];
        std::io::Read::read_exact(&mut cursor, &mut request_id_bytes)
            .map_err(|_| RconError::InvalidResponse)?;
        let request_id = i32::from_le_bytes(request_id_bytes);

        let mut type_bytes = [0_u8; 4];
        std::io::Read::read_exact(&mut cursor, &mut type_bytes)
            .map_err(|_| RconError::InvalidResponse)?;
        let packet_type = i32::from_le_bytes(type_bytes);

        // Remaining bytes are payload + 2 null terminators
        let payload_len = bytes.len() - cursor.position() as usize;
        if payload_len < 2 {
            return Err(RconError::InvalidResponse);
        }

        let mut payload_bytes = vec![0_u8; payload_len - 2];
        std::io::Read::read_exact(&mut cursor, &mut payload_bytes)
            .map_err(|_| RconError::InvalidResponse)?;
        let payload = String::from_utf8_lossy(&payload_bytes).to_string();

        Ok(Self {
            request_id,
            packet_type,
            payload,
        })
    }
}

/// RCON client for remote console access to Minecraft servers.
#[derive(Debug, Clone)]
pub struct RconClient {
    host: String,
    port: u16,
    password: String,
}

impl RconClient {
    #[must_use]
    pub const fn new(host: String, port: u16, password: String) -> Self {
        Self {
            host,
            port,
            password,
        }
    }

    /// Connects and authenticates to the RCON server.
    pub async fn connect(&self, to: Duration) -> Result<RconConnection, RconError> {
        debug!("connecting to RCON at {}:{}", self.host, self.port);

        let addr = format!("{}:{}", self.host, self.port);
        let stream = timeout(to, TcpStream::connect(&addr))
            .await
            .map_err(|_| RconError::Timeout)?
            .map_err(|e| RconError::Connection(e.to_string()))?;

        let mut conn = RconConnection {
            stream,
            request_counter: 1,
        };

        // Send authentication
        let auth_packet = RconPacket {
            request_id: 1,
            packet_type: RconPacketType::Login as i32,
            payload: self.password.clone(),
        };

        conn.send_packet(&auth_packet).await?;
        let response = conn.read_packet(to).await?;

        if response.request_id == -1 {
            return Err(RconError::AuthFailed);
        }

        if response.request_id != auth_packet.request_id {
            return Err(RconError::InvalidResponse);
        }

        debug!("RCON authentication successful");
        Ok(conn)
    }

    /// Sends a single command and returns the response.
    pub async fn execute(&self, command: &str, to: Duration) -> Result<String, RconError> {
        let mut conn = self.connect(to).await?;
        let result = conn.send_command(command, to).await;
        let _ = conn.close().await;
        result
    }
}

/// An active RCON connection.
pub struct RconConnection {
    stream: TcpStream,
    request_counter: i32,
}

impl std::fmt::Debug for RconConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RconConnection")
            .field("request_counter", &self.request_counter)
            .finish_non_exhaustive()
    }
}

impl RconConnection {
    /// Sends a command and returns the response.
    pub async fn send_command(&mut self, command: &str, to: Duration) -> Result<String, RconError> {
        let request_id = self.request_counter;
        self.request_counter += 1;

        let packet = RconPacket {
            request_id,
            packet_type: RconPacketType::Command as i32,
            payload: command.to_string(),
        };

        trace!("sending RCON command: {}", command);
        self.send_packet(&packet).await?;

        // Read response
        let response = self.read_packet(to).await?;

        if response.request_id != request_id {
            return Err(RconError::InvalidResponse);
        }

        Ok(response.payload)
    }

    /// Closes the connection.
    pub async fn close(mut self) -> Result<(), RconError> {
        debug!("closing RCON connection");
        let _ = self.stream.shutdown().await;
        Ok(())
    }

    async fn send_packet(&mut self, packet: &RconPacket) -> Result<(), RconError> {
        let bytes = packet.to_bytes();
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_packet(&mut self, to: Duration) -> Result<RconPacket, RconError> {
        // Read length (4 bytes)
        let mut len_bytes = [0_u8; 4];
        timeout(to, self.stream.read_exact(&mut len_bytes))
            .await
            .map_err(|_| RconError::Timeout)??;
        let len = i32::from_le_bytes(len_bytes) as usize;

        if len > 4096 {
            return Err(RconError::InvalidResponse);
        }

        // Read body
        let mut body = vec![0_u8; len];
        timeout(to, self.stream.read_exact(&mut body))
            .await
            .map_err(|_| RconError::Timeout)??;

        RconPacket::from_bytes(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rcon_packet_roundtrip() {
        let packet = RconPacket {
            request_id: 42,
            packet_type: RconPacketType::Command as i32,
            payload: "say hello".to_string(),
        };

        let bytes = packet.to_bytes();
        let decoded = RconPacket::from_bytes(&bytes[4..]).unwrap(); // skip length prefix

        assert_eq!(decoded.request_id, 42);
        assert_eq!(decoded.packet_type, 2);
        assert_eq!(decoded.payload, "say hello");
    }

    #[test]
    fn test_rcon_packet_empty_payload() {
        let packet = RconPacket {
            request_id: 1,
            packet_type: RconPacketType::Response as i32,
            payload: String::new(),
        };

        let bytes = packet.to_bytes();
        let decoded = RconPacket::from_bytes(&bytes[4..]).unwrap();
        assert_eq!(decoded.payload, "");
    }

    #[test]
    fn test_rcon_packet_from_bytes_too_short() {
        let result = RconPacket::from_bytes(&[0, 0, 0]);
        assert!(matches!(result, Err(RconError::InvalidResponse)));
    }

    #[tokio::test]
    async fn test_rcon_client_connect_no_server() {
        let client = RconClient::new("127.0.0.1".to_string(), 1, "password".to_string());
        let result = client.connect(Duration::from_millis(100)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rcon_client_execute_no_server() {
        let client = RconClient::new("127.0.0.1".to_string(), 1, "password".to_string());
        let result = client.execute("list", Duration::from_millis(100)).await;
        assert!(result.is_err());
    }
}
