use crate::common;
use std::ptr;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_CLIPBOARDUPDATE: u32 = 0x031D;

// Standard clipboard format constants
const CF_BITMAP: u32 = 2;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

static mut LAST_SEQ: u32 = 0;

/// Global shared state for the Win32 callback (wnd_proc can't capture closures)
static LATEST: OnceLock<common::LatestImage> = OnceLock::new();

/// Check if clipboard has image data but no text or file drop
fn is_image_only_clipboard() -> bool {
    unsafe {
        let has_dib = IsClipboardFormatAvailable(CF_DIB) != 0;
        let has_bitmap = IsClipboardFormatAvailable(CF_BITMAP) != 0;
        let has_text = IsClipboardFormatAvailable(CF_UNICODETEXT) != 0;
        let has_hdrop = IsClipboardFormatAvailable(CF_HDROP) != 0;

        (has_dib || has_bitmap) && !has_text && !has_hdrop
    }
}

/// Read CF_DIB data from clipboard and convert to PNG
fn read_clipboard_as_png() -> Option<Vec<u8>> {
    unsafe {
        if OpenClipboard(ptr::null_mut() as HWND) == 0 {
            return None;
        }

        let result = (|| {
            let handle = GetClipboardData(CF_DIB) as *mut u8;
            if handle.is_null() {
                return None;
            }

            let ptr = GlobalLock(handle as *mut _);
            if ptr.is_null() {
                return None;
            }

            let size = GlobalSize(handle as *mut _);
            let data = std::slice::from_raw_parts(ptr as *const u8, size);
            let dib_data = data.to_vec();

            GlobalUnlock(handle as *mut _);
            Some(dib_data)
        })();

        CloseClipboard();

        let dib_data = result?;
        common::dib_to_png(&dib_data)
    }
}

/// Write a file path as text to the clipboard, preserving PNG data
fn write_path_to_clipboard(path: &str, png_data: &[u8]) {
    unsafe {
        if OpenClipboard(ptr::null_mut() as HWND) == 0 {
            return;
        }

        EmptyClipboard();

        // Write file path as CF_UNICODETEXT
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = wide.len() * 2;
        let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        if !hmem.is_null() {
            let dest = GlobalLock(hmem);
            if !dest.is_null() {
                ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, dest as *mut u8, byte_len);
                GlobalUnlock(hmem);
                SetClipboardData(CF_UNICODETEXT, hmem as HANDLE);
            }
        }

        // Also register a custom "PNG" format with the PNG data
        let png_format_name: Vec<u16> = "PNG\0".encode_utf16().collect();
        let png_format = RegisterClipboardFormatW(png_format_name.as_ptr());
        if png_format != 0 {
            let hmem_png = GlobalAlloc(GMEM_MOVEABLE, png_data.len());
            if !hmem_png.is_null() {
                let dest_png = GlobalLock(hmem_png);
                if !dest_png.is_null() {
                    ptr::copy_nonoverlapping(
                        png_data.as_ptr(),
                        dest_png as *mut u8,
                        png_data.len(),
                    );
                    GlobalUnlock(hmem_png);
                    SetClipboardData(png_format, hmem_png as HANDLE);
                }
            }
        }

        CloseClipboard();
    }
}

fn normalize(latest: &common::LatestImage) {
    if !is_image_only_clipboard() {
        return;
    }

    let png_data = match read_clipboard_as_png() {
        Some(d) => d,
        None => {
            common::log("failed to read clipboard image as PNG");
            return;
        }
    };

    let file_path = match common::save_png_to_temp(&png_data) {
        Some(p) => p,
        None => return,
    };

    // Update shared state for HTTP server
    if let Ok(mut guard) = latest.lock() {
        *guard = Some(file_path.clone());
    }

    let path_str = file_path.to_string_lossy().to_string();
    write_path_to_clipboard(&path_str, &png_data);

    let filename = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    common::log(&format!("normalized {filename} ({} bytes)", png_data.len()));

    common::clean_old_temp_files();
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            let seq = GetClipboardSequenceNumber();
            if seq != LAST_SEQ {
                LAST_SEQ = seq;
                if let Some(latest) = LATEST.get() {
                    normalize(latest);
                }
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn run(latest: common::LatestImage) {
    LATEST.set(latest).expect("LATEST already initialized");
    common::ensure_temp_dir();
    common::log(&format!(
        "v{} started (pid {})",
        common::VERSION,
        std::process::id()
    ));

    unsafe {
        let class_name: Vec<u16> = "clipaste_hidden\0".encode_utf16().collect();
        let hinstance = GetModuleHandleW(ptr::null());

        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class_name.as_ptr(),
            style: 0,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut() as HMENU,
            hinstance,
            ptr::null(),
        );

        if hwnd.is_null() {
            common::log("failed to create hidden window");
            std::process::exit(1);
        }

        if AddClipboardFormatListener(hwnd) == 0 {
            common::log("failed to register clipboard listener");
            std::process::exit(1);
        }

        common::log("clipboard listener registered (event-driven, no polling)");

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut() as HWND, 0, 0) > 0 {
            DispatchMessageW(&msg);
        }

        RemoveClipboardFormatListener(hwnd);
    }
}
