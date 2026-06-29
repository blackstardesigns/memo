//! `memo update`: self-update the installed binary from the latest GitHub Release.
//!
//! This mirrors the prebuilt fast-path in `install.sh`, but runs from inside the
//! binary so a user never has to re-run the installer: query the GitHub Releases
//! API for the newest version, download the archive built for this platform,
//! verify its SHA-256 against the published `…-SHA256SUMS.txt`, then atomically
//! replace the running executable in place.
//!
//! Asset names match what `.github/workflows/release.yml` publishes:
//!   `memo-<tag>-<target>.tar.gz`   (Unix)
//!   `memo-<tag>-<target>.zip`      (Windows)
//!   `memo-<tag>-SHA256SUMS.txt`
//! where `<tag>` is e.g. `v0.1.0` and `<target>` is the triple from
//! [`platform_target`] (macOS ships a single `universal-apple-darwin` binary).

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::Deserialize;

const REPO: &str = "blackstardesigns/memo";
const API_BASE: &str = "https://api.github.com/repos/blackstardesigns/memo";
const USER_AGENT: &str = concat!("memo-updater/", env!("CARGO_PKG_VERSION"));

/// The slice of the GitHub Releases API response we use.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Entry point for the `update` subcommand.
///
/// * `include_prerelease` — also consider `rc` / `alpha` / `beta` releases.
/// * `check_only` — report whether an update exists, but don't install it.
pub fn run(include_prerelease: bool, check_only: bool) -> Result<()> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("parsing this binary's own version")?;
    println!("Current version: {current}");

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .build();

    let release = fetch_target_release(&agent, include_prerelease)?;
    let latest = parse_tag(&release.tag_name)
        .with_context(|| format!("parsing release tag '{}'", release.tag_name))?;

    if latest <= current {
        println!("You're already on the latest version ({latest}).");
        if !include_prerelease {
            println!("(Run `memo update --pre` to include pre-releases.)");
        }
        return Ok(());
    }

    println!("Update available: {current} -> {latest}");
    if !release.html_url.is_empty() {
        println!("  {}", release.html_url);
    }
    if check_only {
        println!("Run `memo update` to install it.");
        return Ok(());
    }

    install_release(&agent, &release)?;
    println!("Updated memo {current} -> {latest}. Restart memo to use the new version.");
    Ok(())
}

/// Resolve the release to update to. Without `include_prerelease` this is the
/// latest stable release (GitHub's `releases/latest`); with it, the highest
/// version among recent releases (which includes pre-releases).
fn fetch_target_release(agent: &ureq::Agent, include_prerelease: bool) -> Result<Release> {
    if include_prerelease {
        // `releases/latest` excludes pre-releases, so list recent releases and
        // pick the highest version (compare by semver rather than trusting order).
        let url = format!("{API_BASE}/releases?per_page=30");
        let releases: Vec<Release> = get_json(agent, &url)?;
        releases
            .into_iter()
            .filter_map(|r| parse_tag(&r.tag_name).ok().map(|v| (v, r)))
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, r)| r)
            .ok_or_else(|| anyhow!("no releases found for {REPO}"))
    } else {
        let url = format!("{API_BASE}/releases/latest");
        match agent
            .get(&url)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/vnd.github+json")
            .call()
        {
            Ok(resp) => resp.into_json().context("parsing the latest-release response"),
            // No stable release published yet — point the user at `--pre`.
            Err(ureq::Error::Status(404, _)) => bail!(
                "No stable release found for {REPO}.\n\
                 Run `memo update --pre` to update to the latest pre-release."
            ),
            Err(ureq::Error::Status(code, resp)) => {
                let detail = resp.into_string().unwrap_or_default();
                bail!("GitHub API returned HTTP {code}: {}", detail.trim());
            }
            Err(e) => Err(anyhow!("could not reach GitHub: {e}")),
        }
    }
}

/// GET `url` and decode the JSON body, mapping HTTP/transport errors to context.
fn get_json<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T> {
    let resp = match agent
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let detail = resp.into_string().unwrap_or_default();
            bail!("GitHub API returned HTTP {code}: {}", detail.trim());
        }
        Err(e) => return Err(anyhow!("could not reach GitHub: {e}")),
    };
    resp.into_json().context("parsing GitHub API response")
}

