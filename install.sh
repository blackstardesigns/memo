#!/usr/bin/env sh
#
# Installer for memo.
#
#   One-liner (downloads a prebuilt binary — no Rust or compiler needed):
#     curl -fsSL https://raw.githubusercontent.com/blackstardesigns/memo/main/install.sh | sh
#
#   To install the latest pre-release (rc / alpha / beta):
#     curl -fsSL https://raw.githubusercontent.com/blackstardesigns/memo/main/install.sh | sh -s -- --pre
#
#   From a local clone (builds from source if no prebuilt is available):
#     ./install.sh
#
# What it does:
#   1. Download a prebuilt binary from GitHub Releases (fast path).
#   2. Fall back to building from source if no prebuilt matches the platform.
#   3. Install to a user-writable dir (no sudo / password) and put it on PATH.
#   4. Let you pick a local LLM backend (mlx-lm or Ollama) and set it up.
#
set -eu

REPO="blackstardesigns/memo"
GITHUB_BASE="https://github.com/${REPO}"
GITHUB_API="https://api.github.com/repos/${REPO}"
# Set GITHUB_TOKEN to install from a private repo (e.g. while testing):
#   GITHUB_TOKEN=ghp_... sh install.sh
GITHUB_TOKEN="${GITHUB_TOKEN:-}"

# ── argument parsing ──────────────────────────────────────────────────────────
INSTALL_PRERELEASE=0
for _arg in "$@"; do
  case "$_arg" in
    --pre) INSTALL_PRERELEASE=1 ;;
    *) ;;
  esac
done

# ── styling (auto-disabled when not a terminal, or when NO_COLOR is set) ───────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
  ESC="$(printf '\033')"
  RESET="${ESC}[0m"; BOLD="${ESC}[1m"; DIM="${ESC}[2m"
  RED="${ESC}[31m"; GREEN="${ESC}[32m"; YELLOW="${ESC}[33m"
  BLUE="${ESC}[34m"; MAGENTA="${ESC}[35m"; CYAN="${ESC}[36m"
  VIOLET="${ESC}[38;5;93m"
  # Yellow → orange gradient for the banner (256-color).
  G1="${ESC}[38;5;226m"; G2="${ESC}[38;5;220m"; G3="${ESC}[38;5;214m"
  G4="${ESC}[38;5;208m"; G5="${ESC}[38;5;202m"; G6="${ESC}[38;5;166m"
else
  RESET=""; BOLD=""; DIM=""
  RED=""; GREEN=""; YELLOW=""; BLUE=""; MAGENTA=""; CYAN=""; VIOLET=""
  G1=""; G2=""; G3=""; G4=""; G5=""; G6=""
fi

# Horizontal rule for the summary box (built to avoid hand-counting glyphs).
HBAR=""; _i=0
while [ "$_i" -lt 48 ]; do HBAR="${HBAR}─"; _i=$((_i + 1)); done

# ── helpers ──────────────────────────────────────────────────────────────────
info() { printf ' %s›%s %s\n' "$VIOLET" "$RESET" "$1"; }
warn() { printf ' %s▲%s %s\n' "$YELLOW" "$RESET" "$1" >&2; }
ok()   { printf ' %s✔%s %s\n' "$VIOLET" "$RESET" "$1"; }
die()  { printf ' %s✖ %s%s\n' "${RED}${BOLD}" "$1" "$RESET" >&2; exit 1; }

banner() {
  printf '\n'
  printf '   %s███╗   ███╗███████╗███╗   ███╗ ██████╗ %s\n' "$G1" "$RESET"
  printf '   %s████╗ ████║██╔════╝████╗ ████║██╔═══██╗%s\n' "$G2" "$RESET"
  printf '   %s██╔████╔██║█████╗  ██╔████╔██║██║   ██║%s\n' "$G3" "$RESET"
  printf '   %s██║╚██╔╝██║██╔══╝  ██║╚██╔╝██║██║   ██║%s\n' "$G4" "$RESET"
  printf '   %s██║ ╚═╝ ██║███████╗██║ ╚═╝ ██║╚██████╔╝%s\n' "$G5" "$RESET"
  printf '   %s╚═╝     ╚═╝╚══════╝╚═╝     ╚═╝ ╚═════╝ %s\n' "$G6" "$RESET"
  printf '   %slocal-first notes, refined by AI on your machine%s\n' "$DIM" "$RESET"
  printf '\n'
}

# Run a command while showing a spinner; falls back to plain output when the
# terminal can't render it. Returns the command's exit code; on failure the
# last few lines of its output are shown.
# Usage: spin "message" command [args...]
spin() {
  _msg="$1"; shift
  if [ -z "$VIOLET" ] || [ ! -t 1 ]; then
    info "$_msg"
    "$@"
    return $?
  fi
  _log="$(mktemp 2>/dev/null || printf '%s' "${TMPDIR:-/tmp}/memo-spin.$$")"
  "$@" >"$_log" 2>&1 &
  _pid=$!
  while kill -0 "$_pid" 2>/dev/null; do
    for _f in ⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏; do
      printf '\r %s%s%s %s ' "$VIOLET" "$_f" "$RESET" "$_msg"
      kill -0 "$_pid" 2>/dev/null || break
      sleep 0.1 2>/dev/null || sleep 1
    done
  done
  if wait "$_pid"; then _rc=0; else _rc=$?; fi
  if [ "$_rc" -eq 0 ]; then
    printf '\r %s✔%s %s \n' "$VIOLET" "$RESET" "$_msg"
  else
    printf '\r %s✖%s %s \n' "$RED" "$RESET" "$_msg"
    [ -s "$_log" ] && tail -n 6 "$_log" | sed 's/^/     /' >&2
  fi
  rm -f "$_log" 2>/dev/null || true
  return $_rc
}

# Read a line from the terminal even when the script is piped via curl | sh.
read_tty() {
  if [ -e /dev/tty ]; then
    read -r _tty_answer </dev/tty
  else
    _tty_answer="n"
  fi
  printf '%s' "$_tty_answer"
}

# Yes/no prompt. Falls back to the default when there's no terminal (CI, pipes).
# Usage: ask "Question?" [Y|N]   -> returns 0 for yes, 1 for no.
ask() {
  _q="$1"; _def="${2:-Y}"
  if [ "$_def" = "Y" ]; then _hint="[Y/n]"; else _hint="[y/N]"; fi
  if [ ! -e /dev/tty ] || [ ! -t 1 ]; then
    if [ "$_def" = "Y" ]; then return 0; else return 1; fi
  fi
  printf '   %s%s%s %s%s%s ' "$BOLD" "$_q" "$RESET" "$DIM" "$_hint" "$RESET"
  _a="$(read_tty)"
  case "$_a" in
    [Yy]*) return 0 ;;
    [Nn]*) return 1 ;;
    *)     if [ "$_def" = "Y" ]; then return 0; else return 1; fi ;;
  esac
}

# Passes an Authorization header when GITHUB_TOKEN is set.
_curl() { curl ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} "$@"; }

# Extract a release-asset URL from GitHub releases JSON.
# Usage: _asset_url JSON FILENAME FIELD
# FIELD: "url" (API endpoint, for private repos) or "browser_download_url" (public).
_asset_url() {
  if command -v python3 >/dev/null 2>&1; then
    printf '%s' "$1" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    for a in d.get('assets', []):
        if a.get('name') == sys.argv[1]:
            print(a.get(sys.argv[2], ''))
            break
except Exception:
    pass
" "$2" "$3" 2>/dev/null
  else
    # Fallback (no python3): grep browser_download_url — public repos only.
    printf '%s' "$1" \
      | grep -o "\"browser_download_url\":\"[^\"]*$2[^\"]*\"" \
      | grep -o 'https://[^"]*'
  fi
}

