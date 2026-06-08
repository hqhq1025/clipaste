# clipaste

Fix screenshot paste in terminal AI tools — locally, over SSH, and in WSL2.

**clipaste** is a lightweight clipboard daemon for developers who use terminal-based AI coding tools like Claude Code, Codex CLI, and Cursor. Install with one command via Homebrew (macOS) or PowerShell (Windows), and screenshot paste just works — in Ghostty, Alacritty, iTerm2, Kitty, WezTerm, and more. It also bridges your clipboard to remote servers over SSH and to WSL2 environments. Written in Rust, clipaste uses only 9 MB of RAM with 0% CPU overhead.

**Problem:** You take a screenshot, switch to Claude Code / Codex / Cursor in your terminal, press **Ctrl+V** — nothing happens. Or you're SSH'd into a remote server and can't paste screenshots at all.

**Why:** macOS screenshots only put raw image data (TIFF/PNG) on the clipboard. Terminals like Ghostty and Alacritty can only Cmd+V paste text or file URLs — they can't paste raw image data. Over SSH, the remote server has no access to your local clipboard whatsoever.

**Solution:** clipaste is a tiny background daemon (9 MB RAM, 0% CPU) that:

1. **Local paste:** Saves screenshots as temp PNG files and registers the file path on the clipboard, so **Cmd+V** works in terminals. Also adds the legacy PNGf type so **Ctrl+V** image paste works too.

2. **SSH remote paste:** Runs an HTTP server on `localhost:18340`. Use `clipaste ssh-setup` to configure a remote server — it installs an xclip shim and SSH tunnel so **Ctrl+V** in remote Claude Code fetches the image from your local machine.

## Install

### macOS (Homebrew)

```bash
brew install hqhq1025/clipaste/clipaste
brew services start clipaste
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/hqhq1025/clipaste/main/install.ps1 | iex
```

### Build from source

```bash
git clone https://github.com/hqhq1025/clipaste.git
cd clipaste
cargo build --release
```

## SSH Remote Paste

clipaste can bridge your local clipboard to remote servers over SSH. One-time setup:

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
claude   # Ctrl+V pastes screenshots from your local Mac (Linux remote)
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

Take a screenshot (or copy an image file) on your Mac, run `clipaste-paste` on the
remote, and hand the printed path to Codex / Claude Code — both accept an image
file path. This works the same on Linux and macOS remotes.

### How SSH paste works (Claude Code, Linux remote)

```
Local Mac                          Remote Server (via SSH)
─────────                          ──────────────────────
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
| **Local terminal** | **Ctrl+V** | Claude Code reads clipboard image directly |
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

| Terminal | macOS Cmd+V | macOS Ctrl+V | Windows Ctrl+V | SSH Ctrl+V | WSL2 Ctrl+V |
|----------|:-----------:|:------------:|:--------------:|:----------:|:-----------:|
| Ghostty  | ✅          | ✅           | —              | ✅         | —           |
| Alacritty| ✅          | ✅           | —              | ✅         | —           |
| iTerm2   | ✅          | ✅           | —              | ✅         | —           |
| Terminal.app | ✅       | ✅           | —              | ✅         | —           |
| WezTerm  | ✅          | ✅           | ✅             | ✅         | ✅          |
| Kitty    | ✅          | ✅           | ✅             | ✅         | ✅          |
| Windows Terminal | —   | —            | ✅             | —          | ✅          |

| AI Tool | Local | SSH Remote | WSL2 |
|---------|:-----:|:----------:|:----:|
| Claude Code | ✅ | ✅ Ctrl+V | ✅ Ctrl+V |
| Codex CLI   | ✅ | ⚠️ via `clipaste-paste` | ⚠️ via `clipaste-paste` |
| Cursor CLI  | ✅ | ✅ Ctrl+V | ✅ Ctrl+V |

> Codex reads the clipboard in-process and bypasses the xclip shim, so it can't
> paste images natively over SSH/WSL2 — use the `clipaste-paste` helper. macOS
> remotes (any tool) also use `clipaste-paste`. See [SSH Remote Paste](#ssh-remote-paste).

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

## FAQ

### How do I paste screenshots in Claude Code?

Install clipaste with `brew install hqhq1025/clipaste/clipaste && brew services start clipaste` on macOS, or the PowerShell one-liner on Windows. Once running, take a screenshot and press **Ctrl+V** in Claude Code — the image pastes automatically. No configuration needed. clipaste runs as a background daemon and handles the clipboard conversion for you.

### Why can't I paste images in my terminal on macOS?

macOS screenshots place raw TIFF/PNG image data on the clipboard, but terminals like Ghostty and Alacritty can only paste text or file paths. clipaste fixes this by intercepting clipboard changes, saving the image as a temp PNG file, and putting the file path back on the clipboard so your terminal can paste it.

### How do I paste clipboard images over SSH?

Run `clipaste ssh-setup user@your-server` once on your local machine (add `-p PORT`
for a non-default SSH port). It detects the remote OS, installs a lightweight
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

### How much memory and CPU does clipaste use?

clipaste uses approximately 9 MB of RAM and 0% CPU when idle. It is written in Rust and runs as a tiny background daemon. On macOS it is managed via `brew services`; on Windows it auto-starts via a Registry Run key. It has no runtime dependencies beyond the OS clipboard APIs.

### Which terminals and AI tools does clipaste support?

clipaste works with Ghostty, Alacritty, iTerm2, Terminal.app, WezTerm, Kitty, and Windows Terminal. It supports Claude Code, Codex CLI, and Cursor CLI. **Cmd+V** (macOS local) and **Ctrl+V** (local, plus SSH/WSL2 for shim-based tools like Claude Code) are supported; Codex CLI and macOS remotes use the `clipaste-paste` helper. See the compatibility tables above for the full matrix.

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
