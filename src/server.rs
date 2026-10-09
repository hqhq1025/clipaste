use crate::common::{self, LatestImage, StagedImage};
use std::io::{Read, Write};
use std::net::TcpListener;

/// Start HTTP server in a background thread.
/// Serves the latest screenshot PNG on GET /clipboard/image.
pub fn start(latest: LatestImage, server_only: bool) -> std::io::Result<()> {
    let addr = format!("127.0.0.1:{}", common::DEFAULT_PORT);
    // Bind before the watcher starts, so a stale daemon/tunnel cannot be mistaken
    // for this process's successfully published clipboard.
    let listener = TcpListener::bind(&addr)?;
    common::log(&format!("http server listening on {addr}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let latest = latest.clone();
            std::thread::spawn(move || handle_request(stream, latest, server_only));
        }
    });
    Ok(())
}

/// `mode` appears only in server-only mode, so `doctor` can explain why local
/// paste gets no file path. Every /health reader matches substrings, so older
/// clients keep accepting the body.
pub(crate) fn health_body(server_only: bool) -> String {
    let mode = if server_only {
        ",\"mode\":\"server-only\""
    } else {
        ""
    };
    format!(
        "{{\"status\":\"ok\",\"version\":\"{}\"{mode}}}",
        common::VERSION
    )
}

fn handle_request(mut stream: std::net::TcpStream, latest: LatestImage, server_only: bool) {
    let mut buf = [0u8; 2048];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");

    if first_line.starts_with("GET /health") {
        let body = health_body(server_only);
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
        let staged = latest.lock().ok().and_then(|g| g.clone());
        match staged {
            Some(StagedImage::File(p)) => match std::fs::read(&p) {
                Ok(data) => respond(&mut stream, 200, "image/png", &data),
                Err(_) => respond(&mut stream, 404, "text/plain", b"file not found"),
            },
            Some(StagedImage::Memory(data)) => respond(&mut stream, 200, "image/png", &data),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    fn get(latest: &LatestImage, path: &str) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost:18340\r\n\r\n").as_bytes())
            .unwrap();
        handle_request(server, latest.clone(), false);
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        response
    }

    fn body(response: &[u8]) -> &[u8] {
        let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        &response[end + 4..]
    }

    #[test]
    fn serves_an_image_staged_in_memory() {
        let latest = Arc::new(Mutex::new(common::stage_in_memory(b"png bytes")));
        let response = get(&latest, "/clipboard/image");
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert_eq!(body(&response), b"png bytes");
        assert!(String::from_utf8_lossy(&get(&latest, "/clipboard/type")).contains("\"image\""));
    }

    #[test]
    fn serves_a_cached_file_and_reports_a_deleted_one() {
        let path =
            std::env::temp_dir().join(format!("clipaste-server-test-{}.png", std::process::id()));
        std::fs::write(&path, b"cached bytes").unwrap();
        let latest = Arc::new(Mutex::new(Some(StagedImage::File(path.clone()))));
        assert_eq!(body(&get(&latest, "/clipboard/image")), b"cached bytes");
        std::fs::remove_file(&path).unwrap();
        assert!(get(&latest, "/clipboard/image").starts_with(b"HTTP/1.1 404 "));
    }

    #[test]
    fn reports_no_content_when_nothing_is_staged() {
        let latest = LatestImage::default();
        assert!(get(&latest, "/clipboard/image").starts_with(b"HTTP/1.1 204 "));
    }
}
