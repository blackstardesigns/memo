//! Optional lifecycle management for a local model server (`mlx_lm.server` or
//! `ollama serve`, selected by `provider`).
//!
//! When `auto_start_server` is enabled, [`ManagedServer::start`] launches the
//! server as a child process and dropping the returned guard (on quit or panic)
//! shuts it back down. The guard is held by `App`, which starts it either before
//! the TUI opens (load-at-startup) or lazily on the first refine and then tears it
//! down after an idle period (`server_on_demand`).
//!
//! To stay robust across runs we record the server's PID in a small pid file. If
//! a previous `note` session was force-killed (e.g. the terminal was closed)
//! before it could stop the server, the next launch re-attaches to that process
//! via the pid file and stops it. A server we didn't start — one already
//! listening on the port with no pid file of ours — is left untouched.

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config::{self, Config, Provider};

/// How long to watch a freshly spawned server before deciding it started. A real
/// server stays up to load the model; one that exits within this window failed
/// (e.g. `mlx_lm` not installed in the chosen Python, or `ollama` missing).
const STARTUP_PROBE: Duration = Duration::from_millis(500);

/// A `mlx_lm.server` process whose lifetime this guard owns. Dropping it stops
/// the server.
pub struct ManagedServer {
    /// PID of the server (also its process-group id, since it leads a new group).
    pid: u32,
    /// Present only for a server spawned in this session, so we can reap it.
    child: Option<Child>,
}

impl ManagedServer {
    /// Start (or re-attach to) the server if configured. Returns `Ok(None)` when
    /// auto-start is disabled or an unmanaged server already owns the port.
    pub fn start(cfg: &Config) -> Result<Option<ManagedServer>> {
        if !cfg.auto_start_server {
            return Ok(None);
        }
        let port = port_from_base_url(&cfg.base_url).unwrap_or(default_port(cfg.provider));

        // Re-attach to a server we left running in a previous session so we can
        // shut it down now instead of orphaning it. Only adopt the PID if a live
        // process with that id actually looks like our model server — otherwise a
        // recycled PID could make us signal an unrelated process group on quit.
        if let Some(pid) = read_pidfile() {
            if process_alive(pid) && pid_is_our_server(pid, cfg.provider) {
                return Ok(Some(ManagedServer { pid, child: None }));
            }
            let _ = fs::remove_file(pidfile_path()); // stale, or not our process
        }

        // A server we don't own is already serving here: leave it alone.
        if port_is_open(port) {
            return Ok(None);
        }

        reset_log();
        let mut cmd = build_command(cfg, port)?;
        let mut child = cmd
            .spawn()
            .with_context(|| format!("launching {:?}", cmd.get_program()))?;
        let pid = child.id();

        // If the process dies during the probe window it never really started.
        // Surface the reason (with the log tail) instead of failing silently.
        std::thread::sleep(STARTUP_PROBE);
        if let Ok(Some(status)) = child.try_wait() {
            let hint = match cfg.provider {
                Provider::Mlx => {
                    "This usually means mlx-lm isn't installed in the Python that ran it. \
                     Set `venv` (and/or `mlx_repo`) in your config to an environment that \
                     has mlx-lm installed."
                }
                Provider::Ollama => {
                    "This usually means the `ollama` command isn't on your PATH, or the port \
                     is already in use. Install Ollama from https://ollama.com, or set \
                     `auto_start_server = false` and run `ollama serve` yourself."
                }
            };
            bail!(
                "the {} server exited immediately ({status}).\n\n{hint}\n\n\
                 Recent server log ({}):\n{}",
                cfg.provider.label(),
                log_path().display(),
                log_tail()
            );
        }

        write_pidfile(pid);
        Ok(Some(ManagedServer {
            pid,
            child: Some(child),
        }))
    }

    /// Ask the server's whole process group to exit (SIGTERM), wait briefly, then
    /// force it (SIGKILL). Reaps an owned child and clears the pid file.
    fn shutdown(&mut self) {
        term_group(self.pid);
        for _ in 0..40 {
            if self.exited() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !self.exited() {
            kill_group(self.pid);
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
            }
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
        let _ = fs::remove_file(pidfile_path());
    }

    /// Whether the server has exited. Uses the child handle when we own it (which
    /// also reaps the zombie), otherwise probes the PID directly.
    fn exited(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(Some(_))),
            None => !process_alive(self.pid),
        }
    }
}

impl Drop for ManagedServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Default port for a provider, used when `base_url` has no explicit port.
fn default_port(provider: Provider) -> u16 {
    match provider {
        Provider::Mlx => 8080,
        Provider::Ollama => 11434,
    }
}

