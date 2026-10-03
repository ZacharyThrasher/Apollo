<#
.SYNOPSIS
  Saves a PNG of Apollo's window alone (PrintWindow on its handle). Never the desktop, never
  another window, so nothing else on the screen can end up in the file.

  Also sends keys to that window only (PostMessage to its handle), so it works while another
  application, such as a remote-desktop client, has the focus and nothing reaches that
  application. Windows PowerShell 5.1:

    powershell.exe -NoProfile -File scripts\window-shot.ps1 -Out shot.png
    powershell.exe -NoProfile -File scripts\window-shot.ps1 -Keys Tab,Tab,Tab -Out ring.png
    powershell.exe -NoProfile -File scripts\window-shot.ps1 -Keys Shift+Tab -Out back.png
#>
param(
    [string]$Process = 'apollo',
    [string]$Out = '',
    [string[]]$Keys = @()
)

Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing @'
using System; using System.Runtime.InteropServices; using System.Drawing; using System.Drawing.Imaging;
public static class Shot {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint type);
  public static void Save(IntPtr h, string path) {
    RECT r; GetWindowRect(h, out r);
    using (var bmp = new Bitmap(r.R - r.L, r.B - r.T, PixelFormat.Format32bppArgb))
    using (var g = Graphics.FromImage(bmp)) {
      IntPtr hdc = g.GetHdc();
      PrintWindow(h, hdc, 2);   // PW_RENDERFULLCONTENT: includes the DirectComposition surface
      g.ReleaseHdc(hdc);
      bmp.Save(path, ImageFormat.Png);
    }
  }
}
'@

$proc = Get-Process -Name $Process -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no '$Process' window"; exit 2 }
$hwnd = $proc.MainWindowHandle

$codes = @{ Tab = 0x09; Enter = 0x0D; Escape = 0x1B; Space = 0x20; Left = 0x25; Up = 0x26; Right = 0x27; Down = 0x28 }
function Send-Key([string]$spec) {
    $shift = $false
    if ($spec -like 'Shift+*') { $shift = $true; $spec = $spec.Substring(6) }
    $vk = if ($codes.ContainsKey($spec)) { $codes[$spec] } else { [int][char]$spec.ToUpper() }
    $scan = [Shot]::MapVirtualKey($vk, 0)
    if ($shift) { [void][Shot]::PostMessage($hwnd, 0x100, [IntPtr]0x10, [IntPtr](1 -bor (0x2A -shl 16))) }
    [void][Shot]::PostMessage($hwnd, 0x100, [IntPtr]$vk, [IntPtr](1 -bor ($scan -shl 16)))
    Start-Sleep -Milliseconds 60
    [void][Shot]::PostMessage($hwnd, 0x101, [IntPtr]$vk, [IntPtr](1 -bor ($scan -shl 16) -bor 0xC0000000))
    if ($shift) { [void][Shot]::PostMessage($hwnd, 0x101, [IntPtr]0x10, [IntPtr](1 -bor (0x2A -shl 16) -bor 0xC0000000)) }
    Start-Sleep -Milliseconds 220
}
# `-File` hands a comma list over as one string.
foreach ($k in ($Keys -split ',')) { if ($k) { Send-Key $k.Trim() } }
if ($Out) {
    Start-Sleep -Milliseconds 300
    [Shot]::Save($hwnd, $Out)
    Write-Output ('saved {0} ({1} bytes)' -f $Out, (Get-Item $Out).Length)
}
