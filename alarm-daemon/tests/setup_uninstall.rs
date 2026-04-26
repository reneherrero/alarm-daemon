mod common;

#[test]
fn setup_uninstall_cleans_artifacts() {
    let _env = common::TestEnv::acquire();
    common::require_tools();

    let script = r#"
set -euo pipefail
./setup.sh install
./setup.sh uninstall
test ! -f "$HOME/.local/bin/alarm-daemon"
test ! -d "$HOME/.local/share/helm/sounds"
"#;
    assert!(common::run_bash(script), "uninstall cleanup stage failed");
}
