<#
.SYNOPSIS
  Reads, and drives, Apollo's System Media Transport Controls session from outside the app.

.DESCRIPTION
  Verifies the media overlay the way Windows sees it: the title, artist, album, thumbnail,
  status and timeline another process reads from SMTC, and that a play/pause/next/previous
  request reaches the app. Needs Windows PowerShell 5.1 (the WinRT projection is not in
  PowerShell 7):

    powershell.exe -NoProfile -File scripts\smtc-probe.ps1 -Action status

  Only Apollo's session is ever read in detail or commanded. Other sessions (a browser, another
  player) are listed by app id and status only, so a run can show they were not disturbed.

  -Action  status            Apollo's session: metadata, thumbnail size, status, timeline.
           others            Every other session: app id and status only.
           play|pause|toggle|next|previous
                             The request, addressed to Apollo's session alone.
           seek -Seconds <n> The overlay's scrubber: change the position to n seconds.
           key -Vk <code>    A real media key (0xB3 play/pause, 0xB0 next, 0xB1 previous) sent
                             to Windows, which routes it. Refused unless Windows' current media
                             session is Apollo's, and Apollo is brought to the front and is the
                             foreground window, so the key can never reach another player or be
                             forwarded by a remote-desktop client.
  -Match   Substring of the session's app id that identifies Apollo (default "apollo").
#>
param(
    [Parameter(Mandatory)]
    [ValidateSet('status', 'others', 'play', 'pause', 'toggle', 'next', 'previous', 'seek', 'key')]
    [string]$Action,
    [string]$Match = 'apollo',
    [int]$Vk = 0,
    [double]$Seconds = 0
)

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStreamReference, Windows.Storage.Streams, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStreamWithContentType, Windows.Storage.Streams, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.DataReader, Windows.Storage.Streams, ContentType = WindowsRuntime]

$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
        $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
        $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, $type) {
    $task = $asTask.MakeGenericMethod($type).Invoke($null, @($op))
    $task.Wait(-1) | Out-Null
    $task.Result
}
function AwaitBool($op) { Await $op ([bool]) }

