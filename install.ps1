# Installer for memo — Windows
#
# One-line install (run in PowerShell):
#   irm https://raw.githubusercontent.com/blackstardesigns/memo/main/install.ps1 | iex
#
# What it does:
#   1. Downloads the latest prebuilt memo.exe from GitHub Releases.
#   2. Installs it to %LOCALAPPDATA%\Programs\memo (no administrator rights needed).
#   3. Adds the install directory to your user PATH.
#   4. Creates a default config that points memo at Ollama (the Windows AI backend).
#   5. Checks whether Ollama is installed and tells you what to do if not.

$ErrorActionPreference = 'Stop'

$Repo       = 'blackstardesigns/memo'
$GhBase     = "https://github.com/$Repo"
$Target     = 'x86_64-pc-windows-msvc'
$InstallDir = "$env:LOCALAPPDATA\Programs\memo"

function Write-Info  ($msg) { Write-Host " > $msg"  -ForegroundColor Cyan }
function Write-Ok    ($msg) { Write-Host " + $msg"  -ForegroundColor Green }
function Write-Warn  ($msg) { Write-Host " ! $msg"  -ForegroundColor Yellow }
function Write-Fatal ($msg) { Write-Host " x $msg"  -ForegroundColor Red; exit 1 }

# ── banner (pure ASCII so it renders under any console / encoding) ─────────────
function Show-Banner {
    Write-Host ''
    Write-Host '   #   # ##### #   #  ### ' -ForegroundColor Cyan
    Write-Host '   ## ## #     ## ## #   #' -ForegroundColor Cyan
    Write-Host '   # # # ####  # # # #   #' -ForegroundColor DarkCyan
    Write-Host '   #   # #     #   # #   #' -ForegroundColor Blue
    Write-Host '   #   # ##### #   #  ### ' -ForegroundColor Blue
    Write-Host '   local-first notes, refined by AI on your machine' -ForegroundColor DarkGray
    Write-Host ''
}

# ── summary box (ASCII border, padded so the right edge always lines up) ───────
function Show-Summary ($version) {
    $w   = 48
    $bar = '+' + ('-' * $w) + '+'
    $rows = @(
        "memo $version is installed and ready."
        ''
        'launch       memo'
        'new note     press  n'
        'AI refine    press  Ctrl+R'
        'dictation    hold  F5'
        'help / keys  Alt + \'
    )
    Write-Host ''
    Write-Host "   $bar" -ForegroundColor Green
    foreach ($r in $rows) {
        Write-Host '   |' -ForegroundColor Green -NoNewline
        Write-Host (' ' + $r.PadRight($w - 2) + ' ') -NoNewline
        Write-Host '|' -ForegroundColor Green
    }
    Write-Host "   $bar" -ForegroundColor Green
    Write-Host ''
}

# Yes/no prompt that proceeds automatically when running non-interactively.
function Confirm-Yes ($question, $defaultYes = $true) {
    $hint = if ($defaultYes) { '[Y/n]' } else { '[y/N]' }
    if (-not [Environment]::UserInteractive) { return $defaultYes }
    try {
        $ans = Read-Host "   $question $hint"
    } catch {
        return $defaultYes
    }
    if ($ans -match '^\s*[Yy]') { return $true }
    if ($ans -match '^\s*[Nn]') { return $false }
    return $defaultYes
}

# ── welcome ───────────────────────────────────────────────────────────────────
Show-Banner
Write-Info 'A keyboard-driven note-taker that refines your notes with a local AI.'
Write-Info 'Target: Windows (x86_64) — AI refinement runs through Ollama.'
Write-Host ''
if (-not (Confirm-Yes 'Ready to install memo?')) {
    Write-Info 'Cancelled — nothing was changed.'
    exit 0
}
Write-Host ''

# ── architecture check ────────────────────────────────────────────────────────
$cpuArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($cpuArch -ne 'X64') {
    Write-Fatal "Only 64-bit (x86_64) Windows is supported. Detected: $cpuArch"
}

# ── resolve latest release version ───────────────────────────────────────────
Write-Info 'Checking for latest release...'
try {
    $latest  = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
    $Version = $latest.tag_name
} catch {
    Write-Fatal "Could not fetch latest release from GitHub: $_"
}
if (-not $Version) { Write-Fatal 'Could not determine latest release version.' }

$ZipName  = "memo-$Version-$Target.zip"
$SumsName = "memo-$Version-SHA256SUMS.txt"
$DlUrl    = "$GhBase/releases/download/$Version/$ZipName"
$SumsUrl  = "$GhBase/releases/download/$Version/$SumsName"

# ── download ──────────────────────────────────────────────────────────────────
Write-Info "Downloading memo $Version..."
$TmpDir = Join-Path $env:TEMP "memo-install-$(Get-Random)"
New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null

try {
    Invoke-WebRequest -Uri $DlUrl -OutFile "$TmpDir\$ZipName" -UseBasicParsing
} catch {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
    Write-Fatal "Download failed: $_"
}

# ── verify checksum ───────────────────────────────────────────────────────────
try {
    Invoke-WebRequest -Uri $SumsUrl -OutFile "$TmpDir\$SumsName" -UseBasicParsing
    $sumsContent = Get-Content "$TmpDir\$SumsName"
    $expectedLine = $sumsContent | Where-Object { $_ -match [regex]::Escape($ZipName) }
    if ($expectedLine) {
        $Expected = ($expectedLine -split '\s+')[0].ToLower()
        $Actual   = (Get-FileHash "$TmpDir\$ZipName" -Algorithm SHA256).Hash.ToLower()
        if ($Actual -ne $Expected) {
            Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
            Write-Fatal "Checksum mismatch for $ZipName (expected $Expected, got $Actual). Aborting."
        }
        Write-Ok 'Checksum verified.'
    }
} catch {
    Write-Warn "Could not fetch SHA256SUMS — skipping checksum verification."
}

# ── install ───────────────────────────────────────────────────────────────────
Expand-Archive -Path "$TmpDir\$ZipName" -DestinationPath $TmpDir -Force
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item "$TmpDir\memo.exe" "$InstallDir\memo.exe" -Force
Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue

Write-Ok "memo $Version installed to $InstallDir\memo.exe"

# ── add to user PATH ──────────────────────────────────────────────────────────
$UserPath = [System.Environment]::GetEnvironmentVariable('PATH', 'User')
if ($UserPath -notlike "*$InstallDir*") {
    [System.Environment]::SetEnvironmentVariable('PATH', "$InstallDir;$UserPath", 'User')
    $env:PATH = "$InstallDir;$env:PATH"
    Write-Info "Added $InstallDir to your PATH."
    Write-Info "Open a new terminal window for the change to take effect."
}

# ── create default config (Ollama backend) ────────────────────────────────────
# memo reads its config from ~/.config/memo/config.toml on every platform
# (it uses the user's home directory directly, not %APPDATA%).
$ConfigDir  = "$env:USERPROFILE\.config\memo"
$ConfigFile = "$ConfigDir\config.toml"
if (-not (Test-Path $ConfigFile)) {
    New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
    @'
# memo configuration
# Windows uses Ollama as the local AI backend (mlx-lm is Apple Silicon only).
# Install Ollama from https://ollama.com, then pull a model:
#   ollama pull llama3.1
# memo will start and stop the Ollama server automatically.

provider = "ollama"
base_url = "http://localhost:11434/v1"
model    = "llama3.1"
auto_start_server = true
'@ | Set-Content $ConfigFile -Encoding UTF8
    Write-Info "Created default config at $ConfigFile"
}

# ── check for Ollama ──────────────────────────────────────────────────────────
Write-Host ''
$ollamaCmd = Get-Command ollama -ErrorAction SilentlyContinue
if ($ollamaCmd) {
    Write-Ok "Ollama is installed at $($ollamaCmd.Source)"
    Write-Info "Pull a model if you haven't already:  ollama pull llama3.1"
} else {
    Write-Warn "Ollama is not installed."
    Write-Info "memo uses Ollama for AI refinement on Windows."
    Write-Info "Install it from:  https://ollama.com"
    Write-Info "Then pull a model:  ollama pull llama3.1"
}

# ── done ──────────────────────────────────────────────────────────────────────
Show-Summary $Version
Write-Ok "Open a new terminal and type 'memo' to launch."
