use minecraftd::minecraft::{
    ping::ping_java_server,
    servers::{GameServerInfo, query_servers_online},
};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

async fn read_frame(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut size = 0_usize;
    for shift in (0..35).step_by(7) {
        let byte = stream.read_u8().await?;
        size |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            if size > 1_048_576 {
                return Err(io::Error::other("test packet exceeds frame limit"));
            }
            let mut frame = vec![0; size];
            stream.read_exact(&mut frame).await?;
            return Ok(frame);
        }
    }
    Err(io::Error::other("invalid test packet length"))
}

async fn write_frame(stream: &mut TcpStream, frame: &[u8]) -> io::Result<()> {
    let mut size = frame.len();
    let mut prefix = Vec::new();
    loop {
        let mut byte = (size & 0x7f) as u8;
        size >>= 7;
        if size != 0 {
            byte |= 0x80;
        }
        prefix.push(byte);
        if size == 0 {
            break;
        }
    }
    stream.write_all(&prefix).await?;
    stream.write_all(frame).await
}

async fn serve_status(
    answer_pong: bool,
    clients: usize,
) -> io::Result<(String, JoinHandle<io::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let handle = tokio::spawn(async move {
        for _ in 0..clients {
            let (mut stream, _) = listener.accept().await?;
            let handshake = read_frame(&mut stream).await?;
            assert_eq!(handshake[0], 0);
            assert_eq!(read_frame(&mut stream).await?, vec![0]);
            let json = br#"{"version":{"name":"fixture","protocol":47},"players":{"max":20,"online":3},"description":{"text":"Hello"},"favicon":"data:image/png;base64,fixture"}"#;
            let mut status = vec![0];
            let mut size = json.len();
            loop {
                let mut byte = (size & 0x7f) as u8;
                size >>= 7;
                if size != 0 {
                    byte |= 0x80;
                }
                status.push(byte);
                if size == 0 {
                    break;
                }
            }
            status.extend_from_slice(json);
            write_frame(&mut stream, &status).await?;
            let ping = read_frame(&mut stream).await?;
            assert_eq!(ping[0], 1);
            if answer_pong {
                write_frame(&mut stream, &ping).await?;
            }
        }
        Ok(())
    });
    Ok((address, handle))
}

#[tokio::test]
async fn status_survives_a_failed_latency_measurement() -> Result<(), Box<dyn std::error::Error>> {
    let (address, task) = serve_status(false, 1).await?;
    let result = ping_java_server(&address, Duration::from_secs(2)).await?;
    assert!(result.online);
    assert_eq!(result.description, "Hello");
    assert_eq!(result.current_players, 3);
    assert!(result.latency_ms.is_none());
    task.await??;
    Ok(())
}

#[tokio::test]
async fn query_retains_duplicate_order_and_clears_failed_status()
-> Result<(), Box<dyn std::error::Error>> {
    let (address, task) = serve_status(true, 2).await?;
    let entries = vec![
        GameServerInfo {
            ip: address.clone(),
            name: "first".into(),
            ..Default::default()
        },
        GameServerInfo {
            ip: ":invalid".into(),
            name: "failed".into(),
            online: true,
            latency: Some(99),
            players_online: 9,
            players_max: 99,
            ..Default::default()
        },
        GameServerInfo {
            ip: address,
            name: "duplicate".into(),
            ..Default::default()
        },
    ];
    let result = query_servers_online(entries).await;
    assert_eq!(
        result
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "failed", "duplicate"]
    );
    assert!(result.iter().all(|entry| entry.is_queried));
    for index in [0, 2] {
        assert!(result[index].online);
        assert!(result[index].latency.is_some());
        assert_eq!(result[index].description, "Hello");
        assert_eq!(result[index].players_online, 3);
        assert_eq!(result[index].players_max, 20);
        assert_eq!(result[index].icon_src, "data:image/png;base64,fixture");
    }
    assert!(!result[1].online);
    assert_eq!(result[1].latency, None);
    assert_eq!(result[1].players_online, 0);
    assert_eq!(result[1].players_max, 0);
    tokio::time::timeout(Duration::from_secs(2), task).await???;
    Ok(())
}
// Build the fixture by hand instead of through the parser: packet ID, echoed ping time,
// server GUID, offline magic, unsigned big-endian string length, UTF-8 advertisement.
fn bedrock_pong(request: &[u8], advertisement: &[u8]) -> Vec<u8> {
    let mut pong = vec![0x1c];
    pong.extend_from_slice(&request[1..9]);
    pong.extend_from_slice(&42_u64.to_be_bytes());
    pong.extend_from_slice(&[
        0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56,
        0x78,
    ]);
    pong.extend_from_slice(&(advertisement.len() as u16).to_be_bytes());
    pong.extend_from_slice(advertisement);
    pong
}

