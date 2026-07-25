use crate::common;
use std::io::Write;
use std::process::Command;

/// xclip shim template — {CLIPASTE_URL} will be replaced at install time
const XCLIP_SHIM_TEMPLATE: &str = r#"#!/bin/bash
# clipaste xclip shim — intercepts xclip calls and fetches images from clipaste
# Installed by: clipaste wsl-setup

CLIPASTE_URL="__CLIPASTE_URL__"
REAL_XCLIP="$(PATH=$(echo "$PATH" | sed "s|$HOME/.local/bin:||g") command -v xclip 2>/dev/null)"

case "$*" in
    *"-selection clipboard"*"-t TARGETS"*"-o"*|*"-sel clip"*"-t TARGETS"*"-o"*)
        if curl -sf "${CLIPASTE_URL}/clipboard/type" 2>/dev/null | grep -q '"image"'; then
            echo "TARGETS"
            echo "image/png"
            exit 0
        fi
        ;;
    *"-selection clipboard"*"-t image/png"*"-o"*|*"-sel clip"*"-t image/png"*"-o"*)
        tmpfile=$(mktemp /tmp/clipaste-remote-XXXXXX.png)
        if curl -sf -o "$tmpfile" "${CLIPASTE_URL}/clipboard/image" 2>/dev/null; then
            if [ -s "$tmpfile" ]; then
                cat "$tmpfile"
                rm -f "$tmpfile"
                exit 0
            fi
        fi
        rm -f "$tmpfile"
        ;;
esac

if [ -n "$REAL_XCLIP" ] && [ -x "$REAL_XCLIP" ]; then
    exec "$REAL_XCLIP" "$@"
else
    echo "xclip not found" >&2
    exit 1
fi
"#;

const WL_PASTE_SHIM_TEMPLATE: &str = r#"#!/bin/bash
# clipaste wl-paste shim — for Wayland environments
# Installed by: clipaste wsl-setup

CLIPASTE_URL="__CLIPASTE_URL__"
REAL_WL_PASTE="$(PATH=$(echo "$PATH" | sed "s|$HOME/.local/bin:||g") command -v wl-paste 2>/dev/null)"

case "$*" in
    *"--list-types"*)
        if curl -sf "${CLIPASTE_URL}/clipboard/type" 2>/dev/null | grep -q '"image"'; then
            echo "image/png"
            echo "text/plain"
            exit 0
        fi
        ;;
    *"--type image/"*|*"-t image/"*)
        tmpfile=$(mktemp /tmp/clipaste-remote-XXXXXX.png)
        if curl -sf -o "$tmpfile" "${CLIPASTE_URL}/clipboard/image" 2>/dev/null; then
            if [ -s "$tmpfile" ]; then
                cat "$tmpfile"
                rm -f "$tmpfile"
                exit 0
            fi
        fi
        rm -f "$tmpfile"
        ;;
esac

if [ -n "$REAL_WL_PASTE" ] && [ -x "$REAL_WL_PASTE" ]; then
    exec "$REAL_WL_PASTE" "$@"
else
    echo "wl-paste not found" >&2
    exit 1
fi
"#;

/// clipaste-paste helper — fetches the current clipboard image into a real file
/// on the remote host and prints its path. Unlike the xclip/wl-paste shims (which
/// only work for tools that shell out to those commands, e.g. Claude Code), this
/// works for ANY tool that accepts an image file path — including Codex CLI, which
/// reads the clipboard in-process via X11/NSPasteboard and bypasses the shims.
/// Also the only working path on a macOS remote (where xclip/wl-paste don't apply).
const CLIPASTE_PASTE_TEMPLATE: &str = r#"#!/bin/bash
# clipaste-paste — fetch the current clipboard image from your LOCAL machine
# (through the clipaste SSH tunnel / WSL bridge) into a real file on THIS host,
# then print the path. Use it when your tool can't read the clipboard directly:
#   - Codex CLI (reads clipboard in-process, bypasses the xclip shim)
#   - any macOS remote (xclip/wl-paste don't apply there)
#
# Usage: run `clipaste-paste`, then hand the printed path to your tool.
# Installed by: clipaste ssh-setup / clipaste wsl-setup

CLIPASTE_URL="__CLIPASTE_URL__"

if ! curl -sf "${CLIPASTE_URL}/clipboard/type" 2>/dev/null | grep -q '"image"'; then
    echo "clipaste-paste: no image on clipboard — take a screenshot (or copy an image file) on your local machine first" >&2
    exit 1
fi

out="${TMPDIR:-/tmp}"
out="${out%/}/clipaste-$(date +%s)-$$.png"
if curl -sf -o "$out" "${CLIPASTE_URL}/clipboard/image" 2>/dev/null && [ -s "$out" ]; then
    echo "$out"
    exit 0
fi

rm -f "$out"
echo "clipaste-paste: failed to fetch image from ${CLIPASTE_URL} (is the clipaste daemon running locally and the tunnel up?)" >&2
exit 1
"#;

