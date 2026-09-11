#[cfg(any(target_os = "macos", target_os = "windows", test))]
use image::codecs::png::PngEncoder;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use image::ImageEncoder;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use image::ImageFormat;
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::io::Cursor;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub const VERSION: &str = "2.5.0";
pub const DEFAULT_PORT: u16 = 18340;

pub fn supports_clipboard_host(os: &str) -> bool {
    matches!(os, "macos" | "windows" | "linux")
}

pub fn unsupported_host_message(os: &str) -> String {
    format!(
        "{os} clipboard host is not supported. The daemon requires macOS, Windows, or a Linux desktop.\n\
         Linux is supported as an SSH consumer: run `clipaste ssh-setup user@host` on \
         the clipboard host, then use `clipaste-paste` on the remote.\n\
         In WSL2, run `clipaste wsl-setup` with clipaste.exe running on Windows.\n\
         See https://github.com/hqhq1025/clipaste#supported-platforms"
    )
}

/// Shared state: path to the most recently saved screenshot PNG
pub type LatestImage = Arc<Mutex<Option<PathBuf>>>;

pub fn temp_dir() -> PathBuf {
    cache_root().join("clipaste")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cache_root() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let p = PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            return p.join(".cache");
        }
    }
    std::env::temp_dir()
}

#[cfg(target_os = "windows")]
fn cache_root() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(local);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    std::env::temp_dir()
}

pub fn ensure_temp_dir() {
    if let Err(e) = ensure_cache_dir(&temp_dir()) {
        log(&format!("failed to prepare PNG cache: {e}"));
    }
}

fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    if !fs::symlink_metadata(dir)?.file_type().is_dir() {
        return Err(io::Error::other("PNG cache must be a real directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Minimal HTTP GET via `curl`.
///
/// clipaste deliberately ships with no HTTP client crate — the only things it
/// ever fetches are its own endpoints on loopback or an SSH tunnel, and `curl`
/// is present on every platform we target (Windows 10+ bundles `curl.exe`).
/// Returns `None` on any transport error or non-2xx status (`-f`).
///
/// Timeouts are bounded: callers probe several candidate addresses in sequence,
/// and a firewall that DROPs instead of rejecting would otherwise stall each
/// attempt for minutes.
pub fn http_get(url: &str) -> Option<String> {
    let out = std::process::Command::new("curl")
        .args(["-sf", "--connect-timeout", "2", "-m", "5", url])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Escape a string for embedding in the hand-rolled JSON of `doctor --json`.
/// Kept here (rather than pulling in serde) because it is the only JSON we
/// ever *write* beyond fixed literals.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn log(msg: &str) {
    let now = chrono_lite();
    eprintln!("[{now}] clipaste: {msg}");
}

/// ISO8601-ish timestamp without pulling in chrono
fn chrono_lite() -> String {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    // Good enough for logging — not calendar-accurate but unique and sortable
    format!("{secs}")
}

fn png_cache_name(bytes: &[u8]) -> String {
    format!("shot-sha256-{:x}.png", Sha256::digest(bytes))
}

/// Save PNG bytes at a stable, content-addressed path, reusing identical bytes.
///
/// Published paths (including legacy timestamp paths) are retained until the
/// user explicitly deletes them: clipboard histories can reference them long
/// after the daemon exits. Disk usage therefore grows with unique images.
/// A conflicting/corrupt cache entry is never replaced;
/// saving fails and logs an error so its owner can inspect or explicitly delete it.
pub fn save_png_to_temp(png_data: &[u8]) -> Option<PathBuf> {
    match save_png_in_dir(&temp_dir(), png_data) {
        Ok(path) => Some(path),
        Err(e) => {
            log(&format!("failed to save cached PNG: {e}"));
            None
        }
    }
}

fn verify_cached_png(path: &Path, png_data: &[u8]) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::other("PNG cache entry must be a regular file"));
    }
    let file = fs::File::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let mut reader = io::BufReader::new(file);
    // Streaming comparison bounds additional memory even for a corrupt entry.
    let mut remaining = png_data;
    loop {
        use std::io::BufRead;
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() && remaining.is_empty() {
            return Ok(());
        }
        if chunk.is_empty() || !remaining.starts_with(chunk) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("PNG cache content mismatch at {}", path.display()),
            ));
        }
        let len = chunk.len();
        remaining = &remaining[len..];
        reader.consume(len);
    }
}

struct StagedPng(PathBuf);

impl Drop for StagedPng {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.0) {
            // Windows publication moves the staging file to its final name.
            if e.kind() != io::ErrorKind::NotFound {
                log(&format!(
                    "failed to remove staging file {}: {e}",
                    self.0.display()
                ));
            }
        }
    }
}

