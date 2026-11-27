<#
.SYNOPSIS
    Bundles exactly one x64 and one arm64 MSIX package into a Store .msixbundle and .msixupload.
.DESCRIPTION
    Scans the input directory for `.msix` files, requires exactly two (one `x64` and one `arm64`),
    unpacks each with `makeappx.exe` to verify matching Identity (Name, Publisher, Version) and
    matching PE machine headers on `GreenC.exe` and `greenc-cli.exe`, creates `GreenC-<ver>.msixbundle`
    from a clean staging directory holding only those two packages, verifies the bundle via
    `makeappx unbundle` and `AppxMetadata\AppxBundleManifest.xml`, and packages `GreenC-<ver>.msixupload`
    as a ZIP containing only the `.msixbundle`.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InputDir,
    [string]$OutputDir = "",
    [string]$Version = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")).Path

if (-not (Test-Path -LiteralPath $InputDir -PathType Container)) {
    throw "Input directory not found: '$InputDir'"
}
$ResolvedInputDir = (Resolve-Path -LiteralPath $InputDir).Path

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path (Join-Path (Join-Path $RepoRoot "dist") "msix") "bundle"
}

function Get-PeMachineType {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath
    )

    if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) {
        throw "PE file not found: '$FilePath'"
    }

    $resolvedPath = (Resolve-Path -LiteralPath $FilePath).Path
    $stream = [System.IO.File]::Open($resolvedPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
        if ($stream.Length -lt 64) {
            throw "File '$FilePath' is too small ($($stream.Length) bytes) to be a valid PE executable."
        }
        $reader = New-Object System.IO.BinaryReader($stream)
        try {
            $dosMagic = $reader.ReadUInt16()
            if ($dosMagic -ne [uint16]0x5A4D) {
                throw "File '$FilePath' is not a valid PE executable (missing MZ header)."
            }
            $stream.Position = 0x3C
            $peOffset = [int64]$reader.ReadUInt32()
            if ($peOffset -lt 64 -or ($peOffset + 6) -gt $stream.Length) {
                throw "File '$FilePath' has an invalid PE header offset ($peOffset, file size $($stream.Length))."
            }
            $stream.Position = $peOffset
            $peSignature = $reader.ReadUInt32()
            if ($peSignature -ne [uint32]0x00004550) {
                throw "File '$FilePath' is missing the PE signature at offset $peOffset."
            }
            return [uint16]$reader.ReadUInt16()
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
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

$ExpectedVersionOverride = ""
if (-not [string]::IsNullOrWhiteSpace($Version)) {
    $cleanVersion = $Version.Trim().TrimStart('v', 'V')
    if ($cleanVersion -match '^\d+\.\d+\.\d+$') {
        $ExpectedVersionOverride = "$cleanVersion.0"
    } elseif ($cleanVersion -match '^\d+\.\d+\.\d+\.\d+$') {
        $ExpectedVersionOverride = $cleanVersion
    } else {
        throw "Invalid version '$Version'. Expected a.b.c or a.b.c.0."
    }
}

$msixFiles = @(Get-ChildItem -LiteralPath $ResolvedInputDir -Filter "*.msix" -File -Recurse | Sort-Object -Property FullName)
if ($msixFiles.Count -ne 2) {
    $foundList = if ($msixFiles.Count -eq 0) { "(none)" } else { ($msixFiles | ForEach-Object { $_.FullName }) -join ", " }
    throw "Expected exactly 2 .msix packages (one x64 and one arm64) under '$ResolvedInputDir', found $($msixFiles.Count): $foundList"
}

$MakeAppx = Find-WindowsSdkTool -ToolName "makeappx.exe"
if ([string]::IsNullOrWhiteSpace($MakeAppx)) {
    throw "makeappx.exe not found. Install the Windows 10/11 SDK (Windows Kits)."
}

$tempBase = if (-not [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$TempRoot = Join-Path $tempBase ("greenc-msix-bundle-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null

try {
    $packagesByArch = @{}
    $index = 0

    foreach ($msixFile in $msixFiles) {
        $index++
        $unpackDir = Join-Path $TempRoot "unpack-$index"
        Write-Host "Unpacking $($msixFile.FullName) -> $unpackDir"
        & $MakeAppx unpack /o /p $msixFile.FullName /d $unpackDir
        if ($LASTEXITCODE -ne 0) {
            throw "makeappx unpack failed for '$($msixFile.FullName)' with exit code $LASTEXITCODE"
        }

        $manifestPath = Join-Path $unpackDir "AppxManifest.xml"
        if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
            throw "Unpacked MSIX '$($msixFile.FullName)' is missing AppxManifest.xml"
        }

        [xml]$xml = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8
        $ns = New-Object System.Xml.XmlNamespaceManager($xml.NameTable)
        $ns.AddNamespace("m", $xml.DocumentElement.NamespaceURI)

        $idNode = $xml.SelectSingleNode("/m:Package/m:Identity", $ns)
        if ($null -eq $idNode) {
            throw "AppxManifest.xml in '$($msixFile.FullName)' is missing /Package/Identity"
        }

        $idName = $idNode.GetAttribute("Name")
        $idPublisher = $idNode.GetAttribute("Publisher")
        $idVersion = $idNode.GetAttribute("Version")
        $idArch = $idNode.GetAttribute("ProcessorArchitecture")

        if ([string]::IsNullOrWhiteSpace($idName) -or
            [string]::IsNullOrWhiteSpace($idPublisher) -or
            [string]::IsNullOrWhiteSpace($idVersion) -or
            [string]::IsNullOrWhiteSpace($idArch)) {
            throw "Incomplete Identity attributes in '$($msixFile.FullName)': Name='$idName' Publisher='$idPublisher' Version='$idVersion' Arch='$idArch'"
        }

        if ($idArch -cne "x64" -and $idArch -cne "arm64") {
            throw "Unexpected ProcessorArchitecture '$idArch' in '$($msixFile.FullName)'. Expected 'x64' or 'arm64'."
        }

        if ($packagesByArch.ContainsKey($idArch)) {
            throw "Duplicate '$idArch' MSIX package detected: '$($packagesByArch[$idArch].Path)' and '$($msixFile.FullName)'"
        }

        # Defense-in-depth: verify unpacked binaries match the declared manifest architecture
        $expectedMachine = if ($idArch -ceq "x64") { [uint16]0x8664 } else { [uint16]0xAA64 }
        foreach ($binName in @("GreenC.exe", "greenc-cli.exe")) {
            $binPath = Join-Path $unpackDir $binName
            $actualMachine = Get-PeMachineType -FilePath $binPath
            if ($actualMachine -ne $expectedMachine) {
                throw ("Unpacked binary '{0}' in '{1}' has PE machine 0x{2:X4}, expected 0x{3:X4} ({4})." -f $binName, $msixFile.Name, $actualMachine, $expectedMachine, $idArch)
            }
        }

        $packagesByArch[$idArch] = [PSCustomObject]@{
            Path      = $msixFile.FullName
            Name      = $idName
            Publisher = $idPublisher
            Version   = $idVersion
            Arch      = $idArch
        }
    }

    if (-not $packagesByArch.ContainsKey("x64") -or -not $packagesByArch.ContainsKey("arm64")) {
        $present = @($packagesByArch.Keys | Sort-Object) -join ", "
        throw "Bundle requires exactly one x64 and one arm64 package; found: $present"
    }

    $x64Pkg = $packagesByArch["x64"]
    $arm64Pkg = $packagesByArch["arm64"]

    if ($x64Pkg.Name -cne $arm64Pkg.Name) {
        throw "Identity Name mismatch between x64 ('$($x64Pkg.Name)') and arm64 ('$($arm64Pkg.Name)')."
    }
    if ($x64Pkg.Publisher -cne $arm64Pkg.Publisher) {
        throw "Identity Publisher mismatch between x64 ('$($x64Pkg.Publisher)') and arm64 ('$($arm64Pkg.Publisher)')."
    }
    if ($x64Pkg.Version -cne $arm64Pkg.Version) {
        throw "Identity Version mismatch between x64 ('$($x64Pkg.Version)') and arm64 ('$($arm64Pkg.Version)')."
    }

    $PackageVersion = $x64Pkg.Version
    if ($PackageVersion -notmatch '^\d+\.\d+\.\d+\.\d+$') {
        throw "Invalid package version '$PackageVersion' in MSIX manifest."
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedVersionOverride) -and $PackageVersion -cne $ExpectedVersionOverride) {
        throw "MSIX package version '$PackageVersion' does not match requested version '$ExpectedVersionOverride'."
    }

    Write-Host "Identity verified across x64 and arm64: Name=$($x64Pkg.Name) Publisher=$($x64Pkg.Publisher) Version=$PackageVersion"

    # Stage ONLY the two validated .msix packages in an isolated directory for makeappx bundle
    $bundleInputDir = Join-Path $TempRoot "bundle-input"
    New-Item -ItemType Directory -Force -Path $bundleInputDir | Out-Null
    Copy-Item -LiteralPath $x64Pkg.Path -Destination (Join-Path $bundleInputDir "GreenC-$PackageVersion-x64.msix") -Force
    Copy-Item -LiteralPath $arm64Pkg.Path -Destination (Join-Path $bundleInputDir "GreenC-$PackageVersion-arm64.msix") -Force

    $stagedEntries = @(Get-ChildItem -LiteralPath $bundleInputDir -File)
    if ($stagedEntries.Count -ne 2) {
        throw "Bundle input directory must hold exactly 2 .msix files, found $($stagedEntries.Count)."
    }

    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    $ResolvedOutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
    $BundleFile = Join-Path $ResolvedOutputDir "GreenC-$PackageVersion.msixbundle"
    if (Test-Path -LiteralPath $BundleFile) {
        Remove-Item -LiteralPath $BundleFile -Force
    }

    Write-Host "Creating MSIX bundle with $MakeAppx -> $BundleFile"
    & $MakeAppx bundle /d $bundleInputDir /p $BundleFile /bv $PackageVersion /o
    if ($LASTEXITCODE -ne 0) {
        throw "makeappx bundle failed with exit code $LASTEXITCODE"
    }
    if (-not (Test-Path -LiteralPath $BundleFile -PathType Leaf)) {
        throw "makeappx bundle succeeded but output file is missing: $BundleFile"
    }

    # Verify the produced bundle by unbundling and inspecting AppxBundleManifest.xml
    $unbundleDir = Join-Path $TempRoot "unbundle"
    Write-Host "Verifying MSIX bundle via makeappx unbundle -> $unbundleDir"
    & $MakeAppx unbundle /p $BundleFile /d $unbundleDir /o
    if ($LASTEXITCODE -ne 0) {
        throw "makeappx unbundle failed with exit code $LASTEXITCODE"
    }

    $bundleManifestPath = Join-Path (Join-Path $unbundleDir "AppxMetadata") "AppxBundleManifest.xml"
    if (-not (Test-Path -LiteralPath $bundleManifestPath -PathType Leaf)) {
        throw "Unbundled archive is missing AppxMetadata\AppxBundleManifest.xml"
    }

    [xml]$bundleXml = Get-Content -LiteralPath $bundleManifestPath -Raw -Encoding UTF8
    $bNs = New-Object System.Xml.XmlNamespaceManager($bundleXml.NameTable)
    $bNs.AddNamespace("b", $bundleXml.DocumentElement.NamespaceURI)

    $bundleId = $bundleXml.SelectSingleNode("/b:Bundle/b:Identity", $bNs)
    if ($null -eq $bundleId) {
        throw "AppxBundleManifest.xml is missing /Bundle/Identity"
    }
    if ($bundleId.GetAttribute("Name") -cne $x64Pkg.Name) {
        throw "Bundle Identity Name '$($bundleId.GetAttribute('Name'))' != '$($x64Pkg.Name)'"
    }
    if ($bundleId.GetAttribute("Publisher") -cne $x64Pkg.Publisher) {
        throw "Bundle Identity Publisher '$($bundleId.GetAttribute('Publisher'))' != '$($x64Pkg.Publisher)'"
    }
    if ($bundleId.GetAttribute("Version") -cne $PackageVersion) {
        throw "Bundle Identity Version '$($bundleId.GetAttribute('Version'))' != '$PackageVersion'"
    }

    $allPkgNodes = @($bundleXml.SelectNodes("/b:Bundle/b:Packages/b:Package", $bNs))
    $appPkgNodes = @($allPkgNodes | Where-Object { $_.GetAttribute("Type") -ceq "application" })
    if ($allPkgNodes.Count -ne 2 -or $appPkgNodes.Count -ne 2) {
        throw "AppxBundleManifest.xml must contain exactly 2 Package entries with Type='application'; found total=$($allPkgNodes.Count) application=$($appPkgNodes.Count)."
    }

    $bundleArches = @()
    foreach ($node in $appPkgNodes) {
        $pkgArch = $node.GetAttribute("Architecture")
        $pkgVer = $node.GetAttribute("Version")
        if ($pkgVer -cne $PackageVersion) {
            throw "Bundle application entry '$pkgArch' has Version '$pkgVer', expected '$PackageVersion'."
        }
        $bundleArches += $pkgArch
    }
    $sortedArches = @($bundleArches | Sort-Object)
    if ($sortedArches.Count -ne 2 -or $sortedArches[0] -cne "arm64" -or $sortedArches[1] -cne "x64") {
        throw "AppxBundleManifest.xml application architectures must be exactly arm64 and x64; got: $($sortedArches -join ', ')"
    }

    Write-Host "Bundle manifest verification passed: Name=$($x64Pkg.Name) Version=$PackageVersion Architectures=$($sortedArches -join ', ')"

    # Build .msixupload (ZIP containing ONLY the .msixbundle)
    $MsixUploadFile = Join-Path $ResolvedOutputDir "GreenC-$PackageVersion.msixupload"
    if (Test-Path -LiteralPath $MsixUploadFile) {
        Remove-Item -LiteralPath $MsixUploadFile -Force
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $bundleEntryName = [System.IO.Path]::GetFileName($BundleFile)
    $zip = [System.IO.Compression.ZipFile]::Open($MsixUploadFile, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $zip,
            $BundleFile,
            $bundleEntryName,
            [System.IO.Compression.CompressionLevel]::Optimal
        ) | Out-Null
    } finally {
        $zip.Dispose()
    }

    # Verify .msixupload contains ONLY the expected .msixbundle entry
    $verifyZip = [System.IO.Compression.ZipFile]::OpenRead($MsixUploadFile)
    try {
        $entries = @($verifyZip.Entries)
        if ($entries.Count -ne 1 -or $entries[0].FullName -cne $bundleEntryName) {
            $names = @($entries | ForEach-Object { $_.FullName }) -join ", "
            throw ".msixupload verification failed: expected only '$bundleEntryName', found: $names"
        }
    } finally {
        $verifyZip.Dispose()
    }

    $bundleHash = (Get-FileHash -LiteralPath $BundleFile -Algorithm SHA256).Hash
    $uploadHash = (Get-FileHash -LiteralPath $MsixUploadFile -Algorithm SHA256).Hash
    Write-Host "Created Store bundle: $BundleFile (SHA256=$bundleHash)"
    Write-Host "Created Store upload: $MsixUploadFile (SHA256=$uploadHash)"
} finally {
    if (Test-Path -LiteralPath $TempRoot) {
        Remove-Item -LiteralPath $TempRoot -Recurse -Force
    }
}