/// Shell snippet (shared by the local WSL install and the remote SSH install)
/// that puts `~/.local/bin` on PATH.
///
/// The previous version only appended to rc files that *already existed*, so on
/// a host where neither `~/.bashrc` nor `~/.zshrc` is present — common on a
/// fresh macOS remote, where zsh works fine without a `~/.zshrc` — the shims
/// were installed but never reachable, and `clipaste-paste` came back as
/// "command not found". We now fall back to creating the rc file for the login
/// shell.
const ENSURE_PATH_SNIPPET: &str = r#"
clipaste_ensure_path() {
    rc="$1"
    [ -e "$rc" ] || : > "$rc"
    if ! grep -q 'clipaste PATH' "$rc" 2>/dev/null; then
        printf '\n# clipaste PATH\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$rc"
    fi
}

if ! echo "$PATH" | grep -q "$HOME/.local/bin"; then
    clipaste_touched=0
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        if [ -f "$rc" ]; then
            clipaste_ensure_path "$rc"
            clipaste_touched=1
        fi
    done
    if [ "$clipaste_touched" = "0" ]; then
        case "${SHELL##*/}" in
            zsh) clipaste_ensure_path "$HOME/.zshrc" ;;
            *)   clipaste_ensure_path "$HOME/.bashrc" ;;
        esac
    fi
fi
"#;

/// Append `-p PORT` to an ssh argument list when a custom SSH port is given.
fn ssh_port_args(ssh_port: Option<u16>) -> Vec<String> {
    match ssh_port {
        Some(p) => vec!["-p".to_string(), p.to_string()],
        None => vec![],
    }
}

/// Render the remote `bash -s` payload. Split out from [`install_shims_via_ssh`]
/// so a test can syntax-check it locally — a typo in this generated script would
/// otherwise only ever surface on someone else's remote host.
fn build_remote_setup_script(clipaste_url: &str) -> String {
    let xclip_shim = XCLIP_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);
    let wl_paste_shim = WL_PASTE_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);
    let clipaste_paste = CLIPASTE_PASTE_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);

    format!(
        r#"
set -e
OS="$(uname -s)"
mkdir -p ~/.local/bin

# clipaste-paste: universal helper, installed on every platform
cat > ~/.local/bin/clipaste-paste << 'SHIMEOF'
{clipaste_paste}
SHIMEOF
chmod +x ~/.local/bin/clipaste-paste

# xclip/wl-paste shims: only meaningful on Linux
if [ "$OS" = "Linux" ]; then
cat > ~/.local/bin/xclip << 'SHIMEOF'
{xclip_shim}
SHIMEOF
chmod +x ~/.local/bin/xclip

cat > ~/.local/bin/wl-paste << 'SHIMEOF'
{wl_paste_shim}
SHIMEOF
chmod +x ~/.local/bin/wl-paste
fi

# Ensure ~/.local/bin is in PATH
{ensure_path}
echo "CLIPASTE_OS=$OS"
echo "OK"
"#,
        ensure_path = ENSURE_PATH_SNIPPET,
    )
}

/// Install shims on the remote and report the detected remote OS ("Darwin" /
/// "Linux" / "Unknown").
///
/// A single remote `bash -s` invocation detects the OS via `uname -s` and then
/// always installs `clipaste-paste` (works for every tool, every OS), but
/// installs the xclip/wl-paste shims only on Linux (they are useless on macOS,
/// where tools read the pasteboard directly rather than via xclip). Doing the OS
/// branch on the remote avoids a second ssh round-trip / password prompt and
/// keeps the remote authoritative about its own platform.
fn install_shims_via_ssh(
    host: &str,
    clipaste_url: &str,
    ssh_port: Option<u16>,
) -> Result<String, String> {
    let setup_script = build_remote_setup_script(clipaste_url);

    let mut args = ssh_port_args(ssh_port);
    args.push(host.to_string());
    args.push("bash -s".to_string());

    let result = Command::new("ssh")
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(setup_script.as_bytes())?;
            child.wait_with_output()
        });

    match result {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let os = stdout
                .lines()
                .find_map(|l| l.trim().strip_prefix("CLIPASTE_OS="))
                .unwrap_or("Unknown")
                .to_string();
            Ok(os)
        }
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(format!("SSH error: {e}")),
    }
}

/// Write a shim script to `path` and mark it executable (best-effort, Unix).
fn write_executable(path: &std::path::Path, content: &str, label: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("write {label}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).ok();
    }
    Ok(())
}

fn install_shims_locally(clipaste_url: &str) -> Result<(), String> {
    let xclip_shim = XCLIP_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);
    let wl_paste_shim = WL_PASTE_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);
    let clipaste_paste = CLIPASTE_PASTE_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);

    let bin_dir = dirs_home().join(".local/bin");
    std::fs::create_dir_all(&bin_dir).map_err(|e| format!("mkdir: {e}"))?;

    write_executable(&bin_dir.join("xclip"), &xclip_shim, "xclip")?;
    write_executable(&bin_dir.join("wl-paste"), &wl_paste_shim, "wl-paste")?;
    write_executable(&bin_dir.join("clipaste-paste"), &clipaste_paste, "clipaste-paste")?;

    ensure_local_bin_on_path();

    Ok(())
}

