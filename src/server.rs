use crate::common::{self, LatestImage};
use std::io::{Read, Write};
use std::net::TcpListener;

/// Start HTTP server in a background thread.
/// Serves the latest screenshot PNG on GET /clipboard/image.
pub fn start(latest: LatestImage) -> std::io::Result<()> {
    let addr = format!("127.0.0.1:{}", common::DEFAULT_PORT);
    // Bind before the watcher starts, so a stale daemon/tunnel cannot be mistaken
    // for this process's successfully published clipboard.
    let listener = TcpListener::bind(&addr)?;
    common::log(&format!("http server listening on {addr}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let latest = latest.clone();
            std::thread::spawn(move || handle_request(stream, latest));
        }
    });
    Ok(())
}

fn handle_request(mut stream: std::net::TcpStream, latest: LatestImage) {
    let mut buf = [0u8; 2048];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");

    if first_line.starts_with("GET /health") {
        let body = format!("{{\"status\":\"ok\",\"version\":\"{}\"}}", common::VERSION);
        respond(&mut stream, 200, "application/json", body.as_bytes());
    } else if first_line.starts_with("GET /clipboard/type") {
        let has_image = latest.lock().ok().and_then(|g| g.clone()).is_some();
        let body = if has_image {
            "{\"type\":\"image\",\"format\":\"png\"}"
        } else {
            "{\"type\":\"empty\"}"
        };
        respond(&mut stream, 200, "application/json", body.as_bytes());
    } else if first_line.starts_with("GET /clipboard/image") {
        let path = latest.lock().ok().and_then(|g| g.clone());
        match path {
            Some(p) => match std::fs::read(&p) {
                Ok(data) => respond(&mut stream, 200, "image/png", &data),
                Err(_) => respond(&mut stream, 404, "text/plain", b"file not found"),
            },
            None => respond(&mut stream, 204, "text/plain", b""),
        }
    } else {
        respond(&mut stream, 404, "text/plain", b"not found");
    }
}

fn respond(stream: &mut std::net::TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let status_text = match status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        _ => "Unknown",
    };
    let header = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
