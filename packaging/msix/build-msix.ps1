<#
.SYNOPSIS
    Builds an architecture-specific MSIX package (.msix) for Microsoft Store bundling.
.DESCRIPTION
    Stages the `--features store` release binaries (`GreenC.exe`, `greenc-cli.exe`) for
    the requested architecture (`x64` or `arm64`), verifies the PE machine header of both
    executables against the target architecture (x64 = 0x8664, arm64 = 0xAA64), renders
    `AppxManifest.xml` from `AppxManifest.xml.in` with the Partner Center Identity values
    and `a.b.c.0` package version, generates Store logo PNGs from
    `crates/greenc-app/icons/icon.png`, validates all Store policy invariants, and packs
    `GreenC-<ver>-<arch>.msix` with `makeappx.exe`.

    IMPORTANT:
    - Does NOT create a per-architecture `.msixupload`. Use `build-bundle.ps1` to combine
      the x64 and arm64 `.msix` packages into `GreenC-<ver>.msixbundle` and
      `GreenC-<ver>.msixupload`.
    - For Microsoft Store submission, do NOT self-sign the package; Partner Center signs it.
    - WebView2 note: Tauri v2 links the WebView2 loader statically (for both x64 and arm64)
      and uses the Evergreen Microsoft Edge WebView2 Runtime (pre-installed on Windows 11
      and modern Windows 10 updates; older Windows 10 1903/1909 machines without Edge
      Chromium may prompt or require installing the Evergreen WebView2 Runtime).
#>
[CmdletBinding()]
param(
    [ValidateSet("x64", "arm64")]
    [string]$Architecture = "x64",
    [string]$IdentityName = $env:STORE_IDENTITY_NAME,
    [string]$IdentityPublisher = $env:STORE_IDENTITY_PUBLISHER,
    [string]$PublisherDisplayName = $env:STORE_PUBLISHER_DISPLAY_NAME,
    [string]$Version = "",
    [string]$ExePath = "",
    [string]$CliPath = "",
    [string]$OutputDir = "",
    [switch]$RunWack = $false
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")).Path

$TargetTriple = switch ($Architecture) {
    "x64"   { "x86_64-pc-windows-msvc" }
    "arm64" { "aarch64-pc-windows-msvc" }
    default { throw "Unsupported architecture '$Architecture'. Expected 'x64' or 'arm64'." }
}

$ExpectedPeMachine = switch ($Architecture) {
    "x64"   { [uint16]0x8664 }
    "arm64" { [uint16]0xAA64 }
    default { throw "Unsupported architecture '$Architecture'." }
}

$DefaultTargetReleaseDir = Join-Path (Join-Path (Join-Path $RepoRoot "target") $TargetTriple) "release"

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $DefaultTargetReleaseDir "greenc-app.exe"
}
if ([string]::IsNullOrWhiteSpace($CliPath)) {
    $CliPath = Join-Path $DefaultTargetReleaseDir "greenc-cli.exe"
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path (Join-Path $RepoRoot "dist") "msix"
}

function Get-PeMachineType {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath
    )

    if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) {
        throw "PE file not found: '$FilePath'"
    }

    $stream = [System.IO.File]::Open($FilePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
        if ($stream.Length -lt 64) {
            throw "File '$FilePath' is too small ($($stream.Length) bytes) to be a valid PE executable."
        }
        $reader = New-Object System.IO.BinaryReader($stream)
        try {
            $dosMagic = $reader.ReadUInt16()
            if ($dosMagic -ne [uint16]0x5A4D) {
                throw "File '$FilePath' is not a valid PE executable (missing MZ header, got 0x$($dosMagic.ToString('X4')))."
            }
            $stream.Position = 0x3C
            $peOffset = [int64]$reader.ReadUInt32()
            if ($peOffset -lt 64 -or ($peOffset + 6) -gt $stream.Length) {
                throw "File '$FilePath' has an invalid PE header offset ($peOffset, file size $($stream.Length))."
            }
            $stream.Position = $peOffset
            $peSignature = $reader.ReadUInt32()
            if ($peSignature -ne [uint32]0x00004550) {
                throw "File '$FilePath' is missing the PE signature at offset $peOffset (got 0x$($peSignature.ToString('X8')))."
            }
            return [uint16]$reader.ReadUInt16()
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Assert-PeArchitecture {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string]$ExpectedArch,
        [Parameter(Mandatory = $true)]
        [uint16]$ExpectedMachine
    )

    $actualMachine = Get-PeMachineType -FilePath $FilePath
    if ($actualMachine -ne $ExpectedMachine) {
        $actualHex = "0x{0:X4}" -f $actualMachine
        $expectedHex = "0x{0:X4}" -f $ExpectedMachine
        throw "PE machine mismatch for '$FilePath': expected $ExpectedArch ($expectedHex), got $actualHex."
    }
    Write-Host ("PE header verified: {0} -> 0x{1:X4} ({2})" -f $FilePath, $actualMachine, $ExpectedArch)
}