/// Add `~/.local/bin` to PATH via the shell rc files, mirroring
/// [`ENSURE_PATH_SNIPPET`] for the local (WSL) install.
///
/// Appends to every rc file that exists; if none does, creates the one matching
/// the login shell. Best-effort — a failure here costs the user a manual PATH
/// export, not a broken install, so it must not abort setup.
fn ensure_local_bin_on_path() {
    let home = dirs_home();
    let candidates = [home.join(".bashrc"), home.join(".zshrc")];

    let existing: Vec<_> = candidates.iter().filter(|p| p.is_file()).cloned().collect();
    let to_patch = if existing.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_default();
        let rc = if shell.rsplit('/').next() == Some("zsh") {
            home.join(".zshrc")
        } else {
            home.join(".bashrc")
        };
        vec![rc]
    } else {
        existing
    };

    for rc in to_patch {
        let content = std::fs::read_to_string(&rc).unwrap_or_default();
        if content.contains("clipaste PATH") {
            continue;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&rc) {
            writeln!(f, "\n# clipaste PATH\nexport PATH=\"$HOME/.local/bin:$PATH\"").ok();
        }
    }
}

// ─── SSH Setup ───

pub fn run_ssh(host: &str, ssh_port: Option<u16>) {
    println!("clipaste ssh-setup for {host}");
    if let Some(p) = ssh_port {
        println!("(SSH port {p})");
    }
    println!();

    // Step 1: Check local HTTP server
    print!("[1/3] Checking local clipaste server... ");
    std::io::stdout().flush().unwrap();
    if !check_health(&format!("http://127.0.0.1:{}", common::DEFAULT_PORT)) {
        println!("FAILED");
        eprintln!("  clipaste daemon is not running. Start it first:");
        eprintln!("  brew services start clipaste");
        std::process::exit(1);
    }
    println!("OK");

    // Step 2: Deploy shims to remote (also detects the remote OS)
    let url = format!("http://127.0.0.1:{}", common::DEFAULT_PORT);
    print!("[2/3] Installing helper on {host}... ");
    std::io::stdout().flush().unwrap();
    let remote_os = match install_shims_via_ssh(host, &url, ssh_port) {
        Ok(os) => {
            println!("OK ({os})");
            os
        }
        Err(e) => {
            println!("FAILED");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    };

    // Step 3: Configure SSH RemoteForward (+ custom Port)
    print!("[3/3] Configuring SSH RemoteForward... ");
    std::io::stdout().flush().unwrap();
    match add_remote_forward(host, ssh_port) {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            println!("FAILED");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    }

    println!();
    println!("Setup complete!");
    println!("  1. Open a NEW SSH session: ssh {host}");
    println!("  2. Take a screenshot (or copy an image file) on your Mac");
    if remote_os == "Darwin" {
        // macOS remote: xclip/wl-paste don't apply; tools read the *remote*
        // (empty) pasteboard. The clipaste-paste helper is the working path.
        println!("  3. In the remote shell, run: clipaste-paste");
        println!("     then hand the printed path to Claude Code / Codex");
        println!();
        println!("Note: this is a macOS remote — native Ctrl+V reads the remote Mac's");
        println!("clipboard (empty), so use the `clipaste-paste` helper instead.");
    } else {
        println!("  3a. Claude Code: press Ctrl+V (fetched via the xclip shim)");
        println!("  3b. Codex CLI:  run `clipaste-paste` and paste the printed path");
        println!();
        println!("Note: Codex reads the clipboard in-process and bypasses the xclip");
        println!("shim, so it can't paste images natively over SSH — use clipaste-paste.");
    }
}

// ─── WSL Setup ───

/// One address that might reach the Windows host, plus a human-readable reason.
/// The reason is echoed on success and listed in the failure diagnostic, so the
/// user can see *why* each address was tried.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostCandidate {
    ip: String,
    source: &'static str,
}

const MIRRORED_LOOPBACK: &str = "mirrored-mode loopback";
const RESOLV_NAMESERVER: &str = "/etc/resolv.conf nameserver";
const DEFAULT_GATEWAY: &str = "default gateway";

