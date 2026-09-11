# AGENTS.md

Instructions for coding agents working with clipaste — both **installing it for a
user** and **contributing to this repository**.

clipaste is a clipboard daemon that makes screenshot paste work in terminal AI
tools (Claude Code, Codex CLI, Cursor CLI), locally on macOS/Windows and across
SSH/WSL2 boundaries. Graphical Linux hosts provide read-only PNG clipboard
capture for the SSH bridge.

---

## Part 1 — Installing clipaste for a user

### The one command that tells you what to do

```bash
clipaste doctor --json
```

This is the entry point. It classifies the machine, runs the checks that are
meaningful for that machine, and returns `fix` commands or guidance where available.
Do not guess at the state of a clipaste install — ask `doctor`.

```json
{
  "version": "2.4.1",
  "os": "macos",
  "role": "clipboard-host",
  "status": "warn",
  "checks": [
    {
      "name": "daemon",
      "status": "ok",
      "detail": "daemon responding on 127.0.0.1:18340",
      "fix": null
    },
    {
      "name": "clipboard",
      "status": "warn",
      "detail": "no image staged yet — take a screenshot, then re-run doctor",
      "fix": null
    }
  ]
}
```

| Field | Contract |
|---|---|
| `role` | `clipboard-host` / `ssh-remote` / `wsl2` / `unsupported-host` decides which checks apply |
| `status` | worst of all checks: `ok`, `warn`, `fail` |
| `checks[].status` | `ok` / `warn` / `fail` |
| `checks[].fix` | a remediation command or guidance, or `null` when no single command applies |
| exit code | `0` usable (ok **or** warn), `1` broken, `2` bad arguments |

A `warn` is not a failure. "No screenshot on the clipboard yet" is the normal
state of a freshly installed machine — do not report it to the user as a
problem, and do not try to fix it.

Consumers of the JSON output must accept `unsupported-host` for platforms
without a backend. Do not apply it to Linux as a whole: Linux host diagnostics
must distinguish missing tools, missing session access, and missing compositor
data-control from consumer helper or bridge failures.

### Decide where you are before installing anything

clipaste has two sides and they install differently. Getting this wrong is the
single most common mistake.

| OS / context | Local clipboard-host daemon | Consumer of another host's clipboard |
|---|---|---|
| macOS | Supported | SSH remote via `clipaste-paste` |
| Windows | Supported | Via WSL2 |
| Native Linux desktop | Read-only `image/png` via Wayland data-control or X11/XWayland | Supported over SSH via shims / `clipaste-paste` |
| Headless Linux | No display; host startup fails with guidance | Supported with configured helpers / SSH |
| WSL2 | No; the Windows daemon is required | Supported via `wsl-setup` |

```text
Clipboard host                              Consumer
macOS / Windows / graphical Linux
  PNG cache -> loopback HTTP -> SSH tunnel -> SSH remote
  run ssh-setup on this host                 shims / clipaste-paste

Windows -> HTTP over WSL networking -------> WSL2
  Windows daemon                            run wsl-setup inside the distro

Local macOS / Windows: existing clipboard normalization and paste
Local Linux: read-only PNG capture, no added text-path paste
```

`clipaste doctor --json` reports which side you are on as `role`. Trust it over
`uname`: an SSH session into a Mac is `ssh-remote`, not `clipboard-host`.

The role contract applies in this order:

1. WSL context: `wsl2`, even if SSH environment variables are also present.
2. An actual SSH session: `ssh-remote`, including SSH into macOS.
3. A local macOS or Windows machine, or graphical local Linux session:
   `clipboard-host`. Linux backend access still needs validation.
4. A headless Linux machine with a configured consumer helper: `ssh-remote`,
   even without SSH environment variables; keep checking its helper and bridge.
5. Linux without a display or consumer indicators: `clipboard-host` with a
   failing `backend` check and actionable graphical-session guidance. Other
   platforms without a backend or consumer indicators retain `unsupported-host`.

WSL remains a Windows consumer even when WSLg supplies display variables. SSH
session detection takes precedence over a Linux desktop's display variables.
Do not reclassify those consumers as hosts.

### Install on the clipboard host

macOS:

```bash
brew install hqhq1025/clipaste/clipaste
brew services start clipaste
clipaste doctor --json
```

Windows (PowerShell, no admin needed):

```powershell
irm https://raw.githubusercontent.com/hqhq1025/clipaste/main/install.ps1 | iex
clipaste doctor --json
```

Linux desktop (Rust/Cargo required; Ubuntu package example):

```bash
sudo apt install wl-clipboard xclip curl
git clone https://github.com/hqhq1025/clipaste.git
cd clipaste
cargo install --path .
clipaste
```

Ensure Cargo's binary directory (normally `~/.cargo/bin`) is on `PATH`. Start
`clipaste` as the desktop user in a graphical-session terminal and leave it
running. No Linux service or auto-start entry is installed. In a separate
terminal from that same session, run:

