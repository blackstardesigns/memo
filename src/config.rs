use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};

/// Which local model server `memo` manages when `auto_start_server` is enabled.
/// Both backends are reached over the same OpenAI-compatible chat-completions
/// API, so this only changes how the server is launched — not how requests are
/// made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Apple MLX, launched via `python -m mlx_lm.server` (default).
    #[default]
    Mlx,
    /// Ollama, launched via `ollama serve`.
    Ollama,
}

impl Provider {
    /// Human-readable name used in status messages.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Mlx => "MLX",
            Provider::Ollama => "Ollama",
        }
    }
}

/// User configuration, loaded from `config.toml`. Any field missing from the file
/// falls back to its default (see [`Config::default`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// OpenAI-compatible base URL of the model server, e.g.
    /// `http://localhost:8080/v1` (MLX) or `http://localhost:11434/v1` (Ollama).
    pub base_url: String,
    /// Model name the server should use.
    pub model: String,
    /// API key (usually empty for a local MLX or Ollama server).
    pub api_key: String,
    /// Which local backend `memo` auto-starts: `mlx` (default) or `ollama`. The
    /// chat requests are identical for both; this only selects the server command.
    pub provider: Provider,
    /// Notes directory. Empty => platform default. Supports a leading `~`.
    pub data_dir: String,
    /// The single prompt used when refining a note (Ctrl+R).
    pub refine_prompt: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub request_timeout_secs: u64,
    /// Sequences that force the model to stop generating. Also stripped from the
    /// response, so leaked chat-template tokens never end up in your notes.
    pub stop: Vec<String>,
    /// Automatically start a local `mlx_lm.server` on launch and stop it on quit.
    pub auto_start_server: bool,
    /// When `auto_start_server` is on, defer launching the server until the first
    /// refine (Ctrl+R) instead of at startup, and shut it back down once it has
    /// been idle for `server_idle_timeout_secs`. Keeps the model out of memory
    /// until refinement is actually used. `false` (default) keeps the server
    /// resident for the whole session.
    pub server_on_demand: bool,
    /// On-demand mode only (`server_on_demand = true`): how long, in seconds, to
    /// keep the server alive after the last refine before shutting it down. `0`
    /// stops it immediately after each refine; a larger value keeps the model
    /// warm so back-to-back refines reuse it instead of reloading.
    pub server_idle_timeout_secs: u64,
    /// Path to your local mlx-lm repo / working directory (optional). The server
    /// is launched from here, and a relative `venv` is resolved against it.
    pub mlx_repo: String,
    /// Virtualenv used to run the server: an absolute/`~` path, or a name relative
    /// to `mlx_repo` (e.g. `.venv`). Empty => use `python3` from `PATH`.
    pub venv: String,
    /// Show the keyboard-shortcuts hint bar along the bottom of the screen.
    pub show_shortcuts: bool,

    // --- Speech-to-text dictation ------------------------------------------
    /// Editor key that drives dictation: hold to push-to-talk, double-press to
    /// toggle continuous live listening. Accepts a function key like `"F5"` /
    /// `"f6"`, or a combo like `"ctrl+k"` / `"ctrl+g"`. On macOS, `"F5"` may be the
    /// system Dictation key (disable that shortcut, or use standard function keys,
    /// or pick a combo). Must be a top-level key in the config file (above
    /// `[theme]`). See [`parse_key_binding`].
    pub dictation_key: String,
    /// Which whisper.cpp GGML model to use, e.g. `"base.en"`, `"small.en"`,
    /// `"medium"`. Used to locate/download `ggml-<model>.bin`.
    pub dictation_model: String,
    /// Explicit path to a GGML model file. Empty => a managed cache path derived
    /// from `dictation_model` (see [`Config::resolved_model_path`]).
    pub dictation_model_path: String,
    /// Spoken language hint, e.g. `"en"`, or `"auto"` to let whisper detect it.
    pub dictation_language: String,
    /// Live-listening: trailing silence (ms) that ends a phrase and triggers its
    /// transcription.
    pub dictation_silence_ms: u64,

    /// Colors and padding for the UI.
    pub theme: Theme,
}

