//! `clipaste doctor` — diagnose a clipaste setup and say exactly what to run next.
//!
//! This exists primarily for *coding agents*. An agent installing or repairing
//! clipaste on a user's machine cannot read a tray icon, cannot see a terminal
//! colour, and must not guess. `doctor --json` gives it a stable contract:
//! every check reports a status, a human-readable detail, and — when something
//! is wrong — a `fix` field holding a literal shell command to run.
//!
//! The design rule here is that a check never "passes softly". If the daemon is
//! unreachable we say so and fail, rather than reporting a green tick because
//! the binary happens to be installed. A misleading green is worse than a red
//! for an agent, which will otherwise report success to the user and stop.

use crate::common;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Status::Ok => "✓",
            Status::Warn => "!",
            Status::Fail => "✗",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    /// A literal command the caller can run to fix this. `None` when the check
    /// passed, or when no single command can resolve it.
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Check { name, status: Status::Ok, detail: detail.into(), fix: None }
    }
    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Check { name, status: Status::Warn, detail: detail.into(), fix: None }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Check { name, status: Status::Fail, detail: detail.into(), fix: None }
    }
    fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// Where clipaste is being asked to work. This decides *which* checks are even
/// meaningful: a daemon check makes no sense on a remote host, and a shim check
/// makes no sense on the machine holding the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The machine with the real clipboard — runs the daemon.
    ClipboardHost,
    /// A host reached over SSH; fetches through a RemoteForward tunnel.
    SshRemote,
    /// A WSL2 distro; fetches from clipaste.exe on the Windows host.
    Wsl2,
    /// No local clipboard backend and no evidence of a configured consumer.
    UnsupportedHost,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::ClipboardHost => "clipboard-host",
            Role::SshRemote => "ssh-remote",
            Role::Wsl2 => "wsl2",
            Role::UnsupportedHost => "unsupported-host",
        }
    }
}

/// Classify the current machine.
///
/// Order matters. WSL2 is checked first on Linux because a WSL distro
/// may also carry SSH variables; SSH is checked before the platform default
/// because an SSH'd-into Mac is a *remote*, not a clipboard host — that
/// distinction is exactly what issue #2 turned on.
pub fn detect_role() -> Role {
    classify_role(
        std::env::consts::OS,
        is_wsl(),
        is_ssh_session(),
        installed_helper_url(&home().join(".local/bin/clipaste-paste")).is_some(),
    )
}

fn classify_role(os: &str, wsl: bool, ssh: bool, configured_consumer: bool) -> Role {
    if os == "linux" && wsl {
        return Role::Wsl2;
    }
    if ssh {
        return Role::SshRemote;
    }
    if common::supports_clipboard_host(os) {
        Role::ClipboardHost
    } else if configured_consumer {
        // A configured Linux consumer may be used outside the original SSH
        // session (for example from tmux); let bridge checks diagnose its tunnel.
        Role::SshRemote
    } else {
        Role::UnsupportedHost
    }
}

fn is_wsl() -> bool {
    if std::env::var_os("WSL_DISTRO_NAME").is_some() || std::env::var_os("WSL_INTEROP").is_some() {
        return true;
    }
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| {
            let s = s.to_lowercase();
            s.contains("microsoft") || s.contains("wsl")
        })
        .unwrap_or(false)
}

fn is_ssh_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
        || std::env::var_os("SSH_TTY").is_some()
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// The URL a previously-installed helper was configured with.
///
/// Reading it back from the shim is the only way to know which address setup
/// actually settled on — that value is not recorded anywhere else, and on WSL2
/// it varies with the networking mode (issue #7).
pub fn installed_helper_url(helper: &Path) -> Option<String> {
    let content = std::fs::read_to_string(helper).ok()?;
    parse_helper_url(&content)
}

fn parse_helper_url(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("CLIPASTE_URL=")?;
        let url = rest.trim().trim_matches('"');
        if url.is_empty() || url.contains("__CLIPASTE_URL__") {
            None
        } else {
            Some(url.to_string())
        }
    })
}