```bash
clipaste doctor --json
clipaste ssh-setup user@host
```

For development, `cargo build --release` works from the checkout. Linux retains
`doctor` and consumer setup, including `wsl-setup`; do not add a blanket
`compile_error!` gate that blocks consumer machines.

### Linux backend and verification contract

- `CLIPASTE_BACKEND=auto` is the default. Prefer real system `wl-paste` with
  usable data-control access; otherwise warn and use system `xclip` through an
  accessible `DISPLAY`, or fail with actionable guidance if unavailable.
  HTTP consumer shims do not count as system clipboard tools.
- Native Wayland requires `wl-clipboard >=2.2` for empty-selection watch events.
  All Wayland clipboard accesses use `wl-paste --watch` in bounded one-shot mode,
  which refuses popup fallback and verifies actual compiled-in data-control
  support. Ext-only compositors need `wl-clipboard >=2.3` built with
  `ext-data-control` support; `wlr-data-control` works with compatible 2.2+
  builds. Version 2.1.x triggers the `auto` XWayland fallback or fails without
  `DISPLAY`; a version number alone does not establish protocol support.
- `CLIPASTE_BACKEND=wayland` requires native data-control access and fails
  instead of falling back. `CLIPASTE_BACKEND=x11` requires `xclip` and an
  accessible `DISPLAY`. Use the same override for the daemon and `doctor`.
- Run host diagnostics as the same desktop user and in the same session.
  Wayland needs `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR`; X11/XWayland needs
  `DISPLAY` and display authorization. SSH, `sudo`, stale tmux environments,
  and services can lack that context. Setting a variable alone does not
  establish display access.
- No display produces actionable guidance to start from a graphical desktop or
  configure a consumer. Wayland-only sessions without data-control must fail
  with guidance to use XWayland (`xclip` + `DISPLAY`) or an X11 session. Never
  use focus-stealing windows or arbitrary forced-focus Wayland polling.
- GNOME Wayland may require its XWayland clipboard bridge. Require reporters
  to copy a screenshot from a native Wayland app, re-run `doctor`, then verify
  the image fetched on the remote with `clipaste-paste`. An X11-only test is
  insufficient, and a backend probe is not universal compatibility evidence.
- Linux polls every 300 ms and reads `image/png` without modifying the
  clipboard. It saves to the private stable PNG cache, then serves through
  existing loopback HTTP and SSH shims / `clipaste-paste`. It does not add
  file URLs or text, and does not promise local terminal text-path paste.
  Image file-copy offering only a URI is not supported yet.
- Clipboard clears, non-image content, and recognized private markers clear
  the staged image. Historical cache paths remain; this is not cache erasure.
  Keep cache directories `0700` and files `0600`, with no automatic expiry.
- Preserve existing macOS/Windows clipboard normalization and paste workflows.
  Linux tests do not establish macOS/Windows regression coverage or universal
  desktop compatibility; report exactly which platforms were verified.

### Wire up an SSH remote

Run this on the local macOS, Windows, or graphical Linux clipboard host, not on
the remote consumer. It needs the local daemon running, and it edits the local
`~/.ssh/config`.

```bash
clipaste ssh-setup user@host             # non-interactive; idempotent
clipaste ssh-setup user@host -p 22222    # custom SSH port
```

It detects the remote OS itself and installs the right helpers. Then the user
must open a **new** SSH session — the `RemoteForward` tunnel only exists in
sessions started after the config change. Verify from inside that new session:

```bash
clipaste doctor --json   # role should be "ssh-remote", status "ok"
```

### Wire up WSL2

Run this **inside the WSL2 distro**, with clipaste.exe already running on
Windows:

```bash
clipaste wsl-setup                  # probes for the Windows host address
clipaste wsl-setup --host 127.0.0.1 # skip probing, use this address
```

Both `networkingMode=mirrored` and the default NAT mode are handled. If setup
reports that nothing answered, read its output — it lists every address it tried
and why, and states plainly that NAT-mode WSL cannot reach a loopback-bound
daemon.

### Tell the user the right paste gesture

This differs per tool and per location, and telling a user the wrong one wastes
their time:

| Where | Claude Code / Cursor CLI | Codex CLI |
|---|---|---|
| Local terminal (macOS / Windows) | `Cmd+V` (macOS) / `Ctrl+V` | `Cmd+V` (macOS) / `Ctrl+V` |
| SSH into Linux | `Ctrl+V` | `clipaste-paste`, then paste the printed path |
| SSH into macOS | `clipaste-paste` | `clipaste-paste` |
| WSL2 | `Ctrl+V` | `clipaste-paste`, then paste the printed path |

Codex CLI reads the clipboard in-process (via `arboard`) and never shells out to
`xclip`, so it cannot use the shim. `clipaste-paste` writes the image to a real
file on the current host and prints the path — hand that path to the tool.

The local shortcut row does not apply to Linux hosts. Linux serves clipboard
PNG images to remote consumers without inserting a local text path.

