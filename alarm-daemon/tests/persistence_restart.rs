mod common;

#[test]
fn persistence_survives_restart() {
    let _env = common::TestEnv::acquire();
    common::require_tools();

    assert!(
        common::run_bash(
            r#"
set -euo pipefail
./setup.sh install
"#
        ),
        "setup install failed in persistence test"
    );

    let script = r#"
  start
  busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Arm s "builtin:collision" >/dev/null
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b true" ]]
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"builtin:collision\"" ]]
  stop
  start
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b true" ]]
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"builtin:collision\"" ]]
  busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Disarm >/dev/null
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"\"" ]]
  stop
  start
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status)" == "b false" ]]
  [[ "$(busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control CurrentSound)" == "s \"\"" ]]
  stop
"#;
    assert!(
        common::run_dbus_daemon_session(script),
        "persistence restart stage failed"
    );
}
