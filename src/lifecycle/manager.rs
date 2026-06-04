use crate::core::state::InstanceState;
use crate::lifecycle::task::LifecycleTask;
use std::fmt::Debug;
use tracing::{debug, warn};

#[derive(Debug)]
pub struct LifecycleManager {
    tasks: Vec<Box<dyn LifecycleTask>>,
}

impl LifecycleManager {
    #[must_use]
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    pub fn register_task(&mut self, task: Box<dyn LifecycleTask>) {
        debug!("registered lifecycle task: {}", task.name());
        self.tasks.push(task);
    }

    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub async fn run_start_hooks(&self, instance_id: &str) {
        for task in &self.tasks {
            match task.on_start(instance_id).await {
                Ok(()) => debug!("task {} start hook succeeded", task.name()),
                Err(e) => warn!("task {} start hook failed: {}", task.name(), e),
            }
        }
    }

    pub async fn run_stop_hooks(&self, instance_id: &str) {
        for task in &self.tasks {
            match task.on_stop(instance_id).await {
                Ok(()) => debug!("task {} stop hook succeeded", task.name()),
                Err(e) => warn!("task {} stop hook failed: {}", task.name(), e),
            }
        }
    }

    pub async fn run_state_change_hooks(
        &self,
        instance_id: &str,
        from: InstanceState,
        to: InstanceState,
    ) {
        for task in &self.tasks {
            match task
                .on_state_change(instance_id, from.clone(), to.clone())
                .await
            {
                Ok(()) => {}
                Err(e) => warn!("task {} state change hook failed: {}", task.name(), e),
            }
        }
    }

    pub fn clear(&mut self) {
        self.tasks.clear();
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::InstanceState;
    use crate::lifecycle::task::{LifecycleTask, LoggingTask, NoopTask, TaskError};
    use async_trait::async_trait;

    #[derive(Debug, Clone)]
    struct FailingTask {
        name: String,
    }

    impl FailingTask {
        const fn new(name: String) -> Self {
            Self { name }
        }
    }

    #[async_trait]
    impl LifecycleTask for FailingTask {
        fn name(&self) -> &str {
            &self.name
        }
        async fn on_start(&self, _instance_id: &str) -> Result<(), TaskError> {
            Err(TaskError::Failed {
                name: self.name.clone(),
                message: "start failed".into(),
            })
        }
        async fn on_stop(&self, _instance_id: &str) -> Result<(), TaskError> {
            Err(TaskError::Failed {
                name: self.name.clone(),
                message: "stop failed".into(),
            })
        }
    }

    #[tokio::test]
    async fn test_lifecycle_manager() {
        let mut manager = LifecycleManager::new();
        manager.register_task(Box::new(NoopTask::new("task1".to_string())));
        manager.register_task(Box::new(NoopTask::new("task2".to_string())));
        assert_eq!(manager.task_count(), 2);
        manager.run_start_hooks("test-instance").await;
        manager.run_stop_hooks("test-instance").await;
    }

    #[tokio::test]
    async fn test_lifecycle_manager_failing_hooks() {
        let mut manager = LifecycleManager::new();
        manager.register_task(Box::new(FailingTask::new("fail-start".to_string())));
        manager.register_task(Box::new(NoopTask::new("ok".to_string())));

        // Should not panic even when a hook fails; remaining hooks must still run
        manager.run_start_hooks("test-instance").await;
        manager.run_stop_hooks("test-instance").await;
        assert_eq!(manager.task_count(), 2);
    }

    #[tokio::test]
    async fn test_lifecycle_manager_state_change_hooks() {
        let mut manager = LifecycleManager::new();
        manager.register_task(Box::new(LoggingTask::new("logger".to_string())));
        manager
            .run_state_change_hooks("test", InstanceState::Stopped, InstanceState::Starting)
            .await;
    }

    #[tokio::test]
    async fn test_lifecycle_manager_clear() {
        let mut manager = LifecycleManager::new();
        manager.register_task(Box::new(NoopTask::new("t".to_string())));
        assert_eq!(manager.task_count(), 1);
        manager.clear();
        assert_eq!(manager.task_count(), 0);
    }
}
