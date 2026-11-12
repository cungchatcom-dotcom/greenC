<#
.SYNOPSIS
  Build the Green C:\ installer: dist\GreenC-Setup-<version>.exe

.DESCRIPTION
  1. Checks that the version is consistent (scripts/version.py check)
  2. Runs the test suite (skip with -SkipTests)
  3. Generates the icon and exe version resource (packaging/prepare.py)
  4. Builds dist\GreenC\ with PyInstaller
  5. Compiles the installer with Inno Setup (ISCC.exe)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\build.ps1
#>
param(
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Invoke-Step([string]$Title, [scriptblock]$Action) {
    Write-Host "==> $Title" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0) { throw "$Title failed (exit code $LASTEXITCODE)" }
}

function Find-Iscc {
    $cmd = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($dir in @("$env:LOCALAPPDATA\Programs\Inno Setup 6", "${env:ProgramFiles(x86)}\Inno Setup 6", "$env:ProgramFiles\Inno Setup 6")) {
        $candidate = Join-Path $dir "ISCC.exe"
        if (Test-Path $candidate) { return $candidate }
    }
    throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup"
}

$Iscc = Find-Iscc
Invoke-Step "Sync dependencies" { uv sync --quiet }
Invoke-Step "Check version" { uv run python scripts/version.py check }
$Version = (uv run python scripts/version.py show).Trim()
Write-Host "Version $Version"

if (-not $SkipTests) {
    Invoke-Step "Run tests" { uv run pytest -q }
}

Invoke-Step "Prepare icon and version resource" { uv run python packaging/prepare.py }

if (Test-Path dist\GreenC) { Remove-Item -Recurse -Force dist\GreenC }
Invoke-Step "Build app with PyInstaller" {
    uv run pyinstaller packaging/greenc.spec --noconfirm --clean --log-level WARN --distpath dist --workpath build/pyinstaller
}

Invoke-Step "Compile installer" { & $Iscc /Q "/DAppVersion=$Version" packaging\greenc.iss }

$Setup = Join-Path $Root "dist\GreenC-Setup-$Version.exe"
$Hash = (Get-FileHash $Setup -Algorithm SHA256).Hash
Set-Content -Path "$Setup.sha256" -Value "$Hash  GreenC-Setup-$Version.exe" -Encoding ascii
$SizeMb = [math]::Round((Get-Item $Setup).Length / 1MB, 1)
Write-Host "Done: $Setup ($SizeMb MB)" -ForegroundColor Green
Write-Host "SHA256: $Hash"
