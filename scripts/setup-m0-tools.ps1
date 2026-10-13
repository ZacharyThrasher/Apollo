# Reproduces the Milestone 0 native tools under <repo>/tools (gitignored). Hashes are the
# GitHub-published SHA-256 values recorded when the baseline was made; the script refuses
# anything that does not match.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = Split-Path $PSScriptRoot -Parent
$t = Join-Path $root 'tools'; $d = Join-Path $t 'dl'
New-Item -ItemType Directory -Force $d | Out-Null
$items = @(
  @{n='mpv-dev-x86_64-20261002-git-3186d369f9.7z'; u='https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261002/mpv-dev-x86_64-20261002-git-3186d369f9.7z'; h='d873450cc1a7f881a8a10c33936d9555ad18a3809d827b9dbea55ba55caebcf9'},
  @{n='yt-dlp.exe'; u='https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.exe'; h='66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a'},
  @{n='deno-x86_64-pc-windows-msvc.zip'; u='https://github.com/denoland/deno/releases/download/v2.9.7/deno-x86_64-pc-windows-msvc.zip'; h='a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238'}
)
foreach ($i in $items) {
  $p = Join-Path $d $i.n
  if (-not (Test-Path $p)) { Invoke-WebRequest -Uri $i.u -OutFile $p -UseBasicParsing }
  if ((Get-FileHash $p -Algorithm SHA256).Hash.ToLower() -ne $i.h) { throw "hash mismatch: $($i.n)" }
}
Expand-Archive (Join-Path $d 'deno-x86_64-pc-windows-msvc.zip') (Join-Path $t 'deno') -Force
New-Item -ItemType Directory -Force (Join-Path $t 'yt-dlp') | Out-Null
Copy-Item (Join-Path $d 'yt-dlp.exe') (Join-Path $t 'yt-dlp\yt-dlp.exe') -Force
$mpv = Join-Path $t 'mpv-dev'; New-Item -ItemType Directory -Force $mpv | Out-Null
& "$env:SystemRoot\System32\tar.exe" -xf (Join-Path $d 'mpv-dev-x86_64-20261002-git-3186d369f9.7z') -C $mpv

# MSVC import library for libmpv-2.dll (the package ships only a MinGW .dll.a).
# Found with vswhere (any VS edition, including a CI runner's); the Build Tools path is the fallback.
$vc = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (Test-Path $vswhere) {
  $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if ($vsPath) {
    $found = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
    if (Test-Path $found) { $vc = $found }
  }
}
cmd /c "`"$vc`" >nul 2>&1 && cd /d $mpv && dumpbin /exports libmpv-2.dll > exports.txt"
$names = Get-Content (Join-Path $mpv 'exports.txt') | ForEach-Object { if ($_ -match '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]{8}\s+(mpv_\w+)') { $Matches[1] } }
@('LIBRARY libmpv-2.dll', 'EXPORTS') + $names | Set-Content (Join-Path $mpv 'mpv.def') -Encoding ascii
cmd /c "`"$vc`" >nul 2>&1 && cd /d $mpv && lib /nologo /def:mpv.def /machine:x64 /out:mpv.lib"
Write-Host "tools ready under $t (ffmpeg only needed to regenerate fixtures/audio; see docs/build-baseline.md)"
