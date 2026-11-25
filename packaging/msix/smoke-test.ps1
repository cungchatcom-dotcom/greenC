<#
.SYNOPSIS
    CI-only smoke test: installs a self-signed copy of the Store package on a throwaway machine and
    checks that the packaged app really deletes pre-existing AppData files (MSIX virtualization).
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

$alias = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\greenc-cli.exe'
if (-not (Test-Path $alias)) { throw "execution alias missing: $alias" }

# Pre-existing files of another program inside AppData.
$cache = Join-Path $env:LOCALAPPDATA 'Google\Chrome\User Data\Default\Cache\Cache_Data'
New-Item -ItemType Directory -Force $cache | Out-Null
$victim = Join-Path $cache 'f_000001'
Set-Content -Path $victim -Value ('x' * 65536)
(Get-Item $victim).LastWriteTime = (Get-Date).AddDays(-20)

$out = & $alias clean --apps browser_cache --json 2>&1 | Out-String
Write-Output $out
$exists = Test-Path $victim
$summary = "packaged clean: exit=$LASTEXITCODE victim_still_exists=$exists"
if ($exists) { Write-Output "::error::$summary $($out.Substring(0, [Math]::Min(600, $out.Length)))"; exit 1 }
Write-Output "::notice::$summary"

# The package's own data must live in the virtualized package folder, not in an EXE install's folder.
if (Test-Path (Join-Path $env:LOCALAPPDATA 'GreenC\greenc.db')) { Write-Output '::error::store edition touched the standalone data folder'; exit 1 }
$pkgData = Join-Path $env:LOCALAPPDATA "Packages\$($pkg.PackageFamilyName)\LocalCache\Local\GreenC-Store"
Write-Output "::notice::virtualized data present=$(Test-Path $pkgData) plain GreenC-Store visible=$(Test-Path (Join-Path $env:LOCALAPPDATA 'GreenC-Store'))"

# The vault restore must bring the file back to its real place.
$list = & $alias vault list --json | Out-String | ConvertFrom-Json
$id = $list.archives[0].archive_id
$restoreOut = & $alias vault restore $id --json 2>&1 | Out-String
Write-Output $restoreOut
$virtual = Join-Path $env:LOCALAPPDATA "Packages\$($pkg.PackageFamilyName)\LocalCache\Local\Google\Chrome\User Data\Default\Cache\Cache_Data\f_000001"
Write-Output "::notice::restore exit=$LASTEXITCODE real=$(Test-Path $victim) virtualized=$(Test-Path $virtual) out=$($restoreOut.Substring(0, [Math]::Min(350, $restoreOut.Length)) -replace '\s+', ' ')"
Remove-AppxPackage $pkg.PackageFullName