/// Build the ordered list of addresses to probe for the Windows-side daemon.
///
/// WSL2 has two networking modes and they need *different* addresses:
///
/// - **NAT** (default): WSL sits behind a virtual switch; the Windows host is
///   the vEthernet gateway, which is normally also the `nameserver` line in
///   `/etc/resolv.conf`. With `dnsTunneling=true` that nameserver becomes a
///   virtual DNS endpoint (`10.255.255.254`) instead — hence the gateway
///   candidate, read from the routing table rather than from DNS config.
/// - **mirrored** (`networkingMode=mirrored`): WSL shares the host's network
///   interfaces, so the host is reachable at `127.0.0.1` and the resolv.conf
///   nameserver points at nothing that listens on our port.
///
/// This is issue #7: detection used to read resolv.conf and nothing else, so
/// under mirrored networking it locked onto an unreachable address and aborted.
/// The mode (when `wslinfo` can report it) only decides the *order* — every
/// candidate is probed regardless, so an older WSL without `wslinfo` still lands
/// on the right address.
fn wsl_host_candidates(
    mode: Option<&str>,
    nameserver: Option<String>,
    gateway: Option<String>,
) -> Vec<HostCandidate> {
    let loopback = HostCandidate {
        ip: "127.0.0.1".to_string(),
        source: MIRRORED_LOOPBACK,
    };
    let mirrored = matches!(mode, Some(m) if m.eq_ignore_ascii_case("mirrored"));

    let mut out: Vec<HostCandidate> = Vec::new();
    if mirrored {
        out.push(loopback.clone());
    }
    if let Some(ip) = nameserver {
        out.push(HostCandidate { ip, source: RESOLV_NAMESERVER });
    }
    if let Some(ip) = gateway {
        out.push(HostCandidate { ip, source: DEFAULT_GATEWAY });
    }
    if !mirrored {
        out.push(loopback);
    }

    // Keep first occurrence: NAT usually reports the same IP for nameserver and
    // gateway, and we want the earlier (more specific) reason attached to it.
    let mut seen = Vec::new();
    out.retain(|c| {
        if seen.contains(&c.ip) {
            false
        } else {
            seen.push(c.ip.clone());
            true
        }
    });
    out
}

pub fn run_wsl(host_override: Option<String>) {
    println!("clipaste wsl-setup");
    println!();

    let port = common::DEFAULT_PORT;
    let mode = wsl_networking_mode();

    // Step 1: Find an address where the Windows-side daemon actually answers.
    // Detection and verification are one step: an address that doesn't serve
    // /health is not a usable answer, so reporting it as "detected" would only
    // push the failure downstream into a shim that silently never works.
    print!("[1/2] Locating clipaste on the Windows host... ");
    std::io::stdout().flush().unwrap();

    let win_ip = match host_override {
        Some(ip) => {
            if !check_health(&format!("http://{ip}:{port}")) {
                println!("FAILED");
                eprintln!("  --host {ip}: no clipaste server answering on port {port}");
                eprintln!("  Check that clipaste.exe is running on Windows and that {ip} is right.");
                std::process::exit(1);
            }
            println!("{ip} (--host)");
            ip
        }
        None => {
            let candidates = wsl_host_candidates(
                mode.as_deref(),
                read_resolv_nameserver(),
                read_default_gateway(),
            );
            match candidates
                .iter()
                .find(|c| check_health(&format!("http://{}:{}", c.ip, port)))
            {
                Some(c) => {
                    println!("{} ({})", c.ip, c.source);
                    c.ip.clone()
                }
                None => {
                    println!("FAILED");
                    print_wsl_failure_help(&candidates, mode.as_deref());
                    std::process::exit(1);
                }
            }
        }
    };

    // Step 2: Install shims locally (we're inside WSL2)
    let url = format!("http://{win_ip}:{port}");
    print!("[2/2] Installing xclip/wl-paste shims + clipaste-paste... ");
    std::io::stdout().flush().unwrap();
    match install_shims_locally(&url) {
        Ok(()) => println!("OK"),
        Err(e) => {
            println!("FAILED");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    }

    println!();
    println!("Setup complete! Now:");
    println!("  1. Open a new terminal (or run: source ~/.bashrc)");
    println!("  2. Take a screenshot on Windows (Win+Shift+S)");
    println!("  3a. Claude Code: press Ctrl+V (fetched via the xclip shim)");
    println!("  3b. Codex CLI:  run `clipaste-paste` and paste the printed path");
    println!();
    println!("No SSH tunnel needed — WSL2 connects directly to the Windows host.");
}

/// Explain a failed probe honestly: what was tried, why each address, and the
/// concrete ways out. In particular, clipaste binds to `127.0.0.1` on Windows,
/// which NAT-mode WSL cannot reach at all — that deserves saying out loud
/// rather than leaving the user to guess at a firewall.
fn print_wsl_failure_help(candidates: &[HostCandidate], mode: Option<&str>) {
    let port = common::DEFAULT_PORT;
    eprintln!("  No clipaste server answered. Tried:");
    for c in candidates {
        eprintln!("    - http://{}:{port}/health  ({})", c.ip, c.source);
    }
    match mode {
        Some(m) => eprintln!("  WSL networking mode: {m}"),
        None => eprintln!("  WSL networking mode: unknown (`wslinfo` unavailable)"),
    }
    eprintln!();
    eprintln!("  Checklist:");
    eprintln!("    1. clipaste.exe must be running on the Windows side.");
    eprintln!("    2. clipaste binds to 127.0.0.1 on Windows, which WSL can reach only in");
    eprintln!("       mirrored networking mode. In the default NAT mode, add this to");
    eprintln!("       %USERPROFILE%\\.wslconfig:");
    eprintln!("         [wsl2]");
    eprintln!("         networkingMode=mirrored");
    eprintln!("       then run `wsl --shutdown` in PowerShell and reopen WSL.");
    eprintln!("    3. If the server answers on some other address, pass it explicitly:");
    eprintln!("         clipaste wsl-setup --host <ip>");
}

// ─── Helpers ───

fn check_health(base_url: &str) -> bool {
    // Bounded timeouts matter here: wsl-setup probes several candidate addresses
    // in sequence, and a firewall that DROPs (rather than rejects) would
    // otherwise stall each probe on the TCP connect for minutes.
    let out = Command::new("curl")
        .args([
            "-sf",
            "--connect-timeout",
            "2",
            "-m",
            "5",
            &format!("{base_url}/health"),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => is_clipaste_health_body(&String::from_utf8_lossy(&o.stdout)),
        _ => false,
    }
}

/// Confirm the responder is clipaste, not just *something* on that port.
/// Candidate probing can hit a LAN gateway or an unrelated local service, and
/// pointing the shims at those would fetch garbage instead of a screenshot.
fn is_clipaste_health_body(body: &str) -> bool {
    body.replace(' ', "").contains("\"status\":\"ok\"")
}

/// `wslinfo --networking-mode` prints `nat` or `mirrored`. It only exists on
/// recent WSL builds, so absence is normal and simply leaves the order alone.
fn wsl_networking_mode() -> Option<String> {
    let out = Command::new("wslinfo").arg("--networking-mode").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let mode = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    if mode.is_empty() {
        None
    } else {
        Some(mode)
    }
}

fn read_resolv_nameserver() -> Option<String> {
    let content = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    parse_resolv_nameserver(&content)
}

fn parse_resolv_nameserver(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            return None;
        }
        let rest = line.strip_prefix("nameserver")?;
        let ip = rest.trim();
        // IPv6 nameservers can't be used as-is in our `http://{ip}:{port}` URLs,
        // and WSL's host address is always IPv4 — skip them rather than emit a
        // malformed URL.
        if ip.is_empty() || ip.contains(':') {
            None
        } else {
            Some(ip.split_whitespace().next()?.to_string())
        }
    })
}