/// User-customizable colors and spacing. Color values accept names
/// (`yellow`, `darkgray`, …), `#rrggbb` hex, or a 0–255 palette index.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    /// Selection highlight, search focus, active accents.
    pub accent: String,
    /// Default (unselected) borders.
    pub border: String,
    /// Refined-view accents (borders, etc.).
    pub refined: String,
    /// Color of the ✦ marker shown for refined notes.
    pub star: String,
    /// Editor title-bar foreground / background.
    pub title_fg: String,
    pub title_bg: String,
    /// Footer hint-bar foreground / background.
    pub footer_fg: String,
    pub footer_bg: String,
    /// Transient status messages.
    pub status: String,
    /// Inner padding (cells) for the editor body and note tiles.
    pub padding: u16,
    /// Horizontal divider line between the note header and content.
    pub divider: String,
    /// Use rounded corners on note tiles in the list view.
    pub rounded_tiles: bool,
    /// Color for created/modified timestamps on tiles and in the editor header.
    pub meta: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: "yellow".into(),
            border: "#313244".into(),
            refined: "#313244".into(),
            star: "#313244".into(),
            title_fg: "yellow".into(),
            title_bg: "none".into(),
            footer_fg: "#313244".into(),
            footer_bg: "none".into(),
            status: "green".into(),
            padding: 2,
            divider: "#313244".into(),
            rounded_tiles: true,
            meta: "#585b70".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base_url: "http://localhost:8080/v1".to_string(),
            model: "mlx-community/Llama-3.1-8B-Instruct-4bit".to_string(),
            api_key: String::new(),
            provider: Provider::Mlx,
            data_dir: String::new(),
            refine_prompt: default_refine_prompt(),
            temperature: 0.3,
            max_tokens: 2048,
            request_timeout_secs: 120,
            stop: default_stop(),
            auto_start_server: true,
            server_on_demand: false,
            server_idle_timeout_secs: 120,
            mlx_repo: String::new(),
            venv: String::new(),
            show_shortcuts: true,
            dictation_key: "F5".to_string(),
            dictation_model: "base.en".to_string(),
            dictation_model_path: String::new(),
            dictation_language: "en".to_string(),
            dictation_silence_ms: 700,
            theme: Theme::default(),
        }
    }
}

/// Stop sequences that catch chat-template tokens commonly leaked by instruct
/// models (Llama 3 `<|eot_id|>` …, ChatML `<|im_end|>`) so generation halts and
/// the markers never reach the saved note.
fn default_stop() -> Vec<String> {
    vec![
        "<|eot_id|>".into(),
        "<|end_of_text|>".into(),
        "<|start_header_id|>".into(),
        "<|im_end|>".into(),
    ]
}

fn default_refine_prompt() -> String {
    "You are a careful note editor. Improve the user's raw note: fix grammar and spelling, \
improve clarity and flow, and organise the content using Markdown headings (#, ##) and bullet \
points (-) where helpful. Preserve all facts and meaning, and do not invent information. Return \
ONLY the improved Markdown note, with no preamble or commentary."
        .to_string()
}

impl Config {
    /// Load the config file, creating it with documented defaults if it doesn't exist.
    pub fn load_or_create() -> Result<Config> {
        let path = config_path();
        if path.exists() {
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let cfg: Config =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            Ok(cfg)
        } else {
            fs::create_dir_all(config_dir())?;
            fs::write(&path, DEFAULT_CONFIG_TOML)
                .with_context(|| format!("writing default config to {}", path.display()))?;
            Ok(Config::default())
        }
    }

