# alarm-daemon workspace

[![ci](https://github.com/reneherrero/alarm-daemon/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/reneherrero/alarm-daemon/actions/workflows/ci.yml)
[![license: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Rust workspace housing the Helm sailboat alarm stack. Two crates:

| Crate                                          | Kind   | Purpose                                                                                                           |
|------------------------------------------------|--------|-------------------------------------------------------------------------------------------------------------------|
| [`alarm-daemon/`](./alarm-daemon/)             | binary | Safety-critical `org.helm.AlarmDaemon` D-Bus service. Owns alarm lifecycle across crashes, redeploys, and reboots. See [`alarm-daemon/REQUIREMENTS.md`](./alarm-daemon/REQUIREMENTS.md) for the full design. |
| [`alarm-daemon-client/`](./alarm-daemon-client/) | library | Typed async Rust client for the daemon's D-Bus surface — no manual message construction.                        |

Per-crate READMEs have full build / run / deploy instructions:

- [`alarm-daemon/README.md`](./alarm-daemon/README.md) — CI gate,
  systemd unit, production / Yocto deployment, structured journald
  logging, D-Bus schema contract.
- [`alarm-daemon-client/README.md`](./alarm-daemon-client/README.md) —
  client API surface + example.

## Quick start

```sh
# System dep (cpal links libasound, per FR-6.1):
sudo apt install libasound2-dev      # Debian / Ubuntu / Yocto host

# Run the full quality gate — build, clippy, workspace tests, aarch64
# cross-check, release build + size report. Single source of truth for
# both local runs and GitHub Actions.
./ci.sh

# Install the daemon as a user systemd unit for local hacking:
./alarm-daemon/setup.sh install

# Try the client:
cargo run -p alarm-daemon-client --example basic
```

## Repository layout

```
.
├── alarm-daemon/               daemon crate + data/ (systemd, dbus, sysusers, tmpfiles)
├── alarm-daemon-client/        client library crate
├── ci.sh                       quality gate (build, clippy, test, cross-check, release)
├── rust-toolchain.toml         pinned toolchain + cross targets
├── .github/workflows/ci.yml    GitHub Actions wrapper around ci.sh
├── Cargo.toml                  workspace manifest (shared metadata, lints, release profile)
├── LICENSE-MIT                 MIT license text
└── LICENSE-APACHE              Apache-2.0 license text
```

## Supported production targets

Both arches are first-class and exercised on every CI run:

- `x86_64-unknown-linux-gnu` — dev laptops, CI runners, x86 Yocto / NUC-class edge boxes
- `aarch64-unknown-linux-gnu` — RPi 4/5, NXP i.MX8, TI Sitara, NVIDIA Jetson, arm64 VMs

See [`alarm-daemon/README.md`](./alarm-daemon/README.md#supported-production-targets)
for details on how `ci.sh` handles cross-checking.

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](./LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