/// Build the server command for the configured provider. Output is sent to a log
/// file so it can't corrupt the TUI, and the process leads its own group so we
/// can signal the whole tree on shutdown.
fn build_command(cfg: &Config, port: u16) -> Result<Command> {
    let mut cmd = match cfg.provider {
        Provider::Mlx => build_mlx_command(cfg, port)?,
        Provider::Ollama => build_ollama_command(port),
    };

    cmd.stdin(Stdio::null())
        .stdout(log_target())
        .stderr(log_target());

    // Put the server in its own process group so we can signal the whole tree.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    Ok(cmd)
}

/// Build `python -m mlx_lm.server --model <model> --port <port>`, honouring the
/// configured repo directory and virtualenv.
fn build_mlx_command(cfg: &Config, port: u16) -> Result<Command> {
    let mut cmd = Command::new(python_path(cfg)?);
    cmd.arg("-m")
        .arg("mlx_lm.server")
        .arg("--model")
        .arg(&cfg.model)
        .arg("--port")
        .arg(port.to_string());

    if !cfg.mlx_repo.trim().is_empty() {
        let repo = config::expand_tilde(&cfg.mlx_repo);
        if !repo.is_dir() {
            bail!("`mlx_repo` is not a directory: {}", repo.display());
        }
        cmd.current_dir(repo);
    }

    Ok(cmd)
}

/// Build `ollama serve`, binding it to `port` via the `OLLAMA_HOST` env var
/// (the only way to set the listen address for `ollama serve`). Ollama loads
/// models lazily on the first request, so no model argument is needed here — the
/// model from your config must already be pulled (`ollama pull <model>`).
fn build_ollama_command(port: u16) -> Command {
    let mut cmd = Command::new("ollama");
    cmd.arg("serve")
        .env("OLLAMA_HOST", format!("127.0.0.1:{port}"));
    cmd
}

/// Resolve the Python interpreter to run the server with.
///
/// With a `venv` configured we use `<venv>/bin/python` (a relative venv is
/// resolved against `mlx_repo`). If the venv is set but no interpreter is found
/// there, that's a configuration error we report — rather than silently falling
/// back to a system `python3` that probably lacks `mlx_lm`. With no venv we use
/// `python3` from `PATH`.
fn python_path(cfg: &Config) -> Result<PathBuf> {
    let venv = cfg.venv.trim();
    if venv.is_empty() {
        return Ok(PathBuf::from("python3"));
    }

    let mut dir = config::expand_tilde(venv);
    if dir.is_relative() {
        if cfg.mlx_repo.trim().is_empty() {
            bail!(
                "`venv` is a relative path ({venv}) but `mlx_repo` is empty — use an \
                 absolute venv path, or set `mlx_repo` so it can be resolved."
            );
        }
        dir = config::expand_tilde(&cfg.mlx_repo).join(&dir);
    }

    for name in ["python", "python3"] {
        let py = dir.join("bin").join(name);
        if py.exists() {
            return Ok(py);
        }
    }
    bail!(
        "no Python interpreter found in the configured venv (looked for \
         {0}/bin/python and {0}/bin/python3). Check `venv`/`mlx_repo` in your config.",
        dir.display()
    )
}

fn log_path() -> PathBuf {
    config::config_dir().join("mlx-server.log")
}

/// Append-only handle to the server log (stdout and stderr share it).
fn log_target() -> Stdio {
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        Ok(f) => Stdio::from(f),
        Err(_) => Stdio::null(),
    }
}

/// Truncate the log so its contents reflect only the current launch.
fn reset_log() {
    let _ = fs::create_dir_all(config::config_dir());
    let _ = fs::write(log_path(), b"");
}

/// Last few lines of the server log, for embedding in an error message.
fn log_tail() -> String {
    match fs::read_to_string(log_path()) {
        Ok(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let start = lines.len().saturating_sub(15);
            let tail = lines[start..].join("\n");
            if tail.trim().is_empty() {
                "(server log is empty)".to_string()
            } else {
                tail
            }
        }
        Err(_) => "(no server log)".to_string(),
    }
}

fn pidfile_path() -> PathBuf {
    config::config_dir().join("mlx-server.pid")
}

fn read_pidfile() -> Option<u32> {
    fs::read_to_string(pidfile_path()).ok()?.trim().parse().ok()
}

fn write_pidfile(pid: u32) {
    let _ = fs::create_dir_all(config::config_dir());
    let _ = fs::write(pidfile_path(), pid.to_string());
}

// --- process signalling ----------------------------------------------------
// The server leads its own process group (see `process_group(0)`), so a negative
// PID signals the whole group, catching any child processes it spawned.

#[cfg(unix)]
fn term_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Signal 0 performs error checking but sends nothing: 0 => the process exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn term_group(_pid: u32) {}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    // No portable probe; rely on the owned child handle to drive shutdown.
    false
}

