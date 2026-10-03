<#
.SYNOPSIS
  Dumps, checks and drives Apollo's UI Automation tree: what Narrator, voice control and other
  assistive technology see.

.DESCRIPTION
  GPUI builds its accessibility tree (AccessKit) only once a UI Automation client asks for it,
  so this script is also how the tree gets switched on. Needs Windows PowerShell 5.1:

    powershell.exe -NoProfile -File scripts\uia-dump.ps1
    powershell.exe -NoProfile -File scripts\uia-dump.ps1 -Check
    powershell.exe -NoProfile -File scripts\uia-dump.ps1 -Invoke Library -Check

  The window is found by process name (default "apollo"). Nothing is typed or clicked at screen
  coordinates: -Invoke and -Focus go through UI Automation patterns, so they work with another
  window in front and never send input to it.

  -Check   Fails (exit 1) if any keyboard-focusable or invokable element has no name, or any
           element that takes focus has no role beyond a bare pane.
  -Invoke  Name of an element to activate (its Invoke, SelectionItem or Toggle pattern) before
           dumping; a comma-separated list walks several in order: -Invoke Search,Library.
  -Focus   Name of an element to give keyboard focus through UI Automation before dumping.
  -Quiet   Print only the summary and any problems.
#>
param(
    [string]$Process = 'apollo',
    [switch]$Check,
    [string[]]$Invoke = @(),
    [string]$Focus = '',
    [switch]$Quiet
)

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$AE = [System.Windows.Automation.AutomationElement]

$proc = Get-Process -Name $Process -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no '$Process' window"; exit 2 }
$window = $AE::FromHandle($proc.MainWindowHandle)

function Find-ByName([string]$name) {
    $cond = New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $name)
    $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

foreach ($name in $Invoke) {
    $el = Find-ByName $name
    if (-not $el) { Write-Output "invoke: '$name' not found"; exit 2 }
    $pattern = $null
    if ($el.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        $pattern.Invoke()
    } elseif ($el.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
        $pattern.Select()
    } elseif ($el.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        $pattern.Toggle()
    } else {
        Write-Output "invoke: '$name' has no Invoke, SelectionItem or Toggle pattern"; exit 2
    }
    Write-Output "invoked '$name'"
    Start-Sleep -Milliseconds 700
}
if ($Focus) {
    $el = Find-ByName $Focus
    if (-not $el) { Write-Output "focus: '$Focus' not found"; exit 2 }
    $el.SetFocus()
    Write-Output "focused '$Focus'"
    Start-Sleep -Milliseconds 300
}

$walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
$rows = New-Object System.Collections.Generic.List[object]
function Walk($el, [int]$depth) {
    $c = $el.Current
    $patterns = @()
    try { $patterns = $el.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Identifiers\.Pattern$', '' -replace '^.*\.', '' } } catch {}
    $rows.Add([pscustomobject]@{
            Depth    = $depth
            Type     = ($c.ControlType.ProgrammaticName -replace '^ControlType\.', '')
            Name     = $c.Name
            Focusable = $c.IsKeyboardFocusable
            HasFocus = $c.HasKeyboardFocus
            Enabled  = $c.IsEnabled
            Offscreen = $c.IsOffscreen
            Patterns = ($patterns -join ',')
        })
    $child = $walker.GetFirstChild($el)
    while ($child) {
        Walk $child ($depth + 1)
        $child = $walker.GetNextSibling($child)
    }
}
Walk $window 0

if (-not $Quiet) {
    foreach ($r in $rows) {
        $flags = @()
        if ($r.Focusable) { $flags += 'focusable' }
        if ($r.HasFocus) { $flags += 'FOCUSED' }
        if (-not $r.Enabled) { $flags += 'disabled' }
        if ($r.Offscreen) { $flags += 'offscreen' }
        Write-Output ('{0}{1} "{2}" [{3}] {4}' -f ('  ' * $r.Depth), $r.Type, $r.Name, ($flags -join ' '), $r.Patterns)
    }
}

$focusable = @($rows | Where-Object { $_.Focusable })
$invokable = @($rows | Where-Object { $_.Patterns -match 'Invoke|Toggle|SelectionItem|RangeValue' })
$problems = New-Object System.Collections.Generic.List[string]
foreach ($r in $focusable) {
    if ([string]::IsNullOrWhiteSpace($r.Name)) { $problems.Add("focusable $($r.Type) has no name") }
    if ($r.Type -in 'Pane', 'Custom', 'Group') { $problems.Add("focusable '$($r.Name)' has only a generic role ($($r.Type))") }
}
foreach ($r in $invokable) {
    if ([string]::IsNullOrWhiteSpace($r.Name)) { $problems.Add("$($r.Type) with $($r.Patterns) has no name") }
}
Write-Output ('--- {0} elements, {1} keyboard-focusable, {2} actionable, {3} problems' -f $rows.Count, $focusable.Count, $invokable.Count, $problems.Count)
$problems | Sort-Object -Unique | ForEach-Object { Write-Output "PROBLEM: $_" }
if ($Check -and $problems.Count -gt 0) { exit 1 }
