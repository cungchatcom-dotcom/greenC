<#
.SYNOPSIS
    CI-only smoke test: installs a self-signed copy of the Store package (.msix or .msixbundle)
    on a throwaway machine, asserts the installed architecture matches -ExpectedArchitecture,
    and checks that the packaged app really deletes pre-existing AppData files and restores them
    back to their REAL AppData location with matching content/SHA-256 (not MSIX virtualized LocalCache).
    Never run on a personal computer: it trusts a temporary certificate machine-wide.
#>
[CmdletBinding()]
param(
    [Alias("Msix")]
    [string]$PackagePath = "",
    [ValidateSet("x64", "arm64")]
    [string]$ExpectedArchitecture = $(
        switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()) {
            "arm64" { "arm64" }
            default { "x64" }
        }
    )
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $env:GITHUB_ACTIONS) {
    throw 'Refusing to run outside CI.'
}

function Find-WindowsSdkTool {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ToolName
    )

    $hostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    $preferredArches = if ($hostArch -eq 'arm64') { @('arm64', 'x64') } else { @('x64') }

    $kitRoots = @()
    foreach ($base in @(${env:ProgramFiles(x86)}, ${env:ProgramFiles})) {
        if (-not [string]::IsNullOrWhiteSpace($base)) {
            $binRoot = Join-Path (Join-Path (Join-Path $base 'Windows Kits') '10') 'bin'
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

function Get-PackageIdentityFromArchive {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ArchivePath
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $ext = [System.IO.Path]::GetExtension($ArchivePath).ToLowerInvariant()
    $targetManifest = switch ($ext) {
        '.msix'       { 'appxmanifest.xml' }
        '.msixbundle' { 'appxmetadata/appxbundlemanifest.xml' }
        default       { throw "Unsupported package extension '$ext'. Expected .msix or .msixbundle." }
    }
    $xpath = if ($ext -eq '.msix') { '/m:Package/m:Identity' } else { '/m:Bundle/m:Identity' }

    $zip = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entry = $null
        foreach ($candidate in $zip.Entries) {
            $normalized = $candidate.FullName.Replace('\', '/').ToLowerInvariant()
            if ($normalized -ceq $targetManifest) {
                $entry = $candidate
                break
            }
        }
        if ($null -eq $entry) {
            throw "Manifest '$targetManifest' not found inside '$ArchivePath'."
        }

        $stream = $entry.Open()
        try {
            $reader = New-Object System.IO.StreamReader($stream, [System.Text.Encoding]::UTF8)
            try {
                $manifestText = $reader.ReadToEnd()
            } finally {
                $reader.Dispose()
            }
        } finally {
            $stream.Dispose()
        }
    } finally {
        $zip.Dispose()
    }

    [xml]$xml = $manifestText
    $ns = New-Object System.Xml.XmlNamespaceManager($xml.NameTable)
    $ns.AddNamespace('m', $xml.DocumentElement.NamespaceURI)
    $idNode = $xml.SelectSingleNode($xpath, $ns)
    if ($null -eq $idNode) {
        throw "Identity node '$xpath' not found in '$targetManifest' inside '$ArchivePath'."
    }

    $name = $idNode.GetAttribute('Name')
    $publisher = $idNode.GetAttribute('Publisher')
    $version = $idNode.GetAttribute('Version')
    if ([string]::IsNullOrWhiteSpace($name) -or [string]::IsNullOrWhiteSpace($publisher)) {
        throw "Missing Identity Name or Publisher in '$ArchivePath'."
    }

    return [PSCustomObject]@{
        Name      = $name
        Publisher = $publisher
        Version   = $version
    }
}

if ([string]::IsNullOrWhiteSpace($PackagePath)) {
    $discovered = @(Get-ChildItem -Path (Join-Path 'dist' 'msix') -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Extension -in @('.msixbundle', '.msix') } |
        Sort-Object -Property Name)
    if ($discovered.Count -eq 0) {
        throw 'No .msix or .msixbundle file found under dist\msix. Pass -PackagePath explicitly.'
    }
    $PackagePath = $discovered[0].FullName
}

if (-not (Test-Path -LiteralPath $PackagePath -PathType Leaf)) {
    throw "Package not found: '$PackagePath'"
}
$resolvedPackage = (Resolve-Path -LiteralPath $PackagePath).Path
$ext = [System.IO.Path]::GetExtension($resolvedPackage).ToLowerInvariant()
if ($ext -ne '.msix' -and $ext -ne '.msixbundle') {
    throw "Unsupported package extension '$ext'. Expected .msix or .msixbundle."
}

$signTool = Find-WindowsSdkTool -ToolName 'signtool.exe'
if ([string]::IsNullOrWhiteSpace($signTool)) {
    throw 'signtool.exe not found in Windows Kits or PATH.'
}
Write-Host "Using signtool.exe: $signTool"

$identity = Get-PackageIdentityFromArchive -ArchivePath $resolvedPackage
$subject = $identity.Publisher
$expectedName = $identity.Name
Write-Host "Package identity from archive: Name=$expectedName Publisher=$subject Version=$($identity.Version)"

$cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature -CertStoreLocation Cert:\CurrentUser\My -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
$pwd1 = ConvertTo-SecureString -String ([guid]::NewGuid().ToString()) -Force -AsPlainText
$pfx = Join-Path $env:RUNNER_TEMP 'smoke.pfx'
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $pwd1 | Out-Null
Import-PfxCertificate -FilePath $pfx -CertStoreLocation Cert:\LocalMachine\TrustedPeople -Password $pwd1 | Out-Null

$copy = Join-Path $env:RUNNER_TEMP "smoke$ext"
Copy-Item -LiteralPath $resolvedPackage -Destination $copy -Force

$bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($pwd1)
try {
    $plainPwd = [Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
    & $signTool sign /fd SHA256 /f $pfx /p $plainPwd $copy
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed with exit code $LASTEXITCODE"
    }
} finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
}