    /// Resolve (and create) the directory where notes are stored.
    pub fn resolved_data_dir(&self) -> Result<PathBuf> {
        let dir = if self.data_dir.trim().is_empty() {
            default_data_dir()
        } else {
            expand_tilde(&self.data_dir)
        };
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating data directory {}", dir.display()))?;
        Ok(dir)
    }

    /// Resolve the GGML speech model file path: an explicit `dictation_model_path`
    /// if set, otherwise `<models_dir>/ggml-<dictation_model>.bin` in the managed
    /// cache. The file may not exist yet (it is downloaded on first use).
    pub fn resolved_model_path(&self) -> PathBuf {
        if !self.dictation_model_path.trim().is_empty() {
            expand_tilde(&self.dictation_model_path)
        } else {
            models_dir().join(format!("ggml-{}.bin", self.dictation_model))
        }
    }
}

/// Directory holding the config file, honouring `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("memo");
        }
    }
    home()
        .map(|h| h.join(".config").join("memo"))
        .unwrap_or_else(|| PathBuf::from(".memo"))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Directory for cached/downloaded speech models, alongside the notes data dir.
pub fn models_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("memo").join("models");
        }
    }
    home()
        .map(|h| {
            h.join(".local")
                .join("share")
                .join("memo")
                .join("models")
        })
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn default_data_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("memo").join("notes");
        }
    }
    home()
        .map(|h| {
            h.join(".local")
                .join("share")
                .join("memo")
                .join("notes")
        })
        .unwrap_or_else(|| PathBuf::from("notes"))
}

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Expand a leading `~` / `~/` to the user's home directory.
pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        if let Some(h) = home() {
            return h;
        }
    } else if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = home() {
            return h.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Parse a key-binding string like `"F5"`, `"ctrl+g"`, or `"ctrl+space"` into a
/// `(KeyCode, KeyModifiers)` pair. Tokens are split on `+`; the final token is the
/// key and any preceding tokens are modifiers. Case-insensitive. Falls back to
/// `F5` (no modifiers) if the string can't be understood, so a typo in the config
/// never disables the editor.
pub fn parse_key_binding(s: &str) -> (KeyCode, KeyModifiers) {
    let fallback = (KeyCode::F(5), KeyModifiers::NONE);
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return fallback;
    }

    let mut mods = KeyModifiers::NONE;
    let mut parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
    let key_tok = match parts.pop() {
        Some(k) if !k.is_empty() => k.to_ascii_lowercase(),
        _ => return fallback,
    };
    for m in parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" | "opt" | "option" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            "meta" | "cmd" | "command" | "super" | "win" => mods |= KeyModifiers::SUPER,
            "" => {}
            _ => return fallback, // unknown modifier => don't guess
        }
    }

    let code = match key_tok.as_str() {
        "space" | "spc" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "enter" | "return" | "ret" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        // Function keys: f1..=f24
        s if s.starts_with('f') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit()) => {
            match s[1..].parse::<u8>() {
                Ok(n) if (1..=24).contains(&n) => KeyCode::F(n),
                _ => return fallback,
            }
        }
        // Single character (letter, digit, punctuation).
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        _ => return fallback,
    };
    (code, mods)
}

/// Whether a received key event matches a parsed binding. Compares the key code
/// and the CONTROL/ALT/SUPER modifiers; SHIFT is ignored unless the binding asked
/// for it, since terminals fold shift into the reported character.
pub fn key_matches(code: KeyCode, mods: KeyModifiers, binding: &(KeyCode, KeyModifiers)) -> bool {
    let (want_code, want_mods) = binding;
    if code != *want_code {
        return false;
    }
    const TRACKED: KeyModifiers = KeyModifiers::CONTROL
        .union(KeyModifiers::ALT)
        .union(KeyModifiers::SUPER);
    (mods & TRACKED) == (*want_mods & TRACKED)
}

/// Documented default written to disk on first run. Kept in sync with `config.example.toml`.
const DEFAULT_CONFIG_TOML: &str = r##"# Configuration for memo (the `memo` command).
# Edit the values below, then relaunch `memo`. Run `memo config --path` to locate this file.