/// The NAT-mode Windows host is the default gateway. Read it from `ip route`
/// when iproute2 is present, else straight from `/proc/net/route` (WSL distros
/// like Kali ship both, but minimal images may have neither `ip` nor `route`).
fn read_default_gateway() -> Option<String> {
    if let Ok(out) = Command::new("ip").args(["route", "show", "default"]).output() {
        if out.status.success() {
            if let Some(gw) = parse_ip_route_default(&String::from_utf8_lossy(&out.stdout)) {
                return Some(gw);
            }
        }
    }
    parse_proc_net_route(&std::fs::read_to_string("/proc/net/route").ok()?)
}

/// Parse `default via 172.29.128.1 dev eth0 ...` (first default route wins).
fn parse_ip_route_default(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "default" {
            return None;
        }
        let mut parts = parts.skip_while(|t| *t != "via");
        parts.next()?; // consume "via"
        let gw = parts.next()?;
        if gw.contains(':') {
            None
        } else {
            Some(gw.to_string())
        }
    })
}

/// Parse the default route out of `/proc/net/route`. Gateway addresses there are
/// little-endian hex, e.g. `0180 1FAC` → 172.31.128.1.
fn parse_proc_net_route(content: &str) -> Option<String> {
    content.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 || f[1] != "00000000" {
            return None;
        }
        hex_le_to_ipv4(f[2])
    })
}

fn hex_le_to_ipv4(hex: &str) -> Option<String> {
    let n = u32::from_str_radix(hex, 16).ok()?;
    if n == 0 {
        return None;
    }
    Some(format!(
        "{}.{}.{}.{}",
        n & 0xff,
        (n >> 8) & 0xff,
        (n >> 16) & 0xff,
        (n >> 24) & 0xff
    ))
}

fn add_remote_forward(host: &str, ssh_port: Option<u16>) -> Result<String, String> {
    let ssh_config_path = dirs_home().join(".ssh/config");
    let ssh_dir = ssh_config_path.parent().unwrap();
    if !ssh_dir.exists() {
        std::fs::create_dir_all(ssh_dir)
            .map_err(|e| format!("Cannot create ~/.ssh: {e}"))?;
    }
    let config_content = std::fs::read_to_string(&ssh_config_path).unwrap_or_default();
    let host_pattern = extract_hostname(host);

    match build_ssh_config(&config_content, &host_pattern, ssh_port) {
        (None, msg) => Ok(msg),
        (Some(new_config), msg) => {
            std::fs::write(&ssh_config_path, &new_config)
                .map_err(|e| format!("Cannot write ~/.ssh/config: {e}"))?;
            Ok(msg)
        }
    }
}

