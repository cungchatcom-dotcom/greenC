<#
.SYNOPSIS
    Builds an x64 MSIX package (.msix and .msixupload) for Microsoft Store submission.
.DESCRIPTION
    Stages the `--features store` release binaries (`GreenC.exe`, `greenc-cli.exe`),
    renders `AppxManifest.xml` from `AppxManifest.xml.in` with the Partner Center
    Identity values and `a.b.c.0` package version, generates Store logo PNGs from
    `crates/greenc-app/icons/icon.png`, validates the manifest, and packs with `makeappx.exe`.

    IMPORTANT:
    - For Microsoft Store submission, do NOT self-sign the package; Partner Center signs it.
    - WebView2 note: Tauri v2 links the WebView2 loader statically and uses the Evergreen
      Microsoft Edge WebView2 Runtime (pre-installed on Windows 11 and modern Windows 10
      updates; older Windows 10 1903/1909 machines without Edge Chromium may prompt or
      require installing the Evergreen WebView2 Runtime).
#>
[CmdletBinding()]
param(
    [string]$IdentityName = $env:STORE_IDENTITY_NAME,
    [string]$IdentityPublisher = $env:STORE_IDENTITY_PUBLISHER,
    [string]$PublisherDisplayName = $env:STORE_PUBLISHER_DISPLAY_NAME,
    [string]$Version = "",
    [string]$ExePath = "",
    [string]$CliPath = "",
    [string]$OutputDir = "",
    [switch]$CreateMsixUpload = $true,
    [switch]$RunWack = $false
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $RepoRoot "target\release\greenc-app.exe"
}
if ([string]::IsNullOrWhiteSpace($CliPath)) {
    $CliPath = Join-Path $RepoRoot "target\release\greenc-cli.exe"
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot "dist\msix"
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

if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Desktop binary not found at '$ExePath'. Build with: cargo build --release -p greenc-app --features store"
}
if (-not (Test-Path -LiteralPath $CliPath)) {
    throw "CLI binary not found at '$CliPath'. Build with: cargo build --release -p greenc-cli --features store"
}

function Escape-XmlAttr([string]$Value) {
    return [System.Security.SecurityElement]::Escape($Value)
}

# Prepare clean staging directory
$StageDir = Join-Path $OutputDir "stage"
if (Test-Path -LiteralPath $StageDir) {
    Remove-Item -LiteralPath $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

Copy-Item -LiteralPath $ExePath -Destination (Join-Path $StageDir "GreenC.exe") -Force
Copy-Item -LiteralPath $CliPath -Destination (Join-Path $StageDir "greenc-cli.exe") -Force

# Generate Store logo PNG assets into stage\Assets
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
$ns.AddNamespace("desktop", "http://schemas.microsoft.com/appx/manifest/desktop/windows10")
$ns.AddNamespace("rescap", "http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities")

$idNode = $Xml.SelectSingleNode("/m:Package/m:Identity", $ns)
if ($idNode.ProcessorArchitecture -ne "x64") {
    throw "AppxManifest Identity ProcessorArchitecture must be x64, got '$($idNode.ProcessorArchitecture)'"
}
if ($idNode.Version -ne $PackageVersion) {
    throw "AppxManifest Identity Version mismatch: '$($idNode.Version)' != '$PackageVersion'"
}

$tdf = $Xml.SelectSingleNode("/m:Package/m:Dependencies/m:TargetDeviceFamily", $ns)
if ($tdf.MinVersion -ne "10.0.18362.0") {
    throw "AppxManifest MinVersion must be 10.0.18362.0, got '$($tdf.MinVersion)'"
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
if (-not $startupNode -or $startupNode.GetAttribute("Enabled") -ne "false") {
    throw "desktop:StartupTask must exist with Enabled='false'"
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
    if (-not (Test-Path -LiteralPath $p)) {
        throw "Missing generated MSIX asset: $p"
    }
}

Write-Host "Manifest validation passed: Identity=$($idNode.Name) Version=$PackageVersion Arch=x64 MinVersion=10.0.18362.0"

# WebView2 runtime compatibility reminder for older Windows 10 builds
Write-Host "Note (WebView2): Tauri v2 links WebView2Loader statically and uses the Evergreen WebView2 Runtime (built into Windows 11 and updated Windows 10; unpatched Win10 1903/1909 requires the Microsoft Edge WebView2 Evergreen Runtime)."

# Locate makeappx.exe from Windows 10/11 SDK
$MakeAppx = Get-Command makeappx.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1
if (-not $MakeAppx) {
    $kitRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kitRoot) {
        $MakeAppx = Get-ChildItem -Path $kitRoot -Filter "makeappx.exe" -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\makeappx\.exe$' } |
            Sort-Object FullName -Descending |
            Select-Object -ExpandProperty FullName -First 1
    }
}
if (-not $MakeAppx) {
    throw "makeappx.exe not found. Install the Windows 10/11 SDK (Windows Kits)."
}

$MsixFile = Join-Path $OutputDir "GreenC-$PackageVersion-x64.msix"
if (Test-Path -LiteralPath $MsixFile) {
    Remove-Item -LiteralPath $MsixFile -Force
}

Write-Host "Packing MSIX with $MakeAppx -> $MsixFile"
& $MakeAppx pack /o /v /d $StageDir /p $MsixFile
if ($LASTEXITCODE -ne 0) {
    throw "makeappx pack failed with exit code $LASTEXITCODE"
}

if ($CreateMsixUpload) {
    $MsixUploadFile = Join-Path $OutputDir "GreenC-$PackageVersion-x64.msixupload"
    if (Test-Path -LiteralPath $MsixUploadFile) {
        Remove-Item -LiteralPath $MsixUploadFile -Force
    }
    # A .msixupload package is a ZIP archive containing the unsigned .msix (and optional .appxsym).
    # Use System.IO.Compression.ZipFile directly because Compress-Archive rejects non-.zip extensions.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::Open($MsixUploadFile, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $zip,
            $MsixFile,
            [System.IO.Path]::GetFileName($MsixFile),
            [System.IO.Compression.CompressionLevel]::Optimal
        ) | Out-Null
    } finally {
        $zip.Dispose()
    }
    Write-Host "Created Store upload bundle: $MsixUploadFile"
}

# Optional best-effort tool checks (signtool & Windows App Certification Kit)
$SignTool = Get-Command signtool.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1
if (-not $SignTool) {
    $kitRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kitRoot) {
        $SignTool = Get-ChildItem -Path $kitRoot -Filter "signtool.exe" -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
            Sort-Object FullName -Descending |
            Select-Object -ExpandProperty FullName -First 1
    }
}
if ($SignTool) {
    Write-Host "signtool.exe detected at $SignTool (MSIX left unsigned for Microsoft Store signing)."
} else {
    Write-Host "signtool.exe not found on runner (not required for unsigned Store .msixupload)."
}

if ($RunWack) {
    $AppCert = "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\appcert.exe"
    if (Test-Path -LiteralPath $AppCert) {
        $ReportXml = Join-Path $OutputDir "wack-report.xml"
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

Write-Host "MSIX packaging completed successfully."
