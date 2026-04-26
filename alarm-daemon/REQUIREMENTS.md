# alarm-daemon Requirements

**Component:** `alarm-daemon`
**Project:** Helm (sailboat embedded system)
**Status:** Draft v0.1
**Owner:** TBD

---

## 1. Purpose and scope

### 1.1 Purpose

`alarm-daemon` is the safety-critical service responsible for all time-based and condition-based alerts on the sailboat. It is the single authoritative source for "something needs the skipper's attention right now" events, ranging from a gentle chime to a full emergency alert.

The daemon exists as a separate, long-lived process specifically so that alarms continue to fire correctly even when user-facing applications crash, are killed, or are redeployed. Applications such as `timer-app` and `anchor-app` **arm** the daemon with parameters; the daemon then owns the lifecycle of the alarm independently.

### 1.2 Scope

In scope:

- Scheduling and firing time-based alarms (one-shot, repeating, RTC-backed)
- Evaluating condition-based alarms driven by external data sources (e.g., GPS position for anchor drag)
- Producing audible, visual, and haptic output through the appropriate subsystems
- Persisting armed alarms across process restarts, system reboots, and brownouts
- Providing a stable IPC surface for applications and the shell
- Enforcing the escalation policy (notice → alert → emergency)

Out of scope:

- Rendering alarm UI (the compositor and `helm-shell` handle the overlay)
- Computing domain-specific logic that belongs to an application (e.g., the swing circle math for the anchor alarm is evaluated inside the daemon, but *defining* a swing circle is the app's job; see §4.3)
- Delivering notifications to off-device recipients (SMS, MQTT, etc.) — a later component
- Managing long-term system logs (the logger daemon handles this)

### 1.3 Definitions

| Term | Meaning |
|------|---------|
| Alarm | A configured condition that, when met, triggers output (sound, visual, haptic) |
| Arm | To register an alarm with the daemon so it becomes active |
| Disarm | To cancel an armed alarm before it fires |
| Dismiss | To acknowledge a firing alarm and stop its output |
| Snooze | To temporarily silence a firing alarm, rearming it after a delay |
| Tier | Severity level: notice, alert, or emergency |
| Escalation | Automatic promotion to a higher tier if not dismissed within a window |

---

## 2. Operating environment

### 2.1 Platform

- Linux kernel 6.6+ (Yocto Scarthgap baseline)
- systemd-managed service, started before any user-facing app
- Rust edition 2024, stable toolchain
- Runs as a dedicated unprivileged user (`alarm`) with capabilities only for the specific devices it needs

### 2.2 Dependencies

| Dependency | Purpose | Notes |
|------------|---------|-------|
| `zbus` | D-Bus IPC | Pure Rust |
| `tokio` | Async runtime | Needed for concurrent timer/IPC/audio |
| `cpal` | Audio output | Pure Rust, ALSA backend on Linux |
| `rustix` or `nix` | `CLOCK_BOOTTIME`, RTC ioctls, GPIO | Thin libc wrapper, not a large C dep |
| `redb` | Persistent store for armed alarms | Pure Rust, crash-safe |
| `serde` + `serde_json` | Config parsing | — |
| `tracing` | Structured logging | — |

### 2.3 Resource budget

- RAM: ≤ 30 MB resident
- CPU: ≤ 1% average when idle, ≤ 5% during active alarm
- Disk: ≤ 10 MB for persistent state, including history
- Startup time: armed alarms must be re-evaluated within 2 seconds of daemon start

---

## 3. Functional requirements

### 3.1 Alarm model

**FR-1.1** The daemon shall represent every alarm as a record containing at minimum:

- Unique ID (UUIDv7, monotonically sortable)
- Creator (client identifier, e.g., `timer-app`, `anchor-app`)
- Kind (`time_oneshot`, `time_repeating`, `condition_anchor_drag`, `condition_generic`)
- Tier (`notice`, `alert`, `emergency`)
- State (`armed`, `firing`, `snoozed`, `dismissed`, `expired`, `disarmed`)
- Creation timestamp (wall clock + `CLOCK_BOOTTIME`)
- Trigger parameters (kind-specific)
- Output profile (sound file ID, volume override, use external buzzer flag, haptic flag)
- Dismissal policy (`tap`, `hold_2s`, `double_tap`, `swipe`)
- Optional label (human-readable string, max 64 chars)
- Optional escalation policy (see §3.5)

**FR-1.2** The daemon shall generate IDs internally; clients shall not supply them.

**FR-1.3** The daemon shall reject alarm records that fail validation (unknown kind, missing required parameters, malformed values) with a structured error.

### 3.2 Time-based alarms

**FR-2.1** The daemon shall support one-shot time alarms specified as either:

- An absolute wall-clock time (UTC)
- A relative duration from the arm call (e.g., "20 minutes from now")

**FR-2.2** The daemon shall support repeating time alarms with:

- A base duration (e.g., 20 minutes)
- A maximum repetition count (including `unlimited`)
- An optional variance (± seconds) to prevent lockstep with other alarms

**FR-2.3** The daemon shall schedule time alarms against `CLOCK_BOOTTIME`, which continues to advance during system suspend. `std::time::Instant` shall not be used for scheduling.

**FR-2.4** For any time alarm firing more than 60 seconds in the future, the daemon shall program an RTC wake alarm (`/dev/rtc0`, `RTC_WKALM_SET`) so the system can be safely suspended and still wake in time. The daemon shall coalesce multiple pending alarms into a single RTC wake at the earliest fire time.

**FR-2.5** When an RTC wake fires, the daemon shall re-evaluate all armed alarms within 500 ms of resume.

**FR-2.6** Time alarm accuracy:

- Armed alarm fires within **±1 second** of its scheduled time when the system is not suspended
- Armed alarm fires within **±5 seconds** after a suspend/resume cycle
- Repeating alarms shall not drift: the *nth* repetition is scheduled from the original base time, not from the previous fire time

### 3.3 Condition-based alarms: generic

**FR-3.1** The daemon shall support condition-based alarms evaluated against data streams published by other daemons over D-Bus or Unix sockets.

**FR-3.2** A condition alarm shall specify:

- A data source identifier (e.g., `org.helm.NavDaemon.Position`)
- A predicate expression or a named built-in evaluator
- A debounce policy (minimum consecutive observations before firing)
- A quality gate (conditions under which evaluation is paused, e.g., GPS fix quality below threshold)

**FR-3.3** When the data source becomes unavailable (daemon down, no fresh samples for > configurable timeout), the daemon shall:

- Transition the alarm into a `degraded` sub-state
- After a configurable grace period (default 30 s), emit a tier-`alert` data-loss warning rather than silently suppressing the alarm
- Continue attempting to reconnect

### 3.4 Condition-based alarms: anchor drag

**FR-4.1** The daemon shall implement a first-class `anchor_drag` evaluator, armed with:

- Reference position (lat/lon)
- Swing radius (meters)
- Consecutive-fix threshold (default 5)
- Bearing-consistency threshold (default 60° arc — successive displacement vectors within this arc count as consistent drag)
- Minimum fix quality (HDOP ≤ configurable, satellites ≥ configurable)

**FR-4.2** The daemon shall fire the alarm when **all** of the following hold for the threshold number of consecutive fixes:

- Distance from reference position > swing radius
- Fix quality meets minimum
- Displacement bearings are consistent (within the arc threshold)

**FR-4.3** If fix quality falls below minimum, the daemon shall:

- Pause drag evaluation (do not decrement or reset the consecutive-fix counter)
- After a configurable grace period (default 60 s) of continuous poor fix, emit a tier-`alert` "GPS degraded" warning
- Resume evaluation immediately when quality recovers

**FR-4.4** The daemon shall retain the last 1000 fixes (configurable) for the active anchor alarm in a ring buffer, exposed read-only over D-Bus for the UI to render the swing track.

**FR-4.5** The skipper shall be able to adjust the swing radius at any time without disarming the alarm. The new radius takes effect immediately without resetting the consecutive-fix counter.

### 3.5 Tiers and escalation

**FR-5.1** The daemon shall implement three tiers with the following default behavior:

| Tier | Audio | Visual | Haptic | External buzzer | Dismiss requirement |
|------|-------|--------|--------|-----------------|---------------------|
| `notice` | Single gentle chime | Status bar indicator | No | No | Auto-clears after 10 s |
| `alert` | Repeating tone at 80% volume | Full-screen overlay | Yes | Optional (config) | Explicit dismissal |
| `emergency` | Continuous tone at max volume | Full-screen overlay, cannot be hidden | Yes | Yes (if configured) | Hold-to-confirm (2 s default) |

**FR-5.2** A client may specify an escalation policy when arming an alarm:

- `none` — alarm stays at its initial tier
- `time` — if not dismissed within N seconds, promote one tier (may chain up to `emergency`)
- `repetition` — each repetition of a repeating alarm promotes one tier up to a cap

**FR-5.3** The daemon shall never demote a tier automatically. Demotion requires explicit client action.

**FR-5.4** Tier-`emergency` alarms shall override system-wide mute/volume settings and any active "do not disturb" policy.

### 3.6 Output subsystems

**FR-6.1 Audio.** The daemon shall play alarm sounds through ALSA via `cpal`. It shall:

- Maintain exclusive or shared access to a dedicated ALSA device (configurable) to avoid being muted by other audio clients
- Support WAV and FLAC sound files stored under `/usr/share/helm/sounds/`
- Implement volume ramp-up over 2–10 seconds (configurable per profile) to avoid startle, except for tier `emergency` which starts at full volume
- Survive audio device hot-unplug by logging a warning and attempting recovery every 5 s

**FR-6.2 Visual.** The daemon shall not draw UI directly. It shall emit D-Bus signals that `helm-shell` consumes to render the overlay. The daemon is responsible for ensuring the compositor is notified to wake the display (see §3.7).

**FR-6.3 Haptic.** If a haptic device is present (configurable GPIO or PWM device), the daemon shall pulse it according to the tier's pattern.

**FR-6.4 External buzzer.** The daemon shall support driving a GPIO line as a loud external buzzer, configured as:

- GPIO chip + line (or PWM device)
- Active level (active-high / active-low)
- Pattern (continuous, pulsed with period/duty)
- Tiers that activate it (default: `emergency` only)

**FR-6.5** If any output subsystem fails (audio device missing, GPIO permission error), the daemon shall:

- Log the failure with `tracing::error`
- Continue firing the alarm through remaining available subsystems
- Emit a D-Bus signal `OutputDegraded` so the UI can inform the skipper

### 3.7 Display wake

**FR-7.1** When firing any alarm of tier `alert` or higher, the daemon shall request the compositor to:

- Wake the display if blanked
- Raise the alarm overlay above all other surfaces
- Inhibit screen blanking for the duration of the firing state

**FR-7.2** The compositor integration shall use the custom `helm-shell-v1` protocol (or equivalent D-Bus RPC to `helm-shell`). The daemon shall not attempt to drive DRM/KMS directly.

**FR-7.3** If the compositor is unreachable, the daemon shall continue firing audio and external-buzzer outputs, log the failure, and retry compositor notification every 1 s.

### 3.8 Dismissal and snooze

**FR-8.1** The daemon shall only accept a dismissal request via its authenticated D-Bus interface (see §5), never via keyboard/touch directly. UI components translate user gestures into dismissal calls.

**FR-8.2** The daemon shall honor the dismissal policy specified when the alarm was armed. A `hold_2s` policy means the D-Bus call must include a dwell-time parameter ≥ 2000 ms; calls with shorter dwell shall be rejected.

**FR-8.3** The daemon shall support snooze with a client-specified duration. During snooze:

- State is `snoozed`
- All output stops
- The alarm remains scheduled to re-fire after the snooze duration
- Snooze is available only for alarms whose arm record allows it (clients opt in)

**FR-8.4** Tier-`emergency` alarms shall not be snoozable.

### 3.9 Persistence

**FR-9.1** The daemon shall persist every armed alarm to a transactional local store (`redb`) at `/var/lib/helm/alarms.redb`.

**FR-9.2** Every state transition (armed, firing, snoozed, dismissed, disarmed, expired) shall be durably committed before the daemon acknowledges the transition to its caller.

**FR-9.3** On startup, the daemon shall:

1. Open the persistent store
2. Load all non-terminal alarms (`armed`, `firing`, `snoozed`)
3. For each, compute whether it should have fired during the downtime
4. If it should have fired: transition to `firing` immediately, producing a "missed alarm" event the UI can display
5. If it is still in the future: reschedule normally
6. If it is ambiguous (clock skew, no recent time source): transition to `firing` with tier preserved — err on the side of informing the skipper

**FR-9.4** The daemon shall fsync the persistent store at each commit and tolerate abrupt power loss without corruption. No alarm armed before a power loss shall be silently lost.

**FR-9.5** The daemon shall retain a bounded event history (default 30 days, max 10 MB) for later inspection.

### 3.10 Time source and clock handling

**FR-10.1** The daemon shall treat wall-clock time as potentially incorrect at startup (the RTC may be wrong; GPS may not yet have a fix).

**FR-10.2** The daemon shall subscribe to system clock-change events (e.g., when `chrony` applies a GPS-derived correction) and:

- Re-evaluate all absolute-time alarms against the new wall clock
- Not re-evaluate relative-time (boottime-scheduled) alarms, which are unaffected

**FR-10.3** If a wall-clock jump > 60 seconds occurs, the daemon shall log it and emit a D-Bus signal so the UI can optionally inform the skipper.

---

## 4. Non-functional requirements

### 4.1 Reliability

**NFR-1.1** The daemon shall have a mean time between failures ≥ 30 days under nominal load.

**NFR-1.2** The daemon shall be restarted automatically by systemd on crash with a restart backoff. Persistent state guarantees alarms survive any number of restarts.

**NFR-1.3** The daemon shall not depend on network availability. All required functionality works fully offline.

**NFR-1.4** A single malformed or buggy alarm record must not prevent other alarms from firing. Evaluation errors are contained per alarm.

### 4.2 Safety

**NFR-2.1** The daemon is considered safety-relevant. Changes to its code require review and must be exercised by the test suite before deployment.

**NFR-2.2** A failure to fire shall be visibly worse than a false positive. When in doubt (unclear state, clock uncertainty, partial data), the daemon shall fire rather than suppress.

**NFR-2.3** The daemon shall never call `panic!` in production builds except for detected heap exhaustion. All error paths are handled explicitly. `unwrap()` and `expect()` are forbidden in non-test code and enforced by lint.

**NFR-2.4** A systemd watchdog keepalive shall be sent at least every 10 seconds. If the main loop stalls, the daemon is restarted.

### 4.3 Separation of concerns

**NFR-3.1** The daemon shall not contain domain knowledge beyond what is necessary for generic evaluators and the first-class `anchor_drag` evaluator. New alarm kinds that require complex domain logic shall be added as separate evaluator modules with clear interfaces.

**NFR-3.2** The daemon shall not render UI, play non-alarm sounds, or provide general-purpose scheduling beyond alarms.

### 4.4 Security

**NFR-4.1** The D-Bus interface shall use polkit (or equivalent) to restrict which clients can arm, disarm, or dismiss alarms. Default policy: local user `helm` may arm/disarm/dismiss; other local users may only query state.

**NFR-4.2** The daemon runs as an unprivileged user with `CAP_WAKE_ALARM` and the minimum device access required (audio device, RTC, specific GPIO lines). Root is not required.

**NFR-4.3** All inputs from clients shall be validated. No client-provided string is ever interpreted as a file path, executable, or shell command.

### 4.5 Observability

**NFR-5.1** The daemon shall emit structured logs via `tracing` to the journal. Every alarm state transition is logged with its ID, kind, tier, and cause.

**NFR-5.2** The daemon shall expose a read-only D-Bus introspection interface listing all armed alarms and recent events, suitable for a diagnostics app.

**NFR-5.3** The daemon shall maintain counters (fires, dismissals, failures by subsystem) accessible for diagnostics.

### 4.6 Testability

**NFR-6.1** The daemon shall support a test mode in which `CLOCK_BOOTTIME` and wall clock are replaced by an injectable clock, enabling deterministic fast-forward tests.

**NFR-6.2** Audio, GPIO, and D-Bus outputs shall be abstracted behind traits so they can be replaced with in-memory fakes during testing.

**NFR-6.3** A replay harness shall allow feeding recorded GPS tracks to exercise the anchor-drag evaluator against real-world data.

### 4.7 Maintainability

**NFR-7.1** Code shall pass `cargo clippy --all-targets --all-features -- -D warnings`.

**NFR-7.2** Public APIs (D-Bus interfaces, persisted schemas) shall be versioned. Breaking changes require a new interface version with migration.

**NFR-7.3** Persistent-store schema migrations shall be automatic at startup and reversible in development builds.

---

## 5. IPC surface

### 5.1 D-Bus interfaces

The daemon shall expose the following interfaces under the bus name `org.helm.AlarmDaemon`:

#### 5.1.1 `org.helm.AlarmDaemon.Control`

Methods:

- `ArmTimer(params: dict) -> id: string`
- `ArmAnchor(params: dict) -> id: string`
- `ArmCondition(params: dict) -> id: string`
- `Disarm(id: string) -> ()`
- `Dismiss(id: string, dwell_ms: uint32) -> ()`
- `Snooze(id: string, duration_s: uint32) -> ()`
- `UpdateAnchorRadius(id: string, radius_m: double) -> ()`

#### 5.1.2 `org.helm.AlarmDaemon.Query`

Methods:

- `ListAlarms() -> array<alarm_summary>`
- `GetAlarm(id: string) -> alarm_detail`
- `GetAnchorTrack(id: string, max_points: uint32) -> array<track_point>`
- `GetRecentEvents(since: timestamp) -> array<event>`

#### 5.1.3 `org.helm.AlarmDaemon.Signals`

Signals:

- `AlarmStateChanged(id: string, old_state: string, new_state: string)`
- `AlarmFiring(id: string, tier: string, label: string)`
- `AlarmCleared(id: string)`
- `OutputDegraded(subsystem: string, reason: string)`
- `MissedWhileDown(id: string, scheduled_for: timestamp)`
- `ClockJumped(delta_s: double)`

### 5.2 Parameter schemas

Parameter dictionaries shall follow JSON-compatible types. Detailed schemas (field names, types, required/optional, ranges) shall be maintained in `docs/dbus-schemas.md` and versioned alongside this requirements document.

---

## 6. Configuration

The daemon shall read configuration from `/etc/helm/alarm-daemon.toml` at startup. Configuration includes:

- Audio device name, fallback devices
- GPIO configuration for external buzzer (chip, line, active level)
- Haptic device configuration
- Default escalation policies per tier
- Default dismissal policies per tier
- Default quality gates for anchor-drag (HDOP, sat count, grace periods)
- Persistent store path
- Event history retention (days, max MB)
- Polkit policy file reference

The daemon shall validate the configuration at startup and refuse to start with a clear error message if any value is invalid or out of range.

---

## 7. Acceptance criteria

The daemon is considered acceptable for deployment when all of the following hold:

1. **Core fires reliably:** 1000 consecutive armed one-shot timers fire within ±1 s without loss across a test run including random kill-`-9` of the daemon (which must restart and still fire correctly).
2. **Suspend/resume:** A 30-minute timer set on a device that is suspended within 1 minute of arming fires within ±5 s of the scheduled time upon automatic RTC wake.
3. **Brownout survival:** Pulling power at arbitrary moments during 100 arm/fire/dismiss cycles results in zero silently lost alarms; every alarm either completed normally or fires on next boot with a `MissedWhileDown` event.
4. **Anchor drag — real data:** Against a curated corpus of ≥ 20 recorded GPS tracks covering normal swinging, mild dragging, severe dragging, GPS multipath, and intermittent fixes, the evaluator achieves ≥ 95% true-positive detection with ≤ 1 false positive per 24 hours of swinging-at-anchor time.
5. **Emergency tier overrides:** With the system volume set to 0 and "do not disturb" active, an `emergency` alarm produces full-volume audio and activates the external buzzer within 500 ms of the triggering condition.
6. **Clippy clean:** `cargo clippy --all-targets --all-features -- -D warnings` produces zero findings.
7. **No panics:** A 72-hour soak test with randomized client calls and induced subsystem failures (audio device unplugged, D-Bus restarted, persistent store momentarily read-only) completes with no panics or unexpected exits.
8. **Watchdog verified:** Intentionally stalling the main loop for > 10 seconds results in systemd restart and correct recovery of all armed alarms.

---

## 8. Open questions

1. Should snooze duration have a configurable upper bound per tier? (Proposed: yes — `alert` max 30 min, `emergency` not snoozable.)
2. Do we need cross-daemon alarm deduplication (e.g., suppress GPS-degraded alarms if the nav-daemon has already reported GPS loss)?
3. Should the daemon integrate with an external watchdog beyond systemd (hardware watchdog timer via `/dev/watchdog`)?
4. Audio routing when an external speaker is hot-plugged mid-alarm — continue on original device or migrate?
5. Multi-display behavior when the compositor manages multiple screens (cockpit and nav station) — alarms on both, or primary only with mirror policy?

---

## 9. Change log

| Version | Date | Notes |
|---------|------|-------|
| 0.1 | initial | First draft extracted from architecture discussion |