/// Pure transformation: given the existing `~/.ssh/config` text, inject a
/// `RemoteForward` (and a `Port` when a custom SSH port is given) into the block
/// matching `host_pattern`. Returns `(None, msg)` when nothing needs changing
/// ("already configured"), or `(Some(new_config), msg)` with the rewritten file.
///
/// `RemoteForward` is the idempotency key. A `Port` directive already present in
/// the matching block is left untouched (we never override the user's port).
fn build_ssh_config(
    existing: &str,
    host_pattern: &str,
    ssh_port: Option<u16>,
) -> (Option<String>, String) {
    let port = common::DEFAULT_PORT;
    let forward_line = format!("RemoteForward {port} 127.0.0.1:{port}");

    let lines: Vec<&str> = existing.lines().collect();
    let mut inject_after: Option<usize> = None;
    let mut in_matching_block = false;
    let mut found_existing_forward = false;
    let mut found_existing_port = false;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // New Host block starts — reset per-block matching state.
        // Note: found_existing_* is latching (never reset), so a directive
        // found in an earlier matching block survives later blocks.
        if trimmed.starts_with("Host ") {
            in_matching_block = false;

            let host_value = trimmed.strip_prefix("Host ").unwrap_or("").trim();
            // Skip wildcard-only blocks (Host *)
            if host_value.split_whitespace().all(|h| h.contains('*') || h.contains('?')) {
                continue;
            }
            if host_value.split_whitespace().any(|h| h == host_pattern) {
                in_matching_block = true;
                // Fallback: inject after Host line if block has no HostName
                if inject_after.is_none() {
                    inject_after = Some(i);
                }
            }
        }

        // HostName in current block — prefer injecting after this line
        if trimmed.starts_with("HostName ") || trimmed.starts_with("HostName\t") {
            let hostname_value = trimmed.strip_prefix("HostName").unwrap_or("").trim();
            if hostname_value == host_pattern {
                in_matching_block = true;
            }
            if in_matching_block {
                // Override: prefer injecting after HostName over Host line
                inject_after = Some(i);
            }
        }

        // Check if any matching block already has the forward / port (latching)
        if in_matching_block && trimmed.contains(&forward_line) {
            found_existing_forward = true;
        }
        if in_matching_block
            && (trimmed.starts_with("Port ") || trimmed.starts_with("Port\t"))
        {
            found_existing_port = true;
        }
    }

    // Lines to inject after the chosen anchor (preserve a stable order).
    let mut inject_lines: Vec<String> = Vec::new();
    if !found_existing_forward {
        inject_lines.push(forward_line.clone());
    }
    if let Some(p) = ssh_port {
        if !found_existing_port {
            inject_lines.push(format!("Port {p}"));
        }
    }

    if inject_lines.is_empty() {
        return (None, "already configured".to_string());
    }

    let mut new_config = String::new();
    for (i, line) in lines.iter().enumerate() {
        new_config.push_str(line);
        new_config.push('\n');
        if Some(i) == inject_after {
            for l in &inject_lines {
                new_config.push_str(&format!("    {l}\n"));
            }
        }
    }

    if inject_after.is_none() {
        new_config.push_str(&format!(
            "\n# clipaste remote paste\nHost clipaste-{host_pattern}\n    HostName {host_pattern}\n"
        ));
        if let Some(p) = ssh_port {
            new_config.push_str(&format!("    Port {p}\n"));
        }
        new_config.push_str(&format!("    {forward_line}\n"));
    }

    let msg = if ssh_port.is_some() && !found_existing_port {
        "OK (added RemoteForward + Port)".to_string()
    } else {
        "OK (added RemoteForward)".to_string()
    };
    (Some(new_config), msg)
}

fn dirs_home() -> std::path::PathBuf {
    // HOME is set on Unix/macOS; USERPROFILE is the Windows equivalent
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
}