# Which local backend to auto-start: "mlx" (Apple MLX) or "ollama".
# Both speak the same OpenAI-compatible API, so refinement works the same either
# way — this only selects the server `memo` launches (see auto_start_server).
provider = "mlx"

# OpenAI-compatible base URL of your local model server.
# MLX:    http://localhost:8080/v1   (mlx_lm.server --port 8080)
# Ollama: http://localhost:11434/v1  (ollama serve, default port 11434)
# Start an MLX server with, e.g.:
#   pip install mlx-lm
#   mlx_lm.server --model mlx-community/Llama-3.1-8B-Instruct-4bit --port 8080
base_url = "http://localhost:8080/v1"

# Model name the server should use.
# MLX:    an mlx-community repo, e.g. mlx-community/Llama-3.1-8B-Instruct-4bit
# Ollama: a pulled model tag, e.g. llama3.1 (run `ollama pull llama3.1` first)
model = "mlx-community/Llama-3.1-8B-Instruct-4bit"

# API key. Usually empty for a local MLX or Ollama server.
api_key = ""

# Where notes are stored. Leave empty to use the platform default
# (~/.local/share/memo/notes). Supports a leading ~ for your home directory.
data_dir = ""

# Sampling controls used when refining a note.
temperature = 0.3
max_tokens = 2048

# How long (in seconds) to wait for the model to respond.
request_timeout_secs = 120

# Stop sequences. Bigger instruct models sometimes keep generating past the end
# of their answer and leak chat-template markers like <|eot_id|> or
# <|start_header_id|>assistant<|end_header_id|>. These sequences stop generation
# and are also stripped from the output so they never reach your notes.
stop = ["<|eot_id|>", "<|end_of_text|>", "<|start_header_id|>", "<|im_end|>"]

# --- Local server management -----------------------------------------------
# When true, `memo` starts the server selected by `provider` for you on launch
# (using `model` and the port from `base_url`) and shuts it down when you quit.
# If a server is already listening on that port, `memo` leaves it alone — so for
# Ollama you can keep this on even when the Ollama app is already running.
#   provider = "mlx"    -> python -m mlx_lm.server --model <model> --port <port>
#   provider = "ollama" -> ollama serve  (bound to <port> via OLLAMA_HOST)
auto_start_server = true

# When false (default), the server starts at launch and stays resident until you
# quit, so the model is always ready. Set this to true to instead start it lazily
# on your first refine (Ctrl+R) and shut it back down once it has been idle (see
# server_idle_timeout_secs) — keeping the model out of memory until you actually
# use refinement. Only applies when auto_start_server = true.
server_on_demand = false

# On-demand mode only (server_on_demand = true): how long, in seconds, to keep the
# server running after your last refine before shutting it down. 0 stops it
# immediately after each refine; a larger value keeps the model warm so repeated
# refines don't pay the reload cost.
server_idle_timeout_secs = 120

# The settings below only apply when provider = "mlx".
# Where to launch the server from. Point this at your local mlx-lm checkout if
# you run from source; leave empty to launch from the current directory.
mlx_repo = ""

# Virtualenv to run the server with. Either an absolute path / "~/path", or a
# name relative to mlx_repo (e.g. ".venv"). Empty => use `python3` from PATH.
venv = ""

# Show the keyboard-shortcuts hint bar along the bottom of the screen.
show_shortcuts = true

