mod common;

#[test]
fn dbus_surface_status_arm_disarm_and_list_sounds() {
    let _env = common::TestEnv::acquire();
    common::require_tools();

    let install = r#"
set -euo pipefail
./setup.sh install
test -x "$HOME/.local/bin/alarm-daemon"
"#;
    assert!(common::run_bash(install), "setup install failed in dbus test");

    let functional = r#"
  start
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b false" ]]
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"\"" ]]
  busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Arm s "builtin:collision" >/dev/null
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b true" ]]
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"builtin:collision\"" ]]
  sounds="$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control ListSounds)"
  [[ "$sounds" == *"builtin:collision"* ]]
  [[ "$sounds" == *"builtin:casualty"* ]]
  [[ "$sounds" == *"builtin:klaxon"* ]]
  [[ "$sounds" == *"builtin:surface"* ]]
  busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Snooze u 1 >/dev/null
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b true" ]]
  busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Dismiss >/dev/null
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b false" ]]
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"\"" ]]
"#;
    assert!(
        common::run_dbus_daemon_session(functional),
        "dbus functional stage failed"
    );
}
