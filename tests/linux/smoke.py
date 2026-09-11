"""Opt-in tests: run only in an isolated Xvfb/headless Wayland desktop."""

import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from PIL import Image


mode = sys.argv[1]
binary = os.environ["CLIPASTE_TEST_BINARY"]
base = "http://127.0.0.1:18340"
owners = []
owner_logs = []


def get(path):
    with urllib.request.urlopen(base + path, timeout=2) as response:
        return response.status, response.read()


def wait_for(predicate, detail):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.03)
    raise AssertionError(detail)


def copy(data, mime="image/png", sensitive=False):
    if mode == "x11":
        args = ["xclip", "-selection", "clipboard", "-t", mime, "-i", "-quiet"]
    else:
        args = ["wl-copy", "--foreground", "--type", mime]
        if sensitive:
            args.append("--sensitive")
    owner_log = tempfile.TemporaryFile()
    owner_logs.append(owner_log)
    proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                            stderr=owner_log, start_new_session=True)
    owners.append(proc)
    proc.stdin.write(data)
    proc.stdin.close()


def png(value):
    data = io.BytesIO()
    Image.new("RGBA", (37, 29), (value, 23, 42, 255)).save(data, format="PNG")
    return data.getvalue()


def stop(proc):
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
    proc.wait(timeout=5)


with tempfile.TemporaryDirectory(prefix="clipaste-smoke-") as temp:
    env = dict(os.environ, HOME=temp, XDG_CACHE_HOME=temp + "/cache",
               CLIPASTE_BACKEND=mode)
    for key in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "WSL_DISTRO_NAME", "WSL_INTEROP"]:
        env.pop(key, None)
    log = open(temp + "/daemon.log", "w+")
    daemon = None
    try:
        first, second = png(11), png(99)
        copy(first)
        # Confirm the selection exists before testing capture at daemon startup.
        reader = (["xclip", "-selection", "clipboard", "-t", "image/png", "-o"]
                  if mode == "x11" else ["wl-paste", "--no-newline", "--type", "image/png"])
        wait_for(lambda: subprocess.run(reader, stdout=subprocess.PIPE,
                 stderr=subprocess.DEVNULL, timeout=3).stdout == first, "initial selection")
        daemon = subprocess.Popen([binary], env=env, stdout=log, stderr=log,
                                  start_new_session=True)
        wait_for(lambda: get("/clipboard/image") == (200, first), "first PNG not served")
        assert subprocess.check_output(reader) == first, "watcher modified clipboard"
        health = json.loads(get("/health")[1])
        assert health["status"] == "ok"
        doctor = subprocess.run([binary, "doctor", "--json"], env=env,
                                capture_output=True, timeout=15)
        assert doctor.returncode == 0, doctor.stdout
        report = json.loads(doctor.stdout)
        assert report["role"] == "clipboard-host"
        assert any(c["name"] == "backend" and c["status"] == "ok" for c in report["checks"])

        # Use the shipped installer and consumer helper, with a second isolated
        # HOME. Transport is container loopback here, not a claimed SSH test.
        consumer_home = Path(temp) / "consumer"
        consumer_home.mkdir()
        consumer_env = dict(env, HOME=str(consumer_home), WSL_DISTRO_NAME="Test",
                            TMPDIR=str(consumer_home))
        setup = subprocess.run([binary, "wsl-setup", "--host", "127.0.0.1"],
                               env=consumer_env, capture_output=True, timeout=15)
        assert setup.returncode == 0, setup.stderr
        helper = consumer_home / ".local/bin/clipaste-paste"
        fetched = subprocess.check_output([str(helper)], env=consumer_env, timeout=10)
        assert Path(fetched.decode().strip()).read_bytes() == first
        # The host must bypass these generated HTTP shims even if they precede
        # system tools on PATH, or the next copy would read its own cached PNG.
        stop(daemon)
        env["PATH"] = str(consumer_home / ".local/bin") + ":" + env["PATH"]
        daemon = subprocess.Popen([binary], env=env, stdout=log, stderr=log,
                                  start_new_session=True)
        wait_for(lambda: get("/clipboard/image") == (200, first), "restart with shims")

        duplicate = subprocess.run([binary], env=env, capture_output=True, timeout=10)
        assert duplicate.returncode == 1
        assert b"cannot start HTTP server" in duplicate.stderr
        cache = Path(temp + "/cache/clipaste")
        wait_for(lambda: len(list(cache.glob("*.png"))) == 1, "cache did not deduplicate")
        original_path = next(cache.glob("*.png"))
        original_time = original_path.stat().st_mtime_ns
        copy(second)
        wait_for(lambda: get("/clipboard/image") == (200, second), "replacement PNG not served")
        copy(b"plain text", "text/plain")
        wait_for(lambda: get("/clipboard/image")[0] == 204, "text left stale image")
        assert json.loads(get("/clipboard/type")[1])["type"] == "empty"
        assert original_path.read_bytes() == first, "historical image was deleted"
        copy(first)
        wait_for(lambda: get("/clipboard/image") == (200, first), "recopy not captured")
        assert original_path.stat().st_mtime_ns == original_time
        assert len(list(cache.glob("*.png"))) == 2
        assert (cache.stat().st_mode & 0o777) == 0o700
        assert all(p.stat().st_mode & 0o777 == 0o600 for p in cache.glob("*.png"))
        original_path.unlink()
        wait_for(lambda: original_path.exists() and get("/clipboard/image") == (200, first),
                 "unchanged clipboard did not restore deleted cache")

        copy(b"not actually a PNG")
        wait_for(lambda: get("/clipboard/image")[0] == 204, "corrupt image left stale PNG")
        # Hold bad contents beyond the former three-poll fatal-error threshold.
        time.sleep(2)
        assert daemon.poll() is None, "invalid content killed daemon"
        copy(second)
        wait_for(lambda: get("/clipboard/image") == (200, second), "read did not recover")

        if mode == "wayland":
            help_text = subprocess.check_output(["wl-copy", "--help"], stderr=subprocess.STDOUT)
            if b"--sensitive" in help_text:
                copy(second, sensitive=True)
                wait_for(lambda: get("/clipboard/image")[0] == 204, "sensitive image was staged")
            else:
                print("wayland: wl-copy lacks --sensitive; marker filtering covered by unit tests")
            subprocess.run(["wl-copy", "--clear"], check=True)
            wait_for(lambda: get("/clipboard/image")[0] == 204, "clear left stale image")
            time.sleep(2)
            assert daemon.poll() is None, "empty clipboard killed daemon"
            stop(daemon)
            daemon = subprocess.Popen([binary], env=env, stdout=log, stderr=log,
                                      start_new_session=True)
            wait_for(lambda: get("/health")[0] == 200, "empty clipboard startup failed")
            copy(first)
            wait_for(lambda: get("/clipboard/image") == (200, first), "empty startup recovery")
        print(f"{mode}: startup, HTTP, doctor, port conflict, replacement, text clearing, "
              "deduplication, cache permissions, consumer helper, shim bypass "
              "and clipboard preservation passed")
    except BaseException:
        log.flush()
        log.seek(0)
        print(log.read(), file=sys.stderr)
        for owner, owner_log in zip(owners, owner_logs):
            owner_log.seek(0)
            print(f"clipboard owner {owner.args}, status={owner.poll()}: "
                  f"{owner_log.read().decode(errors='replace')}", file=sys.stderr)
        raise
    finally:
        if daemon is not None:
            stop(daemon)
        for owner in owners:
            stop(owner)
        for owner_log in owner_logs:
            owner_log.close()
        log.close()