# --- Speech-to-text dictation (local, on-device) ---------------------------
# In the editor, drive dictation with this key:
#   hold it          -> push-to-talk: records while held, inserts on release
#   double-press it  -> toggle continuous live listening (appends each phrase)
# A function key like "F5" / "f6", or a combo like "ctrl+k" / "ctrl+g" / "ctrl+space".
# IMPORTANT: this MUST stay a TOP-LEVEL key — keep it ABOVE the [theme] line, or
# TOML treats it as part of [theme] and silently ignores it (so edits do nothing).
# macOS note: F5 is the system Dictation key. If pressing it triggers macOS
# dictation instead of this app, turn that off (System Settings > Keyboard >
# Dictation), or enable "Use F1, F2, etc. keys as standard function keys", or pick
# a combo like "ctrl+k". Hold-to-talk is crispest in terminals with the Kitty
# keyboard protocol (Ghostty, kitty, WezTerm, iTerm2); elsewhere a key-repeat
# heuristic is used.
dictation_key = "F5"

# Which whisper.cpp model to use. The matching ggml-<model>.bin is downloaded on
# first use into ~/.local/share/memo/models/. Options include:
#   tiny.en | base.en | small.en | medium.en | large-v3   (drop ".en" for multilingual)
# Bigger = more accurate but slower and larger.
dictation_model = "base.en"

# Explicit path to a GGML model file. Leave empty to use the managed cache above.
# Supports a leading ~ for your home directory.
dictation_model_path = ""

# Spoken-language hint. Use a code like "en", or "auto" to let whisper detect it.
dictation_language = "en"

# Live listening: how much trailing silence (milliseconds) ends a phrase and
# triggers its transcription.
dictation_silence_ms = 700

# The single prompt used when you press Ctrl+R to refine a note.
refine_prompt = """
You are a careful note editor. Improve the user's raw note: fix grammar and
spelling, improve clarity and flow, and organise the content using Markdown
headings (#, ##) and bullet points (-) where helpful. Preserve all facts and
meaning, and do not invent information. Return ONLY the improved Markdown note,
with no preamble or commentary.
"""