fn is_clipaste_health(body: &str) -> bool {
    body.replace(' ', "").contains("\"status\":\"ok\"")
}

/// Extract `"version":"x.y.z"` from a /health body, for reporting a version skew
/// between the two ends of a tunnel.
fn health_version(body: &str) -> Option<String> {
    let compact = body.replace(' ', "");
    let rest = compact.split("\"version\":\"").nth(1)?;
    let v = rest.split('"').next()?;
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Resolve a command through PATH the way the shell would, so we can tell
/// "installed" apart from "installed but shadowed / not on PATH" — the failure
/// mode that made shipped fixes look broken in issues #2 and #5.
fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let c = dir.join(format!("{cmd}.{ext}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

pub struct Report {
    pub role: Role,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn worst(&self) -> Status {
        if self.checks.iter().any(|c| c.status == Status::Fail) {
            Status::Fail
        } else if self.checks.iter().any(|c| c.status == Status::Warn) {
            Status::Warn
        } else {
            Status::Ok
        }
    }

    /// Exit code contract: 0 = usable (ok or warn), 1 = broken.
    /// Warnings must not fail the process — "no screenshot on the clipboard
    /// yet" is a normal state, not a misconfiguration.
    pub fn exit_code(&self) -> i32 {
        match self.worst() {
            Status::Fail => 1,
            _ => 0,
        }
    }
}

pub fn run(json: bool) -> i32 {
    let report = diagnose();
    if json {
        println!("{}", render_json(&report));
    } else {
        print!("{}", render_human(&report));
    }
    report.exit_code()
}

fn diagnose() -> Report {
    diagnose_role(detect_role())
}

fn diagnose_role(role: Role) -> Report {
    if role == Role::UnsupportedHost {
        return Report {
            role,
            checks: vec![Check::fail(
                "platform",
                common::unsupported_host_message(std::env::consts::OS),
            )],
        };
    }
    let mut checks = Vec::new();

    if which("curl").is_none() {
        checks.push(
            Check::fail("curl", "curl not found on PATH — every clipaste transfer uses it")
                .with_fix(curl_install_hint()),
        );
    }

    match role {
        Role::ClipboardHost => checks.extend(clipboard_host_checks()),
        Role::SshRemote => checks.extend(consumer_checks(Role::SshRemote)),
        Role::Wsl2 => checks.extend(consumer_checks(Role::Wsl2)),
        Role::UnsupportedHost => unreachable!("unsupported host returned above"),
    }

    Report { role, checks }
}

fn curl_install_hint() -> &'static str {
    if cfg!(target_os = "windows") {
        "curl.exe ships with Windows 10+; check that C:\\Windows\\System32 is on PATH"
    } else {
        "install curl with your package manager (e.g. apt install curl)"
    }
}

fn daemon_start_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew services start clipaste"
    } else if cfg!(target_os = "windows") {
        "start clipaste.exe (reinstall: irm https://raw.githubusercontent.com/hqhq1025/clipaste/main/install.ps1 | iex)"
    } else {
        "clipaste"
    }
}

