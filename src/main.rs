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

    // clipaste ssh-setup user@host
    if args.len() >= 3 && args[1] == "ssh-setup" {
        ssh_setup::run_ssh(&args[2]);
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