# UI colors and spacing. Colors accept names (e.g. "yellow", "darkgray",
# "lightblue"), "#rrggbb" hex (e.g. "#ff8800"), a 0-255 palette index, or
# "none"/"transparent" to use the terminal's default color.
[theme]
accent = "yellow"      # selection highlight, search focus, accents
border = "#313244"     # default (unselected) borders
refined = "#313244"    # refined-view accents and the ✦ marker
star = "#313244"       # color of the ✦ marker on refined notes
title_fg = "yellow"    # editor title-bar text
title_bg = "none"      # editor title-bar background
footer_fg = "#313244"  # footer hint-bar text
footer_bg = "none"     # footer hint-bar background
status = "green"       # transient status messages
padding = 2            # inner padding (cells) for the editor body and tiles
divider = "#313244"    # horizontal line separating the note header from content
rounded_tiles = true   # rounded corners on note tiles in the list view
meta = "#585b70"       # created/modified timestamps on tiles and in the editor
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_in_for_empty_file() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.temperature, 0.3);
        assert_eq!(cfg.max_tokens, 2048);
        assert!(!cfg.base_url.is_empty());
        assert!(!cfg.refine_prompt.is_empty());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let cfg: Config = toml::from_str(r#"model = "custom-model""#).unwrap();
        assert_eq!(cfg.model, "custom-model");
        assert_eq!(cfg.request_timeout_secs, 120);
    }

    #[test]
    fn provider_defaults_to_mlx_and_parses_ollama() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.provider, Provider::Mlx);
        let cfg: Config = toml::from_str(r#"provider = "ollama""#).unwrap();
        assert_eq!(cfg.provider, Provider::Ollama);
    }

    #[test]
    fn server_lifecycle_defaults_preserve_eager_startup() {
        // Default behavior is unchanged: the server is managed and resident.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.auto_start_server);
        assert!(!cfg.server_on_demand);
        assert_eq!(cfg.server_idle_timeout_secs, 120);
        // On-demand can be opted into independently of the idle window.
        let cfg: Config =
            toml::from_str("server_on_demand = true\nserver_idle_timeout_secs = 0").unwrap();
        assert!(cfg.server_on_demand);
        assert_eq!(cfg.server_idle_timeout_secs, 0);
    }

    #[test]
    fn embedded_default_config_is_valid_toml() {
        let cfg: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(cfg.base_url, "http://localhost:8080/v1");
        // Server-lifecycle defaults round-trip through the embedded TOML.
        assert!(cfg.auto_start_server);
        assert!(!cfg.server_on_demand);
        assert_eq!(cfg.server_idle_timeout_secs, 120);
        // Dictation defaults round-trip through the embedded TOML.
        assert_eq!(cfg.dictation_key, "F5");
        assert_eq!(cfg.dictation_model, "base.en");
        assert_eq!(cfg.dictation_language, "en");
        assert_eq!(cfg.dictation_silence_ms, 700);
    }

    #[test]
    fn dictation_defaults_fill_in() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.dictation_key, "F5");
        assert_eq!(cfg.dictation_model, "base.en");
        assert!(cfg.dictation_model_path.is_empty());
    }

    #[test]
    fn dictation_key_is_read_from_config_when_top_level() {
        // A top-level dictation_key overrides the default and parses to its binding.
        let toml = "dictation_key = \"ctrl+g\"\n[theme]\naccent = \"red\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.dictation_key, "ctrl+g");
        assert_eq!(
            parse_key_binding(&cfg.dictation_key),
            (KeyCode::Char('g'), KeyModifiers::CONTROL)
        );

        // The embedded default keeps the dictation_key assignment above the
        // [theme] table header so it stays top-level (placing it under [theme]
        // would make TOML ignore it). Anchor on line starts to avoid matching the
        // word "[theme]" inside a comment.
        let key_pos = DEFAULT_CONFIG_TOML.find("\ndictation_key =").unwrap();
        let theme_pos = DEFAULT_CONFIG_TOML.find("\n[theme]").unwrap();
        assert!(
            key_pos < theme_pos,
            "dictation_key assignment must precede the [theme] table"
        );
    }

    #[test]
    fn parses_function_keys() {
        assert_eq!(parse_key_binding("F5"), (KeyCode::F(5), KeyModifiers::NONE));
        assert_eq!(
            parse_key_binding("f12"),
            (KeyCode::F(12), KeyModifiers::NONE)
        );
    }

    #[test]
    fn parses_modifier_combos() {
        assert_eq!(
            parse_key_binding("ctrl+g"),
            (KeyCode::Char('g'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            parse_key_binding("Ctrl+Space"),
            (KeyCode::Char(' '), KeyModifiers::CONTROL)
        );
        assert_eq!(
            parse_key_binding("alt+d"),
            (KeyCode::Char('d'), KeyModifiers::ALT)
        );
    }

    #[test]
    fn invalid_binding_falls_back_to_f5() {
        assert_eq!(parse_key_binding(""), (KeyCode::F(5), KeyModifiers::NONE));
        assert_eq!(
            parse_key_binding("f99"),
            (KeyCode::F(5), KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_binding("hyper+x"),
            (KeyCode::F(5), KeyModifiers::NONE)
        );
    }

    #[test]
    fn key_matches_ignores_shift_unless_requested() {
        let binding = parse_key_binding("ctrl+g");
        assert!(key_matches(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            &binding
        ));
        assert!(!key_matches(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
            &binding
        ));
        let f5 = parse_key_binding("F5");
        assert!(key_matches(KeyCode::F(5), KeyModifiers::NONE, &f5));
        assert!(!key_matches(KeyCode::F(6), KeyModifiers::NONE, &f5));
    }

    #[test]
    fn resolved_model_path_uses_cache_or_override() {
        let mut cfg = Config::default();
        assert!(cfg
            .resolved_model_path()
            .to_string_lossy()
            .ends_with("ggml-base.en.bin"));
        cfg.dictation_model_path = "/tmp/my-model.bin".to_string();
        assert_eq!(
            cfg.resolved_model_path(),
            PathBuf::from("/tmp/my-model.bin")
        );
    }
}
