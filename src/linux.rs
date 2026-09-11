//! Read the desktop clipboard without taking ownership or modifying its formats.
//! The system tools implement the selection protocols, including X11 INCR.

use crate::common::{self, LatestImage};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(300);
const MAX_PNG: usize = 64 * 1024 * 1024;
const MAX_TYPES: usize = 64 * 1024;
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

pub fn install_signal_handlers() -> io::Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only sets an atomic flag; normal code owns cleanup.
        if unsafe { libc::signal(signal, stop_signal as *const () as libc::sighandler_t) }
            == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Wayland,
    X11,
}

pub struct Backend {
    kind: Kind,
    program: PathBuf,
    pub detail: String,
    pub warning: Option<String>,
}

pub fn has_display() -> bool {
    nonempty_env("WAYLAND_DISPLAY") || nonempty_env("DISPLAY")
}

fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|s| !s.is_empty())
}

/// Resolve the real desktop tool, never the HTTP consumer shim installed by us.
fn desktop_tool(name: &str) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        let executable = fs::metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
        if !executable {
            continue;
        }
        let mut header = Vec::new();
        if fs::File::open(&candidate)
            .and_then(|file| file.take(4096).read_to_end(&mut header))
            .is_err()
        {
            continue;
        }
        if String::from_utf8_lossy(&header).contains("CLIPASTE_URL=") {
            continue;
        }
        return fs::canonicalize(&candidate).map_err(|e| format!("{name}: {e}"));
    }
    let package = match name {
        "wl-paste" => "wl-clipboard",
        _ => name,
    };
    Err(format!(
        "{name} not found (clipaste consumer shims do not count). \
         Install {package} with your package manager, e.g. sudo apt install {package}"
    ))
}

pub fn detect() -> Result<Backend, String> {
    let requested = std::env::var("CLIPASTE_BACKEND").unwrap_or_else(|_| "auto".into());
    if !matches!(requested.as_str(), "auto" | "wayland" | "x11") {
        return Err("CLIPASTE_BACKEND must be auto, wayland, or x11".into());
    }
    if !has_display() {
        return Err(
            "No Linux graphical session: WAYLAND_DISPLAY and DISPLAY are unset. \
             Start clipaste from your desktop session, not a headless SSH shell. \
             For a remote consumer, run ssh-setup on the clipboard host."
                .into(),
        );
    }
    let mut wayland_error = None;
    if requested != "x11" && (nonempty_env("WAYLAND_DISPLAY") || requested == "wayland") {
        match wayland_backend() {
            Ok(backend) => return Ok(backend),
            Err(e) if requested == "wayland" => return Err(e),
            Err(e) => wayland_error = Some(e),
        }
    }
    if nonempty_env("DISPLAY") {
        let backend = (|| {
            let program = desktop_tool("xclip")?;
            let backend = Backend {
                kind: Kind::X11,
                detail: format!("X11 clipboard via {}", program.display()),
                program,
                warning: wayland_error.clone().map(|e| {
                    format!(
                        "Native Wayland unavailable: {e}. Using XWayland via DISPLAY; \
                         this relies on your compositor's clipboard bridge. \
                         Verify a screenshot from your Wayland app with clipaste doctor."
                    )
                }),
            };
            backend.types()?;
            Ok(backend)
        })();
        return backend.map_err(|e: String| match wayland_error {
            Some(w) => format!("Wayland: {w}\nXWayland: {e}"),
            None => e,
        });
    }
    Err(wayland_error
        .unwrap_or_else(|| "X11 requires DISPLAY. Run clipaste in a graphical X11 session.".into()))
}

fn wayland_backend() -> Result<Backend, String> {
    let program = desktop_tool("wl-paste")?;
    let version = command_output(&program, &["--version"], MAX_TYPES, TIMEOUT)?;
    if !version.status.success() || !supports_empty_events(&version.stdout) {
        return Err(
            "Wayland requires wl-clipboard 2.2 or later for empty-selection events. \
            Upgrade wl-clipboard or use XWayland (xclip + DISPLAY)."
                .into(),
        );
    }
    let backend = Backend {
        kind: Kind::Wayland,
        detail: format!("Wayland data-control clipboard via {}", program.display()),
        program,
        warning: None,
    };
    backend.types().map_err(|e| {
        format!(
            "Wayland background clipboard access failed: {e}. \
         wl-paste must support the compositor's data-control protocol \
         (ext-only compositors need wl-clipboard 2.3+ built with ext support). \
         Use XWayland (xclip + DISPLAY) or an X11 session if unavailable."
        )
    })?;
    Ok(backend)
}

fn supports_empty_events(version: &[u8]) -> bool {
    let text = String::from_utf8_lossy(version);
    let Some(number) = text.split_whitespace().nth(1) else {
        return false;
    };
    let mut parts = number.split('.');
    matches!((parts.next().and_then(|n| n.parse::<u32>().ok()),
              parts.next().and_then(|n| n.parse::<u32>().ok())),
             (Some(major), Some(minor)) if major > 2 || (major == 2 && minor >= 2))
}

