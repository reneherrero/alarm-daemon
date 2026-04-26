mod common;

#[test]
fn demo_flow_install_arm_snooze_dismiss_uninstall() {
    let _env = common::TestEnv::acquire();
    common::require_tools();

    let script = r#"
set -euo pipefail

./setup.sh install
systemctl --user set-environment ALARM_DAEMON_TRIGGER_DELAY_MS=5000
systemctl --user restart alarm-daemon.service

for _ in $(seq 1 50); do
  if busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

# 1) Arm with selected sound, trigger delay is 5s via env.
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Arm s "builtin:collision" >/dev/null

# 2) Wait for trigger + 3s playback window.
sleep 8

# 3) Snooze for 5s.
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Snooze u 5 >/dev/null

# 4) Wait for second trigger + 3s playback window.
sleep 8

# 5) Dismiss and verify cleared state.
busctl --user call \
  org.helm.AlarmDaemon /org/helm/AlarmDaemon \
  org.helm.AlarmDaemon.Control Dismiss >/dev/null

[[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b false" ]]
[[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"\"" ]]

./setup.sh uninstall
"#;

    assert!(common::run_bash(script), "demo flow integration test failed");
}