fn extract_hostname(host: &str) -> String {
    if let Some(at) = host.rfind('@') {
        host[at + 1..].to_string()
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_hostname_strips_user() {
        assert_eq!(extract_hostname("user@host"), "host");
        assert_eq!(extract_hostname("host"), "host");
        assert_eq!(extract_hostname("a@b@host"), "host");
    }

    #[test]
    fn ssh_port_args_shape() {
        assert!(ssh_port_args(None).is_empty());
        assert_eq!(ssh_port_args(Some(2200)), vec!["-p", "2200"]);
    }

    #[test]
    fn new_block_when_no_match() {
        let (out, msg) = build_ssh_config("", "host", None);
        let out = out.unwrap();
        assert!(out.contains("Host clipaste-host"));
        assert!(out.contains("HostName host"));
        assert!(out.contains("RemoteForward 18340 127.0.0.1:18340"));
        assert!(!out.contains("Port "));
        assert_eq!(msg, "OK (added RemoteForward)");
    }

    #[test]
    fn new_block_with_custom_port() {
        let (out, msg) = build_ssh_config("", "host", Some(22222));
        let out = out.unwrap();
        assert!(out.contains("Host clipaste-host"));
        assert!(out.contains("    Port 22222\n"));
        assert!(out.contains("RemoteForward 18340 127.0.0.1:18340"));
        assert_eq!(msg, "OK (added RemoteForward + Port)");
    }

    #[test]
    fn injects_into_existing_block_after_hostname() {
        let existing = "Host myserver\n    HostName 10.0.0.1\n    User me\n";
        let (out, msg) = build_ssh_config(existing, "myserver", Some(22222));
        let out = out.unwrap();
        // Injected right after HostName, before User
        let hn = out.find("HostName 10.0.0.1").unwrap();
        let fwd = out.find("RemoteForward").unwrap();
        let port = out.find("Port 22222").unwrap();
        let user = out.find("User me").unwrap();
        assert!(hn < fwd && fwd < user);
        assert!(hn < port && port < user);
        assert_eq!(msg, "OK (added RemoteForward + Port)");
    }

    #[test]
    fn idempotent_when_forward_present_no_port_requested() {
        let existing =
            "Host h\n    HostName h\n    RemoteForward 18340 127.0.0.1:18340\n";
        let (out, msg) = build_ssh_config(existing, "h", None);
        assert!(out.is_none());
        assert_eq!(msg, "already configured");
    }

    #[test]
    fn does_not_duplicate_existing_user_port() {
        // Block already has a Port (the user's real SSH port) and the forward.
        let existing =
            "Host h\n    HostName h\n    Port 2222\n    RemoteForward 18340 127.0.0.1:18340\n";
        let (out, msg) = build_ssh_config(existing, "h", Some(2222));
        assert!(out.is_none(), "nothing to add → already configured");
        assert_eq!(msg, "already configured");
    }

    #[test]
    fn adds_only_port_when_forward_already_present() {
        let existing =
            "Host h\n    HostName h\n    RemoteForward 18340 127.0.0.1:18340\n";
        let (out, msg) = build_ssh_config(existing, "h", Some(2222));
        let out = out.unwrap();
        assert!(out.contains("Port 2222"));
        assert_eq!(out.matches("RemoteForward 18340").count(), 1);
        assert_eq!(msg, "OK (added RemoteForward + Port)");
    }

    #[test]
    fn skips_wildcard_block() {
        let existing = "Host *\n    ForwardAgent yes\n";
        let (out, _) = build_ssh_config(existing, "host", None);
        let out = out.unwrap();
        // A dedicated clipaste block is created instead of injecting into Host *
        assert!(out.contains("Host clipaste-host"));
    }

    // ─── WSL host detection (issue #7) ───

    fn ips(c: &[HostCandidate]) -> Vec<&str> {
        c.iter().map(|c| c.ip.as_str()).collect()
    }

    #[test]
    fn nat_mode_prefers_nameserver_then_gateway() {
        let c = wsl_host_candidates(
            Some("nat"),
            Some("172.29.128.1".into()),
            Some("172.29.128.1".into()),
        );
        // nameserver == gateway in plain NAT → deduped, loopback still probed last
        assert_eq!(ips(&c), vec!["172.29.128.1", "127.0.0.1"]);
        assert_eq!(c[0].source, RESOLV_NAMESERVER);
    }

    #[test]
    fn nat_with_dns_tunneling_falls_back_to_gateway() {
        // dnsTunneling=true makes the nameserver a virtual endpoint, not the host
        let c = wsl_host_candidates(
            Some("nat"),
            Some("10.255.255.254".into()),
            Some("172.29.128.1".into()),
        );
        assert_eq!(ips(&c), vec!["10.255.255.254", "172.29.128.1", "127.0.0.1"]);
    }

    #[test]
    fn mirrored_mode_probes_loopback_first() {
        // The issue #7 case: resolv.conf points at 10.255.255.254, host is loopback
        let c = wsl_host_candidates(
            Some("mirrored"),
            Some("10.255.255.254".into()),
            Some("192.168.1.1".into()),
        );
        assert_eq!(ips(&c), vec!["127.0.0.1", "10.255.255.254", "192.168.1.1"]);
        assert_eq!(c[0].source, MIRRORED_LOOPBACK);
    }

    #[test]
    fn unknown_mode_still_reaches_loopback() {
        // Older WSL without `wslinfo`: order is unchanged for NAT, but mirrored
        // setups are still found because loopback is probed as a fallback.
        let c = wsl_host_candidates(None, Some("10.255.255.254".into()), None);
        assert_eq!(ips(&c), vec!["10.255.255.254", "127.0.0.1"]);
    }

    #[test]
    fn loopback_never_duplicated_when_it_is_also_the_nameserver() {
        let c = wsl_host_candidates(Some("mirrored"), Some("127.0.0.1".into()), None);
        assert_eq!(ips(&c), vec!["127.0.0.1"]);
    }

    #[test]
    fn candidates_never_empty() {
        assert_eq!(ips(&wsl_host_candidates(None, None, None)), vec!["127.0.0.1"]);
    }

    #[test]
    fn parses_nameserver() {
        assert_eq!(
            parse_resolv_nameserver("# generated\nnameserver 10.255.255.254\n").as_deref(),
            Some("10.255.255.254")
        );
        assert_eq!(
            parse_resolv_nameserver("search local\nnameserver\t172.29.128.1\n").as_deref(),
            Some("172.29.128.1")
        );
        // Commented-out and IPv6 nameservers are not usable host addresses
        assert_eq!(parse_resolv_nameserver("#nameserver 1.2.3.4\n"), None);
        assert_eq!(parse_resolv_nameserver("nameserver fe80::1\n"), None);
        assert_eq!(parse_resolv_nameserver("search corp\n"), None);
    }

    #[test]
    fn parses_ip_route_default() {
        let out = "default via 172.29.128.1 dev eth0 proto kernel\n\
                   10.0.0.0/8 dev eth0 scope link\n";
        assert_eq!(parse_ip_route_default(out).as_deref(), Some("172.29.128.1"));
        // A default route with no gateway (link-scoped) yields nothing
        assert_eq!(parse_ip_route_default("default dev eth0 scope link\n"), None);
        assert_eq!(parse_ip_route_default(""), None);
    }

    #[test]
    fn parses_proc_net_route() {
        let content = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
                       eth0\t00000000\t01801FAC\t0003\t0\t0\t0\t00000000\n\
                       eth0\t0000FEA9\t00000000\t0001\t0\t0\t0\t0000FFFF\n";
        assert_eq!(parse_proc_net_route(content).as_deref(), Some("172.31.128.1"));
        // No default route (Destination never 00000000)
        let none = "Iface\tDestination\tGateway\n\
                    eth0\t0000FEA9\t00000000\n";
        assert_eq!(parse_proc_net_route(none), None);
    }

    #[test]
    fn health_body_must_identify_clipaste() {
        assert!(is_clipaste_health_body(
            "{\"status\":\"ok\",\"version\":\"2.4.0\"}"
        ));
        assert!(is_clipaste_health_body("{ \"status\": \"ok\" }"));
        // Some unrelated service answering 200 on the same port must not pass
        assert!(!is_clipaste_health_body("<html>It works!</html>"));
        assert!(!is_clipaste_health_body("{\"status\":\"degraded\"}"));
        assert!(!is_clipaste_health_body(""));
    }

    // ─── Generated shell payloads ───

    /// The remote setup script and the shims are assembled by string formatting,
    /// so a syntax error in them would only ever fail on a user's machine. Parse
    /// them locally with `bash -n` instead.
    #[cfg(unix)]
    fn assert_bash_parses(script: &str, label: &str) {
        use std::io::Read;
        let mut child = Command::new("bash")
            .args(["-n"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn bash -n");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let mut err = String::new();
        child.stderr.take().unwrap().read_to_string(&mut err).ok();
        let status = child.wait().unwrap();
        assert!(status.success(), "{label} is not valid bash:\n{err}");
    }

    #[cfg(unix)]
    #[test]
    fn generated_scripts_are_valid_bash() {
        let url = "http://127.0.0.1:18340";
        assert_bash_parses(&build_remote_setup_script(url), "remote setup script");
        assert_bash_parses(
            &XCLIP_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", url),
            "xclip shim",
        );
        assert_bash_parses(
            &WL_PASTE_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", url),
            "wl-paste shim",
        );
        assert_bash_parses(
            &CLIPASTE_PASTE_TEMPLATE.replace("__CLIPASTE_URL__", url),
            "clipaste-paste helper",
        );
    }

    #[test]
    fn remote_script_installs_helper_and_patches_path() {
        let script = build_remote_setup_script("http://127.0.0.1:18340");
        assert!(script.contains("~/.local/bin/clipaste-paste"));
        // PATH must be patched even when no rc file exists yet (macOS remote, issue #2)
        assert!(script.contains("clipaste_ensure_path"));
        assert!(script.contains(r#"[ -e "$rc" ] || : > "$rc""#));
    }

    /// Execute the remote payload against a throwaway HOME. On macOS this
    /// reproduces the issue #2 shape exactly: `uname -s` is Darwin, no rc file
    /// exists yet, and `clipaste-paste` must still end up on PATH.
    #[cfg(unix)]
    #[test]
    fn remote_script_run_installs_into_empty_home() {
        let home = std::env::temp_dir().join("clipaste-remote-script-test");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();

        let script = build_remote_setup_script("http://127.0.0.1:18340");
        let mut child = Command::new("bash")
            .arg("-s")
            .env("HOME", &home)
            .env("SHELL", "/bin/zsh")
            // Without ~/.local/bin already on PATH, the rc-patching branch runs
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn bash -s");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "remote script failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let helper = home.join(".local/bin/clipaste-paste");
        assert!(helper.is_file(), "clipaste-paste not installed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&helper).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "clipaste-paste is not executable");
        }

        // Neither rc file existed → the login shell's rc is created, not skipped
        let zshrc = std::fs::read_to_string(home.join(".zshrc")).expect("~/.zshrc not created");
        assert!(zshrc.contains(r#"export PATH="$HOME/.local/bin:$PATH""#));

        // Re-running must not append a second PATH block
        let mut again = Command::new("bash")
            .arg("-s")
            .env("HOME", &home)
            .env("SHELL", "/bin/zsh")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        again
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(again.wait().unwrap().success());
        let zshrc = std::fs::read_to_string(home.join(".zshrc")).unwrap();
        assert_eq!(zshrc.matches("clipaste PATH").count(), 1);

        let _ = std::fs::remove_dir_all(&home);
    }
}

