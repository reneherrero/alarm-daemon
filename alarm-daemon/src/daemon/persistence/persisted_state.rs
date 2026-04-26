#[derive(Debug, Clone, Default)]
pub struct PersistedState {
    pub armed: bool,
    pub next_fire_unix_ms: Option<i64>,
    pub current_sound: Option<String>,
}