fn offers_png(types: &str) -> bool {
    let types: Vec<_> = types.lines().map(str::trim).collect();
    // Respect clipboard-history exclusions before requesting image bytes.
    !types.iter().any(|t| {
        matches!(
            *t,
            "x-kde-passwordManagerHint"
                | "application/x-keepassxc"
                | "application/x-clipman-ignore"
        )
    }) && types.contains(&"image/png")
}

impl Backend {
    fn success(&self, out: &CommandOutput) -> bool {
        out.status.success()
            || (self.kind == Kind::Wayland && out.status.signal() == Some(libc::SIGTERM))
    }

    fn types(&self) -> Result<String, String> {
        let args: &[&str] = match self.kind {
            // --watch refuses compositors lacking data-control instead of
            // creating a popup. For an empty selection, stop after its callback;
            // for nonempty selections, --list-types prints and exits itself.
            Kind::Wayland => &[
                "--list-types",
                "--watch",
                "/bin/sh",
                "-c",
                "kill -TERM \"$PPID\"",
            ],
            Kind::X11 => &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        };
        let out = command_output(&self.program, args, MAX_TYPES, TIMEOUT)?;
        if self.success(&out) {
            return String::from_utf8(out.stdout).map_err(|_| "Invalid clipboard type list".into());
        }
        let error = out.error();
        let empty = match self.kind {
            Kind::Wayland => matches!(error.trim(), "Nothing is copied" | "No selection"),
            Kind::X11 => error.trim() == "Error: target TARGETS not available",
        };
        if empty {
            Ok(String::new())
        } else {
            Err(format!("{}: {error}", self.program.display()))
        }
    }

    pub fn read_png(&self) -> Result<Option<Vec<u8>>, String> {
        if !offers_png(&self.types()?) {
            return Ok(None);
        }
        let args: &[&str] = match self.kind {
            // One complete transfer in no-popup watch mode. Stop the watcher
            // only after cat has finished writing the PNG to our bounded pipe.
            Kind::Wayland => &[
                "--no-newline",
                "--type",
                "image/png",
                "--watch",
                "/bin/sh",
                "-c",
                "/bin/cat; kill -TERM \"$PPID\"",
            ],
            Kind::X11 => &["-selection", "clipboard", "-t", "image/png", "-o"],
        };
        let out = match command_output(&self.program, args, MAX_PNG, TIMEOUT) {
            Err(e) if e.contains("output exceeds the size limit") => return Ok(None),
            result => result?,
        };
        if !self.success(&out) {
            return Err(format!("Clipboard image read failed: {}", out.error()));
        }
        // Selection transfers can race a copy of text or a privacy-marked item.
        if !offers_png(&self.types()?) {
            return Ok(None);
        }
        Ok(Some(out.stdout))
    }
}

fn validate_png(bytes: &[u8]) -> Result<(), String> {
    let mut reader =
        image::ImageReader::with_format(io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| format!("Invalid or oversized clipboard PNG: {e}"))?;
    Ok(())
}

/// `save` is injected to keep transition tests off the user's persistent cache.
fn stage(
    latest: &LatestImage,
    png: Option<&[u8]>,
    previous: &mut Option<Vec<u8>>,
    save: impl FnOnce(&[u8]) -> Option<PathBuf>,
) -> Result<(), String> {
    let published = latest.lock().unwrap().clone();
    if png == previous.as_deref()
        && (png.is_none() || published.as_ref().is_some_and(|p| p.is_file()))
    {
        return Ok(());
    }
    *latest.lock().unwrap() = None;
    *previous = None;
    if let Some(bytes) = png {
        let path = save(bytes).ok_or("Cannot publish the clipboard PNG cache")?;
        *latest.lock().unwrap() = Some(path);
        *previous = Some(bytes.to_vec());
    }
    Ok(())
}