function Find-WindowsSdkTool {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ToolName
    )

    $hostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    $preferredArches = if ($hostArch -eq "arm64") { @("arm64", "x64") } else { @("x64") }

    $kitRoots = @()
    foreach ($base in @(${env:ProgramFiles(x86)}, ${env:ProgramFiles})) {
        if (-not [string]::IsNullOrWhiteSpace($base)) {
            $binRoot = Join-Path (Join-Path (Join-Path $base "Windows Kits") "10") "bin"
            if (Test-Path -LiteralPath $binRoot -PathType Container) {
                $kitRoots += $binRoot
            }
        }
    }

    foreach ($archDir in $preferredArches) {
        $candidates = @()
        foreach ($kitRoot in $kitRoots) {
            $versionDirs = @(Get-ChildItem -LiteralPath $kitRoot -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' })
            foreach ($vDir in $versionDirs) {
                $toolPath = Join-Path (Join-Path $vDir.FullName $archDir) $ToolName
                if (Test-Path -LiteralPath $toolPath -PathType Leaf) {
                    $candidates += [PSCustomObject]@{
                        Version = [version]$vDir.Name
                        Path    = $toolPath
                    }
                }
            }
        }
        if ($candidates.Count -gt 0) {
            $best = @($candidates | Sort-Object -Property Version -Descending)[0]
            return $best.Path
        }
    }

    $cmd = Get-Command $ToolName -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -ne $cmd) {
        return $cmd.Source
    }
    return $null
}

# Resolve a.b.c version from workspace Cargo.toml if not explicitly supplied
if ([string]::IsNullOrWhiteSpace($Version)) {
    $cargoToml = Get-Content -LiteralPath (Join-Path $RepoRoot "Cargo.toml") -Raw
    if ($cargoToml -match '(?ms)\[workspace\.package\].*?version\s*=\s*"(\d+\.\d+\.\d+)"') {
        $Version = $Matches[1]
    } else {
        throw "Could not read workspace version from Cargo.toml"
    }
}

$CleanVersion = $Version.Trim().TrimStart('v', 'V')
if ($CleanVersion -match '^\d+\.\d+\.\d+$') {
    $PackageVersion = "$CleanVersion.0"
} elseif ($CleanVersion -match '^\d+\.\d+\.\d+\.\d+$') {
    $PackageVersion = $CleanVersion
} else {
    throw "Invalid MSIX version '$Version'. Expected a.b.c or a.b.c.0."
}

$IsPlaceholderIdentity = $false
if ([string]::IsNullOrWhiteSpace($IdentityName)) {
    $IdentityName = "CungChat.GreenC.Preview"
    $IsPlaceholderIdentity = $true
}
if ([string]::IsNullOrWhiteSpace($IdentityPublisher)) {
    $IdentityPublisher = "CN=GreenC-Preview-Not-For-Store-Upload"
    $IsPlaceholderIdentity = $true
}
if ([string]::IsNullOrWhiteSpace($PublisherDisplayName)) {
    $PublisherDisplayName = "Cùng Chat"
}

if ($IsPlaceholderIdentity) {
    Write-Warning "Using placeholder Identity values ($IdentityName / $IdentityPublisher). Set STORE_IDENTITY_NAME, STORE_IDENTITY_PUBLISHER, and STORE_PUBLISHER_DISPLAY_NAME from Partner Center before Store submission."
}

if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "Desktop binary not found at '$ExePath'. Build with: cargo build --release --target $TargetTriple -p greenc-app --features store"
}
if (-not (Test-Path -LiteralPath $CliPath -PathType Leaf)) {
    throw "CLI binary not found at '$CliPath'. Build with: cargo build --release --target $TargetTriple -p greenc-cli --features store"
}

# Verify PE machine type of both executables before staging
Assert-PeArchitecture -FilePath $ExePath -ExpectedArch $Architecture -ExpectedMachine $ExpectedPeMachine
Assert-PeArchitecture -FilePath $CliPath -ExpectedArch $Architecture -ExpectedMachine $ExpectedPeMachine

function Escape-XmlAttr([string]$Value) {
    return [System.Security.SecurityElement]::Escape($Value)
}

# Prepare clean per-architecture staging directory
$StageDir = Join-Path $OutputDir "stage-$Architecture"
if (Test-Path -LiteralPath $StageDir) {
    Remove-Item -LiteralPath $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

$StagedExePath = Join-Path $StageDir "GreenC.exe"
$StagedCliPath = Join-Path $StageDir "greenc-cli.exe"
Copy-Item -LiteralPath $ExePath -Destination $StagedExePath -Force
Copy-Item -LiteralPath $CliPath -Destination $StagedCliPath -Force

# Re-verify staged binaries
Assert-PeArchitecture -FilePath $StagedExePath -ExpectedArch $Architecture -ExpectedMachine $ExpectedPeMachine
Assert-PeArchitecture -FilePath $StagedCliPath -ExpectedArch $Architecture -ExpectedMachine $ExpectedPeMachine

# Generate Store logo PNG assets into stage-<arch>\Assets
$AssetsDir = Join-Path $StageDir "Assets"
& (Join-Path $PSScriptRoot "generate-assets.ps1") `
    -SourceIcon (Join-Path $RepoRoot "crates\greenc-app\icons\icon.png") `
    -OutputDir $AssetsDir

# Render AppxManifest.xml
$TemplatePath = Join-Path $PSScriptRoot "AppxManifest.xml.in"
$ManifestText = Get-Content -LiteralPath $TemplatePath -Raw -Encoding UTF8
$ManifestText = $ManifestText.Replace("@IDENTITY_NAME@", (Escape-XmlAttr $IdentityName))
$ManifestText = $ManifestText.Replace("@IDENTITY_PUBLISHER@", (Escape-XmlAttr $IdentityPublisher))
$ManifestText = $ManifestText.Replace("@PUBLISHER_DISPLAY_NAME@", (Escape-XmlAttr $PublisherDisplayName))
$ManifestText = $ManifestText.Replace("@PACKAGE_VERSION@", $PackageVersion)
$ManifestText = $ManifestText.Replace("@ARCH@", $Architecture)

if ($ManifestText -match '@[A-Z0-9_]+@') {
    throw "Unresolved placeholder found in rendered AppxManifest.xml: $($Matches[0])"
}

$ManifestPath = Join-Path $StageDir "AppxManifest.xml"
[System.IO.File]::WriteAllText($ManifestPath, $ManifestText, [System.Text.UTF8Encoding]::new($false))

# Validate rendered XML and critical Store policy invariants
[xml]$Xml = Get-Content -LiteralPath $ManifestPath -Raw -Encoding UTF8
$ns = New-Object System.Xml.XmlNamespaceManager($Xml.NameTable)
$ns.AddNamespace("m", "http://schemas.microsoft.com/appx/manifest/foundation/windows10")
$ns.AddNamespace("uap", "http://schemas.microsoft.com/appx/manifest/uap/windows10")
$ns.AddNamespace("uap3", "http://schemas.microsoft.com/appx/manifest/uap/windows10/3")
$ns.AddNamespace("desktop", "http://schemas.microsoft.com/appx/manifest/desktop/windows10")
$ns.AddNamespace("rescap", "http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities")

$idNode = $Xml.SelectSingleNode("/m:Package/m:Identity", $ns)
if ($null -eq $idNode) {
    throw "AppxManifest is missing /Package/Identity"
}
if ($idNode.GetAttribute("ProcessorArchitecture") -cne $Architecture) {
    throw "AppxManifest Identity ProcessorArchitecture must be '$Architecture', got '$($idNode.GetAttribute('ProcessorArchitecture'))'"
}
if ($idNode.GetAttribute("Version") -cne $PackageVersion) {
    throw "AppxManifest Identity Version mismatch: '$($idNode.GetAttribute('Version'))' != '$PackageVersion'"
}

$tdf = $Xml.SelectSingleNode("/m:Package/m:Dependencies/m:TargetDeviceFamily", $ns)
if ($null -eq $tdf -or $tdf.GetAttribute("MinVersion") -cne "10.0.18362.0") {
    throw "AppxManifest MinVersion must be 10.0.18362.0"
}

$caps = @($Xml.SelectNodes("/m:Package/m:Capabilities/*", $ns) | ForEach-Object { $_.GetAttribute("Name") })
if ($caps -notcontains "runFullTrust") {
    throw "AppxManifest must declare rescap:Capability Name='runFullTrust'"
}
if ($caps -contains "unvirtualizedResources") {
    throw "AppxManifest MUST NOT declare unvirtualizedResources"
}
if ($caps.Count -ne 1) {
    throw "AppxManifest should only declare runFullTrust; found unexpected capabilities: $($caps -join ', ')"
}

$startupNode = $Xml.SelectSingleNode("//desktop:StartupTask", $ns)
if ($null -eq $startupNode -or $startupNode.GetAttribute("Enabled") -cne "false") {
    throw "desktop:StartupTask must exist with Enabled='false'"
}

$aliasNode = $Xml.SelectSingleNode("//uap3:AppExecutionAlias/desktop:ExecutionAlias", $ns)
if ($null -eq $aliasNode -or $aliasNode.GetAttribute("Alias") -cne "greenc-cli.exe") {
    throw "uap3:AppExecutionAlias must declare desktop:ExecutionAlias Alias='greenc-cli.exe'"
}

# Verify all referenced asset files exist
foreach ($assetFile in @(
    "StoreLogo.png",
    "Square44x44Logo.png",
    "Square71x71Logo.png",
    "Square150x150Logo.png",
    "Wide310x150Logo.png",
    "Square310x310Logo.png"
)) {
    $p = Join-Path $AssetsDir $assetFile
    if (-not (Test-Path -LiteralPath $p -PathType Leaf)) {
        throw "Missing generated MSIX asset: $p"
    }
}

Write-Host "Manifest validation passed: Identity=$($idNode.GetAttribute('Name')) Version=$PackageVersion Arch=$Architecture MinVersion=10.0.18362.0"

# WebView2 runtime compatibility reminder for older Windows 10 builds
Write-Host "Note (WebView2): Tauri v2 links WebView2Loader statically (x64 & arm64) and uses the Evergreen WebView2 Runtime (built into Windows 11 and updated Windows 10; unpatched Win10 1903/1909 requires the Microsoft Edge WebView2 Evergreen Runtime)."

# Locate makeappx.exe from Windows 10/11 SDK
$MakeAppx = Find-WindowsSdkTool -ToolName "makeappx.exe"
if ([string]::IsNullOrWhiteSpace($MakeAppx)) {
    throw "makeappx.exe not found. Install the Windows 10/11 SDK (Windows Kits)."
}

$MsixFile = Join-Path $OutputDir "GreenC-$PackageVersion-$Architecture.msix"
if (Test-Path -LiteralPath $MsixFile) {
    Remove-Item -LiteralPath $MsixFile -Force
}

Write-Host "Packing MSIX with $MakeAppx -> $MsixFile"
& $MakeAppx pack /o /v /d $StageDir /p $MsixFile
if ($LASTEXITCODE -ne 0) {
    throw "makeappx pack failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $MsixFile -PathType Leaf)) {
    throw "makeappx pack succeeded but expected output file is missing: $MsixFile"
}

# Optional best-effort tool checks (signtool & Windows App Certification Kit)
$SignTool = Find-WindowsSdkTool -ToolName "signtool.exe"
if (-not [string]::IsNullOrWhiteSpace($SignTool)) {
    Write-Host "signtool.exe detected at $SignTool (MSIX left unsigned for bundling and Microsoft Store signing)."
} else {
    Write-Host "signtool.exe not found on runner (not required for unsigned Store .msix)."
}

if ($RunWack) {
    $AppCert = $null
    foreach ($base in @(${env:ProgramFiles(x86)}, ${env:ProgramFiles})) {
        if (-not [string]::IsNullOrWhiteSpace($base)) {
            $candidate = Join-Path (Join-Path (Join-Path (Join-Path $base "Windows Kits") "10") "App Certification Kit") "appcert.exe"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $AppCert = $candidate
                break
            }
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($AppCert)) {
        $ReportXml = Join-Path $OutputDir "wack-report-$Architecture.xml"
        Write-Host "Running Windows App Certification Kit (best-effort): $AppCert"
        $prevPref = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            & $AppCert test -appxpackagepath $MsixFile -reportoutputpath $ReportXml
            Write-Host "WACK exited with code $LASTEXITCODE (non-blocking in CI)."
        } catch {
            Write-Warning "WACK execution warning (non-blocking): $_"
        } finally {
            $ErrorActionPreference = $prevPref
        }
    } else {
        Write-Host "Windows App Certification Kit (appcert.exe) not installed on this runner; skipping."
    }
}

Write-Host "MSIX ($Architecture) packaging completed successfully: $MsixFile"
