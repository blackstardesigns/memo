# ai-note-taker — CLAUDE.md

## What this app is

`note` is a keyboard-driven terminal TUI (text-user interface) for capturing and refining notes locally. Key traits:

- **No cloud, no account.** Everything stays on the user's machine.
- **AI refinement** via a local LLM server (MLX on Apple Silicon, or Ollama). Press `Ctrl+R` in the editor and the local model cleans up grammar, adds Markdown structure, and returns a polished version — the original is kept untouched.
- **Local speech-to-text dictation** via whisper.cpp (compiled in-process with `whisper-rs`). Hold the dictation key for push-to-talk; double-press to toggle continuous live listening.
- Notes are plain Markdown files with YAML frontmatter — portable without the app.

The binary is named `note` (`[[bin]] name = "note"`).

---

## How it is built

| Layer | Technology |
|---|---|
| Language | Rust (edition 2021) |
| TUI framework | [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) |
| Text editor widget | [tui-textarea](https://github.com/rhysd/tui-textarea) |
| HTTP (LLM calls) | [ureq](https://github.com/algesten/ureq) (blocking, synchronous — runs on a background thread) |
| Serialization | serde + serde_json + serde_yaml (YAML frontmatter) + toml (config) |
| Audio capture | [cpal](https://github.com/RustAudio/cpal) |
| Speech-to-text | [whisper-rs](https://github.com/tazz4843/whisper-rs) (wraps whisper.cpp; Metal GPU backend on macOS via `features = ["metal"]`) |
| CLI parsing | clap (derive API) |
| IDs | uuid v4 |
| Dates | chrono |

**Build note:** `whisper-rs` pulls in `whisper.cpp` and requires **cmake** and a C/C++ compiler. On this machine cmake lives at `~/Library/Python/3.11/bin/cmake` (not on PATH by default — add it before `cargo build`).

```sh
export PATH="$HOME/Library/Python/3.11/bin:$PATH"
cargo build
```

---

## Codebase structure

```
src/
  main.rs        — entry point: CLI parsing, terminal setup, event loop, panic hook
  app.rs         — App struct (all runtime state), key dispatch, gesture recognizer
  ui/
    mod.rs       — top-level draw() dispatcher; shared helpers (bottom bar, truncate)
    list.rs      — list-screen renderer (tile grid, search bar)
    editor.rs    — editor-screen renderer (title bar, textarea, status)
    drawer.rs    — left drawer (notes/folders tree)
    modal.rs     — all modal overlays (help, confirm, title edit, export, symbol picker…)
  note.rs        — Note / Meta / Folder types; YAML frontmatter parse/serialize
  storage.rs     — Store: read/write/delete notes and folders.json from disk
  llm.rs         — LLM refinement: spawns a background thread, sends chat-completions request
  audio.rs       — microphone capture via cpal; streams PCM to the dictation thread
  dictation.rs   — dictation worker: loads whisper model, runs transcription, sends results back
  server.rs      — ManagedServer: auto-start/stop the mlx_lm or ollama process
  config.rs      — Config struct (TOML), config file path helpers, key-binding parser
  search.rs      — fuzzy search over note titles and content
  theme.rs       — ResolvedTheme: maps config color strings to ratatui Color values
  testutil.rs    — test helpers (HTTP drain, etc.) — compiled only under #[cfg(test)]
```

### Key data flows

1. **Event loop** (`main.rs::run_loop`) polls crossterm at 100 ms, routes `KeyEvent` to `App::on_key_event`, then calls `App::on_tick` for timer-driven work (autosave, dictation drain, spinner, status clear).

2. **LLM refinement** (`llm.rs`) runs on a `std::thread` and communicates back via `mpsc`. `App::poll_refine` drains the channel each tick.

3. **Dictation** (`dictation.rs` + `audio.rs`) runs on its own thread. The gesture recognizer inside `App` (see `GestureState` in `app.rs`) classifies key events into `StartPushToTalk / StopPushToTalk / ToggleLive` and sends commands to the dictation thread via `DictationCmd`. The thread sends `DictationMsg::Text` back when transcription is done.

4. **Storage** (`storage.rs`) is synchronous and simple: `<id>.md` for originals, `<id>.refined.md` for refined versions (sidecar files). Folders are in a single `folders.json` index.

5. **Rendering** (`ui/`) is pure ratatui — stateless per frame, driven entirely from `App`.

---

## Runtime file locations

| Path | Purpose |
|---|---|
| `~/.config/ai-note-taker/config.toml` | User config (auto-created on first launch) |
| `~/.config/ai-note-taker/mlx-server.log` | MLX / Ollama server stdout/stderr |
| `~/.config/ai-note-taker/mlx-server.pid` | PID of the managed server process |
| `~/.config/ai-note-taker/keys-debug.log` | Key event log (only written when `NOTE_DEBUG_KEYS=1`) |
| `~/.local/share/ai-note-taker/notes/` | Note files (`<id>.md`, `<id>.refined.md`) and `folders.json` |
| `~/.local/share/ai-note-taker/models/` | whisper.cpp GGML model files (e.g. `ggml-base.en.bin`) |

---

## Development

```sh
cargo run          # run from source
cargo test         # unit + headless integration tests (uses TestBackend, no terminal needed)
```

Tests live inline in each file (`#[cfg(test)]` modules). The smoke tests in `main.rs` drive the full `App` with synthetic key events against `ratatui::backend::TestBackend` — no terminal or LLM server required.

Set `NOTE_DEBUG_KEYS=1` to log every key event to `~/.config/ai-note-taker/keys-debug.log` (useful for diagnosing dictation issues in different terminals).
