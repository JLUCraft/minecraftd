use std::time::{Duration, Instant};

/// Policy for auto-restarting an instance when it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Whether auto-restart is enabled.
    pub enabled: bool,
    /// Maximum number of restart attempts. -1 = unlimited.
    pub max_retries: i32,
    /// Base backoff duration in milliseconds.
    pub backoff_base_ms: u64,
    /// Maximum backoff duration in milliseconds.
    pub backoff_max_ms: u64,
    /// Time window in seconds after which the retry count resets.
    pub reset_window_secs: u64,
    /// If non-empty, only restart when exit code is in this list.
    /// Empty = restart on any exit code.
    pub exit_code_allowlist: Vec<i32>,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 30000,
            reset_window_secs: 60,
            exit_code_allowlist: Vec::new(),
        }
    }
}

impl RestartPolicy {
    /// Create a policy with auto-restart enabled.
    #[must_use]
    pub fn auto(max_retries: i32) -> Self {
        Self {
            enabled: true,
            max_retries,
            ..Default::default()
        }
    }

    /// Create a policy that never restarts.
    #[must_use]
    pub fn never() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Create a policy that always restarts (unlimited retries).
    #[must_use]
    pub fn always() -> Self {
        Self {
            enabled: true,
            max_retries: -1,
            ..Default::default()
        }
    }
}

/// Tracks restart state for a single instance.
#[derive(Debug)]
pub struct RestartState {
    policy: RestartPolicy,
    attempt_count: u32,
    last_attempt: Option<Instant>,
}

impl RestartState {
    #[must_use]
    pub const fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            attempt_count: 0,
            last_attempt: None,
        }
    }

    /// Returns true if the instance should be restarted after the given exit code.
    pub fn should_restart(&mut self, exit_code: i32) -> bool {
        if !self.policy.enabled {
            return false;
        }

        // Check exit code allowlist
        if !self.policy.exit_code_allowlist.is_empty()
            && !self.policy.exit_code_allowlist.contains(&exit_code)
        {
            return false;
        }

        // Reset attempt count if enough time has passed since last attempt
        if let Some(last) = self.last_attempt {
            let elapsed = last.elapsed().as_secs();
            if elapsed >= self.policy.reset_window_secs {
                self.attempt_count = 0;
            }
        }

        // Check max retries
        if self.policy.max_retries >= 0 && self.attempt_count >= self.policy.max_retries as u32 {
            return false;
        }

        true
    }

    /// Record a restart attempt and return the backoff duration to wait before restarting.
    pub fn record_attempt(&mut self) -> Duration {
        self.attempt_count += 1;
        self.last_attempt = Some(Instant::now());

        let base = self.policy.backoff_base_ms;
        let max = self.policy.backoff_max_ms;
        let attempt = u64::from(self.attempt_count);

        // Exponential backoff: base * 2^(attempt-1), capped at max
        let backoff_ms = (base * (1_u64 << (attempt - 1).min(31))).min(max);
        Duration::from_millis(backoff_ms)
    }

    /// Reset the state (e.g. after a successful manual start).
    pub const fn reset(&mut self) {
        self.attempt_count = 0;
        self.last_attempt = None;
    }

    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    #[must_use]
    pub const fn policy(&self) -> &RestartPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restart_policy_never() {
        let policy = RestartPolicy::never();
        let mut state = RestartState::new(policy);
        assert!(!state.should_restart(1));
    }

    #[test]
    fn test_restart_policy_always() {
        let policy = RestartPolicy::always();
        let mut state = RestartState::new(policy);
        assert!(state.should_restart(0));
        assert!(state.should_restart(1));
        assert!(state.should_restart(-1));
    }

    #[test]
    fn test_restart_max_retries() {
        let policy = RestartPolicy::auto(2);
        let mut state = RestartState::new(policy);

        assert!(state.should_restart(1));
        state.record_attempt();

        assert!(state.should_restart(1));
        state.record_attempt();

        // Max retries reached
        assert!(!state.should_restart(1));
    }

    #[test]
    fn test_restart_backoff_increases() {
        let policy = RestartPolicy {
            enabled: true,
            max_retries: -1,
            backoff_base_ms: 100,
            backoff_max_ms: 10000,
            reset_window_secs: 60,
            exit_code_allowlist: Vec::new(),
        };
        let mut state = RestartState::new(policy);

        let b1 = state.record_attempt();
        assert_eq!(b1, Duration::from_millis(100));

        let b2 = state.record_attempt();
        assert_eq!(b2, Duration::from_millis(200));

        let b3 = state.record_attempt();
        assert_eq!(b3, Duration::from_millis(400));
    }

    #[test]
    fn test_restart_backoff_capped() {
        let policy = RestartPolicy {
            enabled: true,
            max_retries: -1,
            backoff_base_ms: 1000,
            backoff_max_ms: 4000,
            reset_window_secs: 60,
            exit_code_allowlist: Vec::new(),
        };
        let mut state = RestartState::new(policy);

        let b1 = state.record_attempt();
        assert_eq!(b1, Duration::from_secs(1));

        let b2 = state.record_attempt();
        assert_eq!(b2, Duration::from_secs(2));

        let b3 = state.record_attempt();
        assert_eq!(b3, Duration::from_secs(4));

        let b4 = state.record_attempt();
        assert_eq!(b4, Duration::from_secs(4)); // capped
    }

    #[test]
    fn test_exit_code_allowlist() {
        let policy = RestartPolicy {
            enabled: true,
            max_retries: -1,
            backoff_base_ms: 100,
            backoff_max_ms: 1000,
            reset_window_secs: 60,
            exit_code_allowlist: vec![1, 2],
        };
        let mut state = RestartState::new(policy);

        assert!(state.should_restart(1));
        assert!(state.should_restart(2));
        assert!(!state.should_restart(0));
        assert!(!state.should_restart(3));
    }

    #[test]
    fn test_reset_window() {
        let policy = RestartPolicy {
            enabled: true,
            max_retries: 1,
            backoff_base_ms: 10,
            backoff_max_ms: 100,
            reset_window_secs: 0, // immediate reset for testing
            exit_code_allowlist: Vec::new(),
        };
        let mut state = RestartState::new(policy);

        assert!(state.should_restart(1));
        state.record_attempt();

        // Should reset immediately because reset_window_secs = 0
        assert!(state.should_restart(1));
    }

    #[test]
    fn test_reset_clears_attempts() {
        let policy = RestartPolicy::auto(1);
        let mut state = RestartState::new(policy);

        state.record_attempt();
        assert!(!state.should_restart(1));

        state.reset();
        assert!(state.should_restart(1));
    }
}