/// Download, verify, and install the binary from `release`.
fn install_release(agent: &ureq::Agent, release: &Release) -> Result<()> {
    let target = platform_target().ok_or_else(|| {
        anyhow!(
            "no prebuilt binary is published for this platform ({} {}).\n\
             Reinstall from source instead: https://github.com/{REPO}",
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
    })?;
    let tag = &release.tag_name; // e.g. "v0.1.0"
    let archive_name = format!("memo-{tag}-{target}.{}", archive_ext());
    let sums_name = format!("memo-{tag}-SHA256SUMS.txt");

    let archive_url = asset_url(release, &archive_name)
        .ok_or_else(|| anyhow!("release {tag} has no asset named {archive_name}"))?;

    let work = TempDir::new()?;
    let archive_path = work.path().join(&archive_name);

    println!("Downloading {archive_name} ...");
    download(agent, &archive_url, &archive_path)
        .with_context(|| format!("downloading {archive_name}"))?;

    // Verify the checksum, best-effort — exactly like install.sh, a missing sums
    // file or sha tool warns and proceeds rather than aborting the update.
    match asset_url(release, &sums_name) {
        Some(sums_url) => {
            let sums_path = work.path().join(&sums_name);
            if download(agent, &sums_url, &sums_path).is_ok() {
                verify_checksum(&archive_path, &archive_name, &sums_path)?;
            } else {
                eprintln!("warning: could not download {sums_name} — skipping checksum check");
            }
        }
        None => eprintln!("warning: release {tag} has no SHA256SUMS — skipping checksum check"),
    }

    let new_bin = extract_binary(&archive_path, work.path())?;
    let current = current_exe()?;
    replace_executable(&new_bin, &current)
        .with_context(|| format!("installing the new binary over {}", current.display()))?;
    Ok(())
}

/// Find a release asset by exact name and return its download URL.
fn asset_url(release: &Release, name: &str) -> Option<String> {
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.browser_download_url.clone())
}

/// Stream `url` to `dest`. ureq follows the GitHub redirect to object storage.
fn download(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<()> {
    let resp = match agent.get(url).set("User-Agent", USER_AGENT).call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) => bail!("HTTP {code} fetching {url}"),
        Err(e) => return Err(anyhow!("could not fetch {url}: {e}")),
    };
    let mut reader = resp.into_reader();
    let mut file =
        File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(())
}

/// Verify `archive`'s SHA-256 against the line for `archive_name` in `sums_path`.
/// A missing entry or sha tool is a warning, not an error (mirrors install.sh);
/// only an actual mismatch aborts.
fn verify_checksum(archive: &Path, archive_name: &str, sums_path: &Path) -> Result<()> {
    let sums = fs::read_to_string(sums_path).context("reading SHA256SUMS")?;
    let expected = sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        // sha256sum writes "<hash>  <name>"; tolerate a leading '*' (binary mode).
        let name = parts.next()?.trim_start_matches('*');
        (name == archive_name).then(|| hash.to_ascii_lowercase())
    });
    let Some(expected) = expected else {
        eprintln!("warning: {archive_name} not listed in SHA256SUMS — skipping checksum check");
        return Ok(());
    };
    let Some(actual) = sha256_hex(archive) else {
        eprintln!("warning: no sha256 tool (sha256sum/shasum) found — skipping checksum check");
        return Ok(());
    };
    if actual != expected {
        bail!("checksum mismatch for {archive_name}: expected {expected}, got {actual}");
    }
    println!("Checksum verified.");
    Ok(())
}

/// SHA-256 of a file as lowercase hex, via the system `sha256sum` or `shasum`.
/// Returns `None` when neither tool is available.
fn sha256_hex(path: &Path) -> Option<String> {
    let candidates: [(&str, &[&str]); 2] =
        [("sha256sum", &[]), ("shasum", &["-a", "256"])];
    for (cmd, args) in candidates {
        if let Ok(out) = Command::new(cmd).args(args).arg(path).output() {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(hash) = stdout.split_whitespace().next() {
                    return Some(hash.to_ascii_lowercase());
                }
            }
        }
    }
    None
}