$pkg = $null
$failed = $false
try {
    Add-AppxPackage -Path $copy
    $pkgs = @(Get-AppxPackage -Name $expectedName)
    if ($pkgs.Count -eq 0) {
        throw "Installed package '$expectedName' not found via Get-AppxPackage."
    }
    $pkg = $pkgs[0]
    $actualArch = "$($pkg.Architecture)".ToLowerInvariant()
    Write-Output "::notice::installed $($pkg.PackageFullName) arch=$($pkg.Architecture) expected=$ExpectedArchitecture"
    if ($actualArch -cne $ExpectedArchitecture) {
        throw "Installed package architecture mismatch: expected '$ExpectedArchitecture', got '$($pkg.Architecture)' ($($pkg.PackageFullName))"
    }

    $alias = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\greenc-cli.exe'
    if (-not (Test-Path -LiteralPath $alias -PathType Leaf)) {
        throw "execution alias missing: $alias"
    }

    # Pre-existing files of another program inside AppData.
    $cache = Join-Path $env:LOCALAPPDATA 'Google\Chrome\User Data\Default\Cache\Cache_Data'
    New-Item -ItemType Directory -Force $cache | Out-Null
    $victim = Join-Path $cache 'f_000001'
    $expectedPayload = ('x' * 65536)
    Set-Content -LiteralPath $victim -Value $expectedPayload -NoNewline
    (Get-Item -LiteralPath $victim).LastWriteTime = (Get-Date).AddDays(-20)
    $expectedLen = (Get-Item -LiteralPath $victim).Length
    $expectedHash = (Get-FileHash -LiteralPath $victim -Algorithm SHA256).Hash

    $out = & $alias clean --apps browser_cache --json 2>&1 | Out-String
    $cleanExit = $LASTEXITCODE
    Write-Output $out
    $exists = Test-Path -LiteralPath $victim
    $summary = "packaged clean: arch=$($pkg.Architecture) exit=$cleanExit victim_still_exists=$exists"
    if ($cleanExit -ne 0 -or $exists) {
        throw "$summary $($out.Substring(0, [Math]::Min(600, $out.Length)))"
    }
    Write-Output "::notice::$summary"

    # The package's own data must live in the virtualized package folder, not in an EXE install's folder.
    if (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'GreenC\greenc.db')) {
        throw 'store edition touched the standalone data folder'
    }
    $pkgData = Join-Path $env:LOCALAPPDATA "Packages\$($pkg.PackageFamilyName)\LocalCache\Local\GreenC-Store"
    Write-Output "::notice::virtualized data present=$(Test-Path -LiteralPath $pkgData) plain GreenC-Store visible=$(Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'GreenC-Store'))"

    # The vault restore MUST bring the file back to its REAL AppData location with matching content/hash.
    $listRaw = & $alias vault list --json | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "vault list exited with code $LASTEXITCODE : $listRaw"
    }
    $list = $listRaw | ConvertFrom-Json
    $archives = @($list.archives)
    if ($archives.Count -eq 0) {
        throw "vault list returned no archives after clean: $listRaw"
    }
    $id = $archives[0].archive_id
    $restoreOut = & $alias vault restore $id --json 2>&1 | Out-String
    $restoreExit = $LASTEXITCODE
    Write-Output $restoreOut

    $real = Test-Path -LiteralPath $victim
    $virtVictim = Join-Path $env:LOCALAPPDATA "Packages\$($pkg.PackageFamilyName)\LocalCache\Local\Google\Chrome\User Data\Default\Cache\Cache_Data\f_000001"
    $virt = Test-Path -LiteralPath $virtVictim
    Write-Output "::notice::vault restore: exit=$restoreExit real=$real virt=$virt"

    if ($restoreExit -ne 0) {
        $brief = $restoreOut -replace '\s+', ' '
        throw "vault restore exited with code $restoreExit (real=$real virt=$virt) $($brief.Substring(0, [Math]::Min(900, $brief.Length)))"
    }
    if (-not $real) {
        throw "vault restore failed to restore REAL AppData file (real=False, virt=$virt)"
    }

    $actualLen = (Get-Item -LiteralPath $victim).Length
    $actualHash = (Get-FileHash -LiteralPath $victim -Algorithm SHA256).Hash
    $actualText = Get-Content -LiteralPath $victim -Raw
    if ($actualLen -ne $expectedLen -or $actualHash -ne $expectedHash -or $actualText -ne $expectedPayload) {
        throw "restored file content mismatch: expected len=$expectedLen sha256=$expectedHash, got len=$actualLen sha256=$actualHash"
    }

    if (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'GreenC-Store')) {
        throw 'restore helper leaked plain GreenC-Store folder outside MSIX virtualization'
    }

    Write-Output "::notice::vault restore verified: arch=$($pkg.Architecture) real=True len=$actualLen sha256=$actualHash"
} catch {
    Write-Output "::error::$($_.Exception.Message)"
    $failed = $true
} finally {
    if ($null -ne $pkg) {
        Remove-AppxPackage -Package $pkg.PackageFullName
    }
    if (Test-Path -LiteralPath $copy) {
        Remove-Item -LiteralPath $copy -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $pfx) {
        Remove-Item -LiteralPath $pfx -Force -ErrorAction SilentlyContinue
    }
}

if ($failed) {
    exit 1
}
