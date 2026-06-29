
<div align="center">

![banner](banner.png)

**Fast, keyboard-first notes for your terminal — refined by local AI.**

No cloud. No account. No subscription. Your notes stay on your machine.

![GitHub release](https://img.shields.io/github/v/release/blackstardesigns/memo?include_prereleases)
![License](https://img.shields.io/badge/license-MIT-blue)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)
![Local AI](https://img.shields.io/badge/AI-local%20only-green)

</div>


## What is memo?

`memo` is a terminal note-taking app for quickly capturing rough thoughts, meeting notes, ideas, and drafts.

Press `n`, start typing, and save. When you want cleanup, press `Ctrl+R` and `memo` refines your note using a local AI model running on your machine.

The original note is preserved. The refined version is saved separately. Both are editable Markdown files.

---

## Highlights

- Fast terminal UI for creating and browsing notes
- Local AI refinement with **MLX** or **Ollama**
- No data sent to external servers
- Notes saved as plain Markdown files
- Folders, fuzzy search, editable titles, and export
- Original and refined versions kept side by side
- Optional custom one-off refinement prompt with `Ctrl+P`
- Local dictation with `whisper.cpp`
- Configurable colors, spacing, model, backend, and shortcuts

---

## Install

### macOS / Linux

```sh
curl -fsSL https://raw.githubusercontent.com/blackstardesigns/memo/main/install.sh | sh
```

To install the latest pre-release instead:

```sh
curl -fsSL https://raw.githubusercontent.com/blackstardesigns/memo/main/install.sh | sh -s -- --pre
```

Then open a new terminal and run:

```sh
memo
```

### Windows

Run in PowerShell:

```powershell
irm https://raw.githubusercontent.com/blackstardesigns/memo/main/install.ps1 | iex
```

Then open a new terminal and run:

```powershell
memo
```

---

## Requirements

| Platform | AI backend |
|---|---|
| macOS Apple Silicon | MLX default, Ollama optional |
| macOS Intel | Ollama |
| Linux x86_64 / arm64 | Ollama |
| Windows x86_64 | Ollama |

For Linux dictation support, install ALSA:

```sh
sudo apt-get install libasound2t64  # Ubuntu 24.04+
sudo apt-get install libasound2     # older Debian / Ubuntu
sudo dnf install alsa-lib           # Fedora / RHEL
sudo pacman -S alsa-lib             # Arch
```

---

## Local AI setup

`memo` talks to a local OpenAI-compatible server.

| Backend | Best for | Default port |
|---|---|---|
| MLX | Apple Silicon Macs | `8080` |
| Ollama | macOS Intel, Linux, Windows, or Apple Silicon | `11434` |

### MLX on Apple Silicon

The installer can install `mlx-lm` for you.

Default model:

```txt
mlx-community/Llama-3.1-8B-Instruct-4bit
```

Run manually if needed:

```sh
mlx_lm.server --model mlx-community/Llama-3.1-8B-Instruct-4bit --port 8080
```

### Ollama

Install Ollama, then pull a model:

```sh
ollama pull llama3.1
```

Set this in `~/.config/memo/config.toml`:

```toml
provider = "ollama"
base_url = "http://localhost:11434/v1"
model = "llama3.1"
```

With `auto_start_server = true`, `memo` starts `ollama serve` when needed. If Ollama is already running, `memo` leaves it alone.

---

## Using memo

![memo editor view](screenshots/editor-view.png)

### List view

| Key | Action |
|---|---|
| `n` | New note |
| `Ctrl+F` | New folder |
| `m` | Move note to folder |
| `Enter` / `o` | Open note or folder |
| `/` | Search |
| `h` | Toggle drawer |
| `d` | Delete selected item |
| `x` | Export note |
| `?` | Help |
| `q` | Quit |

### Editor

| Key | Action |
|---|---|
| `Ctrl+R` | Refine note with AI |
| `Ctrl+P` | Refine with a custom one-off prompt |
| `Tab` | Switch original/refined view |
| `Ctrl+T` | Rename note |
| `Ctrl+S` | Save |
| `Ctrl+X` | Export |
| `F5` | Dictation |
| `Esc` | Back to list |

Notes autosave after you stop typing, when you switch views, and when you leave the editor.

---

## Dictation

Press `F5` in the editor:

- Hold `F5` for push-to-talk
- Double-press `F5` for continuous listening
- Double-press again, or leave the editor, to stop

Dictation runs locally using `whisper.cpp`. The first use downloads the speech model to:

```txt
~/.local/share/memo/models/
```

On macOS, `F5` may be reserved for system dictation. Disable the macOS shortcut, enable standard function keys, or change `dictation_key` in the config.

---

## Configuration

Config file:

```txt
~/.config/memo/config.toml
```

Open it:

```sh
memo config --edit
```

Print its path:

```sh
memo config --path
```

See `config.example.toml` for all options.

---s

## Build from source

```sh
cargo run
```

Unsupported platforms may also need:

- Rust
- CMake
- C/C++ compiler

---

## Contributing

Issues and pull requests are welcome.

For larger changes, please open an issue first so the approach can be discussed.

---

## License

MIT