/// Extract the `memo` binary from `archive` into `dest`, returning its path.
/// Uses the system `tar`, which auto-detects gzip and (as bsdtar on macOS and
/// Windows 10+) also reads zip — covering every (platform, archive) pair we ship.
fn extract_binary(archive: &Path, dest: &Path) -> Result<PathBuf> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .context("running `tar` to unpack the archive (is `tar` installed?)")?;
    if !status.success() {
        bail!("tar failed to unpack {}", archive.display());
    }
    let bin = dest.join(binary_name());
    if !bin.exists() {
        bail!("archive did not contain the expected `{}` binary", binary_name());
    }
    Ok(bin)
}

/// The running executable's real path (symlinks resolved).
fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the running memo executable")?;
    Ok(fs::canonicalize(&exe).unwrap_or(exe))
}

/// Atomically swap the running executable for `new_bin` (Unix: rename over the
/// old inode, which the live process keeps until it exits).
#[cfg(unix)]
fn replace_executable(new_bin: &Path, current: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = current.parent().unwrap_or_else(|| Path::new("."));
    // Stage in the destination directory so the final rename is same-filesystem
    // (atomic) and can never leave a half-written executable behind.
    let staged = dir.join(format!(".memo-update-{}", uuid::Uuid::new_v4()));
    fs::copy(new_bin, &staged).with_context(|| writable_hint(dir))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    fs::rename(&staged, current).map_err(|e| {
        let _ = fs::remove_file(&staged);
        anyhow!("{e}\n{}", writable_hint(dir))
    })?;
    Ok(())
}

/// Windows can't overwrite a running `.exe`, but it can rename it: move the old
/// one aside, drop the new one in its place, then best-effort remove the backup.
#[cfg(windows)]
fn replace_executable(new_bin: &Path, current: &Path) -> Result<()> {
    let dir = current.parent().unwrap_or_else(|| Path::new("."));
    let backup = dir.join("memo.exe.old");
    let _ = fs::remove_file(&backup); // clear a leftover from a previous update
    fs::rename(current, &backup).with_context(|| writable_hint(dir))?;
    if let Err(e) = fs::copy(new_bin, current) {
        let _ = fs::rename(&backup, current); // roll back so memo still works
        return Err(anyhow!("{e}\n{}", writable_hint(dir)));
    }
    // The old exe is still mapped while we run; cleanup may fail — that's fine,
    // the next update clears it.
    let _ = fs::remove_file(&backup);
    Ok(())
}

fn writable_hint(dir: &Path) -> String {
    format!(
        "cannot write to {} — is it writable? If memo is installed system-wide, \
         re-run the installer, or move memo to a directory you own (e.g. ~/.local/bin).",
        dir.display()
    )
}

/// Release target triple for this platform, or `None` if no prebuilt is shipped.
/// Kept in sync with the matrix in `.github/workflows/release.yml`.
fn platform_target() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        // Releases ship one universal (arm64 + x86_64) macOS binary.
        Some("universal-apple-darwin")
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "x86_64") {
            Some("x86_64-unknown-linux-gnu")
        } else if cfg!(target_arch = "aarch64") {
            Some("aarch64-unknown-linux-gnu")
        } else {
            None
        }
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("x86_64-pc-windows-msvc")
    } else {
        None
    }
}

fn archive_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

fn binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "memo.exe"
    } else {
        "memo"
    }
}

/// Parse a release tag (`v0.1.0`, `v0.2.0-rc1`) into a semver [`Version`].
fn parse_tag(tag: &str) -> Result<Version> {
    Ok(Version::parse(tag.trim_start_matches('v'))?)
}