# Auto-install a system package using the available package manager.
# Usage: _pkg_install <apt-pkg> <dnf/yum-pkg> <pacman/zypper-pkg> <apk-pkg>
_APT_UPDATED=0
_pkg_install() {
  if command -v apt-get >/dev/null 2>&1; then
    if [ "$_APT_UPDATED" = "0" ]; then
      sudo apt-get update -qq || true
      _APT_UPDATED=1
    fi
    sudo apt-get install -y "$1" || return 1
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y "$2" || return 1
  elif command -v yum >/dev/null 2>&1; then
    sudo yum install -y "$2" || return 1
  elif command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --noconfirm "$3" || return 1
  elif command -v zypper >/dev/null 2>&1; then
    sudo zypper install -y "$3" || return 1
  elif command -v apk >/dev/null 2>&1; then
    sudo apk add "$4" || return 1
  else
    return 1
  fi
}

# ── platform detection ────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)
    # The universal binary covers both Apple Silicon and Intel Macs.
    TARGET="universal-apple-darwin"
    PLATFORM="macos"
    OS_LABEL="macOS"
    ;;
  Linux)
    case "$ARCH" in
      x86_64)        TARGET="x86_64-unknown-linux-gnu"  ; PLATFORM="linux" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ; PLATFORM="linux" ;;
      *)             TARGET="" ; PLATFORM="linux" ;;
    esac
    OS_LABEL="Linux"
    ;;
  *)
    TARGET=""
    PLATFORM="unknown"
    OS_LABEL="$OS"
    ;;
esac

# ── welcome ───────────────────────────────────────────────────────────────────
banner
info "A keyboard-driven note-taker that refines your notes with a ${BOLD}local${RESET} AI."
info "Target: ${BOLD}${OS_LABEL} (${ARCH})${RESET}"
printf '\n'
ask "Ready to install memo?" Y || { info "Cancelled — nothing was changed."; exit 0; }
printf '\n'

# ── Linux: ensure ALSA runtime dependency ─────────────────────────────────────
if [ "$PLATFORM" = "linux" ]; then
  _check_alsa() {
    if ldconfig -p 2>/dev/null | grep -q 'libasound\.so\.2'; then
      return 0
    fi
    for _lib in \
      /usr/lib/x86_64-linux-gnu/libasound.so.2 \
      /usr/lib/aarch64-linux-gnu/libasound.so.2 \
      /usr/lib/libasound.so.2 \
      /lib/x86_64-linux-gnu/libasound.so.2 \
      /lib/libasound.so.2; do
      [ -f "$_lib" ] && return 0
    done
    return 1
  }

  if ! _check_alsa; then
    info "ALSA library not found — installing it now (required for microphone access)..."
    if command -v apt-get >/dev/null 2>&1; then
      if [ "$_APT_UPDATED" = "0" ]; then
        sudo apt-get update -qq || true
        _APT_UPDATED=1
      fi
      # Try libasound2t64 first (Ubuntu 24.04+), fall back to libasound2 (older).
      sudo apt-get install -y libasound2t64 2>/dev/null || \
        sudo apt-get install -y libasound2 || true
    else
      _pkg_install "" alsa-lib alsa-lib alsa-lib || true
    fi
    if ! _check_alsa; then
      die "libasound.so.2 not found. The memo binary requires ALSA for microphone access.

  Install it first, then re-run this installer:
    Debian / Ubuntu (<24.04): sudo apt-get install libasound2
    Debian / Ubuntu (24.04+): sudo apt-get install libasound2t64
    Fedora / RHEL:            sudo dnf install alsa-lib
    Arch:                     sudo pacman -S alsa-lib"
    fi
    ok "ALSA library installed."
  fi
fi

# ── pick install directory (always user-writable — never needs a password) ────
# Use /usr/local/bin only when it's writable without elevation; otherwise fall
# back to ~/.local/bin so installing the app never prompts for a sudo password.
if [ -w /usr/local/bin ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

_install_bin() {
  install -m 755 "$1" "$INSTALL_DIR/memo"
}

# ── PATH setup for ~/.local/bin ───────────────────────────────────────────────
_ensure_local_bin_on_path() {
  case ":${PATH}:" in
    *":$HOME/.local/bin:"*) return 0 ;;
  esac
  _shell="$(basename "${SHELL:-sh}")"
  case "$_shell" in
    zsh)  _rc="$HOME/.zshrc" ;;
    bash) _rc="$HOME/.bashrc" ;;
    *)    _rc="$HOME/.profile" ;;
  esac
  printf '\n# Added by memo installer\nexport PATH="%s/.local/bin:$PATH"\n' "$HOME" >> "$_rc"
  info "Added ~/.local/bin to PATH in $_rc — open a new terminal or run: export PATH=\"\$HOME/.local/bin:\$PATH\""
}

# ── prebuilt download ─────────────────────────────────────────────────────────
try_download_prebuilt() {
  [ -n "$TARGET" ] || return 1
  command -v curl >/dev/null 2>&1 || { warn "curl not found — cannot download prebuilt binary"; return 1; }

  # (check runs silently; the download step shows the spinner)
  if [ "$INSTALL_PRERELEASE" = "1" ]; then
    # /releases returns all releases including pre-releases, newest first.
    _all="$(_curl -fsSL "${GITHUB_API}/releases" 2>/dev/null)" || true
    if command -v python3 >/dev/null 2>&1; then
      VERSION="$(printf '%s' "$_all" | python3 -c "
import sys, json
try:
    releases = json.load(sys.stdin)
    if releases:
        print(releases[0].get('tag_name', ''))
except Exception:
    pass
" 2>/dev/null)" || true
      _rel="$(printf '%s' "$_all" | python3 -c "
import sys, json
try:
    releases = json.load(sys.stdin)
    if releases:
        print(json.dumps(releases[0]))
except Exception:
    pass
" 2>/dev/null)" || true
    else
      VERSION="$(printf '%s' "$_all" | grep -o '"tag_name": *"[^"]*"' | head -1 | grep -o 'v[0-9][^"]*')" || true
      _rel="$_all"
    fi
  else
    _rel="$(_curl -fsSL "${GITHUB_API}/releases/latest" 2>/dev/null)" || true
    VERSION="$(printf '%s' "$_rel" | grep -o '"tag_name": *"[^"]*"' | grep -o 'v[0-9][^"]*')" || true
  fi

  if [ -z "$VERSION" ]; then
    warn "Could not determine latest release version"
    [ "$INSTALL_PRERELEASE" = "0" ] && warn "If you want a pre-release, re-run with:  sh install.sh --pre"
    return 1
  fi

  TARBALL="memo-${VERSION}-${TARGET}.tar.gz"
  SUMS="memo-${VERSION}-SHA256SUMS.txt"

  if [ -n "$GITHUB_TOKEN" ]; then
    # Private repo: download via the API asset endpoint (requires auth + Accept header).
    DL_URL="$(_asset_url "$_rel" "$TARBALL" "url")"
    SUMS_URL="$(_asset_url "$_rel" "$SUMS" "url")"
    _dl_asset() { _curl -fsSL -H "Accept: application/octet-stream" "$1" -o "$2"; }
  else
    DL_URL="$(_asset_url "$_rel" "$TARBALL" "browser_download_url")"
    SUMS_URL="$(_asset_url "$_rel" "$SUMS" "browser_download_url")"
    _dl_asset() { _curl -fsSL "$1" -o "$2"; }
  fi

  if [ -z "$DL_URL" ]; then
    warn "No prebuilt binary for $TARGET found in release $VERSION"
    return 1
  fi

  TMPWORK="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$TMPWORK'" EXIT INT TERM

  if ! spin "Downloading memo ${VERSION} (${TARGET})" _dl_asset "$DL_URL" "$TMPWORK/$TARBALL"; then
    warn "Download failed — no prebuilt for $TARGET in release $VERSION"
    return 1
  fi

  # Verify checksum when possible.
  if _dl_asset "$SUMS_URL" "$TMPWORK/$SUMS" 2>/dev/null; then
    EXPECTED="$(grep "$TARBALL" "$TMPWORK/$SUMS" | awk '{print $1}')"
    if [ -n "$EXPECTED" ]; then
      if command -v sha256sum >/dev/null 2>&1; then
        ACTUAL="$(sha256sum "$TMPWORK/$TARBALL" | awk '{print $1}')"
      elif command -v shasum >/dev/null 2>&1; then
        ACTUAL="$(shasum -a 256 "$TMPWORK/$TARBALL" | awk '{print $1}')"
      else
        warn "Cannot verify checksum (no sha256sum or shasum found) — proceeding anyway"
        ACTUAL="$EXPECTED"
      fi
      [ "$ACTUAL" = "$EXPECTED" ] || die "Checksum mismatch for $TARBALL. Aborting."
    fi
  else
    warn "SHA256SUMS not available — skipping checksum verification"
  fi

  tar -C "$TMPWORK" -xzf "$TMPWORK/$TARBALL" memo
  _install_bin "$TMPWORK/memo"
  return 0
}

# ── source build fallback ─────────────────────────────────────────────────────
build_from_source() {
  # Determine the repo root — only works when NOT piped from curl.
  SCRIPT_DIR="$(cd "$(dirname "${0:-}")" 2>/dev/null && pwd)" || SCRIPT_DIR=""
  if [ -z "$SCRIPT_DIR" ] || [ ! -f "$SCRIPT_DIR/Cargo.toml" ]; then
    die "No prebuilt binary is available for $OS/$ARCH.
  To install from source, clone the repo and run the installer directly:
    git clone https://github.com/${REPO}.git
    cd memo
    ./install.sh"
  fi

  # Ensure Rust / cargo.
  if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
    . "$HOME/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    spin "Installing Rust" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    . "$HOME/.cargo/env"
  fi

  # Ensure build prerequisites for whisper.cpp (compiled via CMake at build time).
  if ! command -v cc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
    case "$OS" in
      Darwin)
        die "No C/C++ compiler found. Run: xcode-select --install, then re-run ./install.sh."
        ;;
      *)
        spin "Installing build tools" sh -c "_pkg_install build-essential 'gcc gcc-c++ make' base-devel build-base" || true
        ;;
    esac
    if ! command -v cc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
      die "No C/C++ compiler found. Install build-essential (or equivalent), then re-run ./install.sh."
    fi
  fi
  if ! command -v cmake >/dev/null 2>&1; then
    if [ "$OS" = "Darwin" ] && command -v brew >/dev/null 2>&1; then
      spin "Installing cmake" brew install cmake || true
    else
      spin "Installing cmake" sh -c "_pkg_install cmake cmake cmake cmake" || true
    fi
    if ! command -v cmake >/dev/null 2>&1; then
      case "$OS" in
        Darwin) die "cmake not found. Run: brew install cmake, then re-run ./install.sh." ;;
        *)      die "cmake not found. Install cmake with your package manager, then re-run ./install.sh." ;;
      esac
    fi
  fi

  spin "Building memo from source (first build may take a few minutes)" \
    cargo install --path "$SCRIPT_DIR" --locked --force

  INSTALL_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
}

# ── install summary box ───────────────────────────────────────────────────────
_boxline() { printf '   %s│%s %-46s %s│%s\n' "$YELLOW" "$RESET" "$1" "$YELLOW" "$RESET"; }
print_summary() {
  _ver="${VERSION:-}"
  [ -n "$_ver" ] && _ver=" $_ver"
  printf '\n'
  printf '   %s╭%s╮%s\n' "$YELLOW" "$HBAR" "$RESET"
  _boxline "memo${_ver} is installed and ready."
  _boxline ""
  _boxline "launch          memo"
  _boxline "new note        n"
  _boxline "open note       o  or  Enter"
  _boxline "search          /"
  _boxline "AI refine       Ctrl+R"
  _boxline "custom prompt   Ctrl+P"
  _boxline "save            Ctrl+S"
  _boxline "edit title      Ctrl+T"
  _boxline "toggle refined  Tab"
  _boxline "dictation       hold  F5  (double-press: live)"
  _boxline "help / keys     Ctrl+H"
  printf '   %s╰%s╯%s\n' "$YELLOW" "$HBAR" "$RESET"
  printf '\n'
}

# ── main ──────────────────────────────────────────────────────────────────────
# When invoked from a local clone (Cargo.toml present alongside this script),
# build from source so the installed binary always reflects local changes.
_SCRIPT_DIR="$(cd "$(dirname "${0:-}")" 2>/dev/null && pwd)" || _SCRIPT_DIR=""
_IS_LOCAL_CLONE=0
[ -n "$_SCRIPT_DIR" ] && [ -f "$_SCRIPT_DIR/Cargo.toml" ] && _IS_LOCAL_CLONE=1

