<#
.SYNOPSIS
    Launch-and-survive smoke test for the GreenC desktop app.

.DESCRIPTION
    Starts GreenC.exe with an isolated LOCALAPPDATA/APPDATA (so it never touches the user's real
    GreenC database), waits until the main window exists, then requires the process to stay alive
    for -AliveSeconds (default 60, longer than one 45 s BackgroundGuardian tick). On a crash it
    prints stderr, which carries the Rust panic message, and exits 1.

    This exists because unit and CLI E2E tests never start the Tauri app: a panic in `setup()`
    shipped in an installer once without any test noticing.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\smoke-app.ps1 -Exe dist\GreenC\GreenC.exe
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [int]$AliveSeconds = 60,
    [int]$WindowTimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path
$Work = Join-Path ([IO.Path]::GetTempPath()) ("greenc-smoke-" + [Guid]::NewGuid().ToString("N"))
$Local = Join-Path $Work "Local"
$Roaming = Join-Path $Work "Roaming"
New-Item -ItemType Directory -Force $Local, $Roaming | Out-Null
$ErrLog = Join-Path $Work "stderr.txt"
$OutLog = Join-Path $Work "stdout.txt"

$psi = New-Object System.Diagnostics.ProcessStartInfo $Exe
$psi.UseShellExecute = $false
$psi.RedirectStandardError = $true
$psi.RedirectStandardOutput = $true
$psi.EnvironmentVariables["LOCALAPPDATA"] = $Local
$psi.EnvironmentVariables["APPDATA"] = $Roaming
$psi.EnvironmentVariables["RUST_BACKTRACE"] = "1"
# Never import the machine-wide legacy DB (real Vault archives) into the smoke profile.
$psi.EnvironmentVariables["GREENC_SKIP_LEGACY_MIGRATION"] = "1"

Write-Host "Smoke: starting $Exe (isolated data in $Work)"
$proc = [System.Diagnostics.Process]::Start($psi)
$errTask = $proc.StandardError.ReadToEndAsync()
$outTask = $proc.StandardOutput.ReadToEndAsync()

function Stop-Smoke([int]$code, [string]$message) {
    if (-not $proc.HasExited) { $proc.Kill(); $proc.WaitForExit(5000) | Out-Null }
    # Kill WebView2 children started for this run (they inherit the isolated user data folder).
    Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -like "*$Work*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    $stderr = $errTask.Result
    if ($code -ne 0) {
        Write-Host "Smoke FAILED: $message" -ForegroundColor Red
        if ($stderr) { Write-Host "---- stderr ----"; Write-Host $stderr }
    } else {
        Write-Host "Smoke OK: $message" -ForegroundColor Green
    }
    Remove-Item $Work -Recurse -Force -ErrorAction SilentlyContinue
    exit $code
}

# 1. Main window must appear.
$deadline = (Get-Date).AddSeconds($WindowTimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    if ($proc.HasExited) { Stop-Smoke 1 "process exited during startup (code $($proc.ExitCode))" }
    $proc.Refresh()
    if ($proc.MainWindowHandle -ne [IntPtr]::Zero -and $proc.MainWindowTitle) { break }
    Start-Sleep -Milliseconds 500
}
if ($proc.MainWindowHandle -eq [IntPtr]::Zero) { Stop-Smoke 1 "no main window within $WindowTimeoutSeconds s" }
Write-Host "Smoke: main window '$($proc.MainWindowTitle)' is up"

# 2. Must survive long enough for the background guardian to tick at least once.
$until = (Get-Date).AddSeconds($AliveSeconds)
while ((Get-Date) -lt $until) {
    if ($proc.HasExited) { Stop-Smoke 1 "process exited after window appeared (code $($proc.ExitCode))" }
    Start-Sleep -Seconds 1
}
Stop-Smoke 0 "alive for $AliveSeconds s with window '$($proc.MainWindowTitle)'"
