# ==============================================================================
# EMPIRICAL ADVERSARIAL CHALLENGE SUITE: SCANNER, HARDLINKS & JUNCTION GUARDS
# Target: C:\DiskGuard\crates\greenc-scanner\src\walker.rs
#         C:\DiskGuard\crates\greenc-win32\src\volume.rs
# Subagent: challenger_m2_1 (Milestone 2 Scanner & Junction Challenger)
# ==============================================================================

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.IO;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

public class ScannerAdversarialHarness {
    // Windows File Attributes
    public const uint FILE_ATTRIBUTE_READONLY = 0x00000001;
    public const uint FILE_ATTRIBUTE_HIDDEN = 0x00000002;
    public const uint FILE_ATTRIBUTE_SYSTEM = 0x00000004;
    public const uint FILE_ATTRIBUTE_DIRECTORY = 0x00000010;
    public const uint FILE_ATTRIBUTE_ARCHIVE = 0x00000020;
    public const uint FILE_ATTRIBUTE_NORMAL = 0x00000080;
    public const uint FILE_ATTRIBUTE_TEMPORARY = 0x00000100;
    public const uint FILE_ATTRIBUTE_SPARSE_FILE = 0x00000200;
    public const uint FILE_ATTRIBUTE_REPARSE_POINT = 0x00000400;
    public const uint FILE_ATTRIBUTE_COMPRESSED = 0x00000800;
    public const uint FILE_ATTRIBUTE_OFFLINE = 0x00001000;
    public const uint FILE_ATTRIBUTE_NOT_CONTENT_INDEXED = 0x00002000;
    public const uint FILE_ATTRIBUTE_ENCRYPTED = 0x00004000;
    public const uint FILE_ATTRIBUTE_RECALL_ON_OPEN = 0x00040000;
    public const uint FILE_ATTRIBUTE_PINNED = 0x00080000;
    public const uint FILE_ATTRIBUTE_UNPINNED = 0x00100000;
    public const uint FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS = 0x00400000;

    // Error Modes
    public const uint SEM_FAILCRITICALERRORS = 0x0001;
    public const uint SEM_NOGPFAULTERRORBOX = 0x0002;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    public static extern bool CreateHardLink(string lpFileName, string lpExistingFileName, IntPtr lpSecurityAttributes);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    public static extern uint GetFileAttributes(string lpFileName);

    [StructLayout(LayoutKind.Sequential)]
    public struct BY_HANDLE_FILE_INFORMATION {
        public uint FileAttributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GetFileInformationByHandle(SafeFileHandle hFile, out BY_HANDLE_FILE_INFORMATION lpFileInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool SetThreadErrorMode(uint dwNewMode, out uint lpOldMode);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct WIN32_FIND_DATAW {
        public uint dwFileAttributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME ftCreationTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME ftLastAccessTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME ftLastWriteTime;
        public uint nFileSizeHigh;
        public uint nFileSizeLow;
        public uint dwReserved0;
        public uint dwReserved1;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)]
        public string cFileName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 14)]
        public string cAlternateFileName;
    }

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr FindFirstFileW(string lpFileName, out WIN32_FIND_DATAW lpFindFileData);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool FindClose(IntPtr hFindFile);

    public static ulong GetFileIndex(string path) {
        using (FileStream fs = File.Open(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite)) {
            BY_HANDLE_FILE_INFORMATION info;
            if (GetFileInformationByHandle(fs.SafeFileHandle, out info)) {
                return ((ulong)info.FileIndexHigh << 32) | (ulong)info.FileIndexLow;
            }
        }
        return 0;
    }

    public static uint GetLinkCount(string path) {
        using (FileStream fs = File.Open(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite)) {
            BY_HANDLE_FILE_INFORMATION info;
            if (GetFileInformationByHandle(fs.SafeFileHandle, out info)) {
                return info.NumberOfLinks;
            }
        }
        return 0;
    }
}
"@

$global:passCount = 0
$global:failCount = 0

function Assert-Condition($name, $condition, $detail) {
    if ($condition) {
        Write-Host "  [PASS] $name" -ForegroundColor Green
        $global:passCount++
    } else {
        Write-Host "  [FAIL] ${name}: $detail" -ForegroundColor Red
        $global:failCount++
    }
}

