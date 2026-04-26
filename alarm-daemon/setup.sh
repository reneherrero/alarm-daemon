#!/usr/bin/env bash
#
# Install / update / uninstall the alarm-daemon as a user-level systemd unit.
# Matches the dev-install layout described in README.md. Production system-bus
# deployment (dedicated user, CAP_WAKE_ALARM, polkit) is not handled here.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BIN_NAME="alarm-daemon"
TARGET_DIR="${CARGO_TARGET_DIR:-${WORKSPACE_DIR}/target}"
BIN_SRC="${TARGET_DIR}/release/${BIN_NAME}"
BIN_DEST="${HOME}/.local/bin/${BIN_NAME}"
SOUND_SRC_DIR="${SCRIPT_DIR}/assets/alarms"
SOUND_DEST_DIR="${HOME}/.local/share/helm/sounds"
CUSTOM_SOUND_DIR="${HOME}/.local/share/helm/custom-sounds"
UNIT_NAME="alarm-daemon.service"
UNIT_DEST="${HOME}/.config/systemd/user/${UNIT_NAME}"

log()  { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<EOF
Usage: $(basename "$0") <command>

Commands:
  install     Build, install the binary + systemd user unit, enable and start.
  update      Rebuild, replace the installed binary, restart the unit.
  uninstall   Stop, disable, and remove the unit and installed binary.
  status      Show the unit's current status.
  -h, --help  Show this help.

Paths:
  binary      ${BIN_DEST}
  unit        ${UNIT_DEST}
  sounds      ${SOUND_DEST_DIR}
EOF
}

require() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

require_systemd_user() {
    systemctl --user show-environment >/dev/null 2>&1 \
        || die "systemd --user instance is not available on this session"
}

has_systemd_user() {
    command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1
}

build() {
    require cargo
    log "building release binary"
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
    [[ -x "${BIN_SRC}" ]] || die "expected binary missing after build: ${BIN_SRC}"
}

install_binary() {
    log "installing binary to ${BIN_DEST}"
    install -D -m 0755 "${BIN_SRC}" "${BIN_DEST}"
    case ":${PATH}:" in
        *":${HOME}/.local/bin:"*) ;;
        *) warn "${HOME}/.local/bin is not on PATH — add it if you want to run '${BIN_NAME}' directly" ;;
    esac
}

install_sounds() {
    if [[ ! -d "${SOUND_SRC_DIR}" ]]; then
        warn "bundled sounds directory missing: ${SOUND_SRC_DIR}"
        return
    fi
    if ! compgen -G "${SOUND_SRC_DIR}/*.wav" >/dev/null; then
        warn "no .wav files found in ${SOUND_SRC_DIR}"
        return
    fi
    log "installing bundled sounds to ${SOUND_DEST_DIR}"
    mkdir -p "${SOUND_DEST_DIR}"
    install -m 0644 "${SOUND_SRC_DIR}"/*.wav "${SOUND_DEST_DIR}/"
    mkdir -p "${CUSTOM_SOUND_DIR}"
}

install_unit() {
    log "writing systemd user unit to ${UNIT_DEST}"
    mkdir -p "$(dirname "${UNIT_DEST}")"
    cat > "${UNIT_DEST}" <<EOF
[Unit]
Description=Helm alarm daemon (dev)
Documentation=file://${SCRIPT_DIR}/README.md
After=dbus.socket

[Service]
Type=simple
Environment=ALARM_DAEMON_BUS=session
Environment=ALARM_DAEMON_SOUND_DIR=${SOUND_DEST_DIR}
Environment=ALARM_DAEMON_CUSTOM_SOUND_DIR=${CUSTOM_SOUND_DIR}
Environment=RUST_LOG=info
ExecStart=${BIN_DEST}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF
}

cmd_install() {
    build
    install_binary
    install_sounds
    if has_systemd_user; then
        install_unit
        log "reloading systemd user daemon"
        systemctl --user daemon-reload
        log "enabling and starting ${UNIT_NAME}"
        systemctl --user enable --now "${UNIT_NAME}"
        log "restarting ${UNIT_NAME} to pick up fresh binary and env"
        systemctl --user restart "${UNIT_NAME}"
        systemctl --user --no-pager status "${UNIT_NAME}" || true
    else
        warn "systemd --user unavailable; installed binary/assets only"
    fi
}

cmd_update() {
    build
    install_binary
    install_sounds
    if has_systemd_user; then
        [[ -f "${UNIT_DEST}" ]] || die "unit not installed — run '$0 install' first"
        log "restarting ${UNIT_NAME}"
        systemctl --user restart "${UNIT_NAME}"
        systemctl --user --no-pager status "${UNIT_NAME}" || true
    else
        warn "systemd --user unavailable; updated binary/assets only"
    fi
}

cmd_uninstall() {
    if has_systemd_user; then
        if systemctl --user list-unit-files "${UNIT_NAME}" >/dev/null 2>&1; then
            log "stopping and disabling ${UNIT_NAME}"
            systemctl --user disable --now "${UNIT_NAME}" 2>/dev/null || true
        fi
        if [[ -f "${UNIT_DEST}" ]]; then
            log "removing ${UNIT_DEST}"
            rm -f "${UNIT_DEST}"
            systemctl --user daemon-reload
        fi
    elif [[ -f "${UNIT_DEST}" ]]; then
        log "removing ${UNIT_DEST}"
        rm -f "${UNIT_DEST}"
    fi
    if [[ -f "${BIN_DEST}" ]]; then
        log "removing ${BIN_DEST}"
        rm -f "${BIN_DEST}"
    fi
    if [[ -d "${SOUND_DEST_DIR}" ]]; then
        log "removing ${SOUND_DEST_DIR}"
        rm -rf "${SOUND_DEST_DIR}"
    fi
    log "uninstall complete"
}

cmd_status() {
    if has_systemd_user; then
        systemctl --user --no-pager status "${UNIT_NAME}"
        return
    fi
    if [[ -x "${BIN_DEST}" ]]; then
        log "binary installed: ${BIN_DEST}"
    else
        warn "binary not installed: ${BIN_DEST}"
    fi
    if [[ -d "${SOUND_DEST_DIR}" ]]; then
        log "sounds installed: ${SOUND_DEST_DIR}"
    else
        warn "sounds not installed: ${SOUND_DEST_DIR}"
    fi
}

case "${1:-}" in
    install)    cmd_install ;;
    update)     cmd_update ;;
    uninstall)  cmd_uninstall ;;
    status)     cmd_status ;;
    -h|--help)  usage ;;
    "")         usage; exit 1 ;;
    *)          usage; die "unknown command: $1" ;;
esac
