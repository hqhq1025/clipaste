mod common;
mod server;
mod ssh_setup;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("clipaste {}", common::VERSION);
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        common::print_help();
        return;
    }

    // clipaste ssh-setup [-p PORT] user@host [-p PORT]
    if args.len() >= 3 && args[1] == "ssh-setup" {
        match parse_ssh_setup_args(&args[2..]) {
            Ok((host, ssh_port)) => ssh_setup::run_ssh(&host, ssh_port),
            Err(e) => {
                eprintln!("clipaste ssh-setup: {e}");
                eprintln!("usage: clipaste ssh-setup [-p PORT] user@host");
                std::process::exit(1);
            }
        }
        return;
    }

    // clipaste wsl-setup (run inside WSL2)
    if args.len() >= 2 && args[1] == "wsl-setup" {
        ssh_setup::run_wsl();
        return;
    }

    // Start HTTP server for remote access
    let latest = common::LatestImage::default();
    server::start(latest.clone());

    // Start clipboard watcher (platform-specific)
    #[cfg(target_os = "macos")]
    macos::run(latest);

    #[cfg(target_os = "windows")]
    windows::run(latest);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        eprintln!("clipaste: unsupported platform");
        std::process::exit(1);
    }
}

/// Parse `ssh-setup` arguments into (host, optional SSH port).
///
/// Accepts a `-p PORT` / `--port PORT` flag in any position, e.g.:
///   ssh-setup user@host -p 22222
///   ssh-setup -p 22222 user@host
/// The first non-flag argument is the host. This `-p` is the *SSH connection*
/// port; the clipaste HTTP port stays fixed at `common::DEFAULT_PORT`.
fn parse_ssh_setup_args(args: &[String]) -> Result<(String, Option<u16>), String> {
    let mut host: Option<String> = None;
    let mut ssh_port: Option<u16> = None;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-p" || a == "--port" {
            let val = args
                .get(i + 1)
                .ok_or_else(|| format!("{a} requires a port number"))?;
            ssh_port = Some(
                val.parse::<u16>()
                    .map_err(|_| format!("invalid port: {val}"))?,
            );
            i += 2;
            continue;
        }
        if let Some(rest) = a.strip_prefix("--port=") {
            ssh_port = Some(rest.parse::<u16>().map_err(|_| format!("invalid port: {rest}"))?);
            i += 1;
            continue;
        }
        if let Some(rest) = a.strip_prefix("-p") {
            // -p22222 (attached form)
            if !rest.is_empty() {
                ssh_port = Some(rest.parse::<u16>().map_err(|_| format!("invalid port: {rest}"))?);
                i += 1;
                continue;
            }
        }
        if a.starts_with('-') {
            return Err(format!("unknown flag: {a}"));
        }
        if host.is_none() {
            host = Some(a.clone());
        } else {
            return Err(format!("unexpected argument: {a}"));
        }
        i += 1;
    }

    match host {
        Some(h) => Ok((h, ssh_port)),
        None => Err("missing host (user@host)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_ssh_setup_args;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn host_only() {
        let (h, p) = parse_ssh_setup_args(&s(&["user@host"])).unwrap();
        assert_eq!(h, "user@host");
        assert_eq!(p, None);
    }

    #[test]
    fn host_then_port() {
        let (h, p) = parse_ssh_setup_args(&s(&["user@host", "-p", "22222"])).unwrap();
        assert_eq!(h, "user@host");
        assert_eq!(p, Some(22222));
    }

    #[test]
    fn port_then_host() {
        let (h, p) = parse_ssh_setup_args(&s(&["-p", "2200", "user@host"])).unwrap();
        assert_eq!(h, "user@host");
        assert_eq!(p, Some(2200));
    }

    #[test]
    fn long_and_attached_forms() {
        let (_, p1) = parse_ssh_setup_args(&s(&["h", "--port", "10"])).unwrap();
        assert_eq!(p1, Some(10));
        let (_, p2) = parse_ssh_setup_args(&s(&["h", "--port=11"])).unwrap();
        assert_eq!(p2, Some(11));
        let (_, p3) = parse_ssh_setup_args(&s(&["h", "-p12"])).unwrap();
        assert_eq!(p3, Some(12));
    }

    #[test]
    fn errors() {
        assert!(parse_ssh_setup_args(&s(&["-p"])).is_err()); // missing value
        assert!(parse_ssh_setup_args(&s(&["-p", "abc", "h"])).is_err()); // bad port
        assert!(parse_ssh_setup_args(&s(&["-p", "70000", "h"])).is_err()); // overflow u16
        assert!(parse_ssh_setup_args(&s(&["-x", "h"])).is_err()); // unknown flag
        assert!(parse_ssh_setup_args(&s(&[])).is_err()); // no host
    }
}