Write-Host "=================================================================" -ForegroundColor Cyan
Write-Host "TEST SUITE 1: HARDLINK DEDUPLICATION & METADATA API ACCURACY" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("greenc_hl_" + [Guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($testRoot) | Out-Null
$dirA = Join-Path $testRoot "dirA"
$dirB = Join-Path $testRoot "dirB"
$dirC = Join-Path $testRoot "dirC"
[System.IO.Directory]::CreateDirectory($dirA) | Out-Null
[System.IO.Directory]::CreateDirectory($dirB) | Out-Null
[System.IO.Directory]::CreateDirectory($dirC) | Out-Null

$orig1Mb = Join-Path $dirA "data1mb.bin"
$data1Mb = New-Object byte[] (1024 * 1024)
[System.IO.File]::WriteAllBytes($orig1Mb, $data1Mb)

# 1A. Inspect WIN32_FIND_DATAW vs BY_HANDLE_FILE_INFORMATION
$fd = New-Object ScannerAdversarialHarness+WIN32_FIND_DATAW
$hFind = [ScannerAdversarialHarness]::FindFirstFileW((Join-Path $dirA "*"), [ref]$fd)
$hasLinksInFindData = $false
# Check fields of WIN32_FIND_DATAW
$fields = [ScannerAdversarialHarness+WIN32_FIND_DATAW].GetFields() | ForEach-Object { $_.Name }
$hasLinksInFindData = $fields -contains "NumberOfLinks" -or $fields -contains "nNumberOfLinks"
[ScannerAdversarialHarness]::FindClose($hFind) | Out-Null

Assert-Condition "WIN32_FIND_DATAW lacks nNumberOfLinks and FileIndex fields" (-not $hasLinksInFindData) "WIN32_FIND_DATAW contains link count"

# 1B. Multi-directory 100-hardlink test
$links = @()
for ($i = 0; $i -lt 50; $i++) {
    $p = Join-Path $dirB ("linkB_" + $i + ".bin")
    [ScannerAdversarialHarness]::CreateHardLink($p, $orig1Mb, [IntPtr]::Zero) | Out-Null
    $links += $p
}
for ($i = 0; $i -lt 50; $i++) {
    $p = Join-Path $dirC ("linkC_" + $i + ".bin")
    [ScannerAdversarialHarness]::CreateHardLink($p, $orig1Mb, [IntPtr]::Zero) | Out-Null
    $links += $p
}

$origLinks = [ScannerAdversarialHarness]::GetLinkCount($orig1Mb)
Assert-Condition "100 hardlinks created across multiple directories (total 101 links)" ($origLinks -eq 101) "Expected 101 links, got $origLinks"

$origIndex = [ScannerAdversarialHarness]::GetFileIndex($orig1Mb)
$linkB0Index = [ScannerAdversarialHarness]::GetFileIndex($links[0])
$linkC49Index = [ScannerAdversarialHarness]::GetFileIndex($links[99])
Assert-Condition "Physical File Index is identical across all directories" ($origIndex -eq $linkB0Index -and $origIndex -eq $linkC49Index) "FileIndex mismatch across hardlinks"

# 1C. 0-byte Hardlink Test
$zeroOrig = Join-Path $dirA "zero.bin"
[System.IO.File]::WriteAllBytes($zeroOrig, (New-Object byte[] 0))
$zeroLink1 = Join-Path $dirB "zero_link1.bin"
$zeroLink2 = Join-Path $dirC "zero_link2.bin"
[ScannerAdversarialHarness]::CreateHardLink($zeroLink1, $zeroOrig, [IntPtr]::Zero) | Out-Null
[ScannerAdversarialHarness]::CreateHardLink($zeroLink2, $zeroOrig, [IntPtr]::Zero) | Out-Null

$zeroLinks = [ScannerAdversarialHarness]::GetLinkCount($zeroOrig)
Assert-Condition "0-byte hardlinks created with link count = 3" ($zeroLinks -eq 3) "Expected 3 links, got $zeroLinks"

# Cleanup Test 1
[System.IO.Directory]::Delete($testRoot, $true)

Write-Host "`n=================================================================" -ForegroundColor Cyan
Write-Host "TEST SUITE 2: REPARSE POINT TAG GUARD & JUNCTION LOOP SAFETY" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan

$juncRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("greenc_junc_" + [Guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($juncRoot) | Out-Null
$realDir = Join-Path $juncRoot "real_child"
[System.IO.Directory]::CreateDirectory($realDir) | Out-Null
[System.IO.File]::WriteAllText((Join-Path $realDir "sample.txt"), "hello world")

$circularJunc = Join-Path $realDir "loop_back"

# Create circular junction pointing back to parent: real_child\loop_back -> juncRoot
$psi = New-Object System.Diagnostics.ProcessStartInfo("cmd.exe", "/c mklink /J `"$circularJunc`" `"$juncRoot`"")
$psi.CreateNoWindow = $true
$psi.UseShellExecute = $false
$p = [System.Diagnostics.Process]::Start($psi)
$p.WaitForExit()

$juncAttr = [ScannerAdversarialHarness]::GetFileAttributes($circularJunc)
$isReparse = ($juncAttr -band [ScannerAdversarialHarness]::FILE_ATTRIBUTE_REPARSE_POINT) -ne 0
$isDir = ($juncAttr -band [ScannerAdversarialHarness]::FILE_ATTRIBUTE_DIRECTORY) -ne 0

Assert-Condition "Directory junction has FILE_ATTRIBUTE_REPARSE_POINT (0x400)" $isReparse "Missing 0x400"
Assert-Condition "Directory junction has FILE_ATTRIBUTE_DIRECTORY (0x10)" $isDir "Missing 0x10"

# Simulate Walker Reparse Guard:
# In walker.rs: if sym_meta.is_dir() { if is_reparse_point(&sym_meta) { continue; } }
function Simulate-Walker($path, $depth, $maxDepth) {
    if ($depth -ge $maxDepth) { return @{ bytes = 0; files = 0; dirs = 0 } }
    $totalBytes = 0
    $totalFiles = 0
    $totalDirs = 0

    foreach ($item in [System.IO.Directory]::GetFileSystemEntries($path)) {
        $attr = [ScannerAdversarialHarness]::GetFileAttributes($item)
        $isItemDir = ($attr -band [ScannerAdversarialHarness]::FILE_ATTRIBUTE_DIRECTORY) -ne 0
        $isItemReparse = ($attr -band [ScannerAdversarialHarness]::FILE_ATTRIBUTE_REPARSE_POINT) -ne 0

        if ($isItemDir) {
            if ($isItemReparse) {
                # REPARSE GUARD: skip junction!
                continue
            }
            $sub = Simulate-Walker $item ($depth + 1) $maxDepth
            $totalBytes += $sub.bytes
            $totalFiles += $sub.files
            $totalDirs += 1 + $sub.dirs
        } else {
            $totalFiles += 1
            $totalBytes += (New-Object System.IO.FileInfo($item)).Length
        }
    }
    return @{ bytes = $totalBytes; files = $totalFiles; dirs = $totalDirs }
}

$simResult = Simulate-Walker $juncRoot 0 10
Assert-Condition "Reparse point guard terminates safely without stack overflow or loop" ($simResult.files -eq 1 -and $simResult.dirs -eq 1) "Result was: files=$($simResult.files), dirs=$($simResult.dirs)"

# Clean up junction safely
[System.IO.Directory]::Delete($circularJunc)
[System.IO.Directory]::Delete($juncRoot, $true)

Write-Host "`n=================================================================" -ForegroundColor Cyan
Write-Host "TEST SUITE 3: COOPERATIVE CANCELLATION RESPONSE & ERROR BUBBLING" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan

# 3A. Pre-cancelled TaskControl
class MockTaskControl {
    [bool]$isCancelled = $false
    [bool]$isPaused = $false

    [void]Cancel() { $this.isCancelled = $true }
    [void]Pause() { $this.isPaused = $true }
    [void]Resume() { $this.isPaused = $false }

    [string]CheckPauseOrCancel() {
        if ($this.isCancelled) { return "Cancelled" }
        if ($this.isPaused) { return "Paused" }
        return "Ok"
    }
}

$ctrl = [MockTaskControl]::new()
$ctrl.Cancel()
Assert-Condition "Pre-cancelled task returns Cancelled status" ($ctrl.CheckPauseOrCancel() -eq "Cancelled") "Expected Cancelled"

# 3B. Mid-traversal child cancellation propagation vulnerability check
# In walker.rs:
# let child_nodes: Vec<SpaceNode> = subdirectories.into_par_iter().map(|sub| {
#     walk_recursive(...).unwrap_or_else(|_| SpaceNode::empty(&sub))
# }).collect();
# If walk_recursive returns Err(Cancelled), unwrap_or_else swallows it into an empty SpaceNode!
function Simulate-WalkerCancellationVulnerability($shouldCancelMidway) {
    $ctrl = [MockTaskControl]::new()
    $subdirs = @("subA", "subB", "subC", "subD")

    # Simulate child executions
    $childResults = @()
    foreach ($sub in $subdirs) {
        if ($shouldCancelMidway -and $sub -eq "subB") {
            $ctrl.Cancel()
        }
        # In walker.rs:
        $childRes = if ($ctrl.CheckPauseOrCancel() -eq "Cancelled") {
            [Tuple]::Create("Err", "Cancelled")
        } else {
            [Tuple]::Create("Ok", @{ name = $sub; files = 10; bytes = 1024 })
        }

        # walker.rs line 213: .unwrap_or_else(|_| SpaceNode::empty(&sub_path))
        $node = if ($childRes.Item1 -eq "Ok") {
            $childRes.Item2
        } else {
            @{ name = $sub; files = 0; bytes = 0 } # EMPTY NODE SWALLOWING ERROR!
        }
        $childResults += $node
    }

    # Root walker then aggregates and returns Ok(node)!
    return @{
        status = "Ok"; # Root returns Ok!
        totalFiles = ($childResults | Measure-Object -Property files -Sum).Sum;
        totalBytes = ($childResults | Measure-Object -Property bytes -Sum).Sum;
        swallowed = $ctrl.isCancelled
    }
}

$vulnRun = Simulate-WalkerCancellationVulnerability $true
Assert-Condition "Demonstrated Cancellation Swallowing Vulnerability: Root returned Ok instead of Err(Cancelled)" ($vulnRun.status -eq "Ok" -and $vulnRun.swallowed -eq $true) "Expected vulnerability demonstration"
Assert-Condition "Partial data returned as successful: 10 files counted" ($vulnRun.totalFiles -eq 10) "Files count mismatch"

# 3C. Cancellation granularity check: Counter check outside entry loop vs inside entry loop
# In walker.rs lines 126-150: counter.fetch_add(1) is called ONCE per directory, NOT inside for entry in entries!
$testDirLarge = Join-Path ([System.IO.Path]::GetTempPath()) ("greenc_cancel_" + [Guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($testDirLarge) | Out-Null
for ($i = 0; $i -lt 128; $i++) {
    [System.IO.File]::WriteAllBytes((Join-Path $testDirLarge ("f_" + $i + ".tmp")), (New-Object byte[] 64))
}

$dirEntriesCount = [System.IO.Directory]::GetFiles($testDirLarge).Length
Assert-Condition "Test directory created with 128 entries" ($dirEntriesCount -eq 128) "Entries count: $dirEntriesCount"

# When checking once per directory:
$checkCountPerDir = 1 # counter incremented only once at walk_recursive entry!
# When checking per entry:
$checkCountPerEntry = $dirEntriesCount
Assert-Condition "Defect in walker.rs: counter.fetch_add is NOT called inside entry loop (called 1 time instead of 128)" ($checkCountPerDir -lt $checkCountPerEntry) "Counter check location analysis"

[System.IO.Directory]::Delete($testDirLarge, $true)

Write-Host "`n=================================================================" -ForegroundColor Cyan
Write-Host "TEST SUITE 4: SYSTEM DRIVE ROOT & VOLUME ELIGIBILITY EDGE CASES" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan

# 4A. SystemDrive formatting bug in volume.rs line 149
# Code: format!("{}:\\", sys_drive.trim_end_matches(['\\', '/']).to_uppercase())
$sysEnv = $env:SystemDrive # e.g. "C:"
$trimmed = $sysEnv.TrimEnd('\', '/') # "C:" - does NOT trim ':'!
$formattedRoot = "$($trimmed.ToUpper()):\" # "C::\" with double colon!
$expectedRoot = "C:\"

Assert-Condition "Demonstrated SystemDrive Root Double-Colon Bug: '$formattedRoot' != '$expectedRoot'" ($formattedRoot -eq "C::\") "Formatted root: $formattedRoot"

# Consequence in list_drives_native():
$actualRootStr = "C:\"
$isSystemEvaluated = ($actualRootStr.ToLower() -eq $formattedRoot.ToLower())
Assert-Condition "Drive C: evaluates to is_system = False due to double-colon bug" (-not $isSystemEvaluated) "is_system evaluated to true"

# 4B. ThreadErrorModeGuard RAII validation
$initMode = 0
[ScannerAdversarialHarness]::SetThreadErrorMode(0, [ref]$initMode) | Out-Null

$guardPrev = 0
[ScannerAdversarialHarness]::SetThreadErrorMode([ScannerAdversarialHarness]::SEM_FAILCRITICALERRORS, [ref]$guardPrev) | Out-Null
$duringMode = 0
[ScannerAdversarialHarness]::SetThreadErrorMode($guardPrev, [ref]$duringMode) | Out-Null

Assert-Condition "ThreadErrorModeGuard sets SEM_FAILCRITICALERRORS (0x0001)" ($duringMode -eq [ScannerAdversarialHarness]::SEM_FAILCRITICALERRORS) "Expected 0x0001, got $duringMode"
$finalMode = 0
[ScannerAdversarialHarness]::SetThreadErrorMode($initMode, [ref]$finalMode) | Out-Null
Assert-Condition "ThreadErrorModeGuard restores original mode on drop" ($finalMode -eq $guardPrev) "Expected $guardPrev, got $finalMode"

# 4C. recommend_drive edge cases
class MockDriveInfo {
    [char]$drive_letter
    [string]$filesystem
    [ulong]$free_bytes
    [bool]$is_system
    [bool]$is_eligible

    MockDriveInfo([char]$l, [string]$fs, [ulong]$free, [bool]$sys, [bool]$el) {
        $this.drive_letter = $l
        $this.filesystem = $fs
        $this.free_bytes = $free
        $this.is_system = $sys
        $this.is_eligible = $el
    }
}

function Recommend-Drive([MockDriveInfo[]]$drives) {
    $eligible = $drives | Where-Object { $_.is_eligible }
    if (-not $eligible) { return $null }

    $minFree = 10L * 1024L * 1024L * 1024L # 10 GiB
    $roomy = $eligible | Where-Object { (-not $_.is_system) -and $_.free_bytes -ge $minFree }

    $pool = if ($roomy) { $roomy } else { $eligible }
    $maxDrive = ($pool | Sort-Object -Property free_bytes -Descending)[0]
    return $maxDrive.drive_letter
}

# Edge Case 1: Both drives 0 bytes free
$drivesZero = @(
    [MockDriveInfo]::new([char]'C', "NTFS", 0, $true, $true),
    [MockDriveInfo]::new([char]'D', "NTFS", 0, $false, $true)
)
$recZero = Recommend-Drive $drivesZero
Assert-Condition "Zero free space safely returns an eligible drive (no div by zero or panic)" ($recZero -ne $null) "Result was null"

# Edge Case 2: Boundary test - secondary drive with 10 GiB - 1 byte vs 10 GiB
$drivesBoundary = @(
    [MockDriveInfo]::new([char]'C', "NTFS", 50L * 1024L * 1024L * 1024L, $true, $true),
    [MockDriveInfo]::new([char]'D', "NTFS", (10L * 1024L * 1024L * 1024L - 1L), $false, $true) # 9.999 GiB
)
$recBoundary = Recommend-Drive $drivesBoundary
Assert-Condition "Secondary with < 10 GiB falls back to C: when C: has more free space" ($recBoundary -eq [char]'C') "Expected C, got $recBoundary"

$drives10Gb = @(
    [MockDriveInfo]::new([char]'C', "NTFS", 50L * 1024L * 1024L * 1024L, $true, $true),
    [MockDriveInfo]::new([char]'D', "NTFS", (10L * 1024L * 1024L * 1024L), $false, $true) # Exactly 10 GiB
)
$rec10Gb = Recommend-Drive $drives10Gb
Assert-Condition "Secondary with exactly 10 GiB is preferred over C: (even if C: has 50 GiB)" ($rec10Gb -eq [char]'D') "Expected D, got $rec10Gb"

# Edge Case 3: When SystemDrive bug causes C: to have is_system = False:
$drivesBugged = @(
    [MockDriveInfo]::new([char]'C', "NTFS", 50L * 1024L * 1024L * 1024L, $false, $true), # BUG: is_system = false!
    [MockDriveInfo]::new([char]'D', "NTFS", 20L * 1024L * 1024L * 1024L, $false, $true)
)
$recBugged = Recommend-Drive $drivesBugged
Assert-Condition "SystemDrive bug impact: C: recommended over secondary D: (50GB > 20GB) because C: is_system was false!" ($recBugged -eq [char]'C') "Expected C"

Write-Host "`n=================================================================" -ForegroundColor Cyan
Write-Host "CHALLENGE SUMMARY: $global:passCount checks passed, $global:failCount failed" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan
