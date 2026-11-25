<#
.SYNOPSIS
    Generates required MSIX Store logo PNG assets from the canonical 512x512 icon.
.DESCRIPTION
    Uses .NET System.Drawing (available on Windows PowerShell / PowerShell 7 on Windows)
    so no large duplicate PNG binaries need to be committed to Git.
#>
[CmdletBinding()]
param(
    [string]$SourceIcon = (Join-Path $PSScriptRoot "..\..\crates\greenc-app\icons\icon.png"),
    [string]$OutputDir = (Join-Path $PSScriptRoot "Assets")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $SourceIcon)) {
    throw "Source icon not found: $SourceIcon"
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

# On PowerShell 7+ (.NET 8+), System.Drawing.Common is not part of the base BCL.
# Detect whether [System.Drawing.Image] is available; if not, delegate to Windows PowerShell 5.1
# (powershell.exe, always present on Windows) or Python Pillow (generate-assets.py).
$hasSystemDrawing = $false
try {
    Add-Type -AssemblyName System.Drawing -ErrorAction Stop
    $null = [System.Drawing.Image]
    $hasSystemDrawing = $true
} catch {
    $hasSystemDrawing = $false
}

if (-not $hasSystemDrawing) {
    $winPs = Get-Command powershell.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1
    if ($winPs -and $PSVersionTable.PSEdition -eq "Core") {
        & $winPs -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $PSCommandPath -SourceIcon $SourceIcon -OutputDir $OutputDir
        if ($LASTEXITCODE -ne 0) {
            throw "Windows PowerShell asset generation failed with exit code $LASTEXITCODE"
        }
        return
    }
    $py = Get-Command python -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1
    if (-not $py) {
        $py = Get-Command python3 -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1
    }
    if ($py) {
        & $py (Join-Path $PSScriptRoot "generate-assets.py") --source $SourceIcon --out $OutputDir
        if ($LASTEXITCODE -ne 0) {
            throw "Python asset generation failed with exit code $LASTEXITCODE"
        }
        return
    }
    throw "Neither System.Drawing (Windows PowerShell 5.1) nor Python Pillow is available to generate MSIX PNG assets."
}

$specs = @(
    @{ Name = "StoreLogo.png";         Width = 50;  Height = 50;  Pad = 0.04 }
    @{ Name = "Square44x44Logo.png";   Width = 44;  Height = 44;  Pad = 0.04 }
    @{ Name = "Square71x71Logo.png";   Width = 71;  Height = 71;  Pad = 0.08 }
    @{ Name = "Square150x150Logo.png"; Width = 150; Height = 150; Pad = 0.12 }
    @{ Name = "Wide310x150Logo.png";   Width = 310; Height = 150; Pad = 0.16 }
    @{ Name = "Square310x310Logo.png"; Width = 310; Height = 310; Pad = 0.16 }
)

$srcImg = [System.Drawing.Image]::FromFile((Resolve-Path -LiteralPath $SourceIcon).Path)
try {
    foreach ($spec in $specs) {
        $w = [int]$spec.Width
        $h = [int]$spec.Height
        $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try {
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            try {
                $g.Clear([System.Drawing.Color]::Transparent)
                $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
                $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

                $minSide = [Math]::Min($w, $h)
                $drawSide = [int][Math]::Round($minSide * (1.0 - 2.0 * [double]$spec.Pad))
                $x = [int][Math]::Floor(($w - $drawSide) / 2.0)
                $y = [int][Math]::Floor(($h - $drawSide) / 2.0)

                $destRect = New-Object System.Drawing.Rectangle($x, $y, $drawSide, $drawSide)
                $g.DrawImage($srcImg, $destRect)
            } finally {
                $g.Dispose()
            }

            $outPath = Join-Path $OutputDir $spec.Name
            $bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
            Write-Host "Generated $($spec.Name) (${w}x${h})"
        } finally {
            $bmp.Dispose()
        }
    }
} finally {
    $srcImg.Dispose()
}
