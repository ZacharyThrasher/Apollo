# Installs the newest published Apollo (GitHub Releases) into a folder (default C:\Apollo) with
# Start Menu and Desktop shortcuts, without building anything. From then on the app updates
# itself (Settings > Window > Install updates automatically).
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-latest.ps1 [-Dest C:\Apollo] [-NoShortcuts]
#
# Signed-in sessions live in %LOCALAPPDATA%\Apollo and are untouched.
param(
    [string]$Dest = 'C:\Apollo',
    [switch]$NoShortcuts
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repo = 'ZacharyThrasher/Apollo'

$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers @{ 'User-Agent' = 'Apollo-install' }
$version = $release.tag_name.TrimStart('v')
$name = "Apollo-$version-win-x64.zip"
$zipUrl = ($release.assets | Where-Object name -eq $name).browser_download_url
$sumUrl = ($release.assets | Where-Object name -eq "$name.sha256").browser_download_url
if (-not $zipUrl -or -not $sumUrl) { throw "release $version has no $name" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) "apollo-install-$version"
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $tmp | Out-Null
$zip = Join-Path $tmp $name
Invoke-WebRequest $zipUrl -OutFile $zip -UseBasicParsing
$want = ((Invoke-WebRequest $sumUrl -UseBasicParsing).Content -split '\s+')[0].ToLower()
if ((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower() -ne $want) { throw 'download does not match its checksum' }

# Stop a running copy so its files can be replaced; never touches anything else.
Get-Process -Name 'Apollo', 'apollo-login-helper', 'apollo-resolve-bench' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($Dest, [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object { $_.CloseMainWindow() | Out-Null; if (-not $_.WaitForExit(5000)) { $_.Kill() } }

New-Item -ItemType Directory -Force $Dest | Out-Null
Expand-Archive $zip $Dest -Force
Remove-Item $tmp -Recurse -Force

if (-not $NoShortcuts) {
    $exe = Join-Path $Dest 'Apollo.exe'
    $shell = New-Object -ComObject WScript.Shell
    $targets = @(
        (Join-Path ([Environment]::GetFolderPath('Programs')) 'Apollo.lnk'),
        (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Apollo.lnk')
    )
    $oneDrive = Join-Path $env:USERPROFILE 'OneDrive\Desktop'
    if (Test-Path $oneDrive) { $targets += (Join-Path $oneDrive 'Apollo.lnk') }
    foreach ($lnk in ($targets | Select-Object -Unique)) {
        $s = $shell.CreateShortcut($lnk)
        $s.TargetPath = $exe
        $s.WorkingDirectory = $Dest
        $s.IconLocation = "$exe,0"
        $s.Description = 'Apollo, a YouTube Music player'
        $s.Save()
        Write-Host "shortcut: $lnk"
    }
}
Write-Host "`nInstalled Apollo $version to $Dest"