type BedrockQueryResult =
    Result<minecraftd::minecraft::ping::ServerStatus, minecraftd::minecraft::ping::PingError>;

fn query_bedrock_fixture(
    reply: impl FnOnce(&[u8]) -> Vec<u8> + Send + 'static,
) -> Result<BedrockQueryResult, Box<dyn std::error::Error>> {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let address = socket.local_addr()?;
    let task = std::thread::spawn(move || -> io::Result<()> {
        let mut request = [0; 64];
        let (size, client) = socket.recv_from(&mut request)?;
        assert_eq!(size, 33);
        assert_eq!(request[0], 0x01);
        assert_eq!(
            &request[9..25],
            &[
                0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
                0x56, 0x78,
            ]
        );
        socket.send_to(&reply(&request[..size]), client)?;
        Ok(())
    });
    let result = minecraftd::minecraft::ping::ping_bedrock_server(address, Duration::from_secs(2));
    task.join()
        .map_err(|_| io::Error::other("Bedrock fixture thread failed"))??;
    Ok(result)
}

#[test]
fn bedrock_status_uses_raknet_exchange() -> Result<(), Box<dyn std::error::Error>> {
    let status = query_bedrock_fixture(|request| {
        bedrock_pong(
            request,
            "MCPE;Hello 世界;776;1.21.60;3;20;42;Second line;Survival;1;19132;19133;".as_bytes(),
        )
    })??;
    assert!(status.online);
    assert_eq!(status.version, "1.21.60");
    assert_eq!(status.protocol, 776);
    assert_eq!(status.current_players, 3);
    assert_eq!(status.max_players, 20);
    assert_eq!(status.description, "Hello 世界\nSecond line");
    assert!(status.latency_ms.is_some());
    assert!(status.favicon.is_none());
    let minimal = query_bedrock_fixture(|request| {
        bedrock_pong(request, b"MCPE;Only title;776;1.21.60;0;20")
    })??;
    assert_eq!(minimal.description, "Only title");
    Ok(())
}

#[test]
fn bedrock_rejects_malformed_packets() -> Result<(), Box<dyn std::error::Error>> {
    use minecraftd::minecraft::ping::PingError;
    for corruption in 0..7 {
        let result = query_bedrock_fixture(move |request| {
            let mut pong = bedrock_pong(request, b"MCPE;Hello;776;1.21.60;3;20");
            match corruption {
                0 => pong.truncate(34),
                1 => pong[0] = 0,
                2 => pong[1] ^= 1,
                3 => pong[17] ^= 1,
                4 => pong[34] += 1,
                5 => pong.push(0),
                6 => pong[35] = 0xff,
                _ => unreachable!(),
            }
            pong
        })?;
        assert!(
            matches!(result, Err(PingError::BedrockPacket(_))),
            "corruption {corruption}: {result:?}"
        );
    }
    for advertisement in [
        "MCPE;Hello",
        "JAVA;Hello;776;1.21.60;3;20",
        "MCPE;Hello;bad;1.21.60;3;20",
        "MCPE;Hello;-1;1.21.60;3;20",
        "MCPE;Hello;776;;3;20",
        "MCPE;Hello;776;1.21.60;-1;20",
        "MCPE;Hello;776;1.21.60;4294967296;20",
        "MCPE;Hello;776;1.21.60;3;bad",
    ] {
        let result =
            query_bedrock_fixture(move |request| bedrock_pong(request, advertisement.as_bytes()))?;
        assert!(
            matches!(result, Err(PingError::BedrockPacket(_))),
            "advertisement {advertisement}: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn bedrock_timeout_and_zero_timeout_are_errors() -> Result<(), Box<dyn std::error::Error>> {
    use minecraftd::minecraft::ping::{PingError, ping_bedrock_server};
    // Keep a bound socket alive without responding, avoiding a port-unreachable
    // response that would exercise a different network failure.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0")?;
    let address = silent.local_addr()?;
    assert!(matches!(
        ping_bedrock_server(address, Duration::from_millis(30)),
        Err(PingError::Connection(_))
    ));
    assert!(matches!(
        ping_bedrock_server(address, Duration::ZERO),
        Err(PingError::Connection(_))
    ));
    Ok(())
}
