#!/usr/bin/env pwsh
# Launch the built Tauri app, watch it, and record everything needed to explain
# a missing window or a dead CDP endpoint on a Windows runner: robust process
# tables (tasklist, not computed Get-Process properties), exit codes, the
# WebView2 user-data folder, port state, the application event log and desktop
# screenshots.
#
# If the first launch dies quickly it is retried once without
# WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS, to tell "the app cannot start here"
# apart from "the debugging flags break it".
#
# Writes shots/windows-probe.txt and sets CDP_UP=0|1 in $env:GITHUB_ENV for
# later steps. It never fails on its own; visual.yml's Verdict step decides.
#
#   pwsh tools/visual/windows-probe.ps1 -Seconds 90
param(
    [string]$Exe = 'target\debug\writan-app.exe',
    [string]$OutDir = 'shots',
    [int]$CdpPort = 9222,
    [int]$Seconds = 90
)

$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force $OutDir | Out-Null
$exePath = (Resolve-Path $Exe).Path
$logPath = Join-Path $OutDir 'windows-probe.txt'
Remove-Item $logPath -ErrorAction SilentlyContinue

function Log([string]$Message) {
    $line = '{0} {1}' -f (Get-Date -Format 'HH:mm:ss.fff'), $Message
    Add-Content -Path $logPath -Value $line
    Write-Host $line
}

function ProcessSummary {
    $parts = @()
    foreach ($image in 'writan-app.exe', 'msedgewebview2.exe') {
        $out = (tasklist /FI "IMAGENAME eq $image" /NH 2>&1 | Out-String).Trim()
        $parts += "$image -> " + (($out -split "`r?`n" | Where-Object { $_.Trim() }) -join ' ; ')
    }
    $parts -join ' || '
}

function CdpUp {
    try {
        Invoke-RestMethod "http://127.0.0.1:$CdpPort/json/version" -TimeoutSec 2 | Out-Null
        $true
    } catch { $false }
}

function Shot([string]$Name) {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing -ErrorAction SilentlyContinue
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object Drawing.Bitmap $bounds.Width, $bounds.Height
    $g = [Drawing.Graphics]::FromImage($bitmap)
    $g.CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
    $bitmap.Save((Join-Path $OutDir $Name))
    Log "screenshot $Name ($($bounds.Width)x$($bounds.Height))"
}

function Watch([string]$Label, [int]$Duration) {
    $stdout = Join-Path $OutDir "$Label-stdout.txt"
    $stderr = Join-Path $OutDir "$Label-stderr.txt"
    $process = Start-Process -FilePath $exePath -WorkingDirectory (Split-Path $exePath) -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    Log "[$Label] launched pid=$($process.Id)"
    $t0 = Get-Date
    $result = [pscustomobject]@{ Label = $Label; Alive = $true; ExitCode = $null; Cdp = $false; Seconds = 0 }
    $marks = [System.Collections.Generic.Queue[int]]::new()
    foreach ($mark in 3, 10, 20, 40, 70) { $marks.Enqueue($mark) }
    $earlyShot = $false
    while (((Get-Date) - $t0).TotalSeconds -lt $Duration) {
        $t = [int]((Get-Date) - $t0).TotalSeconds
        $result.Seconds = $t
        $alive = -not $process.HasExited
        if ($marks.Count -and $t -ge $marks.Peek()) {
            $marks.Dequeue() | Out-Null
            $exit = if ($alive) { '-' } else { $process.ExitCode }
            Log "[$Label] t+$t s alive=$alive exit=$exit cdp=$(CdpUp) :: $(ProcessSummary)"
        }
        if (-not $alive) {
            $result.Alive = $false
            $result.ExitCode = $process.ExitCode
            Log "[$Label] process exited around t+$t s with exit code $($process.ExitCode)"
            break
        }
        if (-not $result.Cdp -and (CdpUp)) {
            $result.Cdp = $true
            Log "[$Label] CDP endpoint is up at t+$t s"
        }
        if (-not $earlyShot -and $t -ge 15) { Shot "$Label-desktop-early.png"; $earlyShot = $true }
        if ($result.Cdp -and $earlyShot) { Log "[$Label] window shot taken and CDP reachable; watching stops here so the driver can take over"; break }
        Start-Sleep 2
    }
    if ($result.Alive) { Log "[$Label] still alive when watching stopped at t+$($result.Seconds) s; cdp=$($result.Cdp)" }
    foreach ($file in @($stdout, $stderr)) {
        Log "[$Label] --- $file"
        $tail = (Get-Content $file -Tail 60 -ErrorAction SilentlyContinue | Out-String).Trim()
        if ($tail) { foreach ($line in $tail -split "`r?`n") { Log "    $line" } } else { Log '    (empty)' }
    }
    return $result
}

Log "probe start on $env:COMPUTERNAME; session='$env:SESSIONNAME'; user=$(whoami)"
Log "exe=$exePath"
Log "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'"
Log ('before launch: ' + (ProcessSummary))

$first = Watch 'attempt1' $Seconds
$cdp = $first.Cdp

if (-not $first.Alive -and $first.Seconds -lt 30) {
    Log 'first launch died quickly; retrying without WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'
    $saved = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
    Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
    $second = Watch 'attempt2-no-cdp-args' ([Math]::Min(45, $Seconds))
    if ($saved) { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $saved }
    if ($second.Alive) { Log 'without the CDP flags the app stays alive -> the flags are the problem' }
    else { Log "without the CDP flags it still exits (code $($second.ExitCode))" }
}

Shot 'desktop-late.png'

$eb = Join-Path $env:LOCALAPPDATA 'com.writan.desktop\EBWebView'
if (Test-Path $eb) {
    $files = @(Get-ChildItem $eb -Recurse -File -ErrorAction SilentlyContinue)
    $newest = $files | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    Log "EBWebView: $($files.Count) files, newest $($newest.LastWriteTime) ($($newest.Name))"
} else {
    Log "EBWebView missing at $eb"
}

$events = Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = (Get-Date).AddMinutes(-10) } -ErrorAction SilentlyContinue |
    Where-Object { $_.ProviderName -match 'Application Error|Windows Error|\.NET|Hang' -or $_.Message -match 'writan' }
foreach ($event in $events) {
    $message = ($event.Message -replace "`r?`n", ' ')
    Log ("event {0} [{1}] {2}" -f $event.TimeCreated, $event.ProviderName, $message.Substring(0, [Math]::Min(400, $message.Length)))
}

Log "probe done; cdp=$cdp"
if ($env:GITHUB_ENV) { "CDP_UP=$(if ($cdp) { '1' } else { '0' })" | Add-Content -Path $env:GITHUB_ENV }
