#!/usr/bin/env bash
set -euo pipefail

root=$(mktemp -d)
cleanup() {
    if [[ -n "${sway_pid:-}" ]]; then kill "$sway_pid" 2>/dev/null || true; fi
    if [[ -n "${xvfb_pid:-}" ]]; then kill "$xvfb_pid" 2>/dev/null || true; fi
    rm -rf "$root"
}
trap cleanup EXIT

cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
export CLIPASTE_TEST_BINARY="$PWD/target/debug/clipaste"
export XDG_RUNTIME_DIR="$root/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"

Xvfb -displayfd 3 -screen 0 1024x768x24 3>"$root/display" >"$root/xvfb.log" 2>&1 &
xvfb_pid=$!
for i in {1..100}; do
    [[ -s "$root/display" ]] && break
    sleep 0.05
done
export DISPLAY=":$(cat "$root/display")"
/usr/bin/python3 tests/linux/smoke.py x11

unset DISPLAY
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=pixman
dbus-run-session sway --unsupported-gpu -c /dev/null >"$root/sway.log" 2>&1 &
sway_pid=$!
for i in {1..100}; do
    for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
        if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="${socket##*/}"; break 2; fi
    done
    sleep 0.05
done
if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then cat "$root/sway.log"; exit 1; fi
/usr/bin/python3 tests/linux/smoke.py wayland
