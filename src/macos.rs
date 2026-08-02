use crate::common;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardType, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSDefaultRunLoopMode, NSRunLoop, NSString, NSTimer, NSURL};
use std::cell::Cell;
use std::ptr::NonNull;
use std::time::Instant;

// Custom pasteboard type constants
fn pasteboard_type(s: &str) -> Retained<NSPasteboardType> {
    NSPasteboardType::from_str(s)
}

fn png_type() -> Retained<NSPasteboardType> {
    pasteboard_type("public.png")
}

fn pngf_type() -> Retained<NSPasteboardType> {
    pasteboard_type("com.apple.pboard.type.PNGf")
}

fn tiff_type() -> Retained<NSPasteboardType> {
    pasteboard_type("public.tiff")
}

fn file_url_type() -> Retained<NSPasteboardType> {
    pasteboard_type("public.file-url")
}

fn string_type() -> Retained<NSPasteboardType> {
    pasteboard_type("public.utf8-plain-text")
}

/// Decide, from the pasteboard's type list alone, whether the clipboard holds
/// raw image bits we should stage.
///
/// The list length says nothing about the content. A screenshot advertises two
/// or three types; an image copied out of a Chromium browser advertises eight —
/// the same bits under several flavors, plus the source markup and Chromium's
/// private metadata. What decides it is the presence of image bits together
/// with the absence of a file reference or plain text, because those two mean
/// the user copied a *file* or *text* and staging the image would hijack an
/// ordinary paste.
fn is_image_only(types: &[String]) -> bool {
    let mut has_image = false;
    let mut has_file_url = false;
    let mut has_filenames = false;
    let mut has_string = false;

    for t in types {
        match t.as_str() {
            "public.png" | "public.tiff" => has_image = true,
            "public.file-url" => has_file_url = true,
            "NSFilenamesPboardType" => has_filenames = true,
            "public.utf8-plain-text" => has_string = true,
            _ => {}
        }
    }

    has_image && !has_file_url && !has_filenames && !has_string
}

/// Check if clipboard has image data but no file URL or string
fn is_image_only_clipboard(pb: &NSPasteboard) -> bool {
    let types = match pb.types() {
        Some(t) => t,
        None => return false,
    };

    let list: Vec<String> = (0..types.count())
        .map(|i| types.objectAtIndex(i).to_string())
        .collect();

    is_image_only(&list)
}

/// Read PNG data from clipboard, converting from TIFF if needed
fn read_png_data(pb: &NSPasteboard) -> Option<Vec<u8>> {
    if let Some(data) = pb.dataForType(&png_type()) {
        return Some(data.to_vec());
    }
    if let Some(data) = pb.dataForType(&tiff_type()) {
        return common::tiff_to_png(&data.to_vec());
    }
    None
}

/// Minimal percent-decoding for file URLs (e.g. `%20` → space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// If the clipboard holds a file URL pointing to an image file, return its path.
///
/// This covers the "screenshot saved to disk then copied" case (issue #5): the
/// clipboard carries a `public.file-url`, not image bits. We only accept image
/// extensions the bundled decoder can serve (png/tiff/bmp).
fn image_file_url_on_clipboard(pb: &NSPasteboard) -> Option<std::path::PathBuf> {
    let data = pb.dataForType(&file_url_type())?;
    let raw = String::from_utf8(data.to_vec()).ok()?;
    let raw = raw.trim();
    let path = match raw.strip_prefix("file://") {
        // file:///Users/... → strip scheme, drop the (empty) host, percent-decode
        Some(rest) => percent_decode(rest.trim_start_matches(|c| c != '/')),
        None => raw.to_string(),
    };
    let pb_path = std::path::PathBuf::from(path);
    let ext = pb_path.extension()?.to_str()?.to_lowercase();
    let is_supported_image = matches!(ext.as_str(), "png" | "tif" | "tiff" | "bmp");
    if is_supported_image && pb_path.is_file() {
        Some(pb_path)
    } else {
        None
    }
}

/// Make a clipboard image *file* available to the HTTP server (for remote
/// `clipaste-paste`) without touching the local clipboard — local Cmd+V keeps
/// pasting whatever it pasted before. Issue #5.
fn capture_image_file(src: &std::path::Path, latest: &common::LatestImage) {
    let png = match common::image_file_to_png(src) {
        Some(p) => p,
        None => {
            common::log("skip: clipboard file is not a supported image format");
            return;
        }
    };
    let tmp = match common::save_png_to_temp(&png) {
        Some(p) => p,
        None => return,
    };
    if let Ok(mut guard) = latest.lock() {
        *guard = Some(tmp);
    }
    common::log(&format!(
        "captured clipboard image file ({} bytes) for remote paste",
        png.len()
    ));
    common::clean_old_temp_files();
}

