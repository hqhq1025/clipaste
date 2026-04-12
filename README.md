# clipaste

Fix macOS screenshot paste in terminal AI tools.

**Problem:** You take a screenshot, switch to Claude Code / Codex / Cursor in your terminal, press **Cmd+V** — nothing happens. But if you copy the same image from another app first, it works fine.

**Why:** macOS screenshots only put raw image data (TIFF/PNG) on the clipboard. Terminals like Ghostty and Alacritty can only Cmd+V paste text or file URLs — they can't paste raw image data. Additionally, tools like Claude Code check for the legacy `«class PNGf»` pasteboard type, which screenshots don't provide.

**Solution:** clipaste is a tiny background daemon (~18 MB RAM, 0% CPU) that watches your clipboard. When it detects a screenshot (image data without a file URL), it:

1. Saves the image as a temp PNG file
2. Registers the file path on the clipboard (so **Cmd+V** pastes the path)
3. Adds the legacy PNGf type (so **Ctrl+V** image paste also works)

Your workflow becomes: **Screenshot → Cmd+V → Done.**

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

This downloads the latest release, installs to `%LOCALAPPDATA%\clipaste\`, adds to PATH, and sets auto-start via Registry. No admin required.

### Build from source

Requires Rust toolchain.

```bash
git clone https://github.com/hqhq1025/clipaste.git
cd clipaste
cargo build --release
# Binary at target/release/clipaste (or clipaste.exe on Windows)
```

That's it. clipaste runs in the background and starts automatically on login.

## How it works

```
┌─────────────┐    ┌──────────┐    ┌──────────────────────────┐
│  Screenshot  │───▶│ Clipboard│───▶│ clipaste detects image   │
│  Cmd+Shift+4 │    │ (TIFF)   │    │ without file URL         │
└─────────────┘    └──────────┘    └────────────┬─────────────┘
                                                 │
                                                 ▼
                                   ┌──────────────────────────┐
                                   │ Save temp PNG + register │
                                   │ file URL on clipboard    │
                                   └────────────┬─────────────┘
                                                 │
                        ┌────────────────────────┼──────────────────────┐
                        ▼                        ▼                      ▼
               ┌──────────────┐        ┌──────────────┐       ┌──────────────┐
               │   Cmd+V      │        │   Ctrl+V     │       │  Other apps  │
               │ pastes path  │        │ pastes image │       │ paste image  │
               │ (terminals)  │        │ (AI tools)   │       │  (normal)    │
               └──────────────┘        └──────────────┘       └──────────────┘
```

## Compatibility

| Terminal | macOS Cmd+V | macOS Ctrl+V | Windows Ctrl+V |
|----------|:-----------:|:------------:|:--------------:|
| Ghostty  | ✅          | ✅           | —              |
| Alacritty| ✅          | ✅           | —              |
| iTerm2   | ✅          | ✅           | —              |
| Terminal.app | ✅       | ✅           | —              |
| WezTerm  | ✅          | ✅           | ✅             |
| Kitty    | ✅          | ✅           | ✅             |
| Windows Terminal | —   | —            | ✅             |

| AI Tool | Status |
|---------|:------:|
| Claude Code | ✅ |
| Codex CLI   | ✅ |
| Cursor CLI  | ✅ |

## Managing

### macOS

```bash
brew services info clipaste      # status
brew services restart clipaste   # restart
brew services stop clipaste      # stop
```

### Windows

```powershell
# Stop
taskkill /IM clipaste.exe /F

# Disable auto-start
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "clipaste"

# Uninstall
Remove-Item -Recurse "$env:LOCALAPPDATA\clipaste"
```

## How is this different from...

- **[shotpath](https://hboon.com/shotpath-automatically-copy-macos-screenshot-paths/)** — Monitors screenshot *files* on disk. clipaste works with clipboard screenshots (no file saved).
- **[impaste](https://til.simonwillison.net/macos/impaste)** — A pipe-based tool (`impaste | pbcopy`). clipaste is fully automatic, no manual step needed.
- **[pngpaste](https://github.com/jcsalterego/pngpaste)** — Extracts clipboard images to files. clipaste does the reverse: it makes clipboard images available *as* files for terminals.

## Related issues

This fixes a long-standing pain point across multiple projects:

- [anthropics/claude-code#2102](https://github.com/anthropics/claude-code/issues/2102) — Clipboard Image Parsing Failure on macOS
- [anthropics/claude-code#17042](https://github.com/anthropics/claude-code/issues/17042) — Ctrl+V clipboard paste fails on macOS
- [anthropics/claude-code#26901](https://github.com/anthropics/claude-code/issues/26901) — Image paste from clipboard no longer works
- [openai/codex#6080](https://github.com/openai/codex/issues/6080) — Image pasting issue
- [ghostty-org/ghostty#10478](https://github.com/ghostty-org/ghostty/discussions/10478) — Support pasting screenshot images

## Community

- [LINUX DO](https://linux.do) — Where we first shared this project

## License

MIT
