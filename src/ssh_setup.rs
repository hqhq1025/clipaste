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

out="${TMPDIR:-/tmp}/clipaste-$(date +%s)-$$.png"
if curl -sf -o "$out" "${CLIPASTE_URL}/clipboard/image" 2>/dev/null && [ -s "$out" ]; then
    echo "$out"
    exit 0
fi

rm -f "$out"
echo "clipaste-paste: failed to fetch image from ${CLIPASTE_URL} (is the clipaste daemon running locally and the tunnel up?)" >&2
exit 1
"#;

/// Append `-p PORT` to an ssh argument list when a custom SSH port is given.
fn ssh_port_args(ssh_port: Option<u16>) -> Vec<String> {
    match ssh_port {
        Some(p) => vec!["-p".to_string(), p.to_string()],
        None => vec![],
    }
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
    let xclip_shim = XCLIP_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);
    let wl_paste_shim = WL_PASTE_SHIM_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);
    let clipaste_paste = CLIPASTE_PASTE_TEMPLATE.replace("__CLIPASTE_URL__", clipaste_url);

    let setup_script = format!(
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
if ! echo "$PATH" | grep -q "$HOME/.local/bin"; then
    for rc in ~/.bashrc ~/.zshrc; do
        if [ -f "$rc" ] && ! grep -q 'clipaste PATH' "$rc"; then
            echo '# clipaste PATH' >> "$rc"
            echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$rc"
        fi
    done
fi
echo "CLIPASTE_OS=$OS"
echo "OK"
"#
    );

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

    // Ensure PATH
    let bashrc = dirs_home().join(".bashrc");
    if bashrc.exists() {
        let content = std::fs::read_to_string(&bashrc).unwrap_or_default();
        if !content.contains("clipaste PATH") {
            let mut f = std::fs::OpenOptions::new().append(true).open(&bashrc)
                .map_err(|e| format!("append bashrc: {e}"))?;
            writeln!(f, "\n# clipaste PATH\nexport PATH=\"$HOME/.local/bin:$PATH\"").ok();
        }
    }

    Ok(())
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

pub fn run_wsl() {
    println!("clipaste wsl-setup");
    println!();

    // Step 1: Detect Windows host IP
    print!("[1/3] Detecting Windows host IP... ");
    std::io::stdout().flush().unwrap();
    let win_ip = detect_wsl_host_ip();
    match &win_ip {
        Some(ip) => println!("{ip}"),
        None => {
            println!("FAILED");
            eprintln!("  Cannot detect Windows host IP from /etc/resolv.conf");
            eprintln!("  Make sure you're running this inside WSL2");
            std::process::exit(1);
        }
    }
    let win_ip = win_ip.unwrap();

    // Step 2: Check clipaste HTTP server on Windows host
    let url = format!("http://{win_ip}:{}", common::DEFAULT_PORT);
    print!("[2/3] Checking clipaste on Windows host ({url})... ");
    std::io::stdout().flush().unwrap();
    if !check_health(&url) {
        println!("FAILED");
        eprintln!("  clipaste.exe is not running on Windows, or port {} is blocked.", common::DEFAULT_PORT);
        eprintln!("  Make sure clipaste.exe is running on the Windows side.");
        std::process::exit(1);
    }
    println!("OK");

    // Step 3: Install shims locally (we're inside WSL2)
    print!("[3/3] Installing xclip/wl-paste shims... ");
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
    println!("  3. In Claude Code / Codex, press Ctrl+V");
    println!();
    println!("No SSH tunnel needed — WSL2 connects directly to Windows host.");
}

// ─── Helpers ───

fn check_health(base_url: &str) -> bool {
    Command::new("curl")
        .args(["-sf", &format!("{base_url}/health")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn detect_wsl_host_ip() -> Option<String> {
    // WSL2: Windows host IP is the nameserver in /etc/resolv.conf
    let content = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("nameserver") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return Some(parts[1].to_string());
            }
        }
    }
    None
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
}

