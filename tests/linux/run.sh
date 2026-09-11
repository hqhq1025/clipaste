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
if [[ -z "${CLIPASTE_TEST_BINARY:-}" ]]; then
    cargo build --locked
    export CLIPASTE_TEST_BINARY="$PWD/target/debug/clipaste"
fi
export XDG_RUNTIME_DIR="$root/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"

mkfifo "$root/display"
exec 3<>"$root/display"
Xvfb -displayfd 3 -screen 0 1024x768x24 >"$root/xvfb.log" 2>&1 &
xvfb_pid=$!
if ! read -r -t 30 display_number <&3 || [[ ! "$display_number" =~ ^[0-9]+$ ]]; then
    cat "$root/xvfb.log" >&2
    echo "Xvfb did not report a valid display within 30 seconds" >&2
    exit 1
fi
exec 3>&-
export DISPLAY=":$display_number"
/usr/bin/python3 tests/linux/smoke.py x11

unset DISPLAY WAYLAND_DISPLAY
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=pixman
dbus-run-session sway --unsupported-gpu -c /dev/null >"$root/sway.log" 2>&1 &
sway_pid=$!
for i in {1..600}; do
    for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
        if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="${socket##*/}"; break 2; fi
    done
    sleep 0.05
done
if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then cat "$root/sway.log"; exit 1; fi
/usr/bin/python3 tests/linux/smoke.py wayland
