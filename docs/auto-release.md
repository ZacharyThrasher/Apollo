# Automatic releases and self-update

Every change that lands on `main` is built and published; installed copies of Apollo fetch it
themselves. Nobody has to ask for a release.

## Publishing (`.github/workflows/release.yml`)

- Runs on every push to `main` except docs-only ones (`docs/**`, `*.md`), on a `windows-latest`
  runner, one at a time.
- Installs the pinned tools with `scripts/setup-m0-tools.ps1` (hash-checked libmpv, yt-dlp, deno),
  runs `cargo test -p apollo-updater` (the file-swap tests, on Windows), then `scripts/package.ps1
  -Version 0.1.<run number> -NoShortcuts`, the same package a hand build makes.
- Publishes `Apollo-<version>-win-x64.zip` and `Apollo-<version>-win-x64.zip.sha256` as release
  `v<version>` with generated notes, and keeps the newest 15 releases.
- Run it from the Actions tab on any branch to check a build without publishing (the "publish"
  box only does anything on `main`).
- The repository is public: the workflow reads no secret, cookie or account. Its only token is
  the workflow's own `GITHUB_TOKEN` (contents: write, this repository).

## Updating (`crates/updater`, `crates/app/src/update.rs`)

- Only the packaged install updates itself: `Apollo.exe` with `libmpv-2.dll` and `tools\` beside it.
  A build run from `target\` never replaces itself. A local `package.ps1` build without `-Version`
  is version `0.0.0`, so the first check replaces it with the newest release.
- 45 s after launch, then every 4 hours, Apollo asks GitHub for the latest release. A newer one is
  downloaded to `%LOCALAPPDATA%\Apollo\updates\staged`, checked against its `.sha256` and
  unpacked; only then is it marked ready. The running app is not touched.
- The swap happens when Apollo is not running: at quit, a hidden copy of `Apollo.exe --apply-update`
  waits for the app to exit, renames each file in the way to `<name>.old`, copies the new one in,
  and rolls everything back if any step fails. If the quit never got to it (crash, power loss), the
  next start hands over to the same helper and it starts the new build.
- A build whose swap failed is remembered (`failed.txt`) and not downloaded again; the next release
  is tried. `updates\update.log` has one line per event (versions and outcomes only).
- Settings > Window > Install updates automatically (on). Off: no check, no download, no swap.
- `scripts/install-latest.ps1` installs the newest release into `C:\Apollo` without building, for a
  fresh PC or the first install of the updater.
- The version is shown in the diagnostics report (Settings > Data).
