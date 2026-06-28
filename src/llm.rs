use std::io::Read;
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use crate::config::Config;

/// Upper bound on the model-server response we'll read into memory, so a hostile
/// or buggy server pointed at by `base_url` can't OOM the process.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// Message sent from the background refine thread back to the UI.
pub enum RefineMsg {
    Done(String),
    Error(String),
}

/// Spawn a background thread that refines `content` via the configured LLM and
/// returns a receiver the UI can poll without blocking. `prompt` is the system
/// prompt to use; pass `cfg.refine_prompt.clone()` for the default.
///
/// `wait_ready` is how long to wait for the server's port to start accepting
/// connections before sending the request — used in on-demand mode, where a
/// freshly launched server doesn't open its port until the model has loaded. Pass
/// `Duration::ZERO` to skip the wait (an already-running server fails fast if it
/// is unreachable).
pub fn spawn_refine(
    cfg: &Config,
    prompt: String,
    content: String,
    wait_ready: Duration,
) -> Receiver<RefineMsg> {
    let (tx, rx) = mpsc::channel();
    let base_url = cfg.base_url.clone();
    let model = cfg.model.clone();
    let api_key = cfg.api_key.clone();
    let temperature = cfg.temperature;
    let max_tokens = cfg.max_tokens;
    let timeout = cfg.request_timeout_secs;
    let stop = cfg.stop.clone();

    thread::spawn(move || {
        if !wait_ready.is_zero() {
            wait_for_server(&base_url, wait_ready);
        }
        let result = refine_blocking(
            &base_url,
            &model,
            &api_key,
            &prompt,
            temperature,
            max_tokens,
            timeout,
            &stop,
            &content,
        );
        let msg = match result {
            Ok(text) => RefineMsg::Done(text),
            Err(e) => RefineMsg::Error(format!("{e:#}")),
        };
        let _ = tx.send(msg);
    });
    rx
}

#[allow(clippy::too_many_arguments)]
fn refine_blocking(
    base_url: &str,
    model: &str,
    api_key: &str,
    prompt: &str,
    temperature: f32,
    max_tokens: u32,
    timeout_secs: u64,
    stop: &[String],
    content: &str,
) -> Result<String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    // Each refine is a single, self-contained request: only this note's content is
    // sent ([system prompt, user note]), with no prior turns, so nothing carries
    // over between refinements.
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": content },
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false,
    });
    let stops: Vec<&String> = stop.iter().filter(|s| !s.is_empty()).collect();
    if !stops.is_empty() {
        body["stop"] = serde_json::json!(stops);
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(timeout_secs))
        .build();

    let mut req = agent.post(&url).set("Content-Type", "application/json");
    // Only attach the key over a secure transport (HTTPS, or a loopback host) so
    // it can't leak in cleartext to a remote plain-HTTP endpoint.
    if !api_key.is_empty() && key_transport_is_safe(base_url) {
        req = req.set("Authorization", &format!("Bearer {api_key}"));
    }

    let resp = match req.send_json(body) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let detail = resp.into_string().unwrap_or_default();
            bail!("server returned HTTP {code}: {}", detail.trim());
        }
        Err(e) => return Err(anyhow!("could not reach the model server at {url}: {e}")),
    };

    // Read at most MAX_RESPONSE_BYTES before parsing, instead of trusting the
    // server's Content-Length / sending the whole body straight into the parser.
    let reader = resp.into_reader().take(MAX_RESPONSE_BYTES);
    let value: serde_json::Value =
        serde_json::from_reader(reader).context("parsing model JSON response")?;
    let text = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("unexpected response shape: {value}"))?;
    Ok(sanitize_output(text, stop))
}

/// Whether it's safe to send the API key to `base_url`: only over HTTPS, or to a
/// loopback host (where plain HTTP never leaves the machine).
fn key_transport_is_safe(base_url: &str) -> bool {
    let scheme = base_url
        .split("://")
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    scheme == "https" || host_is_loopback(&url_host(base_url))
}

/// Block until the model server at `base_url` accepts a TCP connection, or
/// `timeout` elapses. In on-demand mode a just-launched `mlx_lm.server` doesn't
/// open its port until the model finishes loading, so firing the request straight
/// away would hit a connection refused. If the address can't be parsed we skip the
/// wait and let the request itself surface any error.
fn wait_for_server(base_url: &str, timeout: Duration) {
    let host = url_host(base_url);
    let Some(port) = url_port(base_url) else {
        return;
    };
    let deadline = Instant::now() + timeout;
    loop {
        if tcp_reachable(&host, port) {
            return;
        }
        if Instant::now() >= deadline {
            return; // gave up; refine_blocking will report the unreachable server
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Whether a TCP connection to `host:port` succeeds within a short timeout.
fn tcp_reachable(host: &str, port: u16) -> bool {
    match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs
            .filter_map(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok())
            .next()
            .is_some(),
        Err(_) => false,
    }
}

/// Extract the port from a URL's authority (mirrors [`url_host`]). `None` when the
/// URL has no explicit port.
fn url_port(base_url: &str) -> Option<u16> {
    let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // For a bracketed IPv6 literal (`[::1]:8080`) the port follows the `]`.
    let after_host = match host_port.strip_prefix('[') {
        Some(rest) => rest.split(']').nth(1).unwrap_or(""),
        None => host_port,
    };
    after_host.rsplit(':').next()?.parse::<u16>().ok()
}

/// Extract the host from a URL's authority, dropping scheme, userinfo and port.
fn url_host(base_url: &str) -> String {
    let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = host_port.strip_prefix('[') {
        // Bracketed IPv6 literal: `[::1]:8080`.
        return rest.split(']').next().unwrap_or("").to_string();
    }
    if let Some((host, port)) = host_port.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return host.to_string();
        }
    }
    host_port.to_string()
}

