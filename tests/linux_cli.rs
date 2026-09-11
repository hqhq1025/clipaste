#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct TestHome(PathBuf);

impl TestHome {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "clipaste-linux-cli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clipaste"));
        // No inherited SSH/WSL variables or curl/ssh executables. These tests
        // must not contact a daemon or alter the real user's configuration.
        cmd.env_clear()
            .env("HOME", &self.0)
            .env("PATH", &self.0)
            .env("XDG_CACHE_HOME", self.0.join("cache"))
            .current_dir(&self.0);
        cmd
    }

    fn script(&self, name: &str, content: &str) {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(Command::new("/bin/sh")
            .arg("-n")
            .arg(path)
            .status()
            .unwrap()
            .success());
    }

    fn fake_curl(&self) {
        self.script(
            "curl",
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" >> \"$HOME/curl-args\"\n\
             if [ \"$CLIPASTE_TEST_CURL_FAIL\" = 1 ]; then exit 22; fi\n\
             printf '%s' '{\"status\":\"ok\",\"version\":\"test\"}'\n",
        );
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn wsl_kernel() -> bool {
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap()
        .to_lowercase();
    kernel.contains("microsoft") || kernel.contains("wsl")
}

#[test]
fn headless_daemon_fails_before_side_effects() {
    let home = TestHome::new();
    {
        let out = home.command().output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(error.contains(if wsl_kernel() {
            "WSL2 is a clipboard consumer"
        } else {
            "No Linux graphical session"
        }));
        assert!(!error.contains("http server"));
        assert!(!error.contains("brew services start"));
        assert_eq!(fs::read_dir(&home.0).unwrap().count(), 0);
    }
}

#[test]
fn native_linux_or_wsl_doctor_reports_the_applicable_failure() {
    let home = TestHome::new();
    let out = home.command().args(["doctor", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let json = stdout(&out);
    assert!(json.contains("\"status\":\"fail\""));
    // Clearing environment variables cannot change a real WSL kernel.
    if wsl_kernel() {
        assert!(json.contains("\"role\":\"wsl2\""));
        assert!(json.contains("\"name\":\"helper\""));
        assert!(!json.contains("\"name\":\"platform\""));
    } else {
        assert!(json.contains("\"role\":\"clipboard-host\""));
        assert!(json.contains("\"name\":\"backend\""));
        assert!(json.contains("\"fix\":null"));
        assert!(!json.contains("\"name\":\"helper\""));
    }
    assert_eq!(fs::read_dir(&home.0).unwrap().count(), 0);
}

#[test]
fn ssh_and_wsl_consumers_keep_their_existing_diagnostic_paths() {
    let home = TestHome::new();
    for (var, value, role) in [
        (
            "SSH_CONNECTION",
            "192.0.2.1 1234 192.0.2.2 22",
            "ssh-remote",
        ),
        ("WSL_DISTRO_NAME", "Ubuntu", "wsl2"),
    ] {
        let out = home
            .command()
            .env(var, value)
            .args(["doctor", "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let json = stdout(&out);
        let expected_role = if wsl_kernel() { "wsl2" } else { role };
        assert!(json.contains(&format!("\"role\":\"{expected_role}\"")));
        assert!(json.contains("\"name\":\"helper\""));
        assert!(!json.contains("\"name\":\"platform\""));
    }
}

#[test]
fn configured_consumer_without_ssh_variables_is_still_diagnosed() {
    let home = TestHome::new();
    let bin = home.0.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    // Intentionally non-executable, so consumer checks stop before HTTP probes.
    fs::write(
        bin.join("clipaste-paste"),
        "CLIPASTE_URL=\"http://127.0.0.1:18340\"\n",
    )
    .unwrap();
    let out = home.command().args(["doctor", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let role = if wsl_kernel() { "wsl2" } else { "ssh-remote" };
    assert!(stdout(&out).contains(&format!("\"role\":\"{role}\"")));
    assert!(stdout(&out).contains("\"name\":\"helper\""));
}

#[test]
fn configured_consumer_checks_its_bridge_without_real_network_access() {
    let home = TestHome::new();
    let bin = home.0.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    home.script(
        ".local/bin/clipaste-paste",
        "#!/bin/sh\nCLIPASTE_URL=\"http://127.0.0.1:18444\"\nexit 0\n",
    );
    home.fake_curl();
    let path = std::env::join_paths([&bin, &home.0]).unwrap();
    for (failure, code, bridge_status) in [("0", 0, "ok"), ("1", 1, "fail")] {
        let out = home
            .command()
            .env("PATH", &path)
            .env("CLIPASTE_TEST_CURL_FAIL", failure)
            .args(["doctor", "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(code));
        assert!(stdout(&out).contains(&format!(
            "\"name\":\"bridge\",\"status\":\"{bridge_status}\""
        )));
        assert!(!stdout(&out).contains("\"name\":\"platform\""));
    }
    let args = fs::read_to_string(home.0.join("curl-args")).unwrap();
    assert_eq!(args.matches("http://127.0.0.1:18444/health").count(), 2);
}

#[test]
fn valid_wsl_setup_still_installs_consumer_helpers() {
    let home = TestHome::new();
    home.fake_curl();
    let out = home
        .command()
        .env("WSL_DISTRO_NAME", "Ubuntu")
        .args(["wsl-setup", "--host", "127.0.0.1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    for name in ["clipaste-paste", "xclip", "wl-paste"] {
        let path = home.0.join(".local/bin").join(name);
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("http://127.0.0.1:18340"));
        assert_ne!(fs::metadata(path).unwrap().permissions().mode() & 0o111, 0);
    }
    assert!(home.0.join(".bashrc").is_file());
    assert!(fs::read_to_string(home.0.join("curl-args"))
        .unwrap()
        .contains("/health"));
}

#[test]
fn help_version_and_consumer_argument_validation_remain_available() {
    let home = TestHome::new();
    for flag in ["--help", "--version"] {
        let out = home.command().arg(flag).output().unwrap();
        assert!(out.status.success());
        if flag == "--help" {
            assert!(stdout(&out).contains("Linux desktop"));
            assert!(stdout(&out).contains("wsl-setup"));
        }
    }
    let out = home
        .command()
        .args(["wsl-setup", "--bad-option"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("unexpected argument"));
    assert!(!error.contains("clipboard host is not supported"));
}

#[test]
fn desktop_backend_checks_dependencies_and_never_executes_consumer_shims() {
    if wsl_kernel() {
        return;
    }
    let home = TestHome::new();
    home.script("xclip", "#!/bin/sh\nCLIPASTE_URL=\"http://127.0.0.1:18340\"\nprintf called > \"$HOME/shim-called\"\n");
    let out = home
        .command()
        .env("DISPLAY", ":999")
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("consumer shims do not count"));
    assert!(!home.0.join("shim-called").exists());
    assert!(!home.0.join("cache").exists());
}

#[test]
fn wayland_failure_and_xwayland_fallback_are_visible() {
    if wsl_kernel() {
        return;
    }
    let home = TestHome::new();
    home.fake_curl();
    home.script(
        "wl-paste",
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'wl-paste 2.3.0\\n'; exit 0; fi\n\
         case \" $* \" in *' --watch '*) printf 'Watch mode requires data-control\\n' >&2; exit 1;; esac\n\
         printf called > \"$HOME/unsafe-paste-called\"\n",
    );
    home.script("xclip", "#!/bin/sh\nprintf 'TARGETS\\ntext/plain\\n'\n");
    let mut command = home.command();
    command
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("DISPLAY", ":999");
    let out = command.args(["doctor", "--json"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("Using XWayland"));
    assert!(!home.0.join("unsafe-paste-called").exists());
    let out = home
        .command()
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("DISPLAY", ":999")
        .env("CLIPASTE_BACKEND", "wayland")
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("Watch mode requires data-control"));
    assert!(!home.0.join("unsafe-paste-called").exists());
}

#[test]
fn native_wayland_backend_accepts_data_control_and_empty_clipboard() {
    if wsl_kernel() {
        return;
    }
    let home = TestHome::new();
    home.fake_curl();
    home.script(
        "wl-paste",
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'wl-paste 2.2.1\\n'; exit 0; fi\n\
         printf 'Nothing is copied\\n' >&2\nexit 1\n",
    );
    let out = home
        .command()
        .env("WAYLAND_DISPLAY", "wayland-test")
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("Wayland data-control clipboard"));
    assert!(!stdout(&out).contains("Using XWayland"));
}

#[test]
fn wayland_requires_empty_selection_event_support() {
    if wsl_kernel() {
        return;
    }
    let home = TestHome::new();
    home.fake_curl();
    for (version, code) in [("2.1.0", 1), ("2.2.1", 0), ("2.3.0", 0)] {
        home.script("wl-paste", &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'wl-paste {version}\\n'; exit 0; fi\n\
             printf called > \"$HOME/paste-called\"\nprintf 'Nothing is copied\\n' >&2\nexit 1\n"
        ));
        let out = home
            .command()
            .env("WAYLAND_DISPLAY", "wayland-test")
            .args(["doctor", "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(code), "{out:?}");
        if code == 1 {
            assert!(stdout(&out).contains("2.2 or later"));
            assert!(!home.0.join("paste-called").exists());
        } else {
            assert!(home.0.join("paste-called").exists());
        }
    }
}

#[test]
fn linux_ssh_setup_reports_linux_start_command_without_connecting() {
    if wsl_kernel() {
        return;
    }
    let home = TestHome::new();
    home.fake_curl();
    let out = home
        .command()
        .env("CLIPASTE_TEST_CURL_FAIL", "1")
        .args(["ssh-setup", "user@example.invalid"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("Linux graphical desktop session"));
    assert!(!error.contains("brew"));
    assert!(!home.0.join(".ssh").exists());
}

#[test]
fn normal_stop_cleans_up_a_hung_clipboard_process_group() {
    if wsl_kernel() {
        return;
    }
    for args in [vec![], vec!["doctor", "--json"]] {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            stop_hung_command(&args, signal);
        }
    }
}

fn stop_hung_command(args: &[&str], signal: libc::c_int) {
    let home = TestHome::new();
    home.script(
        "xclip",
        "#!/bin/sh\n/bin/sleep 30 &\nprintf '%s %s' \"$$\" \"$!\" > \"$HOME/children\"\nwait\n",
    );
    let mut daemon = home
        .command()
        .args(args)
        .env("DISPLAY", ":999")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let children = home.0.join("children");
    while !children.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if !children.exists() {
        let _ = daemon.kill();
        let _ = daemon.wait();
        panic!("clipboard command did not start");
    }
    // The readiness file is written by the hung command, not by a timed guess.
    let pids = fs::read_to_string(children).unwrap();
    unsafe { libc::kill(daemon.id() as i32, signal) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while daemon.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if daemon.try_wait().unwrap().is_none() {
        let _ = daemon.kill();
        let _ = daemon.wait();
        panic!("daemon did not stop");
    }
    for pid in pids.split_whitespace() {
        let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        assert!(
            status.is_empty()
                || status
                    .lines()
                    .any(|line| line.starts_with("State:") && line.contains('Z')),
            "clipboard subprocess {pid} still running: {status}"
        );
    }
}