/// A temp directory removed on drop (avoids pulling in the `tempfile` crate).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<TempDir> {
        let path = std::env::temp_dir().join(format!("memo-update-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path)
            .with_context(|| format!("creating temp dir {}", path.display()))?;
        Ok(TempDir { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_tag_strips_v_and_reads_prerelease() {
        assert_eq!(parse_tag("v0.1.0").unwrap(), Version::new(0, 1, 0));
        let rc = parse_tag("v0.2.0-rc1").unwrap();
        assert_eq!((rc.major, rc.minor, rc.patch), (0, 2, 0));
        assert!(!rc.pre.is_empty());
        // Tags without a leading `v` parse too.
        assert_eq!(parse_tag("1.4.2").unwrap(), Version::new(1, 4, 2));
        assert!(parse_tag("not-a-version").is_err());
    }

    #[test]
    fn semver_orders_prerelease_below_release() {
        // A pre-release is older than its final release, so updating from an rc to
        // the stable of the same number is an upgrade.
        let rc = parse_tag("v0.1.0-rc1").unwrap();
        let stable = parse_tag("v0.1.0").unwrap();
        assert!(rc < stable);
        assert!(parse_tag("v0.1.0").unwrap() < parse_tag("v0.2.0").unwrap());
    }

    #[test]
    fn platform_target_is_known_for_this_build() {
        // The host running these tests is a supported platform.
        assert!(platform_target().is_some());
    }

    #[test]
    fn extract_binary_pulls_memo_from_a_tarball() {
        // Skip on Windows, where the shipped archive is a zip, not the tar.gz we
        // build here (the host `tar` may not read zip in this synthetic case).
        if cfg!(windows) {
            return;
        }
        let work = TempDir::new().unwrap();
        let dir = work.path();
        // Stage a fake `memo` binary and tar+gzip it the same way release.yml does.
        fs::write(dir.join(binary_name()), b"#!/bin/sh\necho hi\n").unwrap();
        let archive = dir.join("memo-vtest.tar.gz");
        let ok = Command::new("tar")
            .arg("-C")
            .arg(dir)
            .arg("-czf")
            .arg(&archive)
            .arg(binary_name())
            .status()
            .unwrap()
            .success();
        assert!(ok, "tar should create the archive");
        // Extract into a clean directory and confirm the binary appears.
        let out = TempDir::new().unwrap();
        let bin = extract_binary(&archive, out.path()).unwrap();
        assert_eq!(bin, out.path().join(binary_name()));
        assert!(bin.exists());
    }

    #[test]
    fn verify_checksum_accepts_match_and_rejects_mismatch() {
        let work = TempDir::new().unwrap();
        let archive = work.path().join("memo-vtest.tar.gz");
        fs::write(&archive, b"some archive bytes").unwrap();
        // Compute the real digest with the same tool verify_checksum uses; if no
        // sha tool exists on this host there's nothing to assert.
        let Some(real) = sha256_hex(&archive) else {
            return;
        };

        let sums = work.path().join("SHA256SUMS.txt");
        let mut f = File::create(&sums).unwrap();
        writeln!(f, "{real}  memo-vtest.tar.gz").unwrap();
        drop(f);
        // Matching digest verifies cleanly.
        verify_checksum(&archive, "memo-vtest.tar.gz", &sums).unwrap();

        // A wrong digest must abort.
        let bad = "0".repeat(64);
        let sums_bad = work.path().join("SHA256SUMS.bad.txt");
        fs::write(&sums_bad, format!("{bad}  memo-vtest.tar.gz\n")).unwrap();
        assert!(verify_checksum(&archive, "memo-vtest.tar.gz", &sums_bad).is_err());

        // An archive not listed in the sums file is skipped, not failed.
        verify_checksum(&archive, "memo-other.tar.gz", &sums).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replace_executable_swaps_contents_and_sets_mode() {
        use std::os::unix::fs::PermissionsExt;
        let work = TempDir::new().unwrap();
        let current = work.path().join("memo");
        fs::write(&current, b"old binary").unwrap();
        let new_bin = work.path().join("memo-new");
        fs::write(&new_bin, b"new binary").unwrap();

        replace_executable(&new_bin, &current).unwrap();
        assert_eq!(fs::read(&current).unwrap(), b"new binary");
        let mode = fs::metadata(&current).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        // No staging files left behind in the directory.
        let leftover = fs::read_dir(work.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with(".memo-update-"));
        assert!(!leftover, "staging file should be renamed away, not left behind");
    }
}