Never tell a user to press `Cmd+V` in an SSH session: that sends the *local*
file path as text, which the remote agent cannot open.

### Things not to do

- Do not install a Linux systemd service automatically or present it as a fix
  for missing desktop access. Follow the reported tool/session/data-control
  guidance. For a consumer, run `ssh-setup` on its clipboard host and reconnect;
  WSL2 still requires the Windows daemon and `wsl-setup` inside the distro.
- Do not classify all Linux machines as hosts or as `unsupported-host`: actual
  SSH sessions and configured headless consumers retain `ssh-remote` diagnostics.
- Do not bind the daemon to `0.0.0.0` or add firewall exceptions to "fix"
  connectivity. It listens on loopback deliberately; the image bytes on that
  port are the user's screen contents.
- Do not write shims by hand. `ssh-setup` / `wsl-setup` generate them with the
  correct URL baked in; a hand-written one drifts silently.
- Do not `kill -9` the daemon to restart it. Use `brew services restart clipaste`
  (macOS), stop `clipaste.exe` normally (Windows), or Ctrl+C in its graphical
  terminal (Linux).
- Do not report success without a passing `clipaste doctor`. "The install
  command exited 0" is not the same as "paste works". On Linux, also verify a
  real screenshot end to end, especially when XWayland fallback is reported.

---

## Part 2 — Contributing to this repository

### Layout

```
src/
├── main.rs        CLI entry + argument parsing (one parser per subcommand)
├── common.rs      version, port, temp-file handling, image conversion, http_get
├── server.rs      HTTP server on 127.0.0.1:18340 (/health, /clipboard/type, /clipboard/image)
├── doctor.rs      environment classification + checks + human/JSON rendering
├── ssh_setup.rs   shim templates, ssh-setup, wsl-setup, host detection
├── linux.rs       read-only PNG polling via system wl-paste / xclip (cfg linux)
├── macos.rs       NSPasteboard watcher (cfg macos)
└── windows.rs     clipboard-format listener (cfg windows)
```

### Build and test

```bash
cargo build
cargo test          # must stay green; no network access required
cargo clippy --all-targets
```

The default `cargo test` suite runs without a daemon, without a network, and
without touching the real `$HOME`. Keep that default isolated; anything that
needs a home directory must take one via an env override, as
`remote_script_run_installs_into_empty_home` does.

Opt-in Linux desktop integration tests use real clipboard tools in isolated
Xvfb and headless Sway sessions, with temporary HOME/cache/runtime directories.
They do not use the real user's clipboard. From the repository root on Linux,
with Rust/Cargo, Clippy, and loopback port `18340` free:

```bash
sudo apt install xvfb xclip sway wl-clipboard dbus-x11 python3-pil curl
bash tests/linux/run.sh
```

The Sway tests require `wl-clipboard >=2.2`; upgrade older distro packages first.
The runner executes unit tests, strict Clippy, and clipboard/HTTP smoke tests.
Alternatively, the existing container recipe builds wl-clipboard 2.3.0; run it
without mounting desktop sockets or the user's home:

```bash
docker build -f tests/linux/Dockerfile -t clipaste-linux-test .
docker run --rm clipaste-linux-test
```

Isolated Xvfb/Sway coverage does not establish compatibility with GNOME,
ext-only compositors, or the reporter's native application.

### Conventions this codebase holds to

- **No new dependencies without a strong reason.** The point of clipaste is that
  it stays lightweight. Linux uses distro clipboard tools with 300 ms polling;
  do not apply macOS/Windows idle measurements to it. HTTP is `curl`; JSON output is
  hand-rolled in `common::json_escape`. If you need serde, justify it.
- **Pure functions for anything with logic.** Parsing, ordering, config
  rewriting, and rendering are all separated from I/O so they can be tested
  directly — see `wsl_host_candidates`, `build_ssh_config`, `render_json`.
- **Generated shell must be syntax-checked in tests.** Every shim and setup
  script is asserted against `bash -n`; a typo there would only ever fail on a
  user's remote host.
- **No silent fallbacks.** When detection fails, list what was tried and why,
  and say what the user should do. A green tick that hides a broken tunnel is
  worse than a red one.
- **Comments explain *why*, not *what*.** Assume the reader can read Rust.

### Releasing

1. Bump `version` in `Cargo.toml` **and** `VERSION` in `src/common.rs` — they
   must match; `doctor` compares the running daemon's reported version against
   the binary's and warns on skew.
2. `cargo test && cargo clippy --all-targets`
3. Tag `vX.Y.Z` and push — `.github/workflows/release.yml` builds macOS
   (aarch64 + x86_64) and Windows artifacts and cuts the GitHub release.
4. Update the Homebrew formula in `hqhq1025/homebrew-clipaste`.

### When fixing a reported issue

Reproduce the failure shape in a test before fixing it. The WSL mirrored-mode
bug (#7) is the model: the fix is a handful of lines, but the tests encode every
networking mode so the next change cannot silently break the others.
