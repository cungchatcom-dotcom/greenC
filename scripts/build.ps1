<#
.SYNOPSIS
    Build the GreenC v2 (Rust + Tauri v2) release binaries and Inno Setup installer:
    dist\GreenC-Setup-<version>.exe (< 10 MB).

.DESCRIPTION
    1. Reads workspace version from Cargo.toml
    2. Compiles Svelte 5 + Tailwind frontend into ui\dist
    3. Runs 50-locale validator and workspace Rust tests (unless -SkipTests)
    4. Builds stripped LTO release binaries (greenc-app -> GreenC.exe, greenc-cli.exe)
    5. Compiles dist\GreenC-Setup-<version>.exe via Inno Setup 6 (ISCC.exe) and verifies < 10 MB size

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$SkipSmoke,
    [switch]$SkipInstaller,
    [switch]$CleanTarget
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path $CargoBin) {
        $env:Path = "$CargoBin;$env:Path"
    }
}

$LlvmMingwBin = Get-ChildItem -Path "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\MartinStorsjo.LLVM-MinGW.UCRT*\llvm-mingw-*\bin" -Directory -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
if ($LlvmMingwBin -and ($env:Path -notlike "*$LlvmMingwBin*")) {
    $env:Path = "$LlvmMingwBin;$env:Path"
}

function Get-RustHostTriple {
    $verOut = & rustc -vV
    $hostLine = $verOut | Where-Object { $_ -match '^host:\s*(.+)$' } | Select-Object -First 1
    if ($hostLine -match '^host:\s*(.+)$') {
        return $Matches[1].Trim()
    }
    return "x86_64-pc-windows-msvc"
}

function Assert-NativeToolchain {
    $hostTriple = Get-RustHostTriple
    if ($hostTriple -match 'msvc') {
        if (Get-Command link.exe -ErrorAction SilentlyContinue) {
            return
        }
        $VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path $VsWhere) {
            $InstallPath = & $VsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            if ($InstallPath) {
                return
            }
        }
        Write-Host ""
        Write-Host "ERROR: Rust ($hostTriple) requires Visual Studio C++ Build Tools (link.exe + Windows SDK)." -ForegroundColor Red
        Write-Host "Install with:" -ForegroundColor Yellow
        Write-Host '  winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"' -ForegroundColor Yellow
        Write-Host ""
        throw "MSVC C++ Build Tools (link.exe) not found."
    } else {
        if ((Get-Command clang.exe -ErrorAction SilentlyContinue) -or (Get-Command gcc.exe -ErrorAction SilentlyContinue)) {
            return
        }
        Write-Host ""
        Write-Host "ERROR: Rust ($hostTriple) requires LLVM-MinGW or MinGW-w64 (clang.exe/gcc.exe) in PATH." -ForegroundColor Red
        Write-Host "Install with:" -ForegroundColor Yellow
        Write-Host "  winget install MartinStorsjo.LLVM-MinGW.UCRT" -ForegroundColor Yellow
        Write-Host ""
        throw "MinGW/LLVM-MinGW toolchain not found."
    }
}

function Invoke-Step([string]$Title, [scriptblock]$Action) {
    Write-Host "==> $Title" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Title failed (exit code $LASTEXITCODE)"
    }
}

