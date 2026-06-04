use crate::core::state::InstanceState;
use async_trait::async_trait;
use std::fmt::Debug;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    #[error("task {name} failed: {message}")]
    Failed { name: String, message: String },
    #[error("task {0} timed out")]
    Timeout(String),
    #[error("task {0} was cancelled")]
    Cancelled(String),
}

#[async_trait]
pub trait LifecycleTask: Send + Sync + Debug {
    fn name(&self) -> &str;
    async fn on_start(&self, instance_id: &str) -> Result<(), TaskError>;
    async fn on_stop(&self, instance_id: &str) -> Result<(), TaskError>;
    async fn on_state_change(
        &self,
        _instance_id: &str,
        _from: InstanceState,
        _to: InstanceState,
    ) -> Result<(), TaskError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NoopTask {
    name: String,
}

impl NoopTask {
    #[must_use]
    pub const fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait]
impl LifecycleTask for NoopTask {
    fn name(&self) -> &str {
        &self.name
    }
    async fn on_start(&self, _instance_id: &str) -> Result<(), TaskError> {
        Ok(())
    }
    async fn on_stop(&self, _instance_id: &str) -> Result<(), TaskError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LoggingTask {
    name: String,
}

impl LoggingTask {
    #[must_use]
    pub const fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait]
impl LifecycleTask for LoggingTask {
    fn name(&self) -> &str {
        &self.name
    }
    async fn on_start(&self, instance_id: &str) -> Result<(), TaskError> {
        tracing::info!("[{}] instance {} starting", self.name, instance_id);
        Ok(())
    }
    async fn on_stop(&self, instance_id: &str) -> Result<(), TaskError> {
        tracing::info!("[{}] instance {} stopping", self.name, instance_id);
        Ok(())
    }
    async fn on_state_change(
        &self,
        instance_id: &str,
        from: InstanceState,
        to: InstanceState,
    ) -> Result<(), TaskError> {
        tracing::info!(
            "[{}] instance {} state changed: {:?} -> {:?}",
            self.name,
            instance_id,
            from,
            to
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_error_display() {
        let e = TaskError::Failed {
            name: "test".into(),
            message: "boom".into(),
        };
        assert!(format!("{e}").contains("boom"));

        let e2 = TaskError::Timeout("t".into());
        assert!(format!("{e2}").contains("timed out"));

        let e3 = TaskError::Cancelled("c".into());
        assert!(format!("{e3}").contains("cancelled"));
    }

    #[tokio::test]
    async fn test_noop_task() {
        let task = NoopTask::new("noop".to_string());
        assert_eq!(task.name(), "noop");
        assert!(task.on_start("i").await.is_ok());
        assert!(task.on_stop("i").await.is_ok());
        assert!(
            task.on_state_change("i", InstanceState::Stopped, InstanceState::Starting)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_logging_task() {
        let task = LoggingTask::new("logger".to_string());
        assert_eq!(task.name(), "logger");
        assert!(task.on_start("i").await.is_ok());
        assert!(task.on_stop("i").await.is_ok());
        assert!(
            task.on_state_change("i", InstanceState::Stopped, InstanceState::Starting)
                .await
                .is_ok()
        );
    }
}
