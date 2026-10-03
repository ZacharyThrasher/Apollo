# Builds Apollo (release, real services) and assembles a self-contained, double-clickable
# folder (default C:\Apollo) plus Start Menu and Desktop shortcuts.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-Dest C:\Apollo] [-SkipBuild] [-NoShortcuts]
#
# Layout of the output folder (the app finds everything relative to its own exe):
#   Apollo.exe              GPUI app, windows subsystem (no console), real services by default
#   libmpv-2.dll            audio engine (loaded at startup; the exe will not start without it)
#   VCRUNTIME140*.dll       MSVC runtime copied from System32 (app-local)
#   apollo-login-helper.exe WebView2 sign-in window (needs the Evergreen WebView2 runtime)
#   apollo-resolve-bench.exe  timing run: how long click -> playable stream takes (close Apollo first)
#   tools\yt-dlp\yt-dlp.exe, tools\deno\deno.exe   pinned resolver tools (hash-checked by the app)
#
# The pinned tools come from <repo>\tools (see scripts\setup-m0-tools.ps1), or from the main
# checkout's tools\ when this runs from a git worktree. Nothing is read from or written to the
# user's account data: sessions live in %LOCALAPPDATA%\Apollo and are untouched by packaging.
param(
    [string]$Dest = 'C:\Apollo',
    [switch]$SkipBuild,
    [switch]$NoShortcuts
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Find-Tools {
    $candidates = @((Join-Path $root 'tools'))
    $common = git -C $root rev-parse --path-format=absolute --git-common-dir 2>$null
    if ($LASTEXITCODE -eq 0 -and $common) {
        $candidates += (Join-Path (Split-Path $common -Parent) 'tools')
    }
    foreach ($c in $candidates) {
        if ((Test-Path (Join-Path $c 'mpv-dev\libmpv-2.dll')) -and
            (Test-Path (Join-Path $c 'yt-dlp\yt-dlp.exe')) -and
            (Test-Path (Join-Path $c 'deno\deno.exe'))) { return (Resolve-Path $c).Path }
    }
    throw "tools\ not found (looked in: $($candidates -join ', ')). Run scripts\setup-m0-tools.ps1."
}
$tools = Find-Tools
Write-Host "tools: $tools"

if (-not $SkipBuild) {
    $env:APOLLO_LIBMPV_DIR = Join-Path $tools 'mpv-dev'
    Push-Location $root
    try {
        # Two invocations: `--bin apollo` would otherwise also filter the helper out.
        cargo build --release -p apollo-app --features real-services --bin apollo
        if ($LASTEXITCODE -ne 0) { throw 'cargo build (apollo) failed' }
        cargo build --release -p apollo-login-helper
        if ($LASTEXITCODE -ne 0) { throw 'cargo build (login helper) failed' }
        cargo build --release -p apollo-m2-probe --bin apollo-resolve-bench
        if ($LASTEXITCODE -ne 0) { throw 'cargo build (resolve bench) failed' }
    } finally { Pop-Location }
}

# The target dir can be redirected (CARGO_TARGET_DIR / config); ask cargo where it is.
$meta = cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $root 'Cargo.toml') | ConvertFrom-Json
$rel = Join-Path $meta.target_directory 'release'
foreach ($f in 'apollo.exe', 'apollo-login-helper.exe', 'apollo-resolve-bench.exe') {
    if (-not (Test-Path (Join-Path $rel $f))) { throw "missing $f in $rel (build first, or drop -SkipBuild)" }
}

# Stop a running copy so its files can be replaced; never touches anything else.
Get-Process -Name 'Apollo', 'apollo-login-helper', 'apollo-resolve-bench' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($Dest, [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object { $_.CloseMainWindow() | Out-Null; if (-not $_.WaitForExit(5000)) { $_.Kill() } }

New-Item -ItemType Directory -Force $Dest | Out-Null
Copy-Item (Join-Path $rel 'apollo.exe') (Join-Path $Dest 'Apollo.exe') -Force
Copy-Item (Join-Path $rel 'apollo-login-helper.exe') (Join-Path $Dest 'apollo-login-helper.exe') -Force
Copy-Item (Join-Path $rel 'apollo-resolve-bench.exe') (Join-Path $Dest 'apollo-resolve-bench.exe') -Force
Copy-Item (Join-Path $tools 'mpv-dev\libmpv-2.dll') (Join-Path $Dest 'libmpv-2.dll') -Force
# apollo.exe imports the MSVC runtime; ship it app-locally so a PC without the redistributable
# still starts (the redistributable's DLLs are licensed for this).
foreach ($dll in 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll') {
    $src = Join-Path $env:windir "System32\$dll"
    if (Test-Path $src) { Copy-Item $src (Join-Path $Dest $dll) -Force }
}
foreach ($sub in 'yt-dlp\yt-dlp.exe', 'deno\deno.exe') {
    $dst = Join-Path $Dest "tools\$sub"
    New-Item -ItemType Directory -Force (Split-Path $dst -Parent) | Out-Null
    Copy-Item (Join-Path $tools $sub) $dst -Force
}

if (-not $NoShortcuts) {
    $exe = Join-Path $Dest 'Apollo.exe'
    $shell = New-Object -ComObject WScript.Shell
    $targets = @(
        (Join-Path ([Environment]::GetFolderPath('Programs')) 'Apollo.lnk'),
        (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Apollo.lnk')
    )
    # OneDrive-backed Desktops are not always what the shell reports; cover the usual one too.
    $oneDrive = Join-Path $env:USERPROFILE 'OneDrive\Desktop'
    if (Test-Path $oneDrive) { $targets += (Join-Path $oneDrive 'Apollo.lnk') }
    $targets = $targets | Select-Object -Unique
    foreach ($lnk in $targets) {
        $s = $shell.CreateShortcut($lnk)
        $s.TargetPath = $exe
        $s.WorkingDirectory = $Dest
        $s.IconLocation = "$exe,0"
        $s.Description = 'Apollo, a YouTube Music player'
        $s.Save()
        Write-Host "shortcut: $lnk"
    }
}

Write-Host "`nPackaged to $Dest"
Get-ChildItem $Dest -Recurse -File | ForEach-Object {
    '{0,10:N1} MB  {1}' -f ($_.Length / 1MB), $_.FullName.Substring($Dest.Length + 1)
}