#[cfg(unix)]
fn publish_no_replace(staged: &Path, path: &Path) -> io::Result<()> {
    fs::hard_link(staged, path)
}

#[cfg(target_os = "windows")]
fn publish_no_replace(staged: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid PNG cache path");
        // Canonicalize the existing parent, not the possibly absent destination,
        // to retain Rust filesystem APIs' support for extended-length paths.
        let parent = path.parent().ok_or_else(invalid)?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let absolute = fs::canonicalize(parent)?.join(path.file_name().ok_or_else(invalid)?);
        let mut wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(invalid());
        }
        wide.push(0);
        Ok(wide)
    }

    let staged = wide_path(staged)?;
    let path = wide_path(path)?;
    // No REPLACE_EXISTING or COPY_ALLOWED: publish by same-volume rename without
    // clobbering a competing writer or requiring hard links on FAT/exFAT.
    // SAFETY: Both buffers are NUL-terminated and remain alive for the call.
    if unsafe { MoveFileExW(staged.as_ptr(), path.as_ptr(), 0) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS) => {
            Err(io::Error::new(io::ErrorKind::AlreadyExists, error))
        }
        _ => Err(error),
    }
}

fn publish_staged_png(staged: &Path, path: &Path, png_data: &[u8]) -> io::Result<()> {
    match publish_no_replace(staged, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => verify_cached_png(path, png_data),
        Err(e) => Err(e),
    }
}