fn normalize(pb: &NSPasteboard, latest: &common::LatestImage) {
    let png_data = match read_png_data(pb) {
        Some(d) => d,
        None => {
            common::log("failed to get PNG data from clipboard");
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

    pb.clearContents();

    // Write file URL as proper pasteboard object (critical for Ghostty Cmd+V)
    let ns_path = NSString::from_str(&path_str);
    let url = NSURL::fileURLWithPath(&ns_path);
    let writing: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
        ProtocolObject::from_retained(url);
    let array = NSArray::from_retained_slice(&[writing]);
    pb.writeObjects(&array);

    // Add PNG and PNGf types for Ctrl+V / osascript compatibility
    let png_t = png_type();
    let pngf_t = pngf_type();
    let types_to_add = NSArray::from_retained_slice(&[png_t.clone(), pngf_t.clone()]);
    unsafe { pb.addTypes_owner(&types_to_add, None) };

    let ns_png_data = NSData::with_bytes(&png_data);
    pb.setData_forType(Some(&ns_png_data), &png_t);
    pb.setData_forType(Some(&ns_png_data), &pngf_t);

    // Also set plain-text path so `pbpaste`, SSH terminal paste, and apps that only
    // read string types get the path. file-url alone isn't enough for those consumers.
    let str_t = string_type();
    let ns_path_text = NSString::from_str(&path_str);
    pb.setString_forType(&ns_path_text, &str_t);

    let filename = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    common::log(&format!("normalized {filename} ({} bytes)", png_data.len()));

    common::clean_old_temp_files();
}

pub fn run(latest: common::LatestImage) {
    common::ensure_temp_dir();
    common::log(&format!(
        "v{} started (pid {})",
        common::VERSION,
        std::process::id()
    ));

    let pb = NSPasteboard::generalPasteboard();

    // Warm up: do a real pasteboard write cycle on a scratch pasteboard
    let warmup_path = common::temp_dir().join(".warmup");
    let _ = std::fs::write(&warmup_path, vec![0u8; 512 * 1024]);
    let _ = std::fs::remove_file(&warmup_path);

    let scratch_name = NSString::from_str("com.clipaste.warmup");
    let scratch = NSPasteboard::pasteboardWithName(&scratch_name);
    scratch.clearContents();
    let dummy_url = NSURL::fileURLWithPath(&NSString::from_str("/tmp"));
    let writing: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
        ProtocolObject::from_retained(dummy_url);
    scratch.writeObjects(&NSArray::from_retained_slice(&[writing]));
    let dummy_types = NSArray::from_retained_slice(&[png_type()]);
    unsafe { scratch.addTypes_owner(&dummy_types, None) };
    scratch.setData_forType(Some(&NSData::with_bytes(&[0u8; 1])), &png_type());

    // State stored in thread-local cells for the timer callback
    thread_local! {
        static LAST_CHANGE: Cell<isize> = Cell::new(0);
        static LAST_NORMALIZE: Cell<Option<Instant>> = Cell::new(None);
    }
    LAST_CHANGE.with(|c| c.set(pb.changeCount()));

    // Use NSTimer + NSRunLoop — same mechanism as Swift's Timer, fires precisely
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let current = pb.changeCount();

        let last = LAST_CHANGE.with(|c| c.get());
        if current == last {
            return;
        }
        LAST_CHANGE.with(|c| c.set(current));

        // Debounce: skip if we normalized within the last 500ms
        let should_skip = LAST_NORMALIZE.with(|c| {
            if let Some(t) = c.get() {
                let elapsed = t.elapsed().as_millis();
                if elapsed < 500 {
                    common::log(&format!("debounce: skipping ({}ms since last)", elapsed));
                    true
                } else {
                    false
                }
            } else {
                false
            }
        });
        if should_skip {
            return;
        }

        if !is_image_only_clipboard(&pb) {
            // Not raw image bits. But the clipboard may hold a *file URL* to an
            // image (screenshot saved to disk then copied) — issue #5. Serve that
            // file to the HTTP server for remote `clipaste-paste`, without
            // rewriting the local clipboard.
            match image_file_url_on_clipboard(&pb) {
                Some(src) => capture_image_file(&src, &latest),
                None => common::log("skip: clipboard has file-url or text"),
            }
            return;
        }

        normalize(&pb, &latest);

        LAST_NORMALIZE.with(|c| c.set(Some(Instant::now())));
        LAST_CHANGE.with(|c| c.set(pb.changeCount()));
    });

    let timer = unsafe {
        NSTimer::timerWithTimeInterval_repeats_block(0.03, true, &block)
    };

    let run_loop = NSRunLoop::currentRunLoop();
    unsafe {
        run_loop.addTimer_forMode(&timer, NSDefaultRunLoopMode);
        run_loop.run();
    }
}

#[cfg(test)]
mod tests {
    use super::is_image_only;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn screenshot_is_staged() {
        assert!(is_image_only(&types(&["public.png", "public.tiff"])));
    }

    /// An image copied out of a Chromium browser (Brave, Chrome, Edge): the
    /// same bits under several flavors, plus the source markup and Chromium's
    /// private metadata. Eight types, no file URL, no plain text — still just
    /// an image. A cap on the number of types rejected this, so the daemon kept
    /// serving whatever was staged before and remote paste produced a stale
    /// screenshot.
    #[test]
    fn browser_image_is_staged() {
        assert!(is_image_only(&types(&[
            "public.png",
            "Apple PNG pasteboard type",
            "public.html",
            "Apple HTML pasteboard type",
            "org.chromium.internal.source-rfh-token",
            "org.chromium.source-url",
            "public.tiff",
            "NeXT TIFF v4.0 pasteboard type",
        ])));
    }

    /// A copied file is a file, however many flavors accompany it.
    #[test]
    fn copied_file_is_not_staged() {
        assert!(!is_image_only(&types(&["public.file-url", "public.png"])));
        assert!(!is_image_only(&types(&[
            "NSFilenamesPboardType",
            "public.png"
        ])));
    }

    /// A web-page selection carries markup and an image alongside the text.
    /// Staging it would hijack an ordinary text paste.
    #[test]
    fn text_selection_is_not_staged() {
        assert!(!is_image_only(&types(&[
            "public.utf8-plain-text",
            "public.html",
            "public.png"
        ])));
    }

    #[test]
    fn empty_clipboard_is_not_staged() {
        assert!(!is_image_only(&[]));
    }
}
