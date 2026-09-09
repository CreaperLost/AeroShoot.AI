use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Idle = 0,
    Preparing = 1,
    Recording = 2,
    Paused = 3,
    Stopping = 4,
    Completed = 5,
    Error = 6,
}

impl From<u8> for SessionState {
    fn from(val: u8) -> Self {
        match val {
            0 => SessionState::Idle,
            1 => SessionState::Preparing,
            2 => SessionState::Recording,
            3 => SessionState::Paused,
            4 => SessionState::Stopping,
            5 => SessionState::Completed,
            _ => SessionState::Error,
        }
    }
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum StateTransitionError {
    #[error("Invalid state transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: SessionState,
        to: SessionState,
    },
}

/// Thread-safe recording session state machine enforcing valid transitions:
/// Idle -> Preparing -> Recording <-> Paused -> Stopping -> Completed -> Idle
#[derive(Debug, Clone)]
pub struct SessionStateMachine {
    state: Arc<AtomicU8>,
}

impl Default for SessionStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStateMachine {
    pub fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(SessionState::Idle as u8)),
        }
    }

    pub fn current(&self) -> SessionState {
        self.state.load(Ordering::SeqCst).into()
    }

    pub fn transition_to(&self, next: SessionState) -> Result<(), StateTransitionError> {
        loop {
            let current_raw = self.state.load(Ordering::SeqCst);
            let current: SessionState = current_raw.into();

            // Validate transition rule
            let valid = match (current, next) {
                (SessionState::Idle, SessionState::Preparing) => true,
                (SessionState::Preparing, SessionState::Recording) => true,
                (SessionState::Preparing, SessionState::Error) => true,
                (SessionState::Recording, SessionState::Paused) => true,
                (SessionState::Recording, SessionState::Stopping) => true,
                (SessionState::Recording, SessionState::Error) => true,
                (SessionState::Paused, SessionState::Recording) => true,
                (SessionState::Paused, SessionState::Stopping) => true,
                (SessionState::Paused, SessionState::Error) => true,
                (SessionState::Stopping, SessionState::Completed) => true,
                (SessionState::Stopping, SessionState::Error) => true,
                // A failed native session must still be stoppable so writers,
                // callbacks, and the project lock are finalized deterministically.
                (SessionState::Error, SessionState::Stopping) => true,
                (SessionState::Completed, SessionState::Idle) => true,
                (SessionState::Completed, SessionState::Preparing) => true,
                (SessionState::Error, SessionState::Idle) => true,
                (SessionState::Error, SessionState::Preparing) => true,
                // Idempotent transitions
                (a, b) if a == b => return Ok(()),
                _ => false,
            };

            if !valid {
                return Err(StateTransitionError::InvalidTransition {
                    from: current,
                    to: next,
                });
            }

            if self
                .state
                .compare_exchange_weak(current_raw, next as u8, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.current() == SessionState::Recording
    }

    pub fn is_paused(&self) -> bool {
        self.current() == SessionState::Paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_recording_lifecycle() {
        let sm = SessionStateMachine::new();
        assert_eq!(sm.current(), SessionState::Idle);

        assert!(sm.transition_to(SessionState::Preparing).is_ok());
        assert_eq!(sm.current(), SessionState::Preparing);

        assert!(sm.transition_to(SessionState::Recording).is_ok());
        assert!(sm.is_recording());

        assert!(sm.transition_to(SessionState::Paused).is_ok());
        assert!(sm.is_paused());

        assert!(sm.transition_to(SessionState::Recording).is_ok());
        assert!(sm.is_recording());

        assert!(sm.transition_to(SessionState::Stopping).is_ok());
        assert_eq!(sm.current(), SessionState::Stopping);

        assert!(sm.transition_to(SessionState::Completed).is_ok());
        assert_eq!(sm.current(), SessionState::Completed);

        assert!(sm.transition_to(SessionState::Idle).is_ok());
        assert_eq!(sm.current(), SessionState::Idle);

        // Test immediate second recording cycle from Completed directly
        assert!(sm.transition_to(SessionState::Preparing).is_ok());
        assert!(sm.transition_to(SessionState::Recording).is_ok());
        assert!(sm.transition_to(SessionState::Stopping).is_ok());
        assert!(sm.transition_to(SessionState::Completed).is_ok());
        assert!(sm.transition_to(SessionState::Preparing).is_ok());
        assert_eq!(sm.current(), SessionState::Preparing);
    }

    #[test]
    fn test_invalid_transitions_rejected() {
        let sm = SessionStateMachine::new();
        // Cannot jump directly from Idle to Recording without Preparing
        let err = sm.transition_to(SessionState::Recording);
        assert_eq!(
            err,
            Err(StateTransitionError::InvalidTransition {
                from: SessionState::Idle,
                to: SessionState::Recording,
            })
        );
    }
}
