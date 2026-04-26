/// Current armed/disarmed status for the daemon.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AlarmStatus {
    /// Alarm is not armed.
    #[default]
    Disarmed,
    /// Alarm is armed.
    Armed,
}

impl AlarmStatus {
    /// Returns whether the alarm is currently armed.
    #[must_use]
    pub fn is_armed(self) -> bool {
        matches!(self, Self::Armed)
    }
}
