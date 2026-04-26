# alarm-daemon-client

Typed Rust client for the `org.helm.AlarmDaemon.Control` D-Bus API.

## What this crate gives you

- No manual D-Bus message construction
- Simple async methods:
  - `list_sounds()`
  - `arm(sound_id)`
  - `disarm()`
  - `snooze(duration_s)`
  - `dismiss()`
  - `status()`
  - `current_sound()`

## Quick start

```rust,no_run
use alarm_daemon_client::AlarmDaemonClient;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = AlarmDaemonClient::connect_session().await?;
let sounds = client.list_sounds().await?;
if let Some(sound_id) = sounds.first() {
    client.arm(sound_id).await?;
}
# Ok(())
# }
```

## Example

Run the included example:

```sh
cargo run -p alarm-daemon-client --example basic
```
