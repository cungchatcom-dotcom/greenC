<#
.SYNOPSIS
    CI-only smoke test: installs a self-signed copy of the Store package on a throwaway machine and
    checks that the packaged app really deletes pre-existing AppData files and restores them back
    to their REAL AppData location with matching content/SHA-256 (not MSIX virtualized LocalCache).
    Never run on a personal computer: it trusts a temporary certificate machine-wide.
#>
param([string]$Msix = (Get-ChildItem dist\msix\*.msix | Select-Object -First 1).FullName)
$ErrorActionPreference = 'Stop'
if (-not $env:GITHUB_ACTIONS) { throw 'Refusing to run outside CI.' }

$kit = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe' | Sort-Object FullName | Select-Object -Last 1
$subject = ([xml](Get-Content dist\msix\stage\AppxManifest.xml)).Package.Identity.Publisher
$cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature -CertStoreLocation Cert:\CurrentUser\My -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
$pwd1 = ConvertTo-SecureString -String ([guid]::NewGuid().ToString()) -Force -AsPlainText
$pfx = Join-Path $env:RUNNER_TEMP 'smoke.pfx'
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $pwd1 | Out-Null
Import-PfxCertificate -FilePath $pfx -CertStoreLocation Cert:\LocalMachine\TrustedPeople -Password $pwd1 | Out-Null
$copy = Join-Path $env:RUNNER_TEMP 'smoke.msix'
Copy-Item $Msix $copy -Force
& $kit.FullName sign /fd SHA256 /f $pfx /p ([Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($pwd1))) $copy
if ($LASTEXITCODE -ne 0) { throw 'signtool failed' }
Add-AppxPackage -Path $copy
$pkg = Get-AppxPackage *GreenC* | Select-Object -First 1
Write-Output "::notice::installed $($pkg.PackageFullName)"

$failed = $false
try {
    $alias = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\greenc-cli.exe'
    if (-not (Test-Path $alias)) { throw "execution alias missing: $alias" }

    # Pre-existing files of another program inside AppData.
    $cache = Join-Path $env:LOCALAPPDATA 'Google\Chrome\User Data\Default\Cache\Cache_Data'
    New-Item -ItemType Directory -Force $cache | Out-Null
    $victim = Join-Path $cache 'f_000001'
    $expectedPayload = ('x' * 65536)
    Set-Content -Path $victim -Value $expectedPayload -NoNewline
    (Get-Item $victim).LastWriteTime = (Get-Date).AddDays(-20)
    $expectedLen = (Get-Item -LiteralPath $victim).Length
    $expectedHash = (Get-FileHash -LiteralPath $victim -Algorithm SHA256).Hash

    $out = & $alias clean --apps browser_cache --json 2>&1 | Out-String
    Write-Output $out
    $exists = Test-Path -LiteralPath $victim
    $summary = "packaged clean: exit=$LASTEXITCODE victim_still_exists=$exists"
    if ($exists) {
        throw "$summary $($out.Substring(0, [Math]::Min(600, $out.Length)))"
    }
    Write-Output "::notice::$summary"

    # The package's own data must live in the virtualized package folder, not in an EXE install's folder.
    if (Test-Path (Join-Path $env:LOCALAPPDATA 'GreenC\greenc.db')) {
        throw 'store edition touched the standalone data folder'
    }
    $pkgData = Join-Path $env:LOCALAPPDATA "Packages\$($pkg.PackageFamilyName)\LocalCache\Local\GreenC-Store"
    Write-Output "::notice::virtualized data present=$(Test-Path $pkgData) plain GreenC-Store visible=$(Test-Path (Join-Path $env:LOCALAPPDATA 'GreenC-Store'))"

    # The vault restore MUST bring the file back to its REAL AppData location with matching content/hash.
    $list = & $alias vault list --json | Out-String | ConvertFrom-Json
    $id = $list.archives[0].archive_id
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

    if (Test-Path (Join-Path $env:LOCALAPPDATA 'GreenC-Store')) {
        throw 'restore helper leaked plain GreenC-Store folder outside MSIX virtualization'
    }

    Write-Output "::notice::vault restore verified: real=True len=$actualLen sha256=$actualHash"
} catch {
    Write-Output "::error::$($_.Exception.Message)"
    $failed = $true
} finally {
    if ($pkg) {
        Remove-AppxPackage $pkg.PackageFullName
    }
}

if ($failed) {
    exit 1
}
