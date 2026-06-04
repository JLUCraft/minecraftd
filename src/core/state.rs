use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// The operational state of a Minecraft instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InstanceState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Busy,
    Error(InstanceError),
}

/// Errors that can occur during state transitions.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    #[error("invalid state transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: InstanceState,
        to: InstanceState,
    },
    #[error("instance is locked by another operation")]
    Locked,
    #[error("instance is not operable in state {0:?}")]
    NotOperable(InstanceState),
}

/// A lightweight error type embedded in `InstanceState::Error`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstanceError {
    pub code: String,
    pub message: String,
}

impl InstanceError {
    /// Creates a new `InstanceError`.
    #[must_use]
    pub const fn new(code: String, message: String) -> Self {
        Self { code, message }
    }
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// Trait for state-machine driven instances.
pub trait StateMachine: Send + Sync {
    fn state(&self) -> InstanceState;

    /// Transitions the state machine to a new state.
    ///
    /// # Errors
    ///
    /// Returns `StateError::InvalidTransition` if the transition is not allowed.
    fn transition(&mut self, to: InstanceState) -> Result<(), StateError> {
        let from = self.state();
        if !Self::is_valid_transition(from.clone(), to.clone()) {
            return Err(StateError::InvalidTransition { from, to });
        }
        self.set_state(to);
        Ok(())
    }

    fn is_operable(&self) -> bool {
        matches!(
            self.state(),
            InstanceState::Stopped | InstanceState::Running | InstanceState::Error(_)
        )
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self.state(),
            InstanceState::Stopped | InstanceState::Error(_)
        )
    }

    fn is_running(&self) -> bool {
        self.state() == InstanceState::Running
    }

    fn set_state(&mut self, state: InstanceState);

    #[must_use]
    fn is_valid_transition(from: InstanceState, to: InstanceState) -> bool {
        use InstanceState::{Busy, Error, Running, Starting, Stopped, Stopping};
        match (from, to) {
            (Stopped | Error(_), Starting)
            | (Stopped | Starting | Stopping | Error(_), Stopped)
            | (Starting, Running | Error(_))
            | (Running, Stopping | Error(_))
            | (Stopping, Error(_))
            | (Busy, _) => true,
            (a, b) if a == b => true,
            _ => false,
        }
    }
}

/// A simple, thread-safe state machine implementation.
#[derive(Debug)]
pub struct AtomicStateMachine {
    state: parking_lot::Mutex<InstanceState>,
}

impl AtomicStateMachine {
    /// Creates a new `AtomicStateMachine` with the given initial state.
    #[must_use]
    pub const fn new(initial: InstanceState) -> Self {
        Self {
            state: parking_lot::Mutex::new(initial),
        }
    }

    /// Creates a new `AtomicStateMachine` in the `Stopped` state.
    #[must_use]
    pub const fn with_stopped() -> Self {
        Self::new(InstanceState::Stopped)
    }
}

impl StateMachine for AtomicStateMachine {
    fn state(&self) -> InstanceState {
        self.state.lock().clone()
    }

    fn set_state(&mut self, state: InstanceState) {
        *self.state.lock() = state;
    }
}

impl Default for AtomicStateMachine {
    fn default() -> Self {
        Self::with_stopped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_transitions() {
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Stopped,
            InstanceState::Starting
        ));
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Starting,
            InstanceState::Running
        ));
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Running,
            InstanceState::Stopping
        ));
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Stopping,
            InstanceState::Stopped
        ));
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Starting,
            InstanceState::Error(InstanceError::new("E".to_string(), "msg".to_string()))
        ));
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Error(InstanceError::new("E".to_string(), "msg".to_string())),
            InstanceState::Stopped
        ));
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Busy,
            InstanceState::Running
        ));
        // Same state is always valid
        assert!(AtomicStateMachine::is_valid_transition(
            InstanceState::Stopped,
            InstanceState::Stopped
        ));
    }

    #[test]
    fn test_invalid_transitions() {
        assert!(!AtomicStateMachine::is_valid_transition(
            InstanceState::Stopped,
            InstanceState::Running
        ));
        assert!(!AtomicStateMachine::is_valid_transition(
            InstanceState::Stopped,
            InstanceState::Stopping
        ));
        assert!(!AtomicStateMachine::is_valid_transition(
            InstanceState::Running,
            InstanceState::Starting
        ));
        assert!(!AtomicStateMachine::is_valid_transition(
            InstanceState::Starting,
            InstanceState::Stopping
        ));
    }

    #[test]
    fn test_atomic_state_machine() {
        let mut sm = AtomicStateMachine::with_stopped();
        assert_eq!(sm.state(), InstanceState::Stopped);
        assert!(sm.is_operable());
        assert!(sm.is_terminal());
        assert!(!sm.is_running());

        sm.transition(InstanceState::Starting).unwrap();
        assert!(!sm.is_operable());
        assert!(!sm.is_terminal());
        assert!(!sm.is_running());

        sm.transition(InstanceState::Running).unwrap();
        assert!(sm.is_operable());
        assert!(!sm.is_terminal());
        assert!(sm.is_running());

        sm.transition(InstanceState::Stopping).unwrap();
        assert!(!sm.is_operable());
        assert!(!sm.is_terminal());
        assert!(!sm.is_running());

        sm.transition(InstanceState::Stopped).unwrap();
        assert!(sm.is_operable());
        assert!(sm.is_terminal());
        assert!(!sm.is_running());
    }

    #[test]
    fn test_instance_error_display() {
        let err = InstanceError::new("TEST".to_string(), "something went wrong".to_string());
        assert_eq!(format!("{err}"), "[TEST] something went wrong");
    }

    #[test]
    fn test_state_error_display() {
        let err = StateError::InvalidTransition {
            from: InstanceState::Stopped,
            to: InstanceState::Running,
        };
        let s = format!("{err}");
        assert!(s.contains("invalid state transition"));
    }

    #[test]
    fn test_error_state_recovery() {
        let mut sm = AtomicStateMachine::with_stopped();
        sm.transition(InstanceState::Starting).unwrap();
        sm.transition(InstanceState::Error(InstanceError::new(
            "FAIL".to_string(),
            "oops".to_string(),
        )))
        .unwrap();
        assert!(sm.is_terminal());
        // Error(_) is considered operable per the current implementation
        assert!(sm.is_operable());

        // Can recover to Stopped
        sm.transition(InstanceState::Stopped).unwrap();
        assert!(sm.is_operable());

        // Or recover to Starting
        sm.transition(InstanceState::Starting).unwrap();
        sm.transition(InstanceState::Error(InstanceError::new(
            "FAIL".to_string(),
            "oops".to_string(),
        )))
        .unwrap();
        sm.transition(InstanceState::Starting).unwrap();
    }
}
