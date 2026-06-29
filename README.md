# memo

A fast, keyboard-driven note-taking app for your terminal that uses a local AI model to clean up and refine your notes — all without sending your data anywhere.

> **Platform:** macOS with Apple Silicon (M1, M2, M3, M4) for the default MLX backend. Prefer something else, or not on Apple Silicon? `memo` also works with [Ollama](https://ollama.com) — which runs on Intel Macs, Linux, and Windows — as a drop-in local backend. See [Using Ollama instead of MLX](#using-ollama-instead-of-mlx).

![note list view](screenshots/list-view.png)

---

## What does it do?

`memo` is a terminal app for capturing and refining notes. You open it, press `n` to create a new note, and start typing. That's it.

The AI part is optional but useful: press `Ctrl+R` in any note and the app sends it to a language model running **on your own machine**. The model cleans up the grammar, improves the structure, and returns a polished version. The original is always kept untouched — you can switch between the two with `Tab`. Press `Ctrl+P` to refine with a one-off custom prompt instead of the default one; the custom prompt is used for that single call only and is not saved anywhere.

Everything stays local. No account required. No internet connection needed after setup.

---

## Why local AI?

Most AI tools send your text to a remote server to process it. With `memo`, the language model runs entirely on your Mac. This means:

- Your notes never leave your machine.
- It works without an internet connection.
- There are no usage fees or subscription costs.
- You are not subject to any third-party data retention policy.

---

## What you get

- Create and browse notes from the terminal.
- Organize notes into folders, each listing the titles of the notes it contains.
- Live fuzzy search across all your notes.
- One-key AI refinement that fixes grammar, improves clarity, and adds Markdown structure — or use a custom one-off prompt with `Ctrl+P`.
- The original and refined versions of every note are both kept and editable.
- Notes are plain Markdown files on disk — readable and portable without the app.
- Editable note titles with automatic timestamps.
- Export any note to a `.md` file.
- A fully configurable color theme.

![note editor view](screenshots/editor-view.png)

---

## Requirements

**macOS (Apple Silicon — M1, M2, M3, M4)**

- macOS 13 (Ventura) or later is recommended.
- An internet connection for the initial setup (to download the app and the AI model).
- No other tools required — the one-line installer downloads a prebuilt binary.

**macOS (Intel)**

- Same as above. AI refinement requires [Ollama](https://ollama.com) on Intel Macs (MLX is Apple Silicon only). See [Using Ollama instead of MLX](#using-ollama-instead-of-mlx).

**Linux (x86_64 and arm64)**

- Install the ALSA audio library for microphone access (dictation):
  ```sh
  sudo apt-get install libasound2t64  # Debian / Ubuntu 24.04+
  sudo apt-get install libasound2     # Debian / Ubuntu (older)
  sudo dnf install alsa-lib           # Fedora / RHEL
  sudo pacman -S alsa-lib             # Arch
  ```
- Everything else (whisper.cpp, etc.) is compiled into the binary.
- AI refinement requires [Ollama](https://ollama.com). See [Using Ollama instead of MLX](#using-ollama-instead-of-mlx).

**Windows (x86_64)**

- Windows 10 or later, in Windows Terminal or PowerShell.
- AI refinement requires [Ollama](https://ollama.com) (mlx-lm is Apple Silicon only); the installer tells you if it's missing.

> Building from source (e.g. on an unsupported platform) additionally needs a C/C++ compiler and CMake to compile the speech engine. The `install.sh` script checks for these and prints instructions if either is missing.

---

## Installation

### macOS and Linux

Open a terminal and run:

```sh
curl -fsSL https://raw.githubusercontent.com/blackstardesigns/memo/main/install.sh | sh
```

> **How to open Terminal on macOS:** Press `Command + Space`, type `Terminal`, and press `Enter`.

This downloads a prebuilt binary — no Rust, CMake, or compiler needed. The installer:

1. Downloads the latest `memo` binary and installs it to `/usr/local/bin` (or `~/.local/bin` if sudo isn't available).
2. On macOS, offers to install `mlx-lm`, the local AI server for refinement (Apple Silicon only).

When asked `Install mlx-lm now? [y/N]`, type `y` and press `Enter` (macOS Apple Silicon).

> On a clone you can also run `./install.sh` directly; if no prebuilt binary matches your platform it builds from source (needs Rust, CMake, and a C/C++ compiler).

### Windows

In PowerShell, run:

```powershell
irm https://raw.githubusercontent.com/blackstardesigns/memo/main/install.ps1 | iex
```

This downloads `memo.exe`, installs it to `%LOCALAPPDATA%\Programs\memo`, adds it to your PATH, and writes a default config that uses [Ollama](https://ollama.com) for AI refinement (install Ollama separately, then run `ollama pull llama3.1`).

### Launch

Open a **new** terminal window (so the updated PATH takes effect) and run:

```sh
memo
```

You should see the note list view. The app is installed.

### Updating

Update to the latest release at any time with:

```sh
memo update
```

This downloads the newest prebuilt binary for your platform, verifies its checksum, and replaces the installed `memo` in place — no need to re-run the installer. Useful flags:

- `memo update --check` — see whether a newer version is available without installing it.
- `memo update --pre` — include pre-releases (`rc` / `alpha` / `beta`), not just stable versions.

Restart memo afterwards to run the new version.

---

## Setting up the AI model

The AI refinement feature requires a language model to be running on your Mac. This section walks through getting one set up.

### How it works

`memo` talks to a local server that loads and runs an AI model. By default, `memo` starts this server automatically when you launch the app and shuts it down when you quit. The first time you use refinement, the model is downloaded, and after that it runs from your local disk. The app reaches the server over the standard OpenAI-compatible API, so any server that speaks it works.

### Choosing a backend: MLX or Ollama

`memo` supports two local backends. Both are reached over the same API, so refinement behaves identically — pick whichever fits your setup. The backend is selected with the `provider` setting in your config.

| | **MLX** (default) | **Ollama** |
|---|---|---|
| `provider` | `"mlx"` | `"ollama"` |
| Hardware | Apple Silicon only | Apple Silicon, Intel Mac, Linux, Windows |
| Models | `mlx-community/*` from Hugging Face | `ollama pull` tags (e.g. `llama3.1`) |
| Server | `mlx_lm.server` (installed via the installer) | `ollama serve` (install from [ollama.com](https://ollama.com)) |
| Default port | `8080` | `11434` |

If you're on Apple Silicon and ran the installer, MLX works out of the box — continue below. To use Ollama instead, skip to [Using Ollama instead of MLX](#using-ollama-instead-of-mlx).

### Choosing a model

Models are downloaded from [Hugging Face](https://huggingface.co), a public platform for sharing AI models. For `memo`, you want a model in the **MLX format**, which is optimized for Apple Silicon.

The default model configured in `memo` is:

```
mlx-community/Llama-3.1-8B-Instruct-4bit
```

This is a good starting point. It is an 8-billion-parameter model compressed to about 4 GB. It runs well on Macs with 16 GB or more of unified memory.

Other options depending on your hardware:

| Model | Size on disk | Memory needed | Speed |
|-------|-------------|---------------|-------|
| `mlx-community/Llama-3.2-3B-Instruct-4bit` | ~2 GB | 8 GB | Fast |
| `mlx-community/Llama-3.1-8B-Instruct-4bit` | ~4 GB | 16 GB | Good balance |
| `mlx-community/Llama-3.1-70B-Instruct-4bit` | ~35 GB | 64 GB+ | Slow, very capable |

To see what memory your Mac has: **Apple menu > About This Mac > Memory**.

You can browse more MLX-compatible models at [huggingface.co/mlx-community](https://huggingface.co/mlx-community).

### Creating a Hugging Face account (optional)

Most models used by `memo` are publicly available and do not require an account. You only need a Hugging Face account if you want to use a model that requires agreeing to its terms of use (such as Meta's Llama models on gated repositories).

If you need an account:

1. Go to [huggingface.co](https://huggingface.co) and click **Sign Up**.
2. Create a free account.
3. Go to your [access tokens page](https://huggingface.co/settings/tokens) and create a new token with **Read** access.
4. In your terminal, run:

```sh
pip install huggingface_hub
huggingface-cli login
```

Paste your token when prompted. This saves the token to your Mac so that `mlx_lm.server` can access gated models.

### Automatic server management (default)

By default, `memo` manages the model server for you. When you launch `memo`, it:

1. Starts the `mlx_lm.server` process using the model and port from your config.
2. On first launch, downloads the model from Hugging Face (this can take several minutes depending on your internet speed and the model size).
3. Shuts the server down cleanly when you quit with `q` or `Esc`.

The first launch after setup will pause briefly at startup while the model loads. Subsequent launches are faster because the model is cached on disk.

Server logs are written to `~/.config/memo/mlx-server.log` if you need to troubleshoot.

### Running the server manually (optional)

If you prefer to manage the server yourself, set `auto_start_server = false` in the config file (see the Configuration section) and run the server in a separate terminal window:

```sh
mlx_lm.server --model mlx-community/Llama-3.1-8B-Instruct-4bit --port 8080
```

The first time you run this, it downloads the model. Subsequent runs load it from disk and are faster.

### Using Ollama instead of MLX

`memo` can use [Ollama](https://ollama.com) as the local model server. Ollama exposes the same OpenAI-compatible API that `memo` already speaks, so refinement works exactly the same — only the server and model names change.

1. Install Ollama (from [ollama.com](https://ollama.com)) and pull a model:

   ```sh
   ollama pull llama3.1
   ```

2. Point `memo` at Ollama in `~/.config/memo/config.toml`:

   ```toml
   provider = "ollama"
   base_url = "http://localhost:11434/v1"   # Ollama's default port
   model    = "llama3.1"                     # the tag you pulled
   ```

With `auto_start_server = true` (the default), `memo` runs `ollama serve` for you on launch — and if Ollama is already running on that port (e.g. the Ollama desktop app), `memo` leaves it alone. Set `auto_start_server = false` to manage it yourself with `ollama serve`.

The `mlx_repo` and `venv` settings are ignored when `provider = "ollama"`. Server output is written to the same log file used for MLX (`~/.config/memo/mlx-server.log`) if you need to troubleshoot.

---

## Using note

Run `memo` to open the app.

![note in use](screenshots/refine-demo.png)

### List view

This is the main screen showing all your notes as tiles.

| Key | Action |
|-----|--------|
| `n` | Create a new note (inside an open folder, it's filed there) |
| `Ctrl+F` | Create a new folder (prompts for a title) |
| `m` | Move the selected note into a folder (or back to the top level) |
| `Enter` or `o` | Open the selected note, or enter the selected folder |
| `/` then type | Search your notes (fuzzy matching) |
| Arrow keys (or `j k l`) | Move the selection |
| `d` | Toggle the notes/folders drawer (a tree on the left) |
| `x` | Delete the selected note or folder (asks for confirmation) |
| `Ctrl+E` | Export the selected note as a `.md` file |
| `Ctrl+H` | Show help |
| `Esc` | Leave the current folder, or quit at the top level |
| `q` | Quit |

### Folders

Press `Ctrl+F` to create a folder and give it a title. Folders appear as tiles among your notes, each listing the titles of the notes it holds. Press `Enter` on a folder to open it (and `Esc` to come back out); inside, `n` creates a note that belongs to that folder. To file an existing note, select it and press `m`, then choose a destination folder (or "None" to move it back to the top level). Deleting a folder keeps its notes — they move back to the top level. Search from the top level looks across every note, including those inside folders.

### The drawer (notes tree)

Press `d` on the home screen to toggle a drawer on the left that shows your whole notes-and-folders hierarchy as a tree. It scrolls independently of the main view. With the drawer focused, navigate with the arrow keys:

- `↑ ↓` move through the tree.
- `→ ←` expand and collapse the selected folder (on a nested note, `←` jumps up to its folder).
- `Enter` opens a note, or expands/collapses a folder.
- `Tab` jumps back to the tiles while leaving the drawer open.
- `d` (or `Esc`) closes the drawer.

If you open a note while the drawer is showing, it stays visible beside the editor (with the open note highlighted) so you can keep your place — though it's toggled and navigated from the home screen, since in the editor every key is part of your note.

### Editor

Press `n` or open an existing note to enter the editor.

| Key | Action |
|-----|--------|
| `Ctrl+R` | Refine the current note with the AI |
| `Ctrl+P` | Refine with a one-off custom system prompt (not saved) |
| `Tab` | Switch between the original and refined version |
| `Ctrl+T` | Edit the note's title |
| `Ctrl+S` | Force save |
| `Ctrl+E` | Export to a `.md` file |
| `Ctrl+M` | Insert a math / equation symbol (∑ picker) |
| `Ctrl+H` | Show help |
| `F5` (hold / double-press) | Dictate: **hold** to push-to-talk, **double-press** to toggle continuous live listening (configurable key) |
| `Esc` | Return to the list view |

**Autosave** — your edits are saved automatically a moment after you stop typing, and also whenever you switch views, trigger a refinement, or leave the editor. You do not need to press save.

**Both views are editable** — after refining, you can continue editing the refined version. `Ctrl+R` will refine whatever is currently in the editor, so you can iterate.

**Markdown formatting** — notes support Markdown. Start a line with `#` for a heading, `##` for a subheading, and `-` for a bullet point.

Notes with a refined version show a small marker in the list view. Opening such a note takes you straight to the refined version; press `Tab` to see the original.

### Dictation (speech to text)

You can talk instead of type. In the editor:

- **Hold `F5`** to push-to-talk: it records while you hold the key and inserts the transcribed text at the cursor when you let go.
- **Double-press `F5`** to toggle continuous live listening: the app keeps listening and appends each phrase as you pause between sentences. Double-press again (or leave the editor) to stop.

Transcription runs **entirely on your machine** with a local [whisper.cpp](https://github.com/ggerganov/whisper.cpp) model — nothing is sent anywhere, in keeping with the rest of the app. The first time you dictate, the speech model (`ggml-base.en.bin` by default, ~150 MB) is downloaded to `~/.local/share/memo/models/`. After that it loads from disk. Because dictation comes out raw, `Ctrl+R` is handy afterwards to clean up grammar and punctuation.

The dictation key and model are configurable — see `dictation_key`, `dictation_model`, and the other `dictation_*` settings in the [Configuration](#configuration) section. Two gotchas worth knowing: the `dictation_key` line must stay **above** the `[theme]` section in the config (otherwise TOML treats it as a theme setting and ignores it); and on macOS **`F5` is the system Dictation key**, so if pressing it triggers macOS's own dictation, disable that shortcut (System Settings → Keyboard → Dictation), enable "Use F1, F2, etc. keys as standard function keys", or pick a combo like `ctrl+k`. Hold-to-talk is most precise in terminals that support the Kitty keyboard protocol (Ghostty, kitty, WezTerm, recent iTerm2); in other terminals it falls back to a key-repeat heuristic.

---

## Configuration

The config file is created automatically on first launch at:

```
~/.config/memo/config.toml
```

To open it in your default editor, run:

```sh
note config --edit
```

To print the file path:

```sh
note config --path
```

### Key settings

```toml
# Which local backend note auto-starts: "mlx" (default) or "ollama".
provider = "mlx"

# The URL of the local model server.
base_url = "http://localhost:8080/v1"

# The model to use. Must match what the server is running.
model = "mlx-community/Llama-3.1-8B-Instruct-4bit"

# Where your notes are stored.
data_dir = ""   # leave empty to use the default (~/.local/share/memo/notes)

# Whether note starts and stops the model server for you.
auto_start_server = true

# The prompt sent to the AI when you press Ctrl+R.
refine_prompt = """
You are a careful note editor. Improve the user's raw note: fix grammar and
spelling, improve clarity and flow, and organise the content using Markdown
headings (#, ##) and bullet points (-) where helpful. Preserve all facts and
meaning, and do not invent information. Return ONLY the improved Markdown note,
with no preamble or commentary.
"""
```

See [`config.example.toml`](config.example.toml) for the full list of options with comments.

### Theme

The `[theme]` section in the config lets you change the colors and spacing of the UI. Color values accept names (`yellow`, `darkgray`, `lightblue`), hex codes (`#ff8800`), a 0–255 terminal palette index, or `none` to use your terminal's default.

```toml
[theme]
accent    = "yellow"    # selection highlight and search focus
border    = "darkgray"  # borders on unselected tiles
refined   = "darkgray"  # accents in the refined view
star      = "magenta"   # marker on refined notes
title_fg  = "yellow"    # editor title bar text
title_bg  = "none"      # editor title bar background
footer_fg = "darkgray"  # keyboard hint bar text
footer_bg = "none"      # keyboard hint bar background
status    = "green"     # status messages (e.g. "Refining…", "Done")
padding       = 2           # inner padding in cells
divider       = "darkgray"  # line between the note header and body
rounded_tiles = true        # rounded corners on note tiles in the list view
meta          = "darkgray"  # created/modified timestamps on tiles and in the editor
```

Set `show_shortcuts = false` at the top level to hide the keyboard hint bar at the bottom of the screen.

---

## Where notes are stored

Each note is a plain Markdown file stored in:

```
~/.local/share/memo/notes/
```

Files have a small YAML header followed by the note content:

```markdown
---
id: 3f2a1b4c-...
title: My Note Title
created: 2026-06-23T11:53:00Z
modified: 2026-06-23T12:00:00Z
---
# Heading

- A bullet point
```

When you refine a note, the refined version is saved as a second file alongside the original (`<id>.refined.md`). The original file is never modified.

Folders are tracked in a small `folders.json` index in the same directory; each note records the folder it belongs to in its own frontmatter (a `folder:` id), so the Markdown files remain self-contained.

You can open, search, copy, or back up these files with any standard file tool. They are not locked to this app.

---

## Troubleshooting

**`note: command not found` after install**

Open a new terminal window. If the problem persists, run:

```sh
source ~/.cargo/env
```

Then try `memo` again.

**The AI refinement hangs or times out**

The model may still be loading, especially on first use. Wait 30–60 seconds and try again. Check the server log at `~/.config/memo/mlx-server.log` for details.

If using a larger model, it may simply need more time. You can increase the timeout in the config:

```toml
request_timeout_secs = 300
```

**The refined output contains strange tokens like `<|eot_id|>`**

Add these to the `stop` list in your config:

```toml
stop = ["<|eot_id|>", "<|end_of_text|>", "<|start_header_id|>", "<|im_end|>"]
```

Any sequences that slip past the stop list are automatically stripped from the output.

**The server was not shut down cleanly (e.g. the terminal was force-closed)**

`memo` tracks the server PID in `~/.config/memo/mlx-server.pid`. On the next launch, it reads this file and stops any leftover server process before starting a new one.

---

## Contributing

Contributions are welcome. Please open an issue to discuss significant changes before submitting a pull request.

To build from source during development:

```sh
cargo run
```

---

## License

MIT
