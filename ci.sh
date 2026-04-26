#!/usr/bin/env bash
#
# ci.sh — single entry-point for the alarm-daemon quality gate.
#
# Runs the same checks a contributor expects to pass before pushing AND that
# any external CI (GitHub Actions / GitLab CI / Yocto autobuilder / …) should
# wrap. Keep this script the source of truth: don't duplicate the steps in a
# CI YAML — call this script from there instead.
#
# Stages, in order:
#
#   1. cargo build           (native, dev profile — fast feedback on type
#                             errors before paying for clippy)
#   2. cargo clippy          (--all-targets --all-features -D warnings)
#   3. cargo test            (workspace, includes the dbus-run-session
#                             integration tests)
#   4. cargo check (cross)   (no-link compile against every supported
#                             production target that ISN'T the build
#                             host, catching portability regressions
#                             before they hit a bitbake build. The host
#                             target is already covered by stage 1.)
#   5. cargo build --release (and report the stripped binary size, so
#                             release-profile budget regressions are
#                             visible in CI logs)
#
# Supported production targets are listed in $PRODUCTION_TARGETS below,
# and mirror `targets = [...]` in rust-toolchain.toml — keep the two in
# sync. rustup installs the rust-std from the toml; ci.sh exercises it
# here.
#
# The cross-compile stage uses `PKG_CONFIG_ALLOW_CROSS=1` because alsa-sys
# refuses cross builds without an explicit sysroot. For a no-link `cargo
# check` the host's libasound headers are sufficient (they're arch-neutral);
# a real Yocto build supplies a proper sysroot via the SDK and this env var
# is unnecessary there.
#
# Usage:
#
#   ./ci.sh                # run all stages
#   SKIP_CROSS=1 ./ci.sh   # skip stage 4 (handy on machines without
#                            cross rust-std components installed)
#   VERBOSE=1 ./ci.sh      # stream cargo output instead of summarising

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# Keep in sync with rust-toolchain.toml :: targets.
PRODUCTION_TARGETS=(
    x86_64-unknown-linux-gnu
    aarch64-unknown-linux-gnu
)

# ----- pretty output ---------------------------------------------------------

if [[ -t 1 ]]; then
    BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'; RED=$'\033[31m'; RESET=$'\033[0m'
else
    BOLD=''; DIM=''; GREEN=''; RED=''; RESET=''
fi

step() { printf '\n%s==> %s%s\n' "$BOLD" "$*" "$RESET"; }
ok()   { printf '%s    ok%s %s\n'   "$GREEN" "$RESET" "$*"; }
fail() { printf '%s    !!%s %s\n'   "$RED"   "$RESET" "$*"; }

# Run a stage, hiding noisy output unless VERBOSE=1 or the stage failed.
run_stage() {
    local label="$1"; shift
    step "$label"
    local started=$SECONDS
    local log; log="$(mktemp)"
    if [[ "${VERBOSE:-0}" == "1" ]]; then
        if "$@" 2>&1 | tee "$log"; then
            ok "$label  (${DIM}$((SECONDS - started))s${RESET})"
        else
            fail "$label  (see output above)"
            exit 1
        fi
    else
        if "$@" >"$log" 2>&1; then
            ok "$label  (${DIM}$((SECONDS - started))s${RESET})"
        else
            fail "$label"
            tail -n 30 "$log"
            exit 1
        fi
    fi
    rm -f "$log"
}

# ----- stages ----------------------------------------------------------------

run_stage "cargo build (native)" \
    cargo build --workspace --all-targets

run_stage "cargo clippy (-D warnings)" \
    cargo clippy --workspace --all-targets --all-features -- -D warnings

run_stage "cargo test (workspace)" \
    cargo test --workspace --no-fail-fast

host_target="$(rustc -vV | awk '/^host:/ {print $2}')"

if [[ "${SKIP_CROSS:-0}" == "1" ]]; then
    step "cargo check (cross)  ${DIM}— skipped (SKIP_CROSS=1)${RESET}"
else
    for target in "${PRODUCTION_TARGETS[@]}"; do
        if [[ "$target" == "$host_target" ]]; then
            step "cargo check (cross: $target)  ${DIM}— native host, already covered by stage 1${RESET}"
            continue
        fi
        # PKG_CONFIG_ALLOW_CROSS=1 lets alsa-sys's build.rs read the host's
        # libasound headers when generating Rust bindings against the cross
        # target. For a no-link `cargo check` this is correct — Yocto /
        # an SDK supplies a proper cross sysroot in production.
        PKG_CONFIG_ALLOW_CROSS=1 run_stage \
            "cargo check (cross: $target)" \
            cargo check --workspace --target "$target" --all-targets
    done
fi

run_stage "cargo build --release (host)" \
    cargo build --release --workspace

# Report the stripped binary size so release-profile regressions show up in
# CI logs as a bisectable diff.
release_bin="target/release/alarm-daemon"
if [[ -x "$release_bin" ]]; then
    size_bytes=$(stat -c '%s' "$release_bin")
    size_h=$(numfmt --to=iec --suffix=B "$size_bytes")
    printf '\n%s==> release artifact%s\n    host = %s\n    %s = %s (%s bytes)\n' \
        "$BOLD" "$RESET" "$host_target" "$release_bin" "$size_h" "$size_bytes"
fi

printf '\n%s==> all stages passed%s\n' "$BOLD$GREEN" "$RESET"