fn save_png_in_dir(dir: &Path, png_data: &[u8]) -> io::Result<PathBuf> {
    ensure_cache_dir(dir)?;
    let path = dir.join(png_cache_name(png_data));
    match verify_cached_png(&path, png_data) {
        Ok(()) => return Ok(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (staged, mut file) = loop {
        let scratch = dir.join(format!(
            ".png-stage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&scratch) {
            Ok(file) => break (StagedPng(scratch), file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(png_data)?;
    file.sync_all()?;
    drop(file);
    // Publish only the complete, flushed, closed file, without replacing an entry.
    publish_staged_png(&staged.0, &path, png_data)?;
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(path)
}

/// Kept for existing callers. Published PNGs have no automatic expiry because
/// external clipboard histories retain their paths. Remove them explicitly to
/// reclaim disk space; interrupted staging files may also be removed manually.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub fn clean_old_temp_files() {}

/// Convert TIFF bytes to PNG bytes
#[cfg(target_os = "macos")]
pub fn tiff_to_png(tiff_data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(tiff_data, ImageFormat::Tiff).ok()?;
    let rgba = img.to_rgba8();
    let mut buf = Vec::new();
    PngEncoder::new(Cursor::new(&mut buf))
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(buf)
}

/// Read an image file from disk and return PNG bytes.
///
/// Used when the clipboard holds a *file URL* pointing to an image (e.g. a macOS
/// screenshot saved to disk, then copied in Finder) — see issue #5. Only formats
/// the bundled `image` decoder supports are handled (png passthrough, tiff, bmp);
/// macOS screenshots are PNG by default, which is the common case. Returns None
/// for unsupported formats so the caller can skip cleanly rather than serve
/// mislabeled bytes.
#[cfg(target_os = "macos")]
pub fn image_file_to_png(path: &std::path::Path) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let bytes = fs::read(path).ok()?;
    match ext.as_str() {
        "png" => Some(bytes), // already PNG — pass through unchanged
        "tif" | "tiff" => tiff_to_png(&bytes),
        "bmp" => {
            let img = image::load_from_memory_with_format(&bytes, ImageFormat::Bmp).ok()?;
            let rgba = img.to_rgba8();
            let mut buf = Vec::new();
            PngEncoder::new(Cursor::new(&mut buf))
                .write_image(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .ok()?;
            Some(buf)
        }
        _ => None,
    }
}

/// Convert Windows DIB (CF_DIB) bytes to PNG bytes
#[cfg(target_os = "windows")]
pub fn dib_to_png(dib_data: &[u8]) -> Option<Vec<u8>> {
    // CF_DIB is a BITMAPINFOHEADER followed by pixel data
    // The image crate's BMP decoder expects a full BMP file header,
    // so we prepend a minimal BITMAPFILEHEADER
    if dib_data.len() < 40 {
        return None;
    }

    // Read BITMAPINFOHEADER fields
    let header_size = u32::from_le_bytes(dib_data[0..4].try_into().ok()?) as usize;
    let bits_per_pixel = u16::from_le_bytes(dib_data[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(dib_data[16..20].try_into().ok()?);

    // Calculate color table size
    let color_table_size = if bits_per_pixel <= 8 {
        (1 << bits_per_pixel) * 4
    } else if compression == 3 {
        // BI_BITFIELDS: 3 DWORD masks
        12
    } else {
        0
    };

    let pixel_offset = 14 + header_size + color_table_size; // 14 = BITMAPFILEHEADER
    let file_size = 14 + dib_data.len();

    // Build BMP file header (14 bytes)
    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes()); // reserved1
    bmp.extend_from_slice(&0u16.to_le_bytes()); // reserved2
    bmp.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib_data);

    let img = image::load_from_memory_with_format(&bmp, ImageFormat::Bmp).ok()?;
    let rgba = img.to_rgba8();
    let mut buf = Vec::new();
    PngEncoder::new(Cursor::new(&mut buf))
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(buf)
}

pub fn print_help() {
    println!(
        "clipaste v{VERSION} — Fix screenshot paste in terminals (local + SSH + WSL2)

USAGE
  clipaste                       Run daemon (macOS, Windows, Linux desktop)
  clipaste doctor [--json]       Diagnose this machine and print the fix command
  clipaste ssh-setup user@host   Configure remote server for image paste via SSH
                                 (add -p PORT for a custom SSH port)
  clipaste wsl-setup             Configure WSL2 for image paste from Windows host
                                 (add --host IP to skip host auto-detection)
  clipaste --version             Print version
  clipaste --help                Show this help

FOR CODING AGENTS
  `clipaste doctor --json` is the machine-readable entry point. Every check
  carries name/status/detail/fix; `fix` is a literal command to run.
  Exit 0 = usable, 1 = broken, 2 = bad arguments. All setup commands are
  non-interactive. See AGENTS.md in the repo for the full install recipe.

  On the remote, ssh-setup/wsl-setup also install a `clipaste-paste` command:
  it fetches the current clipboard image into a real file and prints its path.
  Use it with Codex CLI (which bypasses the xclip shim) or any macOS remote.

WHAT IT DOES
  Local:  Watches the clipboard. When a screenshot is detected, saves it as
          a cached PNG. macOS/Windows also register the file path for pasting.
          Linux reads image/png without modifying the clipboard.

  SSH:    Runs an HTTP server on port {DEFAULT_PORT}. Use 'ssh-setup' to
          configure SSH RemoteForward + xclip shim on a remote server.
          Claude Code (Linux remote) pastes natively with Ctrl+V; Codex CLI
          and macOS remotes use the `clipaste-paste` helper.

  WSL2:   Run 'wsl-setup' inside WSL2 to install xclip shim that fetches
          images from clipaste.exe on the Windows host. No SSH needed.
          The Windows host address is probed automatically (mirrored and NAT
          networking modes both work); override it with --host IP if needed.

COMPATIBILITY
  Host:    macOS, Windows, Linux desktop (Wayland data-control or X11/XWayland)
  macOS:   Ghostty, Alacritty, iTerm2, Terminal.app, WezTerm, Kitty
  Windows: Windows Terminal, PowerShell, cmd.exe
  Remote:  Linux and macOS via SSH from a supported clipboard host
  WSL2:    Consumer only; requires clipaste.exe running on Windows

  Linux:   Install wl-clipboard 2.2+ for native Wayland, or xclip
           for X11/XWayland, plus curl. Run inside your graphical session.
           Auto mode warns before using XWayland if data-control is unavailable.
           CLIPASTE_BACKEND=wayland or x11 selects a backend explicitly.
           No local text-path injection; SSH consumers use the existing helpers.

MORE INFO
  https://github.com/hqhq1025/clipaste"
    );
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "clipaste-common-test-{}-{}",
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
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn png(value: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&[value, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    #[test]
    fn creates_missing_cache_directory() {
        let dir = TestDir::new();
        let bytes = png(1);
        let path = save_png_in_dir(&dir.0.join("cache"), &bytes).unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn publication_preserves_complete_file_after_staging_cleanup() {
        let dir = TestDir::new();
        let staged = StagedPng(dir.0.join(".png-stage-test"));
        let path = dir.0.join("published.png");
        let bytes = png(1);
        fs::write(&staged.0, &bytes).unwrap();
        publish_staged_png(&staged.0, &path, &bytes).unwrap();
        #[cfg(target_os = "windows")]
        assert!(!staged.0.exists());
        #[cfg(unix)]
        assert!(staged.0.exists());
        drop(staged);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn publication_verifies_competing_writer_without_replacing_it() {
        let dir = TestDir::new();
        let path = dir.0.join("published.png");
        let bytes = png(1);
        for existing in [&bytes, &png(2)] {
            fs::write(&path, existing).unwrap();
            let staged = StagedPng(dir.0.join(".png-stage-test"));
            fs::write(&staged.0, &bytes).unwrap();
            let result = publish_staged_png(&staged.0, &path, &bytes);
            if existing == &bytes {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
            }
            assert_eq!(fs::read(&path).unwrap(), *existing);
            assert_eq!(fs::read(&staged.0).unwrap(), bytes);
            drop(staged);
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn publication_failure_does_not_leave_staging_files() {
        let dir = TestDir::new();
        let staged = StagedPng(dir.0.join(".png-stage-test"));
        let path = dir.0.join("missing").join("published.png");
        let bytes = png(1);
        fs::write(&staged.0, &bytes).unwrap();
        assert!(publish_staged_png(&staged.0, &path, &bytes).is_err());
        drop(staged);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }

    #[test]
    fn rapid_distinct_images_never_overwrite_each_other() {
        let dir = TestDir::new();
        let saved: Vec<_> = (0..32)
            .map(|value| {
                let bytes = png(value);
                (save_png_in_dir(&dir.0, &bytes).unwrap(), bytes)
            })
            .collect();
        for (path, bytes) in saved {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn identical_bytes_reuse_published_file_across_processes() {
        let dir = TestDir::new();
        let bytes = png(9);
        let path = save_png_in_dir(&dir.0, &bytes).unwrap();
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "common::cache_tests::cache_process_child"])
            .env("CLIPASTE_TEST_CACHE_DIR", &dir.0)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), old);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn cache_process_child() {
        if let Some(dir) = std::env::var_os("CLIPASTE_TEST_CACHE_DIR") {
            save_png_in_dir(Path::new(&dir), &png(9)).unwrap();
        }
    }

    #[test]
    fn cache_names_have_a_fixed_algorithm() {
        assert_eq!(
            png_cache_name(b""),
            "shot-sha256-e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.png"
        );
        assert_eq!(
            png_cache_name(b"a"),
            "shot-sha256-ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb.png"
        );
    }

    #[test]
    fn conflicting_or_corrupt_file_is_not_overwritten() {
        let dir = TestDir::new();
        let bytes = png(1);
        let path = dir.0.join(png_cache_name(&bytes));
        for corrupt in [b"truncated".as_slice(), png(2).as_slice()] {
            fs::write(&path, corrupt).unwrap();
            let error = save_png_in_dir(&dir.0, &bytes).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read(&path).unwrap(), corrupt);
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn concurrent_writers_publish_complete_deduplicated_files() {
        let dir = TestDir::new();
        let barrier = std::sync::Barrier::new(16);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|n| {
                    let barrier = &barrier;
                    let dir = &dir.0;
                    scope.spawn(move || {
                        let bytes = png(n % 4);
                        barrier.wait();
                        let path = save_png_in_dir(dir, &bytes).unwrap();
                        assert_eq!(fs::read(&path).unwrap(), bytes);
                        (n % 4, path)
                    })
                })
                .collect();
            for handle in handles {
                let (value, path) = handle.join().unwrap();
                assert_eq!(path, dir.0.join(png_cache_name(&png(value))));
            }
        });
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 4);
    }

    #[test]
    fn cleanup_retains_published_and_legacy_paths() {
        let dir = TestDir::new();
        let current = save_png_in_dir(&dir.0, &png(1)).unwrap();
        let legacy = dir.0.join("shot-old-timestamp.png");
        fs::write(&legacy, png(2)).unwrap();
        for path in [&current, &legacy] {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH)
                        .set_accessed(SystemTime::UNIX_EPOCH),
                )
                .unwrap();
        }
        // This compatibility hook is intentionally a no-op and reads no HOME.
        clean_old_temp_files();
        assert_eq!(fs::read(current).unwrap(), png(1));
        assert_eq!(fs::read(legacy).unwrap(), png(2));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cache_entries_and_directories_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = TestDir::new();
        let cache = dir.0.join("cache");
        fs::create_dir(&cache).unwrap();
        let bytes = png(1);
        let outside = dir.0.join("outside.png");
        fs::write(&outside, &bytes).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        let path = cache.join(png_cache_name(&bytes));
        symlink(&outside, &path).unwrap();
        assert!(save_png_in_dir(&cache, &bytes).is_err());
        assert_eq!(fs::read(&outside).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let linked_dir = dir.0.join("linked-cache");
        symlink(&cache, &linked_dir).unwrap();
        assert!(save_png_in_dir(&linked_dir, &bytes).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cache_directory_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        let path = save_png_in_dir(&dir.0, &png(1)).unwrap();
        assert_eq!(
            fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(save_png_in_dir(&dir.0, &png(1)).unwrap(), path);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
