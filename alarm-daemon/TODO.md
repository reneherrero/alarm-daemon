# alarm-daemon TODO

Tracks remaining work for the `alarm-daemon` Rust crate against
[`alarm-daemon-requirements.md`](./alarm-daemon-requirements.md).

## Done

- [x] Cargo crate scaffolded (edition 2024); lints deny `unwrap`/`expect`/`panic`/`todo`/`unimplemented` and forbid `unsafe` (NFR-2.3, NFR-7.1)
- [x] `org.helm.AlarmDaemon.Control` D-Bus surface live with `Arm(sound_id)`, `Disarm()`, `Status() -> b`, plus `StateChanged(b)` signal
- [x] In-memory `AlarmDaemon` state with idempotent transitions and explicit `Transition` reporting
- [x] Unit tests (3) green, `cargo clippy --all-targets --all-features -- -D warnings` clean, live `dbus-run-session` round-trip verified

## Core data model & API shape

- [ ] **#6** Per-alarm registry with UUIDv7 IDs — FR-1.1, FR-1.2
- [ ] **#7** `ArmTimer` one-shot (relative + absolute) — FR-2.1
- [ ] **#8** `CLOCK_BOOTTIME`-backed scheduler — FR-2.3, FR-2.6
- [ ] **#9** `Disarm(id)`, `Dismiss(id, dwell_ms)`, `Snooze(id, duration_s)` — §5.1.1, FR-8
- [ ] **#10** Query interface (`ListAlarms`, `GetAlarm`) — §5.1.2
- [ ] **#11** Signal set: `AlarmStateChanged` / `AlarmFiring` / `AlarmCleared` — §5.1.3

## Durability & advanced timing

- [x] **#12** redb persistence + startup recovery (`MissedWhileDown`) — FR-9.1–9.4 (minimal v0.1: global armed state + next-fire metadata + startup recovery flag/log)
- [ ] **#13** Repeating timers (no-drift + variance) — FR-2.2, FR-2.6
- [ ] **#14** RTC wake (`RTC_WKALM_SET`) for >60 s alarms, coalesced — FR-2.4, FR-2.5

## Severity & outputs

- [ ] **#15** Tier model + escalation engine (notice → alert → emergency) — FR-5
- [x] **#16** Audio output via `cpal` with volume ramp — FR-6.1
- [x] **#28** Custom alarm sound registration (consumer-supplied WAV/FLAC) — extends FR-6.1
- [ ] **#17** Compositor wake (display, overlay, blank inhibit) — §3.7
- [ ] **#18** Haptic + external-buzzer GPIO output — FR-6.3, FR-6.4
- [ ] **#19** `OutputDegraded` signal + per-subsystem failure isolation — FR-6.5

## Evaluators

- [ ] **#20** Anchor drag evaluator + swing-track ring buffer (`GetAnchorTrack`) — FR-4
- [ ] **#21** Generic condition evaluator framework (data sources, debounce, quality gate, degraded sub-state) — FR-3

## Operational concerns

- [ ] **#22** Configuration loader (`/etc/helm/alarm-daemon.toml`) — §6
- [ ] **#23** Polkit authorization on Control methods — NFR-4.1
- [ ] **#24** systemd watchdog (sd_notify keepalive ≤ 10 s) — NFR-2.4
- [ ] **#25** Clock-jump detection + `ClockJumped` signal — FR-10
- [ ] **#26** Bounded event history (30 days / 10 MB) + `GetRecentEvents` — FR-9.5, §5.1.2
- [ ] **#27** Test mode: injectable clock + GPS-track replay harness — NFR-6.1, NFR-6.3

## Notes for #28 — Custom alarm sounds

The bundled set under `/usr/share/helm/sounds/` (FR-6.1) is read-only. Consumer
apps (timer-app, anchor-app, future plugins) should be able to register their
own sounds at runtime. Sketch:

- **Interface**: dedicated `org.helm.AlarmDaemon.Sounds` (or extend `Control`):
  - `RegisterSound(name: string, format: string, data: array<byte>) -> sound_id: string`
  - `UnregisterSound(sound_id: string) -> ()`
  - `ListSounds() -> array<sound_summary>` — fields: id, name, source (`builtin` | `custom`), byte size, sha256
- **Storage**: writable directory `/var/lib/helm/sounds/`, kept distinct from the read-only bundle. The daemon owns the on-disk filename — never derived from client input (NFR-4.3 — no client-provided string is interpreted as a path).
- **Validation**: magic-byte sniffing for WAV/FLAC (do not trust the `format` parameter); reject if `cpal`/decoder cannot open the file; cap per-sound size (e.g. 5 MiB) and total custom-sound disk use within the 10 MB persistence budget (§2.3).
- **Lifecycle**: persist `sound_id ↔ filename` in redb (§FR-9). `UnregisterSound` is refused if any armed alarm references the sound — caller must rearm with a different sound first, or pass an explicit `force` flag that rewrites references to a built-in fallback.
- **Authorization**: same polkit policy as arm/disarm — only the local `helm` user may register or unregister (NFR-4.1). `ListSounds` is open.
- **Sound IDs are namespaced**: `builtin:<name>` for bundled, `custom:<uuid>` for registered. The alarm record's *Output profile* sound-file-ID field (FR-1.1) accepts either form, with one canonical resolver for both.
- **Acceptance**: a registered custom sound survives a daemon restart and continues to play for any alarm that references it; deleting the underlying file behind the daemon's back is detected at fire time, the daemon logs the failure, swaps in a built-in fallback, and emits `OutputDegraded` (FR-6.5).
