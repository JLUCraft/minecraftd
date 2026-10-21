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
