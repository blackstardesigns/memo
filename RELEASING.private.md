# Releasing `memo` (maintainer runbook — PRIVATE)

This file lives only in the private dev repo. The `*.private.*` rule in
`.gitattributes` keeps it out of the published source.

## How it works

```
PRIVATE repo (blackstardesigns/ai-note-taker)     PUBLIC repo (vars.PUBLIC_REPO)
──────────────────────────────────────────       ──────────────────────────────
 develop here                                      main = one commit per release
 Actions → "Publish to public repo"  ─dispatch─┐   .github/workflows/release.yml
   (.github/workflows/publish.yml)             │
   1. git archive HEAD  (drops export-ignore'd files)
   2. clone public repo with the PAT
   3. wipe tree, commit "Release vX.Y.Z", tag vX.Y.Z
   4. push main + tag ──(PAT auth triggers CI)──►  builds 4 targets + macOS universal,
                                                    publishes GitHub Release + SHA256SUMS
                                                    (free public-repo Actions minutes)
```

- **Privacy:** only the committed source survives, minus everything marked
  `export-ignore` in `.gitattributes` (`.claude/`, `publish.yml`, `*.private.*`).
  No dev history, branch names, or commit messages reach the public repo.
- **Why a PAT and not GITHUB_TOKEN:** a push made with `GITHUB_TOKEN` cannot
  trigger another workflow. The PAT-authenticated push to the public repo is what
  fires `release.yml` there.
- **Why build in the public repo:** public repos get free standard-runner minutes
  (macOS is otherwise billed at 10×). `publish.yml` is only a few seconds of git.

## One-time setup

1. **Fine-grained PAT** — github.com → Settings → Developer settings →
   Fine-grained tokens → Generate:
   - Resource owner: your account.
   - Repository access: **Only select repositories** → the public repo only.
   - Permissions: **Contents → Read and write** AND **Workflows → Read and write**
     (Workflows write is required to push `.github/workflows/release.yml`; without
     it the push is rejected). Metadata read is added automatically.
2. **In this private repo** → Settings → Secrets and variables → Actions:
   - Secret `PUBLIC_REPO_TOKEN` = the PAT.
   - Variable `PUBLIC_REPO` = the public repo as `owner/name`.
3. If the public repo already has commits on `main`, nothing else is needed; if
   it is empty, `publish.yml` creates `main` automatically on the first run.

## Cutting a release

1. On `develop`, bump `version` in `Cargo.toml`.
2. `cargo build` (refreshes `Cargo.lock` so the public build's `--locked` passes).
3. Commit: `git commit -am "release: vX.Y.Z"`.
4. Run the publish workflow:
   - Actions tab → **Publish to public repo** → Run workflow → enter the version, or
   - `gh workflow run publish.yml -f version=X.Y.Z`
   - First time? tick **dry_run** to preview the snapshot without pushing.
5. The public tag push starts `release.yml`. Watch it at
   `https://github.com/<PUBLIC_REPO>/actions`. On success the release at
   `…/releases/latest` has 5 archives (macOS aarch64 / x86_64 / universal, Linux
   x86_64, Windows x86_64) plus `memo-vX.Y.Z-SHA256SUMS.txt`.

Pre-release: use a hyphenated version, e.g. `0.2.0-rc1`. `release.yml` marks any
hyphenated tag as a GitHub pre-release.

## Verify before the first real push

```sh
# Exactly what would be published — confirm no .claude/, publish.yml, or *.private.*:
git archive HEAD | tar -t | sort
```

## Troubleshooting

- **Public `release.yml` didn't run after publish.** The PAT is missing the
  **Workflows: write** permission, or expired. Re-issue the PAT.
- **`publish.yml` push rejected (403).** PAT not scoped to the public repo, or
  lacks Contents: write.
- **`PUBLIC_REPO`/`PUBLIC_REPO_TOKEN` not set.** `publish.yml` fails early with a
  clear message — add them in Actions settings.
- **`cargo build --locked` fails in the public build.** `Cargo.lock` wasn't
  committed after the version bump. Re-run step 2–3.
- **`release.yml` ran in this private repo.** Only happens if a `v*` tag is pushed
  here directly; the `github.repository != 'blackstardesigns/ai-note-taker'` guard
  skips the jobs. Don't push `v*` tags to the private repo — use `publish.yml`.

## Adding a platform target

Add a row to the `matrix.include` in `.github/workflows/release.yml` (and any
needed system-deps step). Native deps: `whisper-rs` compiles whisper.cpp via CMake
(any C++ toolchain), and `cpal` uses CoreAudio (macOS) / WASAPI (Windows) / ALSA
(Linux, `libasound2-dev` to build, `libasound2` at runtime).