function Find-Iscc {
    $cmd = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($dir in @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6",
        "${env:ProgramFiles(x86)}\Inno Setup 6",
        "$env:ProgramFiles\Inno Setup 6"
    )) {
        $candidate = Join-Path $dir "ISCC.exe"
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

# 1. Extract version from workspace Cargo.toml
$CargoToml = Get-Content -Path (Join-Path $Root "Cargo.toml") -Raw
if ($CargoToml -match 'version\s*=\s*"([^"]+)"') {
    $Version = $Matches[1]
} else {
    throw "Could not parse workspace version from Cargo.toml"
}
$HostTriple = Get-RustHostTriple
Write-Host "Building GreenC v$Version (Rust $HostTriple)" -ForegroundColor Green

# 2. Frontend Svelte 5 static asset build
Invoke-Step "Install & check Svelte 5 UI" {
    if (-not (Test-Path (Join-Path $Root "ui\node_modules"))) {
        npm --prefix ui install
    }
    npm --prefix ui run check
}
Invoke-Step "Build Svelte 5 production bundle" {
    npm --prefix ui run build
}

# 3. Verify native linker & run Rust 50-locale check + test suite
Assert-NativeToolchain
if (-not $SkipTests) {
    Invoke-Step "Validate all 50 embedded locales" {
        cargo run -p greenc-i18n -- check
    }
    Invoke-Step "Run workspace unit & integration tests" {
        cargo test --workspace
    }
}

# 4. Build LTO stripped release binaries
Invoke-Step "Build release binaries (greenc-app + greenc-cli)" {
    cargo build --release --workspace
}

$StageDir = Join-Path $Root "dist\GreenC"
if (Test-Path $StageDir) {
    Remove-Item -Recurse -Force $StageDir
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

$AppSrc = Join-Path $Root "target\release\greenc-app.exe"
$CliSrc = Join-Path $Root "target\release\greenc-cli.exe"
Copy-Item -Path $AppSrc -Destination (Join-Path $StageDir "GreenC.exe") -Force
Copy-Item -Path $CliSrc -Destination (Join-Path $StageDir "greenc-cli.exe") -Force

if ($HostTriple -notmatch 'msvc') {
    $WvDll = Get-ChildItem -Path (Join-Path $Root "target\release\build\webview2-com-sys-*\out\x64\WebView2Loader.dll") -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
    if (-not $WvDll) {
        $WvDll = Get-ChildItem -Path "$env:USERPROFILE\.cargo\registry\src\*\webview2-com-sys-*\x64\WebView2Loader.dll" -ErrorAction SilentlyContinue |
            Select-Object -First 1 -ExpandProperty FullName
    }
    if ($WvDll) {
        Copy-Item -Path $WvDll -Destination (Join-Path $StageDir "WebView2Loader.dll") -Force
        Write-Host "Staged WebView2Loader.dll from $WvDll" -ForegroundColor DarkCyan
    } else {
        Write-Warning "WebView2Loader.dll not found for non-MSVC target!"
    }
}

# 4b. Launch the staged app and require it to survive a guardian tick. Unit and CLI tests never
#     start the Tauri app, so this is the only gate that catches a crash at startup.
if (-not $SkipSmoke) {
    Invoke-Step "Smoke-test staged GreenC.exe (launch + survive 60 s)" {
        & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "smoke-app.ps1") `
            -Exe (Join-Path $StageDir "GreenC.exe") -AliveSeconds 60
    }
}

# 5. Compile Inno Setup 6 installer
if (-not $SkipInstaller) {
    $Iscc = Find-Iscc
    if (-not $Iscc) {
        throw "Inno Setup 6 (ISCC.exe) not found. Install with: winget install JRSoftware.InnoSetup"
    }
    Invoke-Step "Compile Inno Setup installer" {
        & $Iscc /Q "/DAppVersion=$Version" (Join-Path $Root "packaging\greenc.iss")
    }

    $SetupPath = Join-Path $Root "dist\GreenC-Setup-$Version.exe"
    $SizeBytes = (Get-Item $SetupPath).Length
    $SizeMb = [math]::Round($SizeBytes / 1MB, 2)
    if ($SizeBytes -gt (10 * 1024 * 1024)) {
        throw "Installer size ($SizeMb MB) exceeds < 10 MB constitution budget!"
    }

    $Hash = (Get-FileHash $SetupPath -Algorithm SHA256).Hash
    Set-Content -Path "$SetupPath.sha256" -Value "$Hash  GreenC-Setup-$Version.exe" -Encoding ascii
    Write-Host "Done: $SetupPath ($SizeMb MB < 10 MB budget)" -ForegroundColor Green
    Write-Host "SHA256: $Hash"
}

if ($CleanTarget) {
    Invoke-Step "Clean intermediate Cargo target directory" {
        cargo clean
    }
}