if [ "$_IS_LOCAL_CLONE" = "1" ]; then
  _DEST_DIR="$INSTALL_DIR"
  build_from_source
  # build_from_source installs to ~/.cargo/bin and resets INSTALL_DIR to that.
  # If the originally determined install dir is different (e.g. /usr/local/bin),
  # also copy there so the locally-built binary supersedes any old prebuilt.
  if [ "$_DEST_DIR" != "$INSTALL_DIR" ] && [ -f "$INSTALL_DIR/memo" ]; then
    _src_bin="$INSTALL_DIR/memo"
    INSTALL_DIR="$_DEST_DIR"
    _install_bin "$_src_bin"
  fi
  SOURCE_BUILD=1
elif try_download_prebuilt; then
  ok "memo ${VERSION} installed to $INSTALL_DIR/memo"
  SOURCE_BUILD=0
else
  build_from_source
  SOURCE_BUILD=1
fi

if [ "$INSTALL_DIR" = "$HOME/.local/bin" ]; then
  _ensure_local_bin_on_path
fi

# ── choose the local LLM backend (mlx-lm or Ollama) ───────────────────────────
is_interactive() { [ -e /dev/tty ] && [ -t 1 ]; }

# pip/uv install of mlx-lm, trying whichever installer is available.
_pip_install_mlx() {
  if command -v uv >/dev/null 2>&1; then
    spin "Installing mlx-lm with uv" uv pip install --quiet mlx-lm
  elif command -v pip3 >/dev/null 2>&1; then
    spin "Installing mlx-lm with pip3" pip3 install --quiet --upgrade mlx-lm
  elif command -v pip >/dev/null 2>&1; then
    spin "Installing mlx-lm with pip" pip install --quiet --upgrade mlx-lm
  elif command -v python3 >/dev/null 2>&1; then
    spin "Installing mlx-lm with python3 -m pip" python3 -m pip install --quiet --upgrade mlx-lm
  else
    warn "Python not found. Install manually:  pip install mlx-lm"
    return 1
  fi
}

setup_mlx() {
  if [ "$OS" != "Darwin" ]; then
    warn "mlx-lm runs on Apple Silicon only — use Ollama on this platform instead."
    return 0
  fi
  if python3 -c "import mlx_lm" 2>/dev/null; then
    ok "mlx-lm already installed."
    return 0
  fi
  info "${BOLD}mlx-lm${RESET} is the on-device AI server for Apple Silicon."
  if is_interactive && ask "Install mlx-lm now?" Y; then
    _pip_install_mlx && ok "mlx-lm installed." \
      || warn "mlx-lm install failed. Try manually:  pip install mlx-lm"
  else
    info "Install it later with:  pip install mlx-lm"
  fi
}

