use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;

/// A health check that can be run periodically against an instance.
#[async_trait]
pub trait HealthCheck: Send + Sync + Debug {
    /// Returns true if the instance is healthy.
    async fn check(&self) -> bool;
    /// Human-readable name of this check.
    fn name(&self) -> &'static str;
}

/// Checks if the process is still running via `ProcessHandle::is_running()`.
pub struct ProcessAliveCheck {
    checker: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl std::fmt::Debug for ProcessAliveCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessAliveCheck").finish_non_exhaustive()
    }
}

impl ProcessAliveCheck {
    pub fn new<F>(is_running: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            checker: Arc::new(is_running),
        }
    }
}

#[async_trait]
impl HealthCheck for ProcessAliveCheck {
    async fn check(&self) -> bool {
        (self.checker)()
    }

    fn name(&self) -> &'static str {
        "process_alive"
    }
}

/// A composite health check that fails if any sub-check fails.
#[derive(Debug)]
pub struct CompositeHealthCheck {
    checks: Vec<Box<dyn HealthCheck>>,
}

impl CompositeHealthCheck {
    #[must_use]
    pub fn new(checks: Vec<Box<dyn HealthCheck>>) -> Self {
        Self { checks }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self { checks: Vec::new() }
    }

    pub fn add(&mut self, check: Box<dyn HealthCheck>) {
        self.checks.push(check);
    }
}

#[async_trait]
impl HealthCheck for CompositeHealthCheck {
    async fn check(&self) -> bool {
        for check in &self.checks {
            if !check.check().await {
                return false;
            }
        }
        true
    }

    fn name(&self) -> &'static str {
        "composite"
    }
}

/// A health check that always passes (useful as a placeholder).
#[derive(Debug, Clone, Default)]
pub struct AlwaysHealthyCheck;

#[async_trait]
impl HealthCheck for AlwaysHealthyCheck {
    async fn check(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "always_healthy"
    }
}

/// A health check that always fails (useful for testing).
#[derive(Debug, Clone, Default)]
pub struct AlwaysUnhealthyCheck;

#[async_trait]
impl HealthCheck for AlwaysUnhealthyCheck {
    async fn check(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "always_unhealthy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_alive_check_true() {
        let check = ProcessAliveCheck::new(|| true);
        assert!(check.check().await);
        assert_eq!(check.name(), "process_alive");
    }

    #[tokio::test]
    async fn test_process_alive_check_false() {
        let check = ProcessAliveCheck::new(|| false);
        assert!(!check.check().await);
    }

    #[tokio::test]
    async fn test_composite_all_pass() {
        let composite = CompositeHealthCheck::new(vec![
            Box::new(AlwaysHealthyCheck),
            Box::new(AlwaysHealthyCheck),
        ]);
        assert!(composite.check().await);
    }

    #[tokio::test]
    async fn test_composite_one_fails() {
        let composite = CompositeHealthCheck::new(vec![
            Box::new(AlwaysHealthyCheck),
            Box::new(AlwaysUnhealthyCheck),
        ]);
        assert!(!composite.check().await);
    }

    #[tokio::test]
    async fn test_composite_empty_passes() {
        let composite = CompositeHealthCheck::empty();
        assert!(composite.check().await);
    }

    #[tokio::test]
    async fn test_always_healthy() {
        let check = AlwaysHealthyCheck;
        assert!(check.check().await);
    }

    #[tokio::test]
    async fn test_always_unhealthy() {
        let check = AlwaysUnhealthyCheck;
        assert!(!check.check().await);
    }
}
