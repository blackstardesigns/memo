//! `memo --setup`: re-run the local-LLM backend chooser after install.
//!
//! Mirrors the selector in `install.sh` (MLX / Ollama / Skip): pick a backend
//! with the arrow keys, confirm with space/enter, cancel with Esc. The choice
//! updates `provider` / `base_url` / `model` in `config.toml` (preserving the
//! file's comments) and optionally installs the chosen backend.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use crossterm::event::{poll, read, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use crossterm::{cursor, execute, queue};

use crate::config::{self, Config};

/// Entry point for `memo --setup`.
pub fn run() -> Result<()> {
    let is_macos = cfg!(target_os = "macos");
    let mlx_here = is_macos && mlx_installed();
    let ollama = which("ollama");

    // Three options, mirroring the installer's labels.
    let mlx_note = if is_macos {
        "(recommended)"
    } else {
        "(Apple Silicon only)"
    };
    let mut mlx_label = format!("MLX (mlx-lm)   on-device, Apple Silicon   {mlx_note}");
    if mlx_here {
        mlx_label.push_str("   ✔ installed");
    }
    let mut ollama_label = String::from("Ollama         cross-platform: Intel, Linux, Windows");
    if ollama.is_some() {
        ollama_label.push_str("   ✔ installed");
    }
    let options = [
        mlx_label,
        ollama_label,
        String::from("Skip — don't change the server now"),
    ];
    // Default highlight: MLX on macOS (Apple Silicon), Ollama elsewhere.
    let default = if is_macos { 0 } else { 1 };

    println!();
    println!("  Choose a local LLM server for memo's AI refinement:");
    println!("  (↑/↓ move · space/enter select · esc cancel)");
    println!();

    let choice = match select(&options, default)? {
        Some(i) => i,
        None => {
            println!("  Setup cancelled — nothing changed.");
            return Ok(());
        }
    };

    match choice {
        0 => setup_mlx(is_macos, mlx_here),
        1 => setup_ollama(is_macos, ollama),
        _ => {
            println!("  Skipped — the server configuration was left unchanged.");
            Ok(())
        }
    }
}

/// Interactive radio selector. `Some(index)` on confirm; `None` on Esc/`q`/Ctrl-C
/// or when stdin/stdout isn't an interactive terminal (so a piped `memo --setup`
/// changes nothing).
fn select(options: &[String], default: usize) -> Result<Option<usize>> {
    if options.is_empty() {
        return Ok(None);
    }
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        // Not an interactive terminal: don't guess or change anything.
        return Ok(None);
    }

    let mut out = io::stdout();
    enable_raw_mode()?;
    let _ = execute!(out, cursor::Hide);
    // Discard anything typed before the prompt appeared so a stray early
    // keystroke can't move or confirm the selection.
    while poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
        let _ = read();
    }

    let mut cur = default.min(options.len() - 1);
    paint(&mut out, options, cur, true)?;

    let selected = loop {
        match read() {
            Ok(Event::Key(k)) => {
                if k.kind == KeyEventKind::Release {
                    continue;
                }
                match k.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        cur = if cur == 0 { options.len() - 1 } else { cur - 1 };
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        cur = (cur + 1) % options.len();
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => break Some(cur),
                    KeyCode::Esc | KeyCode::Char('q') => break None,
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        break None
                    }
                    _ => continue,
                }
                paint(&mut out, options, cur, false)?;
            }
            Ok(_) => {}
            Err(_) => break None,
        }
    };

    let _ = execute!(out, cursor::Show);
    disable_raw_mode()?;
    println!();
    Ok(selected)
}