setup_ollama() {
  if command -v ollama >/dev/null 2>&1; then
    ok "Ollama already installed at $(command -v ollama)"
  else
    warn "Ollama is not installed."
    if [ "$OS" = "Darwin" ] && command -v brew >/dev/null 2>&1 && is_interactive && ask "Install Ollama now with Homebrew?" Y; then
      spin "Installing Ollama with Homebrew" brew install ollama && ok "Ollama installed." \
        || warn "Ollama install failed. Install it from:  https://ollama.com/download"
    elif [ "$OS" = "Linux" ]; then
      info "Install Ollama with:  curl -fsSL https://ollama.com/install.sh | sh"
    else
      info "Install Ollama from:  https://ollama.com/download"
    fi
  fi
  info "Then pull a model:  ollama pull llama3.1"

  # Point memo at Ollama by writing a config — but never clobber an existing one.
  _cfg_dir="${XDG_CONFIG_HOME:-$HOME/.config}/memo"
  _cfg_file="$_cfg_dir/config.toml"
  if [ -f "$_cfg_file" ]; then
    info "Existing config left untouched:  $_cfg_file"
  else
    mkdir -p "$_cfg_dir"
    cat > "$_cfg_file" <<'EOF'
# memo configuration — using Ollama as the local AI backend.
# Install Ollama from https://ollama.com, then:  ollama pull llama3.1
# memo starts and stops the Ollama server for you on launch.
provider = "ollama"
base_url = "http://localhost:11434/v1"
model    = "llama3.1"
auto_start_server = true
EOF
    ok "Configured memo to use Ollama  ($_cfg_file)"
  fi
}

# Interactive radio-button selector driven by the keyboard:
#   ↑/↓ (or k/j) move the highlight, space or enter confirms it.
# Draws the list on /dev/tty and prints the chosen 1-based index to stdout, so
# call it as:  idx="$(radio_select DEFAULT "label one" "label two" ...)"
# Returns non-zero (printing DEFAULT) when no raw-input terminal is available,
# letting the caller fall back to a plain prompt or the default.
radio_select() {
  _def="$1"; shift
  _n=$#
  # Gate on /dev/tty being a real terminal — not on [ -t 1 ], because we are
  # usually called inside $(...) where stdout is a pipe. All of our I/O goes to
  # /dev/tty, and `stty -g` succeeds only when it is an actual terminal.
  command -v stty >/dev/null 2>&1 || { printf '%s' "$_def"; return 1; }
  [ -e /dev/tty ] || { printf '%s' "$_def"; return 1; }
  _esc="$(printf '\033')"; _cr="$(printf '\r')"
  _saved="$(stty -g </dev/tty 2>/dev/null)" || { printf '%s' "$_def"; return 1; }

  _cleanup() {
    stty "$_saved" </dev/tty 2>/dev/null || true
    printf '%s[?25h' "$_esc" >/dev/tty   # show cursor again
  }
  trap '_cleanup; exit 130' INT TERM
  if ! stty -echo -icanon min 1 time 0 </dev/tty 2>/dev/null; then
    _cleanup; trap - INT TERM; printf '%s' "$_def"; return 1
  fi
  printf '%s[?25l' "$_esc" >/dev/tty       # hide cursor while choosing

  _cur="$_def"
  _draw() {
    _i=1
    for _lab in "$@"; do
      if [ "$_i" = "$_cur" ]; then
        printf '\r%s[K   %s◉%s %s%s%s\n' "$_esc" "${BOLD}${VIOLET}" "$RESET" "${BOLD}${YELLOW}" "$_lab" "$RESET" >/dev/tty
      else
        printf '\r%s[K   %s○ %s%s\n' "$_esc" "$DIM" "$_lab" "$RESET" >/dev/tty
      fi
      _i=$((_i + 1))
    done
  }
  _draw "$@"

  while :; do
    _key="$(dd if=/dev/tty bs=1 count=1 2>/dev/null || true)"
    case "$_key" in
      ''|"$_cr"|' ') break ;;                       # enter / space → confirm
      'k'|'K') _cur=$((_cur - 1)) ;;
      'j'|'J') _cur=$((_cur + 1)) ;;
      'q'|'Q') _cur=0; break ;;                      # q → cancel
      "$_esc")
        # Read the rest of an escape sequence with a brief timeout so a lone
        # Esc keypress can't block the loop.
        stty time 1 min 0 </dev/tty 2>/dev/null || true
        _k2="$(dd if=/dev/tty bs=1 count=1 2>/dev/null || true)"
        _k3="$(dd if=/dev/tty bs=1 count=1 2>/dev/null || true)"
        stty time 0 min 1 </dev/tty 2>/dev/null || true
        case "$_k2$_k3" in
          '[A'|'OA') _cur=$((_cur - 1)) ;;
          '[B'|'OB') _cur=$((_cur + 1)) ;;
          '') _cur=0; break ;;                       # bare Esc → cancel
          *) : ;;                                    # other escape seq → ignore
        esac
        ;;
      *) : ;;
    esac
    [ "$_cur" -lt 1 ] && _cur="$_n"
    [ "$_cur" -gt "$_n" ] && _cur=1
    printf '%s[%dA' "$_esc" "$_n" >/dev/tty          # back up to repaint the list
    _draw "$@"
  done

  _cleanup
  trap - INT TERM
  printf '%s' "$_cur"
  return 0
}

