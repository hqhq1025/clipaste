use image::codecs::png::PngEncoder;
use image::{ImageEncoder, ImageFormat};
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub const VERSION: &str = "2.2.1";
pub const DEFAULT_PORT: u16 = 18340;

/// Shared state: path to the most recently saved screenshot PNG
pub type LatestImage = Arc<Mutex<Option<PathBuf>>>;

pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("clipaste")
}

pub fn ensure_temp_dir() {
    let _ = fs::create_dir_all(temp_dir());
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

fn timestamp_for_filename() -> String {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", d.as_millis())
}

pub fn save_png_to_temp(png_data: &[u8]) -> Option<PathBuf> {
    let name = format!("screenshot-{}.png", timestamp_for_filename());
    let path = temp_dir().join(name);
    match fs::write(&path, png_data) {
        Ok(()) => Some(path),
        Err(e) => {
            log(&format!("failed to save temp PNG: {e}"));
            None
        }
    }
}

pub fn clean_old_temp_files() {
    let dir = temp_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let cutoff = SystemTime::now() - Duration::from_secs(3600);
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if let Ok(created) = meta.created() {
                if created < cutoff {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Convert TIFF bytes to PNG bytes
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
  clipaste                       Run daemon (clipboard watcher + HTTP server)
  clipaste ssh-setup user@host   Configure remote server for image paste via SSH
  clipaste wsl-setup             Configure WSL2 for image paste from Windows host
  clipaste --version             Print version
  clipaste --help                Show this help

WHAT IT DOES
  Local:  Watches the clipboard. When a screenshot is detected, saves it as
          a temp PNG and registers the file path. Cmd+V / Ctrl+V just work.

  SSH:    Runs an HTTP server on port {DEFAULT_PORT}. Use 'ssh-setup' to
          configure SSH RemoteForward + xclip shim on a remote server.

  WSL2:   Run 'wsl-setup' inside WSL2 to install xclip shim that fetches
          images from clipaste.exe on the Windows host. No SSH needed.

COMPATIBILITY
  macOS:   Ghostty, Alacritty, iTerm2, Terminal.app, WezTerm, Kitty
  Windows: Windows Terminal, PowerShell, cmd.exe
  Remote:  Any Linux server via SSH
  WSL2:    Ubuntu, Debian, Fedora, Arch on WSL2

MORE INFO
  https://github.com/hqhq1025/clipaste"
    );
}
