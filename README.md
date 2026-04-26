# Alarm Workspace

This repository is a Rust workspace with two projects:

- `alarm-daemon/` - D-Bus alarm service binary (`alarm-daemon`)
- `alarm-daemon-client/` - typed Rust client crate for consumers

## Quick start

```sh
# Build everything
cargo build --workspace

# Run checks
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run daemon integration tests
cargo test -p alarm-daemon --tests

# Run client example
cargo run -p alarm-daemon-client --example basic
```
# alarm-daemon

Safety-critical alarm service for the Helm sailboat system. Owns the lifecycle
of every time- and condition-based alarm independently of the apps that arm it,
so alerts keep firing across app crashes, redeploys, and reboots.

See [`alarm-daemon-requirements.md`](./alarm-daemon-requirements.md) for the
full design, and [`todo.md`](./todo.md) for what's implemented vs planned.

> **Status — v0.1.** Only the `Arm` / `Disarm` / `Status` / `StateChanged`
> slice of `org.helm.AlarmDaemon.Control` is live, backed by in-memory state.
> Persistence, scheduling, evaluators, audio output, polkit, and the systemd
> watchdog will land in later steps. Treat the instructions below as a dev
> install; production packaging (system-bus policy, dedicated `alarm` user,
> `CAP_WAKE_ALARM`, `/var/lib/helm/alarms.redb`) is not yet scoped here.

## Build

Requires Rust 1.85+ (edition 2024) and ALSA development headers (cpal links
libasound at build time, per FR-6.1).

```sh
sudo apt install libasound2-dev   # Debian/Ubuntu/Yocto host
cargo build --release
```

The binary lands at `target/release/alarm-daemon`.

Lint gate used by CI:

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Run

The daemon chooses its bus via `ALARM_DAEMON_BUS` (default `session`). Log
level follows `RUST_LOG` (default `info`).

### Session bus (development)

```sh
RUST_LOG=info ALARM_DAEMON_BUS=session ./target/release/alarm-daemon
```

Verify in another shell:

```sh
busctl --user call \
    org.helm.AlarmDaemon /org/helm/AlarmDaemon \
    org.helm.AlarmDaemon.Control Status
# b false

busctl --user call \
    org.helm.AlarmDaemon /org/helm/AlarmDaemon \
    org.helm.AlarmDaemon.Control Arm s "builtin:collision"

busctl --user monitor org.helm.AlarmDaemon   # watch StateChanged signals
```

For an isolated round-trip on a workstation without a running session bus:

```sh
dbus-run-session -- ./target/release/alarm-daemon
```

### System bus

Running on the system bus requires a policy file granting the daemon its
well-known name, plus (eventually) polkit rules for `Arm` / `Disarm`. A
minimal policy lives at `/etc/dbus-1/system.d/org.helm.AlarmDaemon.conf`:

```xml
<!DOCTYPE busconfig PUBLIC
 "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <policy user="helm">
    <allow own="org.helm.AlarmDaemon"/>
  </policy>
  <policy context="default">
    <allow send_destination="org.helm.AlarmDaemon"
           send_interface="org.helm.AlarmDaemon.Control"/>
    <allow send_destination="org.helm.AlarmDaemon"
           send_interface="org.freedesktop.DBus.Introspectable"/>
  </policy>
</busconfig>
```

Then start the daemon as the `helm` user with `ALARM_DAEMON_BUS=system`.
A polkit policy (NFR-4.1) is still TODO; until it lands, any local user can
call `Arm`/`Disarm` through the policy above.

## Install as a systemd unit

The target deployment is a system unit owned by a dedicated `alarm` user with
`CAP_WAKE_ALARM` and watchdog keepalive (NFR-2.4, NFR-4.2). That unit is not
yet written — the v0.1 daemon has no watchdog loop and no persistent state to
recover, so installing it system-wide today gains nothing.

For local development, use the bundled script to install a **user** unit:

```sh
./setup.sh install     # build, install to ~/.local/bin, enable & start
./setup.sh update      # rebuild and restart
./setup.sh status      # show unit status
./setup.sh uninstall   # stop, disable, and remove binary + unit
```

The script writes `~/.config/systemd/user/alarm-daemon.service` pointing at
`~/.local/bin/alarm-daemon`, with `ALARM_DAEMON_BUS=session` and
`RUST_LOG=info`. Follow the journal with:

```sh
journalctl --user -u alarm-daemon.service -f
```

## D-Bus surface (current)

- Bus name: `org.helm.AlarmDaemon`
- Object path: `/org/helm/AlarmDaemon`
- Interface: `org.helm.AlarmDaemon.Control`
  - `Arm(s sound_id) -> ()` — validates sound and schedules trigger
  - `Disarm() -> ()` — idempotent
  - `Snooze(u duration_s) -> ()` — stop now, re-trigger after delay
  - `Dismiss() -> ()` — stop and clear alarm state
  - `Status() -> b` — `true` when armed
  - `CurrentSound() -> s` — currently selected sound ID, or empty string
  - `ListSounds() -> as` — sound IDs (`builtin:<name>`, `custom:<id>`)
  - `StateChanged(b armed)` — signal, emitted only on actual transitions

The full method set (`ArmTimer`, `ArmAnchor`, `ArmCondition`, `Disarm(id)`,
`Dismiss`, `Snooze`, `ListAlarms`, …) will grow on top of this without
renaming the bus name or object path.

## API quick reference (simple)

All methods are on:

- Bus name: `org.helm.AlarmDaemon`
- Object path: `/org/helm/AlarmDaemon`
- Interface: `org.helm.AlarmDaemon.Control`

### Methods

- `ListSounds() -> as`
  - Returns all installed sound IDs you can use with `Arm`.
  - Example IDs: `builtin:collision`, `builtin:casualty`.

- `Arm(s sound_id) -> ()`
  - Arms the daemon using the selected sound.
  - The daemon validates `sound_id`, schedules the trigger, and changes state to armed.
  - If `sound_id` is invalid, the call fails.

- `Status() -> b`
  - `true` means currently armed.
  - `false` means currently disarmed.

- `CurrentSound() -> s`
  - Returns the currently selected sound ID while armed.
  - Returns empty string when no sound is selected.

- `Disarm() -> ()`
  - Cancels pending trigger and stops active playback.
  - Clears the currently selected sound.

- `Snooze(u duration_s) -> ()`
  - Stops current playback and schedules the same sound again after `duration_s`.
  - Keeps the alarm armed.

- `Dismiss() -> ()`
  - Acknowledge/stop the alarm and clear armed state.

### Signal

- `StateChanged(b armed)`
  - Emitted when the armed/disarmed state actually changes.

### Typical client flow

1. Call `ListSounds()`.
2. Pick one `sound_id`.
3. Call `Arm(sound_id)`.
4. Optionally poll `Status()` / `CurrentSound()` or listen for `StateChanged`.
5. Use `Snooze(duration_s)` to delay, or `Dismiss()` to clear.

### `busctl` examples

```sh
# 1) List available sounds
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control ListSounds

# 2) Arm with a specific sound
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Arm s "builtin:collision"

# 3) Check armed/disarmed status
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Status

# 4) Check currently selected sound
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control CurrentSound

# 5) Disarm
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Disarm

# 6) Snooze for 5 seconds
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Snooze u 5

# 7) Dismiss
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Dismiss
```