fn clipboard_host_checks() -> Vec<Check> {
    let base = format!("http://127.0.0.1:{}", common::DEFAULT_PORT);
    let mut checks = Vec::new();

    match common::http_get(&format!("{base}/health")) {
        Some(body) if is_clipaste_health(&body) => {
            let running = health_version(&body).unwrap_or_else(|| "unknown".into());
            let mut detail = format!("daemon responding on 127.0.0.1:{}", common::DEFAULT_PORT);
            if running != common::VERSION {
                // A stale daemon is the classic "I upgraded but nothing changed"
                // trap: the new binary is on disk, the old one still owns the port.
                detail = format!(
                    "daemon on port {} reports v{running}, but this binary is v{} — the running daemon is stale",
                    common::DEFAULT_PORT,
                    common::VERSION
                );
                checks.push(Check::warn("daemon", detail).with_fix(restart_hint()));
            } else {
                checks.push(Check::ok("daemon", detail));
            }
        }
        Some(_) => checks.push(
            Check::fail(
                "daemon",
                format!(
                    "something is listening on 127.0.0.1:{} but it is not clipaste",
                    common::DEFAULT_PORT
                ),
            )
            .with_fix("free the port, then restart clipaste"),
        ),
        None => checks.push(
            Check::fail(
                "daemon",
                format!("no clipaste daemon on 127.0.0.1:{}", common::DEFAULT_PORT),
            )
            .with_fix(daemon_start_hint()),
        ),
    }

    // Only meaningful once the daemon answered; skip the noise otherwise.
    if checks.iter().any(|c| c.name == "daemon" && c.status != Status::Fail) {
        match common::http_get(&format!("{base}/clipboard/type")) {
            Some(body) if body.contains("\"image\"") => {
                checks.push(Check::ok("clipboard", "an image is staged and ready to serve"))
            }
            Some(_) => checks.push(Check::warn(
                "clipboard",
                "no image staged yet — take a screenshot, then re-run doctor",
            )),
            None => checks.push(Check::warn("clipboard", "could not read clipboard state")),
        }
    }

    let cache = common::temp_dir();
    common::ensure_temp_dir();
    if cache.is_dir() {
        checks.push(Check::ok("cache-dir", cache.display().to_string()));
    } else {
        checks.push(
            Check::fail("cache-dir", format!("cannot create {}", cache.display()))
                .with_fix(format!("mkdir -p {}", cache.display())),
        );
    }

    checks
}

fn restart_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew services restart clipaste"
    } else if cfg!(target_os = "windows") {
        "taskkill /IM clipaste.exe /F, then start the new clipaste.exe"
    } else {
        "restart the clipaste daemon"
    }
}

/// Checks for a machine that *consumes* the clipboard: an SSH remote or a WSL2
/// distro. Both fetch over HTTP from the clipboard host; only the address and
/// the setup command differ.
fn consumer_checks(role: Role) -> Vec<Check> {
    let bin = home().join(".local/bin");
    let helper = bin.join("clipaste-paste");
    let mut checks = Vec::new();

    let setup_hint = match role {
        Role::Wsl2 => "clipaste wsl-setup".to_string(),
        _ => format!(
            "run on your LOCAL machine: clipaste ssh-setup {}",
            std::env::var("USER").unwrap_or_else(|_| "user".into()) + "@this-host"
        ),
    };

    if !is_executable(&helper) {
        checks.push(
            Check::fail(
                "helper",
                format!("{} is missing — setup has not run here", helper.display()),
            )
            .with_fix(&setup_hint),
        );
        // Everything downstream depends on the helper existing; reporting six
        // more failures would bury the one that matters.
        return checks;
    }
    checks.push(Check::ok("helper", helper.display().to_string()));

    match which("clipaste-paste") {
        Some(p) if p == helper => checks.push(Check::ok("helper-on-path", "clipaste-paste resolves")),
        Some(p) => checks.push(Check::warn(
            "helper-on-path",
            format!("clipaste-paste resolves to {} instead", p.display()),
        )),
        None => checks.push(
            Check::fail(
                "helper-on-path",
                format!("{} is not on PATH", bin.display()),
            )
            .with_fix("export PATH=\"$HOME/.local/bin:$PATH\"  # add to your shell rc"),
        ),
    }

    let url = match installed_helper_url(&helper) {
        Some(u) => u,
        None => {
            checks.push(
                Check::fail("helper-url", "installed helper has no CLIPASTE_URL")
                    .with_fix(&setup_hint),
            );
            return checks;
        }
    };

    match common::http_get(&format!("{url}/health")) {
        Some(body) if is_clipaste_health(&body) => {
            let v = health_version(&body).unwrap_or_else(|| "unknown".into());
            checks.push(Check::ok("bridge", format!("{url} responding (daemon v{v})")));
        }
        _ => {
            let fix = match role {
                Role::Wsl2 => "clipaste wsl-setup   # re-probe the Windows host address".to_string(),
                _ => "reconnect: the SSH RemoteForward is only active inside a session opened after ssh-setup".to_string(),
            };
            checks.push(
                Check::fail("bridge", format!("{url} is not answering")).with_fix(fix),
            );
        }
    }

    // The xclip shim is what makes Claude Code's native Ctrl+V work. It is
    // Linux-only by design: on macOS nothing shells out to xclip, so its
    // absence there is correct rather than a problem.
    if cfg!(target_os = "linux") {
        let shim = bin.join("xclip");
        if !is_executable(&shim) {
            checks.push(
                Check::warn("xclip-shim", "not installed — Ctrl+V in Claude Code will not fetch")
                    .with_fix(&setup_hint),
            );
        } else {
            match which("xclip") {
                Some(p) if p == shim => {
                    checks.push(Check::ok("xclip-shim", "shim takes precedence on PATH"))
                }
                Some(p) => checks.push(
                    Check::warn(
                        "xclip-shim",
                        format!("shadowed by {} — the real xclip wins on PATH", p.display()),
                    )
                    .with_fix("put $HOME/.local/bin BEFORE the system paths in PATH"),
                ),
                None => checks.push(Check::warn("xclip-shim", "installed but not on PATH")),
            }
        }
    } else if cfg!(target_os = "macos") {
        checks.push(Check::ok(
            "xclip-shim",
            "not applicable on a macOS host — use clipaste-paste",
        ));
    }

    checks
}