// --- PID ownership verification ---------------------------------------------
// A PID alone doesn't prove the live process is *our* server: after a previous
// session was force-killed without clearing the pid file, the OS may recycle that
// PID for something unrelated. Before adopting (and later signalling) a PID, we
// confirm the running process's command line looks like the server we manage.

/// Marker substring identifying our server in a process's command line.
fn server_marker(provider: Provider) -> &'static str {
    match provider {
        Provider::Mlx => "mlx_lm.server",
        Provider::Ollama => "ollama",
    }
}

/// Whether `command` (a process command line) looks like the server we manage.
fn command_is_server(command: &str, provider: Provider) -> bool {
    command.contains(server_marker(provider))
}

/// True only when a live `pid` looks like our model server, so we never
/// SIGTERM/SIGKILL a process group that merely reuses a recycled PID.
fn pid_is_our_server(pid: u32, provider: Provider) -> bool {
    match process_command(pid) {
        Some(cmd) => command_is_server(&cmd, provider),
        None => false,
    }
}

/// The command line of `pid` via `ps`. `None` when it can't be determined (the
/// process is gone, `ps` is unavailable, or a non-unix platform) — in which case
/// we decline to adopt the PID.
#[cfg(unix)]
fn process_command(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("command=")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cmd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if cmd.is_empty() {
        None
    } else {
        Some(cmd)
    }
}

#[cfg(not(unix))]
fn process_command(_pid: u32) -> Option<String> {
    None
}

/// Extract the port from a base URL like `http://localhost:8080/v1`.
fn port_from_base_url(base_url: &str) -> Option<u16> {
    let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    host_port.rsplit(':').next()?.parse::<u16>().ok()
}

/// True if something is already accepting connections on `127.0.0.1:port`.
fn port_is_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_server_matches_only_real_servers() {
        assert!(command_is_server(
            "/opt/venv/bin/python -m mlx_lm.server --model x --port 8080",
            Provider::Mlx
        ));
        assert!(command_is_server("ollama serve", Provider::Ollama));
        // Wrong provider's marker must not match.
        assert!(!command_is_server("ollama serve", Provider::Mlx));
        assert!(!command_is_server(
            "python -m mlx_lm.server",
            Provider::Ollama
        ));
        // An unrelated process that reused the PID must never be adopted.
        assert!(!command_is_server("/usr/bin/vim notes.md", Provider::Mlx));
        assert!(!command_is_server(
            "/usr/bin/vim notes.md",
            Provider::Ollama
        ));
    }

    #[test]
    fn parses_port_from_base_url() {
        assert_eq!(port_from_base_url("http://localhost:8080/v1"), Some(8080));
        assert_eq!(port_from_base_url("http://127.0.0.1:1234"), Some(1234));
        assert_eq!(
            port_from_base_url("https://user:pw@host:9000/v1"),
            Some(9000)
        );
        assert_eq!(port_from_base_url("http://localhost/v1"), None);
    }

    #[cfg(unix)]
    #[test]
    fn detects_live_and_dead_pids() {
        assert!(process_alive(std::process::id()));
        // PID 0 maps to "current process group"; a very high PID is almost
        // certainly free, so it should read as not alive.
        assert!(!process_alive(2_000_000_000));
    }

    #[test]
    fn python_path_defaults_to_python3_without_venv() {
        let cfg = Config::default();
        assert_eq!(python_path(&cfg).unwrap(), PathBuf::from("python3"));
    }

    #[test]
    fn default_port_matches_provider() {
        assert_eq!(default_port(Provider::Mlx), 8080);
        assert_eq!(default_port(Provider::Ollama), 11434);
    }

    #[test]
    fn ollama_command_serves_on_configured_port() {
        let cmd = build_ollama_command(11434);
        assert_eq!(cmd.get_program(), "ollama");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["serve"]);
        let host = cmd
            .get_envs()
            .find(|(k, _)| *k == "OLLAMA_HOST")
            .and_then(|(_, v)| v)
            .unwrap();
        assert_eq!(host, "127.0.0.1:11434");
    }

    #[test]
    fn python_path_errors_when_venv_interpreter_missing() {
        let cfg = Config {
            venv: "/definitely/not/a/real/venv".into(),
            ..Config::default()
        };
        // Must NOT silently fall back to python3 — that's the bug we're fixing.
        let err = python_path(&cfg).unwrap_err();
        assert!(format!("{err:#}").contains("/definitely/not/a/real/venv"));
    }

    #[test]
    fn python_path_errors_on_relative_venv_without_repo() {
        let cfg = Config {
            venv: ".venv".into(),
            mlx_repo: String::new(),
            ..Config::default()
        };
        let err = python_path(&cfg).unwrap_err();
        assert!(format!("{err:#}").contains("relative"));
    }
}
