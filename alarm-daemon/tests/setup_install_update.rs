mod common;

#[test]
fn setup_install_and_update() {
    let _env = common::TestEnv::acquire();
    common::require_tools();

    let script = r#"
set -euo pipefail
./setup.sh install
./setup.sh update
test -x "$HOME/.local/bin/alarm-daemon"
test -f "$HOME/.local/share/helm/sounds/collision.wav"
test -f "$HOME/.local/share/helm/sounds/casualty.wav"
test -f "$HOME/.local/share/helm/sounds/klaxon.wav"
test -f "$HOME/.local/share/helm/sounds/surface.wav"
"#;
    assert!(common::run_bash(script), "install/update stage failed");
}