setup_skip() {
  info "Skipping local LLM setup — no server was installed or configured."
  info "memo still captures and edits notes; AI refine (Ctrl+R) needs a server."
  info "Set one up anytime by running:  ${BOLD}memo --setup${RESET}"
}

setup_backend() {
  # Build plain-text option labels (the selector adds the highlight styling).
  _mlx_label="mlx-lm    on-device, Apple Silicon"
  if [ "$OS" = "Darwin" ]; then _mlx_label="$_mlx_label   (recommended)"
  else _mlx_label="$_mlx_label   (Apple Silicon only)"; fi
  [ "$OS" = "Darwin" ] && python3 -c "import mlx_lm" 2>/dev/null && _mlx_label="$_mlx_label   ✔ installed"
  _oll_label="Ollama    cross-platform: Intel, Linux, Windows"
  command -v ollama >/dev/null 2>&1 && _oll_label="$_oll_label   ✔ installed"
  _skip_label="Skip — don't set up a server now"

  # Default: mlx on macOS (Apple Silicon), Ollama everywhere else.
  if [ "$OS" = "Darwin" ]; then _default="1"; else _default="2"; fi

  printf '\n'
  info "Choose a ${BOLD}local LLM server${RESET} for AI refinement:"
  is_interactive && info "${DIM}↑/↓ move · space/enter select · esc cancel${RESET}"
  printf '\n'

  if _choice="$(radio_select "$_default" "$_mlx_label" "$_oll_label" "$_skip_label")"; then
    : # selected via the radio UI
  else
    # No raw-input UI — show a numbered menu (interactive) or take the default.
    printf '     1) %s\n' "$_mlx_label"
    printf '     2) %s\n' "$_oll_label"
    printf '     3) %s\n' "$_skip_label"
    if is_interactive; then
      printf '   %sSelect backend [1/2/3]%s %s(default %s)%s ' "$BOLD" "$RESET" "$DIM" "$_default" "$RESET"
      case "$(read_tty)" in
        1) _choice="1" ;;
        2) _choice="2" ;;
        3) _choice="3" ;;
        *) _choice="$_default" ;;
      esac
    else
      _choice="$_default"
    fi
  fi
  printf '\n'

  case "$_choice" in
    1) setup_mlx ;;
    2) setup_ollama ;;
    0) info "Setup cancelled."; setup_skip ;;
    *) setup_skip ;;
  esac
}

setup_backend

# ── done ──────────────────────────────────────────────────────────────────────
print_summary
if [ "$SOURCE_BUILD" = "1" ]; then
  info "You may need to open a new terminal first (or run: source \"\$HOME/.cargo/env\")."
fi
