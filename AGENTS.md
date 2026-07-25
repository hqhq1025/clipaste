# AGENTS.md

Instructions for coding agents working with clipaste — both **installing it for a
user** and **contributing to this repository**.

clipaste is a clipboard daemon that makes screenshot paste work in terminal AI
tools (Claude Code, Codex CLI, Cursor CLI), locally and across SSH/WSL2
boundaries.

---

## Part 1 — Installing clipaste for a user

### The one command that tells you what to do

```bash
clipaste doctor --json
```

This is the entry point. It classifies the machine, runs the checks that are
meaningful for that machine, and returns a `fix` command for anything broken.
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
| `role` | `clipboard-host` / `ssh-remote` / `wsl2` — decides which checks apply |
| `status` | worst of all checks: `ok`, `warn`, `fail` |
| `checks[].status` | `ok` / `warn` / `fail` |
| `checks[].fix` | a literal command to run, or `null` |
| exit code | `0` usable (ok **or** warn), `1` broken, `2` bad arguments |

A `warn` is not a failure. "No screenshot on the clipboard yet" is the normal
state of a freshly installed machine — do not report it to the user as a
problem, and do not try to fix it.

### Decide where you are before installing anything

clipaste has two sides and they install differently. Getting this wrong is the
single most common mistake.

```
        the machine holding the clipboard          the machine running the agent
        (where you press Cmd+Shift+4)              (where Claude Code runs)
        ─────────────────────────────              ─────────────────────────────
              macOS / Windows            ──HTTP──►      same machine
                                                        SSH remote
              runs the daemon                            WSL2 distro
              install: brew / install.ps1                install: run setup FROM the
                                                         clipboard host, not here
```

`clipaste doctor --json` reports which side you are on as `role`. Trust it over
`uname`: an SSH session into a Mac is `ssh-remote`, not `clipboard-host`.

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

From source (any platform with a Rust toolchain):

```bash
git clone https://github.com/hqhq1025/clipaste.git
cd clipaste && cargo build --release
```

### Wire up an SSH remote

Run this **on the clipboard host**, not on the remote. It needs the local daemon
running, and it edits the local `~/.ssh/config`.

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
| Local terminal | `Cmd+V` (macOS) / `Ctrl+V` | `Cmd+V` / `Ctrl+V` |
| SSH into Linux | `Ctrl+V` | `clipaste-paste`, then paste the printed path |
| SSH into macOS | `clipaste-paste` | `clipaste-paste` |
| WSL2 | `Ctrl+V` | `clipaste-paste`, then paste the printed path |

Codex CLI reads the clipboard in-process (via `arboard`) and never shells out to
`xclip`, so it cannot use the shim. `clipaste-paste` writes the image to a real
file on the current host and prints the path — hand that path to the tool.

Never tell a user to press `Cmd+V` in an SSH session: that sends the *local*
file path as text, which the remote agent cannot open.

### Things not to do

- Do not bind the daemon to `0.0.0.0` or add firewall exceptions to "fix"
  connectivity. It listens on loopback deliberately; the image bytes on that
  port are the user's screen contents.
- Do not write shims by hand. `ssh-setup` / `wsl-setup` generate them with the
  correct URL baked in; a hand-written one drifts silently.
- Do not `kill -9` the daemon to restart it. Use `brew services restart clipaste`
  (macOS) or stop `clipaste.exe` normally (Windows).
- Do not report success without a passing `clipaste doctor`. "The install
  command exited 0" is not the same as "paste works".

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
├── macos.rs       NSPasteboard watcher (cfg macos)
└── windows.rs     clipboard-format listener (cfg windows)
```

### Build and test

```bash
cargo build
cargo test          # must stay green; no network access required
cargo clippy --all-targets
```

Tests run without a daemon, without a network, and without touching the real
`$HOME`. Keep it that way — anything that needs a home directory must take one
via an env override, as `remote_script_run_installs_into_empty_home` does.

### Conventions this codebase holds to

- **No new dependencies without a strong reason.** The point of clipaste is that
  it is 9 MB of RAM and a handful of syscalls. HTTP is `curl`; JSON output is
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
