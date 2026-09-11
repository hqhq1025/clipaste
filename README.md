# clipaste

Fix screenshot paste in terminal AI tools — locally, over SSH, and in WSL2.

**[hqhq1025.github.io/clipaste](https://hqhq1025.github.io/clipaste/)** · [AGENTS.md](AGENTS.md) · [Issues](https://github.com/hqhq1025/clipaste/issues)

**clipaste** is a lightweight Rust clipboard daemon for developers who use terminal-based AI coding tools like Claude Code, Codex CLI, and Cursor. It fixes local screenshot paste on macOS and Windows, bridges their clipboards to remote servers over SSH, and connects Windows to WSL2. Graphical Linux hosts can also serve clipboard PNG images over SSH using the read-only backend described below.

**Problem:** You take a screenshot, switch to Claude Code / Codex / Cursor in your terminal, press **Ctrl+V** — nothing happens. Or you're SSH'd into a remote server and can't paste screenshots at all.

**Why:** macOS screenshots only put raw image data (TIFF/PNG) on the clipboard. Terminals like Ghostty and Alacritty can only Cmd+V paste text or file URLs — they can't paste raw image data. Over SSH, the remote server has no access to your local clipboard whatsoever.

**Solution:** clipaste is a background daemon that:

1. **Local paste on macOS / Windows:** Saves screenshots as PNG files and adds a path alongside the image. On macOS, this enables **Cmd+V** in terminals and adds the legacy PNGf type for **Ctrl+V** image paste. Linux reads the clipboard without adding a path or changing its formats.

2. **SSH remote paste:** Runs an HTTP server on `localhost:18340`. Use `clipaste ssh-setup` to configure a remote server — it installs an xclip shim and SSH tunnel so **Ctrl+V** in remote Claude Code fetches the image from your local machine.

## Install

### Supported platforms

The clipboard-host daemon runs on macOS, Windows, and graphical Linux sessions
with a usable clipboard backend. Linux hosting is read-only and accepts
`image/png`; desktop/compositor compatibility must be verified in the user's session.

| OS / context | Local clipboard-host daemon | Consumer of another host's clipboard |
|---|---|---|
| macOS | Supported | SSH remote via `clipaste-paste` |
| Windows | Supported | Via WSL2, as described below |
| Native Linux desktop | PNG hosting via Wayland data-control or X11/XWayland; see requirements below | Supported over SSH via shims / `clipaste-paste` |
| Headless Linux | No desktop clipboard; host startup fails with guidance | Supported with configured helpers / SSH |
| WSL2 | No; the Windows daemon is required | Supported via `wsl-setup` |

Linux consumer shims fetch images from an existing macOS, Windows, or graphical
Linux host daemon. The Linux host uses real system clipboard tools, never these
HTTP shims. Run `ssh-setup` on the clipboard host with its daemon running.
On macOS remotes, use `clipaste-paste`.

### Clipboard history and cache

Starting with v2.4.2, macOS normalization preserves source formats and marks
modified copies so compatible history managers, including Maccy 2.6.1, can
replace the original entry. Identical PNG bytes reuse a stable cache path.
Cached images no longer expire automatically after one hour, so saved history
paths remain usable. Storage grows with unique images; deleting cached files
invalidates history entries that reference those paths.
See [clipboard history compatibility](docs/clipboard-history.md) for details.

Linux also uses the private stable PNG cache (`~/.cache/clipaste`, directory
mode `0700`, files `0600`). Clearing the clipboard, copying non-image content,
or encountering a recognized private marker clears the staged image served over
HTTP. Historical cached paths are retained; clearing the clipboard does not erase them.

### macOS (Homebrew)

```bash
brew install hqhq1025/clipaste/clipaste
brew services start clipaste
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/hqhq1025/clipaste/main/install.ps1 | iex
```

### Linux desktop (from source)

Install Rust/Cargo first. On Ubuntu, install the distro clipboard tools and
`curl`, then build and install from the source checkout. These Linux instructions
describe source builds, not a tagged Linux binary release:

```bash
sudo apt install wl-clipboard xclip curl
git clone https://github.com/hqhq1025/clipaste.git
cd clipaste
cargo install --path .
clipaste
```

Run `clipaste` as your desktop user in a terminal opened from the graphical
session, and leave it running. Ensure Cargo's binary directory (`~/.cargo/bin`
by default) is on `PATH`. No Linux service or automatic startup is installed.
In a separate terminal in the same desktop session, verify and configure SSH:

