use crate::core::event::EventBus;
use crate::core::process::ProcessSpawner;
use crate::instance::base::InstanceCore;
use crate::instance::client::{ClientConfig, ClientInstance};
use crate::instance::path::InstancePath;
use crate::instance::server::{ServerConfig, ServerInstance};
use crate::lifecycle::manager::LifecycleManager;
use std::fmt::Debug;

/// Builder for constructing instances with composable capabilities.
///
/// The type parameters track the spawner and event bus types through the build chain,
/// enabling compile-time verification of the final instance type.
pub struct InstanceBuilder<S = (), E = ()> {
    id: String,
    path: InstancePath,
    spawner: S,
    event_bus: E,
    lifecycle: LifecycleManager,
}

impl InstanceBuilder<(), ()> {
    /// Creates a new instance builder with the given identifier and file-system path.
    #[must_use]
    pub fn new(id: String, path: InstancePath) -> Self {
        Self {
            id,
            path,
            spawner: (),
            event_bus: (),
            lifecycle: LifecycleManager::new(),
        }
    }
}

impl<E> InstanceBuilder<(), E> {
    /// Sets the process spawner.
    pub fn spawner<S: ProcessSpawner>(self, spawner: S) -> InstanceBuilder<S, E> {
        InstanceBuilder {
            id: self.id,
            path: self.path,
            spawner,
            event_bus: self.event_bus,
            lifecycle: self.lifecycle,
        }
    }
}

impl<S> InstanceBuilder<S, ()> {
    /// Sets the event bus.
    pub fn event_bus<E: EventBus>(self, bus: E) -> InstanceBuilder<S, E> {
        InstanceBuilder {
            id: self.id,
            path: self.path,
            spawner: self.spawner,
            event_bus: bus,
            lifecycle: self.lifecycle,
        }
    }
}

impl<S, E> InstanceBuilder<S, E>
where
    S: ProcessSpawner + Debug,
    E: EventBus + Debug,
{
    /// Adds a lifecycle task.
    #[must_use]
    pub fn lifecycle_task(
        mut self,
        task: impl crate::lifecycle::task::LifecycleTask + 'static,
    ) -> Self {
        self.lifecycle.register_task(Box::new(task));
        self
    }

    /// Builds a server instance.
    pub fn build_server(self, config: ServerConfig) -> ServerInstance<S, E> {
        let mut core = InstanceCore::new(self.id, self.path, self.spawner, self.event_bus);
        core.lifecycle = self.lifecycle;
        ServerInstance::with_core(core, config)
    }

    /// Builds a client instance.
    pub fn build_client(self, config: ClientConfig) -> ClientInstance<S, E> {
        let mut core = InstanceCore::new(self.id, self.path, self.spawner, self.event_bus);
        core.lifecycle = self.lifecycle;
        ClientInstance::with_core(core, config)
    }
}

/// Unit types implement the traits as no-ops for the builder's initial state.
/// These are never used in practice since the builder transitions away from () immediately.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::TokioBroadcastBus;
    use crate::instance::base::Instance;
    use crate::instance::client::ClientConfig;
    use crate::instance::server::ServerConfig;
    use crate::lifecycle::task::NoopTask;
    use crate::process::local::LocalSpawner;

    #[test]
    fn test_builder_type_chain() {
        let builder = InstanceBuilder::new("test".to_string(), InstancePath::new("/tmp/test"))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new());
        let _: InstanceBuilder<LocalSpawner, TokioBroadcastBus> = builder;
    }

    #[test]
    fn test_builder_lifecycle_task() {
        let builder = InstanceBuilder::new("srv".to_string(), InstancePath::new("/tmp/srv"))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .lifecycle_task(NoopTask::new("hook".to_string()));
        assert_eq!(builder.lifecycle.task_count(), 1);
    }

    #[test]
    fn test_builder_build_server() {
        let server = InstanceBuilder::new("srv".to_string(), InstancePath::new("/tmp/srv"))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_server(ServerConfig {
                start_command: "echo hello".into(),
                ..Default::default()
            });
        assert_eq!(server.id(), "srv");
        assert_eq!(server.config().start_command, "echo hello");
    }

    #[test]
    fn test_builder_build_client() {
        let client = InstanceBuilder::new("cl".to_string(), InstancePath::new("/tmp/cl"))
            .spawner(LocalSpawner::new())
            .event_bus(TokioBroadcastBus::new())
            .build_client(ClientConfig {
                name: "MyClient".into(),
                ..Default::default()
            });
        assert_eq!(client.id(), "cl");
        assert_eq!(client.config().name, "MyClient");
    }
}
