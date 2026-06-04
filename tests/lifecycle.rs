use minecraftd::core::event::TokioBroadcastBus;
use minecraftd::core::state::InstanceState;
use minecraftd::instance::base::Instance;
use minecraftd::instance::builder::InstanceBuilder;
use minecraftd::instance::path::InstancePath;
use minecraftd::instance::server::ServerConfig;
use minecraftd::lifecycle::task::LoggingTask;
use minecraftd::process::local::LocalSpawner;

/// Integration test demonstrating the full server lifecycle.
#[tokio::test]
async fn test_server_lifecycle() {
    let mut server = InstanceBuilder::new("test-server".to_string(), InstancePath::new("."))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .lifecycle_task(LoggingTask::new("test-monitor".to_string()))
        .build_server(ServerConfig {
            start_command: "echo 'started' && sleep 0.1".into(),
            stop_command: "stop".into(),
            ..Default::default()
        });

    assert_eq!(server.state(), InstanceState::Stopped);
    assert!(server.is_startable());
    assert!(!server.is_stoppable());

    server.start().await.unwrap();
    assert!(server.is_running() || server.state() == InstanceState::Starting);

    // Wait for process to complete naturally
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
}

/// Test state machine transitions.
#[test]
fn test_state_machine_transitions() {
    use minecraftd::core::state::{AtomicStateMachine, StateMachine};

    let mut sm = AtomicStateMachine::with_stopped();
    assert_eq!(sm.state(), InstanceState::Stopped);

    sm.transition(InstanceState::Starting).unwrap();
    assert_eq!(sm.state(), InstanceState::Starting);

    sm.transition(InstanceState::Running).unwrap();
    assert_eq!(sm.state(), InstanceState::Running);

    sm.transition(InstanceState::Stopping).unwrap();
    assert_eq!(sm.state(), InstanceState::Stopping);

    sm.transition(InstanceState::Stopped).unwrap();
    assert_eq!(sm.state(), InstanceState::Stopped);
}

/// Test that invalid transitions are rejected.
#[test]
fn test_invalid_transitions() {
    use minecraftd::core::state::{AtomicStateMachine, StateError, StateMachine};

    let mut sm = AtomicStateMachine::with_stopped();

    let result = sm.transition(InstanceState::Running);
    assert!(matches!(result, Err(StateError::InvalidTransition { .. })));

    let result = sm.transition(InstanceState::Stopping);
    assert!(matches!(result, Err(StateError::InvalidTransition { .. })));
}

/// Test event bus subscription and emission.
#[test]
fn test_event_bus() {
    use minecraftd::core::event::{EventBus, InstanceEvent, TokioBroadcastBus};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let bus = TokioBroadcastBus::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    bus.subscribe(
        "instance:start",
        Box::new(move |evt| {
            if evt.as_any().downcast_ref::<InstanceEvent>().is_some() {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }
        }),
    );

    bus.emit(Box::new(InstanceEvent::Start {
        instance_id: "test".into(),
    }));

    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Test circular buffer overflow behavior.
#[test]
fn test_circular_buffer_overflow() {
    use minecraftd::util::buffer::CircularBuffer;

    let mut buf = CircularBuffer::new(3);
    buf.push(1);
    buf.push(2);
    buf.push(3);
    assert!(!buf.was_overflowed());

    buf.push(4);
    assert!(buf.was_overflowed());
    assert_eq!(buf.overflow_count(), 1);

    let items: Vec<i32> = buf.drain();
    assert_eq!(items, vec![2, 3, 4]);
}

/// Test path safety validation.
#[test]
fn test_path_safety() {
    use minecraftd::util::path::{normalize_relative_path, validate_path_safety};

    assert!(validate_path_safety("versions/1.20.10").is_ok());
    assert!(validate_path_safety("versions/../etc/passwd").is_err());
    assert!(validate_path_safety("foo\0bar").is_err());

    assert_eq!(
        normalize_relative_path("foo/bar/baz").unwrap(),
        std::path::PathBuf::from("foo/bar/baz")
    );
}

/// Test client instance lifecycle.
#[tokio::test]
async fn test_client_lifecycle() {
    use minecraftd::core::event::TokioBroadcastBus;
    use minecraftd::core::state::InstanceState;
    use minecraftd::instance::base::Instance;
    use minecraftd::instance::builder::InstanceBuilder;
    use minecraftd::instance::client::ClientConfig;
    use minecraftd::process::local::LocalSpawner;

    let tmpdir = tempfile::tempdir().unwrap();
    let instance_dir = tmpdir.path().join("instance");
    std::fs::create_dir_all(&instance_dir).unwrap();
    let version_path = tmpdir.path().join("versions").join("1.20.10");
    std::fs::create_dir_all(&version_path).unwrap();

    let mut client =
        InstanceBuilder::new("test-client".to_string(), InstancePath::new(&instance_dir))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig {
                name: "Test".into(),
                version_path,
                java: minecraftd::instance::client::JavaConfig {
                    exec_path: "echo".into(),
                    ..Default::default()
                },
                ..Default::default()
            });

    assert_eq!(client.state(), InstanceState::Stopped);
    assert!(client.is_startable());
    assert!(!client.is_stoppable());

    client.start().await.unwrap();
    assert!(client.is_running() || client.state() == InstanceState::Starting);

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

/// Test event bus wildcard subscription.
#[test]
fn test_event_bus_wildcard() {
    use minecraftd::core::event::{EventBus, InstanceEvent, TokioBroadcastBus};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let bus = TokioBroadcastBus::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();

    bus.subscribe_all(Box::new(move |_evt| {
        c.fetch_add(1, Ordering::SeqCst);
    }));

    bus.emit(Box::new(InstanceEvent::Start {
        instance_id: "a".into(),
    }));
    bus.emit(Box::new(InstanceEvent::Stop {
        instance_id: "a".into(),
        exit_code: 0,
    }));

    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

/// Test FileConfig async persist roundtrip.
#[tokio::test]
async fn test_file_config_roundtrip() {
    use minecraftd::config::file::FileConfig;
    use minecraftd::core::config::ConfigProvider;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
    struct TestCfg {
        name: String,
        value: u32,
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("roundtrip.json");

    let mut cfg = FileConfig::with_config(
        path.clone(),
        TestCfg {
            name: "orig".into(),
            value: 42,
        },
    )
    .await
    .unwrap();
    cfg.get_mut().name = "updated".into();
    cfg.persist().await.unwrap();

    let loaded = FileConfig::<TestCfg>::new(path).await.unwrap();
    assert_eq!(loaded.get().name, "updated");
    assert_eq!(loaded.get().value, 42);
}