```bash
clipaste doctor --json
clipaste ssh-setup user@your-server
```

Take a screenshot copied as image data, re-run `doctor`, then open a new SSH
session and test `clipaste-paste` or the supported remote tool's paste gesture.
An empty clipboard warning before taking the screenshot is normal.

| `CLIPASTE_BACKEND` | Selection and requirements |
|---|---|
| `auto` (default) | Prefer native Wayland when data-control is available; otherwise use system `xclip` through `DISPLAY`, warning when falling back from Wayland to XWayland |
| `wayland` | Require real system `wl-paste` with usable data-control access as described below; fail instead of falling back |
| `x11` | Require real system `xclip` and a usable `DISPLAY`, using X11 or the compositor's XWayland clipboard bridge |

For example, `CLIPASTE_BACKEND=x11 clipaste` selects X11 explicitly. Use the
same override for diagnostics: `CLIPASTE_BACKEND=x11 clipaste doctor --json`.
Native Wayland requires `wl-clipboard >=2.2` for empty-selection watch events.
All Wayland clipboard accesses use `wl-paste --watch` in bounded one-shot mode,
which refuses popup fallback and verifies actual compiled-in data-control
support. Ext-only compositors need `wl-clipboard >=2.3` built with
`ext-data-control` support; `wlr-data-control` works with compatible 2.2+ builds.
With 2.1.x or unavailable data-control, `auto` warns and falls back to XWayland
through system `xclip` and an accessible `DISPLAY`; without that, it fails with
guidance to upgrade or use an X11 session. No forced-focus polling is used.

GNOME Wayland may need the XWayland clipboard bridge. Its behavior depends on
the compositor and source application: reporters must test a screenshot copied
from a native Wayland app and verify the image received on the remote. An
X11-only copy test does not establish native Wayland compatibility. Universal
desktop/compositor compatibility has not been tested.

Linux polls every 300 ms and reads only `image/png`. The pipeline is clipboard
PNG -> private stable PNG cache -> existing loopback HTTP server -> SSH shims /
`clipaste-paste`. It does not write file URLs or text back to the Linux clipboard,
so local terminal text-path paste is not promised. Copying an image file in a
file manager that offers only a URI is not supported yet; copy the image pixels
or a screenshot instead. Existing macOS and Windows paste workflows remain unchanged.

### Build from source

For development on any supported platform, use `cargo build --release` from the
checkout. Linux builds also retain `doctor` and consumer setup commands,
including `wsl-setup`; WSL2 remains a consumer of the Windows daemon.

