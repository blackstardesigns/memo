//! Helpers shared across the crate's tests.

use std::io::Read;
use std::net::TcpStream;

/// Read a full HTTP request (headers + any `Content-Length` body) from `stream`.
///
/// Mock servers must consume the entire request before responding and closing,
/// otherwise the client can observe a reset mid-send and the response read fails.
pub fn drain_http_request(stream: &mut TcpStream) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
            let content_len = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= p + 4 + content_len {
                return;
            }
        }
    }
}