pub fn run(backend: Backend, latest: LatestImage) -> Result<(), String> {
    common::log(&backend.detail);
    if let Some(warning) = &backend.warning {
        common::log(warning);
    }
    let mut previous = None;
    let mut rejected = None;
    let mut failures = 0;
    while !STOP.load(Ordering::Relaxed) {
        let result = backend.read_png().and_then(|mut png| {
            if let Some(bytes) = &png {
                if rejected.as_ref() == Some(bytes) {
                    png = None;
                } else if previous.as_ref() == Some(bytes) {
                    rejected = None;
                } else if let Err(e) = validate_png(bytes) {
                    if rejected.as_ref() != Some(bytes) {
                        common::log(&e);
                    }
                    rejected = png.take();
                } else {
                    rejected = None;
                }
            }
            stage(
                &latest,
                png.as_deref(),
                &mut previous,
                common::save_png_to_temp,
            )
        });
        match result {
            Ok(()) => failures = 0,
            Err(e) => {
                if STOP.load(Ordering::Relaxed) {
                    break;
                }
                *latest.lock().unwrap() = None;
                previous = None;
                failures += 1;
                common::log(&e);
                if failures >= 3 {
                    return Err("Clipboard access failed three times; stopping the daemon. \
                        Check the desktop session and run clipaste doctor before restarting."
                        .into());
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    *latest.lock().unwrap() = None;
    Ok(())
}

#[derive(Debug)]
struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CommandOutput {
    fn error(&self) -> String {
        let error = String::from_utf8_lossy(&self.stderr);
        if error.trim().is_empty() {
            format!("exited with {}", self.status)
        } else {
            error.trim().to_string()
        }
    }
}

struct ChildGroup(Child);

impl Drop for ChildGroup {
    fn drop(&mut self) {
        // SAFETY: process_group(0) made this child the leader of its own group.
        // Kill descendants too: wl-paste delegates the actual pipe read to cat.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fd belongs to a live child pipe; no pointer arguments are involved.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut buf = [0; 8192];
    // Yield even when a producer never closes its pipe, so timeout/stderr are
    // checked and one busy pipe cannot starve the other.
    for _ in 0..32 {
        match pipe.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                if bytes.len() + n > limit {
                    return Err(io::Error::other(
                        "clipboard command output exceeds the size limit",
                    ));
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn command_output(
    program: impl AsRef<OsStr>,
    args: &[&str],
    limit: usize,
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut child = ChildGroup(
        Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let mut stdout = child.0.stdout.take().unwrap();
    let mut stderr = child.0.stderr.take().unwrap();
    nonblocking(stdout.as_raw_fd()).map_err(|e| e.to_string())?;
    nonblocking(stderr.as_raw_fd()).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let mut out = Vec::new();
    let mut err = Vec::new();
    loop {
        let out_done = drain(&mut stdout, &mut out, limit).map_err(|e| e.to_string())?;
        let err_done = drain(&mut stderr, &mut err, MAX_TYPES).map_err(|e| e.to_string())?;
        if out_done && err_done {
            if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
                return Ok(CommandOutput {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "Clipboard command timed out after {} ms",
                timeout.as_millis()
            ));
        }
        if STOP.load(Ordering::Relaxed) {
            return Err("Clipboard read interrupted".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_and_sensitive_types_are_explicit() {
        assert!(offers_png("TARGETS\nimage/png\ntext/html\ntext/plain\n"));
        assert!(!offers_png("image/png\nx-kde-passwordManagerHint\n"));
        assert!(!offers_png("text/uri-list\n"));
        assert!(!supports_empty_events(b"wl-paste 2.1.0"));
        assert!(supports_empty_events(b"wl-paste 2.2.1"));
        assert!(supports_empty_events(b"wl-paste 2.3.0"));
    }

    #[test]
    fn transitions_deduplicate_clear_and_retry_cache_failures() {
        let latest = LatestImage::default();
        let mut previous = None;
        stage(&latest, Some(b"a"), &mut previous, |_| {
            Some(std::env::current_exe().unwrap())
        })
        .unwrap();
        stage(&latest, Some(b"a"), &mut previous, |_| {
            panic!("duplicate write")
        })
        .unwrap();
        assert_eq!(
            *latest.lock().unwrap(),
            Some(std::env::current_exe().unwrap())
        );
        stage(&latest, None, &mut previous, |_| panic!("non-image write")).unwrap();
        assert!(latest.lock().unwrap().is_none());
        assert!(stage(&latest, Some(b"b"), &mut previous, |_| None).is_err());
        assert!(latest.lock().unwrap().is_none());
        stage(&latest, Some(b"b"), &mut previous, |_| {
            Some(PathBuf::from("b"))
        })
        .unwrap();
        assert_eq!(*latest.lock().unwrap(), Some(PathBuf::from("b")));
    }

    #[test]
    fn rejects_corrupt_or_mislabeled_images() {
        assert!(validate_png(b"not a png").is_err());
        let mut png = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        validate_png(&png).unwrap();
        assert!(validate_png(&png[..png.len() / 2]).is_err());
    }

    #[test]
    fn commands_have_bounded_time_and_output_even_with_descendants() {
        let start = Instant::now();
        assert!(command_output(
            "/bin/sh",
            &["-c", "sleep 30 & wait"],
            1024,
            Duration::from_millis(50)
        )
        .unwrap_err()
        .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            command_output("/bin/sh", &["-c", "printf 12345"], 4, TIMEOUT)
                .unwrap_err()
                .contains("size limit")
        );
        let out = command_output(
            "/bin/sh",
            &["-c", "printf ok; printf error >&2; exit 7"],
            10,
            TIMEOUT,
        )
        .unwrap();
        assert_eq!(out.stdout, b"ok");
        assert_eq!(out.error(), "error");
        assert_eq!(out.status.code(), Some(7));
    }
}
