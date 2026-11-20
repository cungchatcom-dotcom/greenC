<#
.SYNOPSIS
    GreenC v2 Opaque-Box E2E Test Suite PowerShell Launcher.
.DESCRIPTION
    Runs the 4-tier E2E test suite (Tier 1: Feature Coverage, Tier 2: Boundary & Stress,
    Tier 3: Combinations, Tier 4: Real-World Scenarios).
.PARAMETER Tier
    Which tier to execute: '1', '2', '3', '4', or 'all' (default: 'all').
.PARAMETER Feature
    Optional feature filter (e.g. 'F01', 'F19').
.PARAMETER Verbose
    Enable detailed console logs.
#>

[CmdletBinding()]
param (
    [ValidateSet('1', '2', '3', '4', 'all')]
    [string]$Tier = 'all',

    [string]$Feature = '',

    [switch]$VerboseOutput
)

$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "================================================================================" -ForegroundColor Cyan
Write-Host "  GreenC v2 Complete Platform — Opaque-Box E2E Test Suite Launcher" -ForegroundColor Cyan
Write-Host "  Workspace: $ScriptDir" -ForegroundColor Cyan
Write-Host "  Selected Tier: $Tier | Feature: $(if ($Feature) { $Feature } else { 'All' })" -ForegroundColor Cyan
Write-Host "================================================================================" -ForegroundColor Cyan

$RunnerPath = Join-Path $ScriptDir "tests\e2e\test_runner.py"

if (-not (Test-Path $RunnerPath)) {
    Write-Error "Test runner script not found at $RunnerPath"
}

$Arguments = @($RunnerPath)

if ($Tier -eq 'all') {
    $Arguments += "--all"
} else {
    $Arguments += "--tier"
    $Arguments += $Tier
}

if ($Feature) {
    $Arguments += "--feature"
    $Arguments += $Feature
}

$ReportMd = Join-Path $ScriptDir "test_report.md"
$ReportJson = Join-Path $ScriptDir "test_report.json"
$Arguments += "--report-md"
$Arguments += $ReportMd
$Arguments += "--report-json"
$Arguments += $ReportJson

if ($VerboseOutput) {
    $Arguments += "-v"
}

# Find appropriate Python launcher
if (Get-Command uv -ErrorAction SilentlyContinue) {
    Write-Host "[INFO] Executing runner with uv run python..." -ForegroundColor Green
    & uv run python @Arguments
} else {
    Write-Host "[INFO] Executing runner with python..." -ForegroundColor Green
    & python @Arguments
}

if ($LASTEXITCODE -eq 0) {
    Write-Host "`n[SUCCESS] E2E Test Suite PASSED (All tiers green)." -ForegroundColor Green
} else {
    Write-Host "`n[FAILURE] E2E Test Suite encountered failures (Exit code: $LASTEXITCODE)." -ForegroundColor Red
}

exit $LASTEXITCODE
