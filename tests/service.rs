use minecraftd::core::event::TokioBroadcastBus;
use minecraftd::core::state::InstanceState;
use minecraftd::instance::builder::InstanceBuilder;
use minecraftd::instance::path::InstancePath;
use minecraftd::instance::server::ServerConfig;
use minecraftd::process::local::LocalSpawner;
use minecraftd::service::instance::InstanceService;
use minecraftd::service::restart::RestartPolicy;

/// Test that a service can start and stop a server instance.
#[tokio::test]
async fn test_service_start_stop_server() {
    let server = InstanceBuilder::new("integration-service".to_string(), InstancePath::new("."))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(ServerConfig {
            start_command: "sh -c 'sleep 10'".into(),
            stop_command: "^C".into(),
            ..Default::default()
        });
    let service = InstanceService::new(server, RestartPolicy::never());

    assert_eq!(service.state(), InstanceState::Stopped);

    service.start().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    assert_eq!(service.state(), InstanceState::Running);

    service.kill().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    assert_eq!(service.state(), InstanceState::Stopped);

    service.shutdown().await.unwrap();
}

/// Test state watch subscription.
#[tokio::test]
async fn test_service_state_watch() {
    let server = InstanceBuilder::new("integration-watch".to_string(), InstancePath::new("."))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(ServerConfig {
            start_command: "sh -c 'sleep 10'".into(),
            ..Default::default()
        });
    let service = InstanceService::new(server, RestartPolicy::never());

    let mut watch = service.state_watch();
    assert_eq!(*watch.borrow(), InstanceState::Stopped);

    service.start().await.unwrap();
    watch.changed().await.unwrap();
    assert_eq!(*watch.borrow(), InstanceState::Running);

    service.kill().await.unwrap();
    watch.changed().await.unwrap();
    assert_eq!(*watch.borrow(), InstanceState::Stopped);

    service.shutdown().await.unwrap();
}

/// Test auto-restart with a crashing process.
#[tokio::test]
async fn test_service_auto_restart_crash() {
    let server = InstanceBuilder::new("integration-restart".to_string(), InstancePath::new("."))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(ServerConfig {
            start_command: "sh -c 'sleep 0.05'".into(), // exits quickly
            ..Default::default()
        });
    let restart_policy = RestartPolicy {
        enabled: true,
        max_retries: 1,
        backoff_base_ms: 50,
        backoff_max_ms: 200,
        reset_window_secs: 60,
        exit_code_allowlist: vec![],
    };
    let service = InstanceService::new(server, restart_policy);

    service.start().await.unwrap();

    // Wait for: start -> exit -> backoff -> restart -> exit -> max retries
    tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;
    assert_eq!(service.state(), InstanceState::Stopped);

    service.shutdown().await.unwrap();
}

/// Test that operations fail gracefully after shutdown.
#[tokio::test]
async fn test_service_operations_after_shutdown() {
    let server = InstanceBuilder::new("integration-shutdown".to_string(), InstancePath::new("."))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(ServerConfig {
            start_command: "sh -c 'sleep 10'".into(),
            stop_command: "^C".into(),
            ..Default::default()
        });
    let service = InstanceService::new(server, RestartPolicy::never());

    service.shutdown().await.unwrap();

    // Give supervisor time to actually terminate
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // All control operations should fail after supervisor has shut down
    assert!(service.start().await.is_err());
    assert!(service.stop().await.is_err());
    assert!(service.kill().await.is_err());
    assert!(service.restart().await.is_err());
    assert!(service.send_command("list".into()).await.is_err());
}

/// Test restart command.
#[tokio::test]
async fn test_service_restart_command() {
    let server = InstanceBuilder::new(
        "integration-restart-cmd".to_string(),
        InstancePath::new("."),
    )
    .spawner(LocalSpawner::new())
    .event_bus(TokioBroadcastBus::new())
    .build_server(ServerConfig {
        start_command: "sh -c 'sleep 10'".into(),
        ..Default::default()
    });
    let service = InstanceService::new(server, RestartPolicy::never());

    service.start().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    assert_eq!(service.state(), InstanceState::Running);

    service.restart().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
    assert_eq!(service.state(), InstanceState::Running);

    service.shutdown().await.unwrap();
}