$manager = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) `
    ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
$sessions = @($manager.GetSessions())
$apollo = $sessions | Where-Object { $_.SourceAppUserModelId -like "*$Match*" } | Select-Object -First 1
$others = @($sessions | Where-Object { $_.SourceAppUserModelId -notlike "*$Match*" })

function Describe-Apollo {
    if (-not $apollo) { Write-Output 'apollo session: NONE'; return }
    $props = Await ($apollo.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])
    $info = $apollo.GetPlaybackInfo()
    $tl = $apollo.GetTimelineProperties()
    $thumbBytes = 0
    if ($props.Thumbnail) {
        # The stream's Size is not reliable for a thumbnail another process owns: count what reads.
        try {
            $stream = Await ($props.Thumbnail.OpenReadAsync()) ([Windows.Storage.Streams.IRandomAccessStreamWithContentType])
            $thumbType = $stream.ContentType
            $asRead = [System.IO.WindowsRuntimeStreamExtensions].GetMethods() | Where-Object {
                $_.Name -eq 'AsStreamForRead' -and $_.GetParameters().Count -eq 1 -and
                $_.GetParameters()[0].ParameterType.Name -eq 'IInputStream' } | Select-Object -First 1
            $net = $asRead.Invoke($null, @($stream))
            $ms = New-Object System.IO.MemoryStream
            $net.CopyTo($ms)
            $thumbBytes = $ms.Length
        } catch { $thumbBytes = -1; $thumbType = $_.Exception.Message }
    }
    Write-Output ("app id      : {0}" -f $apollo.SourceAppUserModelId)
    Write-Output ("title       : {0}" -f $props.Title)
    Write-Output ("artist      : {0}" -f $props.Artist)
    Write-Output ("album       : {0}" -f $props.AlbumTitle)
    Write-Output ("thumbnail   : {0}" -f $(if ($props.Thumbnail) { "present, $thumbBytes bytes $thumbType" } else { 'none' }))
    Write-Output ("status      : {0}" -f $info.PlaybackStatus)
    Write-Output ("controls    : play={0} pause={1} next={2} prev={3} seek={4}" -f $info.Controls.IsPlayEnabled, $info.Controls.IsPauseEnabled, $info.Controls.IsNextEnabled, $info.Controls.IsPreviousEnabled, $info.Controls.IsPlaybackPositionEnabled)
    Write-Output ("timeline    : position={0:N1}s start={1:N1}s end={2:N1}s" -f $tl.Position.TotalSeconds, $tl.StartTime.TotalSeconds, $tl.EndTime.TotalSeconds)
}

switch ($Action) {
    'status' { Describe-Apollo }
    'others' {
        foreach ($s in $others) {
            Write-Output ("{0} -> {1}" -f $s.SourceAppUserModelId, $s.GetPlaybackInfo().PlaybackStatus)
        }
        if (-not $others) { Write-Output '(no other sessions)' }
    }
    { $_ -in 'play', 'pause', 'toggle', 'next', 'previous', 'seek' } {
        if (-not $apollo) { Write-Output 'apollo session: NONE'; exit 2 }
        $ok = switch ($Action) {
            'play' { AwaitBool ($apollo.TryPlayAsync()) }
            'pause' { AwaitBool ($apollo.TryPauseAsync()) }
            'toggle' { AwaitBool ($apollo.TryTogglePlayPauseAsync()) }
            'next' { AwaitBool ($apollo.TrySkipNextAsync()) }
            'previous' { AwaitBool ($apollo.TrySkipPreviousAsync()) }
            'seek' { AwaitBool ($apollo.TryChangePlaybackPositionAsync([int64]($Seconds * 10000000))) }
        }
        Write-Output ("{0} -> accepted: {1}" -f $Action, $ok)
    }
    'key' {
        if ($Vk -notin 0xB3, 0xB0, 0xB1) { Write-Output 'use -Vk 0xB3 (play/pause), 0xB0 (next) or 0xB1 (previous)'; exit 2 }
        $current = $manager.GetCurrentSession()
        if (-not $apollo -or -not $current -or $current.SourceAppUserModelId -notlike "*$Match*") {
            Write-Output 'REFUSED: the current media session is not Apollo, so a media key would go elsewhere'
            exit 3
        }
        Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Keys {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern void SwitchToThisWindow(IntPtr h, bool altTab);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
'@
        # A key goes to whatever window has the focus first. A remote-desktop client in front
        # would forward it to the other machine, so Apollo must be the foreground window, or
        # nothing is sent.
        $proc = Get-Process | Where-Object { $_.ProcessName -like "*$Match*" -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1
        if (-not $proc) { Write-Output 'REFUSED: no Apollo window'; exit 3 }
        [void][Keys]::ShowWindow($proc.MainWindowHandle, 9)
        [void][Keys]::SetForegroundWindow($proc.MainWindowHandle)
        [Keys]::SwitchToThisWindow($proc.MainWindowHandle, $true)
        Start-Sleep -Milliseconds 400
        $fgPid = 0
        [void][Keys]::GetWindowThreadProcessId([Keys]::GetForegroundWindow(), [ref]$fgPid)
        if ($fgPid -ne $proc.Id) {
            Write-Output ('REFUSED: could not bring Apollo to the front (foreground is {0})' -f (Get-Process -Id $fgPid).ProcessName)
            exit 3
        }
        # Media keys are extended keys (KEYEVENTF_EXTENDEDKEY = 1, KEYEVENTF_KEYUP = 2).
        $scan = [byte][Keys]::MapVirtualKey([uint32]$Vk, 0)
        [Keys]::keybd_event([byte]$Vk, $scan, 1, [UIntPtr]::Zero)
        [Keys]::keybd_event([byte]$Vk, $scan, 3, [UIntPtr]::Zero)
        Write-Output ("key 0x{0:X2} sent to the foreground window (Apollo)" -f $Vk)
    }
}