/// Draw the option list. On the first call it prints in place; afterwards it
/// moves back up over the previous render and repaints, so the list updates
/// without scrolling.
fn paint(out: &mut impl Write, options: &[String], cur: usize, first: bool) -> Result<()> {
    if !first {
        queue!(out, cursor::MoveUp(options.len() as u16))?;
    }
    for (i, opt) in options.iter().enumerate() {
        queue!(out, cursor::MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        if i == cur {
            queue!(
                out,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print(format!("  ◉ {opt}")),
                SetAttribute(Attribute::Reset),
                ResetColor,
            )?;
        } else {
            queue!(
                out,
                SetForegroundColor(Color::DarkGrey),
                Print(format!("  ○ {opt}")),
                ResetColor,
            )?;
        }
        queue!(out, Print("\r\n"))?; // raw mode: explicit CRLF
    }
    out.flush()?;
    Ok(())
}

fn setup_mlx(is_macos: bool, already: bool) -> Result<()> {
    if !is_macos {
        println!("  ⚠ MLX runs on Apple Silicon only — choose Ollama on this platform.");
        return Ok(());
    }
    apply_provider(
        "mlx",
        "http://localhost:8080/v1",
        "mlx-community/Llama-3.1-8B-Instruct-4bit",
    )?;
    println!("  ✔ memo will use MLX (mlx-lm).");
    if already {
        println!("  mlx-lm is already installed.");
        return Ok(());
    }
    if confirm("Install mlx-lm now?", true) {
        if install_mlx() {
            println!("  ✔ mlx-lm installed.");
        } else {
            println!("  ⚠ Could not install mlx-lm automatically. Run:  pip install mlx-lm");
        }
    } else {
        println!("  Install it later with:  pip install mlx-lm");
    }
    Ok(())
}

fn setup_ollama(is_macos: bool, ollama: Option<PathBuf>) -> Result<()> {
    apply_provider("ollama", "http://localhost:11434/v1", "llama3.1")?;
    println!("  ✔ memo will use Ollama.");
    match ollama {
        Some(p) => println!("  Ollama is installed at {}.", p.display()),
        None => {
            println!("  ⚠ Ollama is not installed.");
            if is_macos
                && which("brew").is_some()
                && confirm("Install Ollama now with Homebrew?", true)
            {
                if run_cmd("brew", &["install", "ollama"]) {
                    println!("  ✔ Ollama installed.");
                } else {
                    println!("  ⚠ Install failed. Get it from:  https://ollama.com/download");
                }
            } else if cfg!(target_os = "linux") {
                println!("  Install it with:  curl -fsSL https://ollama.com/install.sh | sh");
            } else {
                println!("  Install it from:  https://ollama.com/download");
            }
        }
    }
    println!("  Then pull a model:  ollama pull llama3.1");
    Ok(())
}

/// Try the available Python package managers, in order of preference.
fn install_mlx() -> bool {
    if which("uv").is_some() {
        run_cmd("uv", &["pip", "install", "mlx-lm"])
    } else if which("pip3").is_some() {
        run_cmd("pip3", &["install", "--upgrade", "mlx-lm"])
    } else if which("pip").is_some() {
        run_cmd("pip", &["install", "--upgrade", "mlx-lm"])
    } else if which("python3").is_some() {
        run_cmd("python3", &["-m", "pip", "install", "--upgrade", "mlx-lm"])
    } else {
        println!("  ⚠ Python not found. Install it, then run:  pip install mlx-lm");
        false
    }
}

/// Update `provider` / `base_url` / `model` in `config.toml`, preserving the
/// rest of the file (comments and all other settings), then verify it parses.
fn apply_provider(provider: &str, base_url: &str, model: &str) -> Result<()> {
    // Ensure the file exists with documented defaults.
    Config::load_or_create()?;
    let path = config::config_path();
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let text = set_toml_value(&text, "provider", provider);
    let text = set_toml_value(&text, "base_url", base_url);
    let text = set_toml_value(&text, "model", model);
    std::fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
    // Confirm the edited file is still valid.
    Config::load_or_create()?;
    Ok(())
}

/// Replace the value of a top-level `key = "..."` assignment (one appearing
/// before the first `[table]` header), preserving everything else. Inserts the
/// key before the first table if it isn't already present at top level.
fn set_toml_value(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let new_line = format!("{key} = \"{value}\"");
    let first_table = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let eq_space = format!("{key} =");
    let eq_tight = format!("{key}=");
    let found = lines[..first_table].iter().position(|l| {
        let t = l.trim_start();
        t.starts_with(&eq_space) || t.starts_with(&eq_tight)
    });
    match found {
        Some(i) => lines[i] = new_line,
        None => lines.insert(first_table, new_line),
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Simple y/N prompt on the terminal; returns `default_yes` when non-interactive.
fn confirm(question: &str, default_yes: bool) -> bool {
    if !io::stdin().is_terminal() {
        return default_yes;
    }
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("  {question} {hint} ");
    let _ = io::stdout().flush();
    let mut s = String::new();
    if io::stdin().read_line(&mut s).is_err() {
        return default_yes;
    }
    match s.trim().chars().next() {
        Some('y') | Some('Y') => true,
        Some('n') | Some('N') => false,
        _ => default_yes,
    }
}

/// Run a command, echoing it first; returns whether it exited successfully.
fn run_cmd(prog: &str, args: &[&str]) -> bool {
    println!("  → {prog} {}", args.join(" "));
    Command::new(prog)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// First match for `name` on `$PATH`, if any.
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let p = dir.join(name);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    })
}

/// Whether the `mlx_lm` Python package is importable.
fn mlx_installed() -> bool {
    Command::new("python3")
        .args(["-c", "import mlx_lm"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::set_toml_value;

    #[test]
    fn replaces_existing_value_and_keeps_comments() {
        let src = "# a comment\nprovider = \"mlx\"\nbase_url = \"http://localhost:8080/v1\"\nmodel = \"x\"\n\n[theme]\naccent = \"yellow\"\n";
        let out = set_toml_value(src, "provider", "ollama");
        assert!(out.contains("provider = \"ollama\""));
        assert!(out.contains("# a comment"));
        assert!(out.contains("[theme]"));
        assert!(out.contains("accent = \"yellow\""));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn does_not_touch_similarly_named_keys() {
        let src = "model = \"x\"\ndictation_model = \"base.en\"\n";
        let out = set_toml_value(src, "model", "llama3.1");
        assert!(out.contains("model = \"llama3.1\""));
        assert!(out.contains("dictation_model = \"base.en\""));
    }

    #[test]
    fn inserts_missing_key_before_first_table() {
        let src = "model = \"x\"\n[theme]\naccent = \"yellow\"\n";
        let out = set_toml_value(src, "provider", "ollama");
        let prov = out.find("provider = \"ollama\"").unwrap();
        let theme = out.find("[theme]").unwrap();
        assert!(prov < theme, "provider must be inserted before [theme]");
    }

    #[test]
    fn edited_default_config_still_parses() {
        // Apply all three edits to the embedded default and confirm it round-trips.
        let mut text = crate::config::DEFAULT_CONFIG_TOML.to_string();
        text = set_toml_value(&text, "provider", "ollama");
        text = set_toml_value(&text, "base_url", "http://localhost:11434/v1");
        text = set_toml_value(&text, "model", "llama3.1");
        let cfg: crate::config::Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.provider, crate::config::Provider::Ollama);
        assert_eq!(cfg.base_url, "http://localhost:11434/v1");
        assert_eq!(cfg.model, "llama3.1");
    }
}
