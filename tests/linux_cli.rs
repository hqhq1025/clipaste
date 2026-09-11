#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

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
fn unsupported_daemon_and_host_setup_fail_before_side_effects() {
    let home = TestHome::new();
    for args in [vec![], vec!["ssh-setup", "user@example.invalid"]] {
        let out = home.command().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(error.contains("Linux clipboard host is not supported"));
        assert!(error.contains("macOS or Windows"));
        assert!(error.contains("clipaste-paste"));
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
        assert!(json.contains("\"role\":\"unsupported-host\""));
        assert!(json.contains("\"name\":\"platform\""));
        assert!(json.contains("\"fix\":null"));
        assert!(!json.contains("\"name\":\"curl\""));
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
            assert!(stdout(&out).contains("native Linux clipboard hosts are unsupported"));
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