// ─── Rendering ───

fn render_human(r: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "clipaste doctor — v{} on {} ({})\n\n",
        common::VERSION,
        std::env::consts::OS,
        r.role.as_str()
    ));
    for c in &r.checks {
        out.push_str(&format!("  {} {:<14} {}\n", c.status.glyph(), c.name, c.detail));
        if let Some(fix) = &c.fix {
            out.push_str(&format!("      → {fix}\n"));
        }
    }
    out.push('\n');
    out.push_str(match r.worst() {
        Status::Ok => "All good.\n",
        Status::Warn => "Usable, with warnings above.\n",
        Status::Fail if r.role == Role::UnsupportedHost => {
            "Local clipboard hosting is unavailable on this platform; see the supported roles above.\n"
        }
        Status::Fail => "Not working — run the → commands above.\n",
    });
    out
}

fn render_json(r: &Report) -> String {
    let e = common::json_escape;
    let checks: Vec<String> = r
        .checks
        .iter()
        .map(|c| {
            let fix = match &c.fix {
                Some(f) => format!("\"{}\"", e(f)),
                None => "null".to_string(),
            };
            format!(
                "{{\"name\":\"{}\",\"status\":\"{}\",\"detail\":\"{}\",\"fix\":{}}}",
                e(c.name),
                c.status.as_str(),
                e(&c.detail),
                fix
            )
        })
        .collect();

    format!(
        "{{\"version\":\"{}\",\"os\":\"{}\",\"role\":\"{}\",\"status\":\"{}\",\"checks\":[{}]}}",
        e(common::VERSION),
        e(std::env::consts::OS),
        r.role.as_str(),
        r.worst().as_str(),
        checks.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_linux_is_not_assumed_to_be_an_ssh_remote() {
        assert_eq!(
            classify_role("linux", false, false, false),
            Role::UnsupportedHost
        );
    }

    #[test]
    fn role_detection_preserves_existing_consumers_and_supported_hosts() {
        assert_eq!(classify_role("linux", false, true, false), Role::SshRemote);
        assert_eq!(classify_role("linux", false, false, true), Role::SshRemote);
        assert_eq!(classify_role("linux", true, true, true), Role::Wsl2);
        assert_eq!(classify_role("macos", false, true, true), Role::SshRemote);
        assert_eq!(classify_role("macos", false, false, true), Role::ClipboardHost);
        assert_eq!(classify_role("windows", true, false, true), Role::ClipboardHost);
    }

    #[test]
    fn unsupported_host_reports_capability_without_install_or_network_checks() {
        let report = diagnose_role(Role::UnsupportedHost);
        assert_eq!(report.exit_code(), 1);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].name, "platform");
        assert!(report.checks[0].fix.is_none());
        let json = render_json(&report);
        assert!(json.contains("\"role\":\"unsupported-host\""));
        assert!(json.contains("\"status\":\"fail\""));
        assert!(json.contains("macOS or Windows"));
        let human = render_human(&report);
        assert!(human.contains("clipaste ssh-setup"));
        assert!(!human.contains("run the → commands"));
    }

    #[test]
    fn parses_helper_url() {
        let shim = "#!/bin/bash\n# comment\nCLIPASTE_URL=\"http://127.0.0.1:18340\"\necho hi\n";
        assert_eq!(
            parse_helper_url(shim).as_deref(),
            Some("http://127.0.0.1:18340")
        );
        // An un-substituted template must not be reported as a configured URL
        assert_eq!(
            parse_helper_url("CLIPASTE_URL=\"__CLIPASTE_URL__\"\n"),
            None
        );
        assert_eq!(parse_helper_url("no url here\n"), None);
    }

    #[test]
    fn reads_health_fields() {
        let body = "{\"status\":\"ok\",\"version\":\"2.4.1\"}";
        assert!(is_clipaste_health(body));
        assert_eq!(health_version(body).as_deref(), Some("2.4.1"));
        assert!(!is_clipaste_health("<html>nginx</html>"));
        assert_eq!(health_version("{\"status\":\"ok\"}"), None);
    }

    #[test]
    fn worst_status_and_exit_code() {
        let mk = |s: Status| Check { name: "x", status: s, detail: String::new(), fix: None };

        let ok = Report { role: Role::ClipboardHost, checks: vec![mk(Status::Ok)] };
        assert_eq!(ok.worst(), Status::Ok);
        assert_eq!(ok.exit_code(), 0);

        // A warning is a usable state — "no screenshot yet" must not exit 1
        let warn = Report {
            role: Role::ClipboardHost,
            checks: vec![mk(Status::Ok), mk(Status::Warn)],
        };
        assert_eq!(warn.worst(), Status::Warn);
        assert_eq!(warn.exit_code(), 0);

        let fail = Report {
            role: Role::Wsl2,
            checks: vec![mk(Status::Ok), mk(Status::Warn), mk(Status::Fail)],
        };
        assert_eq!(fail.worst(), Status::Fail);
        assert_eq!(fail.exit_code(), 1);
    }

    #[test]
    fn json_is_well_formed_and_escaped() {
        let r = Report {
            role: Role::SshRemote,
            checks: vec![Check::fail("bridge", "a \"quoted\" thing\nwith newline")
                .with_fix("echo \\ backslash")],
        };
        let json = render_json(&r);
        assert!(json.contains("\"role\":\"ssh-remote\""));
        assert!(json.contains("\"status\":\"fail\""));
        assert!(json.contains("\\\"quoted\\\""));
        assert!(json.contains("\\n"));
        assert!(json.contains("\\\\ backslash"));
        // No raw control characters may survive into the payload
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_reports_null_fix_when_healthy() {
        let r = Report {
            role: Role::ClipboardHost,
            checks: vec![Check::ok("daemon", "responding")],
        };
        assert!(render_json(&r).contains("\"fix\":null"));
    }

    #[test]
    fn human_output_lists_every_fix() {
        let r = Report {
            role: Role::Wsl2,
            checks: vec![Check::fail("helper", "missing").with_fix("clipaste wsl-setup")],
        };
        let text = render_human(&r);
        assert!(text.contains("✗ helper"));
        assert!(text.contains("→ clipaste wsl-setup"));
        assert!(text.contains("Not working"));
    }
}
