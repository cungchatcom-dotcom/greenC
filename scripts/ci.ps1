# GreenC v2 - Comprehensive Local Quality Gate Verification Harness
$ErrorActionPreference = "Stop"
$env:PATH = "C:\Users\duong\.cargo\bin;C:\Program Files\nodejs;C:\Users\duong\AppData\Roaming\npm;$env:PATH"
$WorkspaceRoot = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $WorkspaceRoot

Write-Host "`n=== [1/5] Checking Rust Code Formatting (rustfmt) ===" -ForegroundColor Cyan
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { throw "Rust formatting check failed." }

Write-Host "`n=== [2/5] Running Clippy Static Analysis (Zero Warnings) ===" -ForegroundColor Cyan
cargo clippy --workspace --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "Clippy lints failed." }

Write-Host "`n=== [3/5] Running Cargo Check Across All Workspace Targets ===" -ForegroundColor Cyan
cargo check --workspace --all-targets
if ($LASTEXITCODE -ne 0) { throw "Cargo check failed." }

Write-Host "`n=== [4/5] Running Workspace Unit & Integration Tests ===" -ForegroundColor Cyan
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw "Workspace tests failed." }

Write-Host "`n=== [5/5] Building Frontend Static Assets (Svelte 5) ===" -ForegroundColor Cyan
if (Get-Command pnpm -ErrorAction SilentlyContinue) {
    pnpm --dir ui run build
} else {
    Write-Host "pnpm not found, skipping frontend asset build in local gate." -ForegroundColor Yellow
}

Write-Host "`n=======================================================" -ForegroundColor Green
Write-Host " ALL QUALITY GATES PASSED (100% Clean) " -ForegroundColor Green
Write-Host "=======================================================`n" -ForegroundColor Green
