# alarm-daemon

[![ci](https://github.com/reneherrero/alarm-daemon/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/reneherrero/alarm-daemon/actions/workflows/ci.yml)

Safety-critical alarm service for the Helm sailboat system. Owns the lifecycle
of every time- and condition-based alarm independently of the apps that arm it,
so alerts keep firing across app crashes, redeploys, and reboots.

See [`alarm-daemon-requirements.md`](./alarm-daemon-requirements.md) for the
full design, and [`todo.md`](./todo.md) for what's implemented vs planned.

> **Status — v0.1.** The `org.helm.AlarmDaemon.Control` interface ships with
> `Arm` / `Disarm` / `Snooze` / `Dismiss` / `Status` / `CurrentSound` /
> `ListSounds` and the `StateChanged` signal. Armed state is persisted to
> `redb`, with a `MissedWhileDown` recovery flag on startup. Audio output is
> live via `cpal` (WAV/FLAC). The Yocto / system-bus deployment story is
> first-class: a hardened system unit (`Type=notify` + `WatchdogSec=10s`),
> dbus policy + introspection schema, sysusers/tmpfiles drop-ins and
> packaging notes ship in `data/` (see *Production deployment* below).
> Logging auto-switches to `tracing-journald` under any systemd unit so
> `journalctl -o json` exposes structured fields. Release artifact is
> ~3.6 MB stripped (LTO + `panic=abort`). A portable `ci.sh` runs the
> whole quality gate including a no-link `aarch64-unknown-linux-gnu`
> cross-check. Still deferred: per-alarm records keyed by UUIDv7,
> evaluators / scheduling, the `OutputDegraded` signal, and polkit.

## Build

Requires Rust 1.85+ (edition 2024) and ALSA development headers (cpal links
libasound at build time, per FR-6.1).

```sh
sudo apt install libasound2-dev   # Debian/Ubuntu/Yocto host
cargo build --release
```

The binary lands at `target/release/alarm-daemon`. The workspace
`[profile.release]` is tuned for embedded deployment (see
[`Cargo.toml`](../Cargo.toml)):

- `lto = "thin"` + `codegen-units = 1` for whole-workspace inlining
- `strip = "symbols"` to drop debuginfo from the shipped artifact
- `panic = "abort"` because clippy already denies `panic`/`unwrap`/`expect`
  in `src/`, so a runtime panic is a true bug and aborting hands recovery
  to systemd's `Restart=on-failure` instead of leaving the daemon
  half-alive

Net effect on the binary: ~3.6 MB stripped (vs. ~6.4 MB with default
release settings), no unwind tables, fits comfortably in typical Yocto
`${IMAGE_ROOTFS_SIZE}` budgets.

## Quality gate (`ci.sh`)

The full gate — build, clippy, test, cross-check, release build — is
captured in [`../ci.sh`](../ci.sh). Run it before pushing:

```sh
./ci.sh                # all stages, ~40s on a warm cache
SKIP_CROSS=1 ./ci.sh   # skip stage 4 if cross rust-std is unavailable
VERBOSE=1 ./ci.sh      # stream cargo output instead of summarising
```

Stages, in order:

1. `cargo build --workspace --all-targets`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo test --workspace --no-fail-fast` (incl. integration tests
   under `dbus-run-session`)
4. `PKG_CONFIG_ALLOW_CROSS=1 cargo check --target <T>` for every
   supported production target `T` that **isn't** the build host —
   no-link smoke test that catches portability regressions before they
   hit a real bitbake build, without needing a sysroot on dev machines
   (Yocto / an SDK supplies a real one in production).
5. `cargo build --release` and report the stripped binary size, so
   release-profile budget regressions show up as a bisectable diff in CI
   logs.

### Supported production targets

The workspace is first-class on both:

| Triple                       | Typical deployment                                     |
|------------------------------|--------------------------------------------------------|
| `x86_64-unknown-linux-gnu`   | dev laptops, CI runners, NUC-class edge boxes          |
| `aarch64-unknown-linux-gnu`  | RPi 4/5, NXP i.MX8, TI Sitara, NVIDIA Jetson, arm64 VMs |

Both are pinned in [`../rust-toolchain.toml`](../rust-toolchain.toml), so
`rustup` installs both `rust-std` components automatically the first
time anyone runs cargo in this repo, and `ci.sh` auto-detects the host
triple and cross-checks the **other** one on every run — x86 hosts
catch arm64 breakage and vice versa. To add a third target (e.g.
`armv7-unknown-linux-gnueabihf` for 32-bit ARMv7 boards), append it to
both the `targets = […]` list in `rust-toolchain.toml` and
`PRODUCTION_TARGETS=(…)` in `ci.sh`.

### CI

GitHub Actions wraps `./ci.sh` on every push and PR to `main` —
see [`../.github/workflows/ci.yml`](../.github/workflows/ci.yml). The
workflow itself is intentionally thin: install `libasound2-dev pkg-config
dbus`, restore the cargo / `target/` cache (`Swatinem/rust-cache`), then
shell out to `./ci.sh`. The release-binary size is published to the
workflow run summary so budget regressions are visible without digging
into logs.

If you need to wrap `./ci.sh` from a different CI host (GitLab CI, Yocto
autobuilder, Drone, …), keep it the single source of truth — never
duplicate stages in CI YAML. Lint gate alone:

```sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
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

### System bus (manual)

For a hand-rolled system-bus install on a development host, copy the
ready-made files out of [`data/`](./data/) (see [`data/README.md`](./data/README.md)
for the full mapping):

```sh
sudo install -D -m 0755 target/release/alarm-daemon \
    /usr/bin/alarm-daemon
sudo install -D -m 0644 data/systemd/alarm-daemon.service \
    /lib/systemd/system/alarm-daemon.service
sudo install -D -m 0644 data/dbus/org.helm.AlarmDaemon.conf \
    /usr/share/dbus-1/system.d/org.helm.AlarmDaemon.conf
sudo install -D -m 0644 data/dbus/org.helm.AlarmDaemon.Control.xml \
    /usr/share/dbus-1/interfaces/org.helm.AlarmDaemon.Control.xml
sudo install -D -m 0644 data/sysusers.d/helm-alarm-daemon.conf \
    /usr/lib/sysusers.d/helm-alarm-daemon.conf
sudo install -D -m 0644 data/tmpfiles.d/helm-alarm-daemon.conf \
    /usr/lib/tmpfiles.d/helm-alarm-daemon.conf

sudo systemd-sysusers
sudo systemd-tmpfiles --create
sudo systemctl daemon-reload
sudo systemctl enable --now alarm-daemon.service
```

The bundled unit runs the daemon as the `helm-alarm` system user, listens on
the system bus, and stores state under `/var/lib/helm`. A polkit policy
(NFR-4.1) is still TODO; until it lands, the dbus policy file restricts
callers to the `helm` group plus `root`.

## Install as a systemd user unit (developer / CI)

`setup.sh` is the developer/CI installer — it builds, drops the binary in
`~/.local/bin`, and writes a **user** systemd unit running on the **session**
bus. CI uses the same script so there's exactly one tested install path for
contributors:

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

### Structured logging

When the daemon is launched by systemd (any unit type — detected via
`$INVOCATION_ID`), it streams events directly to journald via
`tracing-journald`, so every `info!(key = value, …)` becomes a typed
journal field. Outside systemd (`cargo run`, integration tests,
`dbus-run-session`) it falls back to ANSI-coloured stderr. Both paths
honour `RUST_LOG`.

Query structured fields once installed:

```sh
# All events emitted while bound to the session bus:
journalctl --user -u alarm-daemon.service F_BUS=session

# Render every event as JSON for downstream shippers:
journalctl --user -u alarm-daemon.service -o json | jq '{
    ts: ._SOURCE_REALTIME_TIMESTAMP,
    msg: .MESSAGE,
    bus: .F_BUS,
    db_path: .F_DB_PATH,
    backend: .F_LOGGING
}'
```

Field names are prefixed with `F_` (the `tracing-journald` default),
keeping them disjoint from systemd's own `_*` metadata.

For production / Yocto, see [Production deployment (Yocto)](#production-deployment-yocto)
below — `setup.sh` is intentionally not used there.

## Production deployment (Yocto)

The repository ships a complete set of distro-packaging assets under
[`data/`](./data/). A Yocto recipe (or any distro packager) installs them
verbatim — nothing in `setup.sh` is involved.

| File in `data/`                              | Installed at                                                  |
|----------------------------------------------|---------------------------------------------------------------|
| `systemd/alarm-daemon.service`               | `/lib/systemd/system/alarm-daemon.service`                    |
| `dbus/org.helm.AlarmDaemon.conf`             | `/usr/share/dbus-1/system.d/org.helm.AlarmDaemon.conf`        |
| `dbus/org.helm.AlarmDaemon.service`          | `/usr/share/dbus-1/system-services/org.helm.AlarmDaemon.service` |
| `dbus/org.helm.AlarmDaemon.Control.xml`      | `/usr/share/dbus-1/interfaces/org.helm.AlarmDaemon.Control.xml` |
| `sysusers.d/helm-alarm-daemon.conf`          | `/usr/lib/sysusers.d/helm-alarm-daemon.conf`                  |
| `tmpfiles.d/helm-alarm-daemon.conf`          | `/usr/lib/tmpfiles.d/helm-alarm-daemon.conf`                  |
| `etc/alarm-daemon.conf.example` (optional)   | `/etc/helm/alarm-daemon.conf` (rename when installing)        |

The daemon binary lands at `/usr/bin/alarm-daemon` and the bundled WAV sounds
at `/usr/share/helm/sounds/*.wav`. State lives at
`/var/lib/helm/alarm-daemon.redb` (the directory is created and chmod'd by
the bundled tmpfiles drop-in).

Highlights of the bundled system unit:

- `Type=notify` + `WatchdogSec=10s` — the daemon sends `READY=1` once it has
  owned the bus name and pings `WATCHDOG=1` at half the timeout. A runtime
  stall causes systemd to kill + restart it instead of leaving alarms dead;
  `STOPPING=1` is sent on graceful shutdown so journald shows a clean exit.
- `User=helm-alarm` / `Group=helm` — least-privileged dedicated identity.
- `EnvironmentFile=-/etc/helm/alarm-daemon.conf` — distro / image overrides
  without editing the unit (template at `data/etc/alarm-daemon.conf.example`).
- `CapabilityBoundingSet=CAP_WAKE_ALARM` / `AmbientCapabilities=CAP_WAKE_ALARM` —
  ready for the `CLOCK_BOOTTIME_ALARM`-based scheduler.
- `ProtectSystem=strict`, `ProtectHome=true`, `ReadWritePaths=/var/lib/helm`,
  `RestrictAddressFamilies=AF_UNIX`, `MemoryDenyWriteExecute=true`,
  `SystemCallFilter=@system-service ~@privileged @resources` — passes
  `systemd-analyze security` with a low exposure score.

A complete Yocto recipe sketch (cargo-bitbake style, with `useradd.bbclass`
hookup) lives at [`data/README.md`](./data/README.md#yocto-recipe-sketch).

To verify a deployment on the target:

```sh
# As any user:
busctl --system list | grep org.helm.AlarmDaemon
busctl --system call org.helm.AlarmDaemon /org/helm/AlarmDaemon \
    org.helm.AlarmDaemon.Control Status

# As a `helm`-group member:
busctl --system call org.helm.AlarmDaemon /org/helm/AlarmDaemon \
    org.helm.AlarmDaemon.Control Arm s "builtin:collision"
```

### Renaming the user / group

`helm-alarm` and `helm` are referenced from four files in `data/` only — see
[`data/README.md`](./data/README.md#renaming-the-user--group) for the patch
points if a vendor layer needs different names.

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

## License

Dual-licensed at your option under:

- Apache License, Version 2.0 ([`../LICENSE-APACHE`](../LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`../LICENSE-MIT`](../LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this crate by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