fn host_is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Clean up a model response: cut it at the first stop sequence and remove any
/// remaining chat-template special tokens (e.g. `<|eot_id|>`) so the markers
/// never end up saved in a note, even if the server ignored the `stop` param.
fn sanitize_output(text: &str, stop: &[String]) -> String {
    let mut cut = text.len();
    for s in stop.iter().filter(|s| !s.is_empty()) {
        if let Some(i) = text.find(s.as_str()) {
            cut = cut.min(i);
        }
    }
    let truncated = &text[..cut];
    strip_special_tokens(truncated).trim().to_string()
}

/// Remove `<|...|>`-style special tokens, keeping the surrounding text intact.
fn strip_special_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<|") {
        out.push_str(&rest[..start]);
        match rest[start..].find("|>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out, // unterminated token: drop the trailing fragment
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// Spawn a one-shot HTTP server that replies with `status`/`body` and returns its port.
    fn mock_server(status: &'static str, body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                crate::testutil::drain_http_request(&mut stream);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        port
    }

    #[test]
    fn parses_openai_response_and_trims() {
        let port = mock_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"  Refined output  "}}]}"#,
        );
        let url = format!("http://127.0.0.1:{port}/v1");
        let out = refine_blocking(&url, "m", "", "p", 0.3, 64, 10, &[], "raw note").unwrap();
        assert_eq!(out, "Refined output");
    }

    #[test]
    fn strips_stop_sequences_and_special_tokens() {
        let stop = ["<|eot_id|>".to_string()];
        // Cut at the stop sequence and drop a leaked header token before it.
        let out = sanitize_output(
            "# Notes\n- done<|eot_id|><|start_header_id|>assistant<|end_header_id|>",
            &stop,
        );
        assert_eq!(out, "# Notes\n- done");
        // Stray tokens with no stop match are still removed.
        let out = sanitize_output("hello <|im_end|> world", &[]);
        assert_eq!(out, "hello  world".trim());
    }

    #[test]
    fn reports_http_error_status() {
        let port = mock_server("500 Internal Server Error", "boom");
        let url = format!("http://127.0.0.1:{port}/v1");
        let err = refine_blocking(&url, "m", "", "p", 0.3, 64, 10, &[], "x").unwrap_err();
        assert!(format!("{err:#}").contains("500"));
    }

    #[test]
    fn reports_unreachable_server() {
        // Nothing is listening on this port.
        let err = refine_blocking("http://127.0.0.1:1/v1", "m", "", "p", 0.3, 64, 2, &[], "x")
            .unwrap_err();
        assert!(format!("{err:#}").contains("could not reach"));
    }

    #[test]
    fn parses_port_for_readiness_wait() {
        assert_eq!(url_port("http://localhost:8080/v1"), Some(8080));
        assert_eq!(url_port("http://127.0.0.1:11434/v1"), Some(11434));
        assert_eq!(url_port("https://user:pw@host:9000/v1"), Some(9000));
        assert_eq!(url_port("http://[::1]:8080/v1"), Some(8080));
        // No explicit port: nothing to wait on, so the request is sent straight away.
        assert_eq!(url_port("http://localhost/v1"), None);
        assert_eq!(url_port("http://[::1]/v1"), None);
    }

    #[test]
    fn zero_wait_returns_immediately() {
        // wait_for_server must be a no-op when given a zero timeout against a
        // closed port (otherwise on-demand=off refines would stall).
        let start = Instant::now();
        wait_for_server("http://127.0.0.1:1/v1", Duration::ZERO);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn key_sent_only_over_https_or_loopback() {
        // Loopback over plain HTTP is fine (stays on the machine).
        assert!(key_transport_is_safe("http://localhost:8080/v1"));
        assert!(key_transport_is_safe("http://127.0.0.1:11434/v1"));
        assert!(key_transport_is_safe("http://[::1]:8080/v1"));
        // HTTPS anywhere is fine.
        assert!(key_transport_is_safe("https://api.example.com/v1"));
        // Plain HTTP to a remote host must NOT carry the key.
        assert!(!key_transport_is_safe("http://api.example.com/v1"));
        assert!(!key_transport_is_safe("http://192.168.1.10:8080/v1"));
    }
}