For opt-in verification with real clipboard tools, run `bash tests/linux/run.sh`
from the repository root on Linux. It starts isolated Xvfb and headless Sway
sessions and uses temporary test state, not your real desktop clipboard.
See [test prerequisites and container recipe](AGENTS.md#build-and-test).
This does not replace the native-app screenshot check for your own compositor.

## SSH Remote Paste

clipaste can bridge your local clipboard to remote servers over SSH. Run this
one-time setup on your local macOS, Windows, or graphical Linux clipboard host,
with its daemon running, not on the remote consumer:

```bash
clipaste ssh-setup user@your-server
clipaste ssh-setup user@your-server -p 22222   # custom SSH port
```

This automatically:
- Detects the remote OS (`uname -s`) and installs the right helpers
- On a Linux remote: installs an xclip/wl-paste shim (`~/.local/bin/`)
- Installs a universal `clipaste-paste` command on every remote
- Adds `RemoteForward 18340` (and `Port` if you passed `-p`) to your `~/.ssh/config`
- No extra tools needed on the remote server (just `curl`)

After setup, open a **new** SSH session:

```bash
ssh user@your-server
claude   # Ctrl+V pastes screenshots from your clipboard host (Linux remote)
codex    # run `clipaste-paste`, then paste the printed path (see below)
```

### Pasting in Codex CLI / on a macOS remote

Codex CLI reads the clipboard **in-process** (via X11/NSPasteboard) and bypasses
the xclip shim, so it can't paste images natively over SSH. macOS remotes have
the same gap (the tool reads the remote Mac's own, empty clipboard). For both,
use the `clipaste-paste` helper that `ssh-setup` installs:

```bash
clipaste-paste            # → /tmp/clipaste-<ts>.png  (a real file on the remote)
```

Copy a screenshot on the clipboard host, run `clipaste-paste` on the remote,
and hand the printed path to Codex / Claude Code. On macOS, copying an image
file also works; Linux currently requires clipboard `image/png`, not a file URI.
The helper works on both Linux and macOS remotes.

### How SSH paste works (Claude Code, Linux remote)

```
Clipboard host                     Remote Server (via SSH)
──────────────                     ──────────────────────
Screenshot                         Claude Code runs "xclip"
    │                                      │
    ▼                                      ▼
clipaste saves PNG              xclip shim intercepts call
    │                                      │
    ▼                                      ▼
HTTP server ◄──── SSH RemoteForward ────► curl localhost:18340
(:18340)           (tunnel)                    │
    │                                          ▼
    └──── serves PNG ─────────────────► Image delivered ✅
```

## WSL2 Paste

If you run Claude Code / Codex inside WSL2, clipaste bridges the Windows clipboard to WSL2. Run this **inside WSL2**:

```bash
clipaste wsl-setup
```

This installs the same xclip shim, pointed at clipaste.exe running on your Windows host. No SSH tunnel needed — WSL2 connects directly.

**Prerequisites:** clipaste.exe must be running on the Windows side (installed via the PowerShell one-liner above).

### WSL2 networking modes

`wsl-setup` finds the Windows host by probing candidate addresses and keeping the
first one that actually answers `/health` — so both WSL2 networking modes work:

| `networkingMode` | Windows host is reachable at | Notes |
|---|---|---|
| `mirrored` | `127.0.0.1` | WSL shares the host's interfaces. With `dnsTunneling=true` the `/etc/resolv.conf` nameserver is a virtual DNS endpoint (`10.255.255.254`), **not** the host — probing loopback is what makes this work. |
| `nat` (default) | the vEthernet gateway | Usually the `/etc/resolv.conf` nameserver; read from the routing table when DNS tunneling replaces it. |

If auto-detection picks nothing, pass the address yourself:

```bash
clipaste wsl-setup --host 127.0.0.1
```

> **NAT mode on older Windows:** clipaste binds to `127.0.0.1` on the Windows
> side, which NAT-mode WSL cannot reach. Prefer `networkingMode=mirrored` in
> `%USERPROFILE%\.wslconfig` (Windows 11 22H2+). If mirrored mode is unavailable,
> forward the port on the Windows host from an elevated PowerShell:
> ```powershell
> netsh interface portproxy add v4tov4 listenport=18340 `
>   listenaddress=(Get-NetIPAddress -InterfaceAlias 'vEthernet (WSL*)' -AddressFamily IPv4).IPAddress `
>   connectport=18340 connectaddress=127.0.0.1
> ```
> then run `clipaste wsl-setup` again.

```
Windows Host                       WSL2
────────────                       ────
Win+Shift+S screenshot             Claude Code runs "xclip"
    │                                      │
    ▼                                      ▼
clipaste.exe saves PNG          xclip shim intercepts call
    │                                      │
    ▼                                      ▼
HTTP server ◄──── WSL2 network ────────► curl $WIN_HOST:18340
(:18340)        (direct, no tunnel)        │
    │                                      ▼
    └──── serves PNG ──────────────► Image delivered ✅
```

## Paste shortcuts

| Scenario | Shortcut | How it works |
|----------|----------|-------------|
| **Local terminal (macOS)** | **Cmd+V** | Ghostty/iTerm2 paste file path → tool reads file |
| **Local terminal (macOS / Windows)** | **Ctrl+V** | Claude Code reads clipboard image directly |
| **SSH remote — Claude Code (Linux)** | **Ctrl+V** | xclip shim → HTTP tunnel → local PNG |
| **SSH remote — Codex / macOS remote** | `clipaste-paste` | helper fetches PNG → paste the printed path |
| **WSL2 — Claude Code** | **Ctrl+V** | xclip shim → HTTP → Windows host PNG |
| **WSL2 — Codex** | `clipaste-paste` | helper fetches PNG → paste the printed path |

**Tip:** On a Linux remote, Claude Code pastes with Ctrl+V. Codex CLI and macOS
remotes use the `clipaste-paste` helper instead (Codex bypasses the xclip shim).

> **Important:** In an SSH session with Claude Code, **use Ctrl+V**, never Cmd+V —
> Cmd+V pastes the local Mac path as text, which the remote agent cannot read.
> Ctrl+V triggers the xclip shim, which fetches the image through the SSH tunnel.
> For **Codex CLI** (which doesn't use the shim) or a **macOS remote**, run
> `clipaste-paste` and hand the printed path to the agent.

## Compatibility

Local paste shortcuts below apply to macOS and Windows. Linux hosting supplies
PNG images to the SSH bridge without adding local text-path paste. SSH Ctrl+V
means a Linux consumer using the shims, backed by any supported clipboard host;
macOS remotes use `clipaste-paste`. WSL2 always requires the Windows daemon.
The Linux backend does not change the existing macOS/Windows workflows, and
these tables do not certify every Linux desktop, compositor, or application.

| Terminal | macOS Cmd+V | macOS Ctrl+V | Windows Ctrl+V | SSH Ctrl+V | WSL2 Ctrl+V |
|----------|:-----------:|:------------:|:--------------:|:----------:|:-----------:|
| Ghostty  | ✅          | ✅           | —              | ✅         | —           |
| Alacritty| ✅          | ✅           | —              | ✅         | —           |
| iTerm2   | ✅          | ✅           | —              | ✅         | —           |
| Terminal.app | ✅       | ✅           | —              | ✅         | —           |
| WezTerm  | ✅          | ✅           | ✅             | ✅         | ✅          |
| Kitty    | ✅          | ✅           | ✅             | ✅         | ✅          |
| Windows Terminal | —   | —            | ✅             | —          | ✅          |

| AI Tool | Local macOS / Windows | SSH Remote | WSL2 |
|---------|:-----:|:----------:|:----:|
| Claude Code | ✅ | ✅ Ctrl+V | ✅ Ctrl+V |
| Codex CLI   | ✅ | ⚠️ via `clipaste-paste` | ⚠️ via `clipaste-paste` |
| Cursor CLI  | ✅ | ✅ Ctrl+V | ✅ Ctrl+V |

> Codex reads the clipboard in-process and bypasses the xclip shim, so it can't
> paste images natively over SSH/WSL2 — use the `clipaste-paste` helper. macOS
> remotes (any tool) also use `clipaste-paste`. See [SSH Remote Paste](#ssh-remote-paste).

## For coding agents

clipaste is built to be installed and repaired by an agent, not just by a human
reading docs. Point your agent at [AGENTS.md](AGENTS.md), or give it one command:

```bash
clipaste doctor --json
```

The role contract is `clipboard-host` / `ssh-remote` / `wsl2` / `unsupported-host`.
It selects the applicable checks and returns remediation commands or guidance in
`fix` where available:

```json
{
  "version": "2.4.1",
  "os": "linux",
  "role": "ssh-remote",
  "status": "fail",
  "checks": [
    { "name": "helper", "status": "ok",   "detail": "~/.local/bin/clipaste-paste", "fix": null },
    { "name": "bridge", "status": "fail", "detail": "http://127.0.0.1:18340 is not answering",
      "fix": "reconnect: the SSH RemoteForward is only active inside a session opened after ssh-setup" }
  ]
}
```

Exit code is `0` when usable (including warnings), `1` when broken, `2` on bad
arguments. Every setup command is non-interactive and idempotent, so an agent
can run them unattended.

WSL detection takes precedence over SSH and remains `wsl2`, even with graphical
environment variables. Actual SSH sessions remain `ssh-remote`, including SSH
into macOS or a Linux desktop. A graphical local Linux session is a
`clipboard-host`; a headless Linux machine with a configured consumer helper
remains `ssh-remote` even without SSH variables. Headless Linux with no consumer
indicators gets `clipboard-host` diagnostics with a failing `backend` check.
`unsupported-host` remains in the contract for platforms without a backend;
it is not a blanket Linux result.

Run Linux host diagnostics as the same desktop user, from the same graphical
session as the daemon, with the same `CLIPASTE_BACKEND` override. Wayland needs
the session's `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR`; X11/XWayland needs `DISPLAY`
and authorization to that display. An SSH shell, `sudo`, a stale tmux session,
or a service missing that environment may produce different diagnostics.
Setting a display variable alone does not create or authorize a desktop session.
With no display, host startup fails with guidance to run from the desktop or
configure a consumer. Missing tools and missing data-control need their stated
remedies; installing `curl` or a systemd service does not grant clipboard access.

## Managing

### macOS

```bash
brew services info clipaste      # status
brew services restart clipaste   # restart
brew services stop clipaste      # stop
```

### Windows

```powershell
taskkill /IM clipaste.exe /F                      # stop
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "clipaste"  # disable auto-start
```

### Linux

Keep `clipaste` running in its graphical-session terminal; stop it with Ctrl+C.
Restart it there after changing `CLIPASTE_BACKEND`. No Linux service is installed
automatically. Run `clipaste doctor --json` in a separate desktop terminal.

## FAQ

### How do I paste screenshots in Claude Code?

Install clipaste with `brew install hqhq1025/clipaste/clipaste && brew services start clipaste` on macOS, or the PowerShell one-liner on Windows. Once running, take a screenshot and press **Ctrl+V** in Claude Code — the image pastes automatically. No configuration needed. clipaste runs as a background daemon and handles the clipboard conversion for you.

For a Linux clipboard host, follow [Linux desktop setup](#linux-desktop-from-source)
and the SSH workflow. The read-only backend does not add local text-path paste.

### Why can't I paste images in my terminal on macOS?

macOS screenshots place raw TIFF/PNG image data on the clipboard, but terminals like Ghostty and Alacritty can only paste text or file paths. clipaste fixes this by intercepting clipboard changes, saving the image as a temp PNG file, and putting the file path back on the clipboard so your terminal can paste it.

### How do I paste clipboard images over SSH?

Run `clipaste ssh-setup user@your-server` once on your local macOS, Windows, or
graphical Linux clipboard host with its daemon running (add `-p PORT` for a
non-default SSH port). It detects the remote OS, installs a lightweight
xclip shim (Linux) plus a universal `clipaste-paste` helper, and configures an SSH
tunnel. After setup, open a new SSH session:

- **Claude Code on a Linux remote:** press **Ctrl+V** — the image is fetched
  through the tunnel automatically.
- **Codex CLI, or any tool on a macOS remote:** run `clipaste-paste` and hand the
  printed path to the agent. Codex reads the clipboard in-process and bypasses the
  xclip shim, so it cannot paste natively over SSH; the helper is the working path.

### Does clipaste work with WSL2?

Yes. Run `clipaste wsl-setup` inside your WSL2 environment. This installs an xclip
shim (used by Claude Code) plus the `clipaste-paste` helper (used by Codex),
connecting directly to clipaste.exe on the Windows host — no SSH tunnel needed.
After setup, **Ctrl+V** in Claude Code fetches screenshots from the Windows
clipboard; for Codex, run `clipaste-paste` and paste the printed path.
Both `networkingMode=mirrored` and the default NAT mode are detected
automatically — see [WSL2 networking modes](#wsl2-networking-modes).

### How much memory and CPU does clipaste use?

The existing macOS/Windows footprint is approximately 9 MB of RAM with no
measurable idle CPU. That is not a Linux measurement: Linux polls every 300 ms
and invokes system clipboard tools. macOS uses `brew services`; Windows uses a
Registry Run key. Linux runs from the graphical session and requires the distro
tools listed above.

### Which terminals and AI tools does clipaste support?

clipaste works with Ghostty, Alacritty, iTerm2, Terminal.app, WezTerm, Kitty, and Windows Terminal. It supports Claude Code, Codex CLI, and Cursor CLI. **Cmd+V** (macOS local) and **Ctrl+V** (local, plus SSH/WSL2 for shim-based tools like Claude Code) are supported; Codex CLI and macOS remotes use the `clipaste-paste` helper. See the compatibility tables above for the full matrix.

Local shortcuts here refer to macOS/Windows. Linux host compatibility depends
on clipboard access and the source app; verify it with a real screenshot.

## How is this different from...

- **[cc-clip](https://github.com/ShunmeiCho/cc-clip)** — SSH clipboard bridge only. clipaste handles both local paste fix AND SSH bridge in one tool, with no dependencies on the remote server (just `curl`).
- **[shotpath](https://hboon.com/shotpath-automatically-copy-macos-screenshot-paths/)** — Monitors screenshot *files* on disk. clipaste works with clipboard screenshots (no file saved to Desktop).
- **[impaste](https://til.simonwillison.net/macos/impaste)** — A pipe-based tool (`impaste | pbcopy`). clipaste is fully automatic, no manual step needed.
- **[pngpaste](https://github.com/jcsalterego/pngpaste)** — Extracts clipboard images to files. clipaste does the reverse: it makes clipboard images available *as* files for terminals.

## Related issues

This fixes a long-standing pain point across multiple projects:

**Local paste (macOS/Windows):**
- [anthropics/claude-code#2102](https://github.com/anthropics/claude-code/issues/2102) — Clipboard Image Parsing Failure on macOS
- [anthropics/claude-code#17042](https://github.com/anthropics/claude-code/issues/17042) — Ctrl+V clipboard paste fails on macOS
- [anthropics/claude-code#26901](https://github.com/anthropics/claude-code/issues/26901) — Image paste from clipboard no longer works
- [openai/codex#6080](https://github.com/openai/codex/issues/6080) — Image pasting issue
- [ghostty-org/ghostty#10478](https://github.com/ghostty-org/ghostty/discussions/10478) — Support pasting screenshot images

**SSH remote paste:**
- [anthropics/claude-code#5277](https://github.com/anthropics/claude-code/issues/5277) — Image paste in SSH/SFTP
- [anthropics/claude-code#13738](https://github.com/anthropics/claude-code/issues/13738) — Clipboard image paste not working in WSL
- [anthropics/claude-code#8324](https://github.com/anthropics/claude-code/issues/8324) — Can't paste image from clipboard on Linux

## Community

- [LINUX DO](https://linux.do) — Where we first shared this project

## License

MIT
