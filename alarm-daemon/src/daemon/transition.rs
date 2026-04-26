use crate::daemon::AlarmStatus;

/// Result of an arm/disarm call: did the state actually change?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Requested state already matched current state.
    NoChange,
    /// State changed from one value to another.
    Changed {
        /// Previous state before the transition.
        from: AlarmStatus,
        /// New state after the transition.
        to: AlarmStatus,
    },
}
