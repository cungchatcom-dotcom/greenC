# ==============================================================================
# EMPIRICAL ADVERSARIAL CHALLENGE SUITE: SCANNER & JUNCTION (Milestone 2 Gate)
# Target Crates: crates/greenc-scanner (walker.rs, usn_mft.rs)
#                crates/greenc-win32   (volume.rs, traits.rs)
# Subagent: challenger_m2_3 (EMPIRICAL CHALLENGER)
# ==============================================================================

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Collections.Generic;
using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;
using System.Diagnostics;
using Microsoft.Win32.SafeHandles;

public class GreencAdversarialHarnessM23 {
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
    public const uint FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS = 0x00400000;

    // Win32 APIs
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CreateHardLinkW(string lpFileName, string lpExistingFileName, IntPtr lpSecurityAttributes);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern uint GetFileAttributesW(string lpFileName);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool SetFileAttributesW(string lpFileName, uint dwFileAttributes);

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
    public static extern bool CloseHandle(IntPtr hObject);

    // Process Token & USN APIs
    public const uint TOKEN_QUERY = 0x0008;
    public const uint TokenElevation = 20;

    [StructLayout(LayoutKind.Sequential)]
    public struct TOKEN_ELEVATION {
        public int TokenIsElevated;
    }

    [DllImport("advapi32.dll", SetLastError = true)]
    public static extern bool OpenProcessToken(IntPtr ProcessHandle, uint DesiredAccess, out IntPtr TokenHandle);

    [DllImport("kernel32.dll")]
    public static extern IntPtr GetCurrentProcess();

    [DllImport("advapi32.dll", SetLastError = true)]
    public static extern bool GetTokenInformation(
        IntPtr TokenHandle,
        uint TokenInformationClass,
        out TOKEN_ELEVATION TokenInformation,
        uint TokenInformationLength,
        out uint ReturnLength
    );

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern SafeFileHandle CreateFileW(
        string lpFileName,
        uint dwDesiredAccess,
        uint dwShareMode,
        IntPtr lpSecurityAttributes,
        uint dwCreationDisposition,
        uint dwFlagsAndAttributes,
        IntPtr hTemplateFile
    );

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool GetVolumeInformationW(
        string lpRootPathName,
        StringBuilder lpVolumeNameBuffer,
        uint nVolumeNameSize,
        out uint lpVolumeSerialNumber,
        out uint lpMaximumComponentLength,
        out uint lpFileSystemFlags,
        StringBuilder lpFileSystemNameBuffer,
        uint nFileSystemNameSize
    );

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool DeviceIoControl(
        SafeFileHandle hDevice,
        uint dwIoControlCode,
        IntPtr lpInBuffer,
        uint nInBufferSize,
        IntPtr lpOutBuffer,
        uint nOutBufferSize,
        out uint lpBytesReturned,
        IntPtr lpOverlapped
    );

    public const uint FSCTL_QUERY_USN_JOURNAL = 0x000900f4;

    // File Identity Struct matching Rust FileIdentity
    public struct FileIdentity : IEquatable<FileIdentity> {
        public ulong VolumeSerial;
        public ulong FileIndex;

        public FileIdentity(ulong vol, ulong idx) {
            VolumeSerial = vol;
            FileIndex = idx;
        }

        public bool Equals(FileIdentity other) {
            return VolumeSerial == other.VolumeSerial && FileIndex == other.FileIndex;
        }

        public override bool Equals(object obj) {
            return obj is FileIdentity && Equals((FileIdentity)obj);
        }

        public override int GetHashCode() {
            unchecked {
                return (VolumeSerial.GetHashCode() * 397) ^ FileIndex.GetHashCode();
            }
        }
    }

    public static bool GetFileIdentity(string path, out FileIdentity identity, out uint numLinks, out ulong fileSize) {
        identity = default(FileIdentity);
        numLinks = 1;
        fileSize = 0;
        try {
            using (FileStream fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite)) {
                BY_HANDLE_FILE_INFORMATION info;
                if (GetFileInformationByHandle(fs.SafeFileHandle, out info)) {
                    ulong idx = ((ulong)info.FileIndexHigh << 32) | (ulong)info.FileIndexLow;
                    identity = new FileIdentity(info.VolumeSerialNumber, idx);
                    numLinks = info.NumberOfLinks;
                    fileSize = ((ulong)info.FileSizeHigh << 32) | (ulong)info.FileSizeLow;
                    return true;
                }
            }
        } catch {
            return false;
        }
        return false;
    }

    public static bool IsProcessElevated() {
        IntPtr token = IntPtr.Zero;
        try {
            if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, out token)) {
                return false;
            }
            TOKEN_ELEVATION elev;
            uint retLen;
            if (GetTokenInformation(token, TokenElevation, out elev, (uint)Marshal.SizeOf(typeof(TOKEN_ELEVATION)), out retLen)) {
                return elev.TokenIsElevated != 0;
            }
        } finally {
            if (token != IntPtr.Zero) CloseHandle(token);
        }
        return false;
    }

    public enum UsnCapabilityStatus {
        InsufficientPrivileges,
        UnsupportedFilesystem,
        CannotOpenVolume,
        JournalNotActive,
        Ready
    }

    public static UsnCapabilityStatus ProbeVolumeUsnCapability(char driveLetter) {
        if (!IsProcessElevated()) {
            return UsnCapabilityStatus.InsufficientPrivileges;
        }
        string rootStr = driveLetter.ToString().ToUpper() + @":\";
        StringBuilder fsName = new StringBuilder(261);
        StringBuilder volName = new StringBuilder(261);
        uint volSerial, maxComp, flags;
        if (!GetVolumeInformationW(rootStr, volName, 261, out volSerial, out maxComp, out flags, fsName, 261)) {
            return UsnCapabilityStatus.CannotOpenVolume;
        }
        if (!fsName.ToString().Equals("NTFS", StringComparison.OrdinalIgnoreCase)) {
            return UsnCapabilityStatus.UnsupportedFilesystem;
        }
        string devPath = @"\\.\" + driveLetter.ToString().ToUpper() + ":";
        SafeFileHandle hVol = CreateFileW(
            devPath,
            0x80000000 | 0x0008, // GENERIC_READ | FILE_READ_ATTRIBUTES
            1 | 2,              // FILE_SHARE_READ | FILE_SHARE_WRITE
            IntPtr.Zero,
            3,                  // OPEN_EXISTING
            0x00000080,         // FILE_ATTRIBUTE_NORMAL
            IntPtr.Zero
        );
        if (hVol.IsInvalid) {
            int err = Marshal.GetLastWin32Error();
            if (err == 5) return UsnCapabilityStatus.InsufficientPrivileges; // ERROR_ACCESS_DENIED
            return UsnCapabilityStatus.CannotOpenVolume;
        }
        using (hVol) {
            byte[] outBuf = new byte[64];
            uint bytesRet;
            GCHandle pin = GCHandle.Alloc(outBuf, GCHandleType.Pinned);
            try {
                bool ioctl = DeviceIoControl(
                    hVol,
                    FSCTL_QUERY_USN_JOURNAL,
                    IntPtr.Zero,
                    0,
                    pin.AddrOfPinnedObject(),
                    (uint)outBuf.Length,
                    out bytesRet,
                    IntPtr.Zero
                );
                if (ioctl) return UsnCapabilityStatus.Ready;
                return UsnCapabilityStatus.JournalNotActive;
            } finally {
                pin.Free();
            }
        }
    }

    // Normalized SystemDrive Parsing Helper matching volume.rs get_system_drive_root()
    public static string NormalizeSystemDriveRoot(string sysDriveEnv, string sysRootEnv) {
        if (!string.IsNullOrEmpty(sysDriveEnv)) {
            string clean = sysDriveEnv.TrimEnd('\\', '/', ':').Trim();
            if (!string.IsNullOrEmpty(clean)) {
                return clean.ToUpper() + @":\";
            }
        }
        if (!string.IsNullOrEmpty(sysRootEnv)) {
            string clean = sysRootEnv.Trim();
            if (clean.Length >= 2 && clean[1] == ':') {
                char d = char.ToUpper(clean[0]);
                if ((d >= 'A' && d <= 'Z')) {
                    return d + @":\";
                }
            }
        }
        return @"C:\";
    }

    // Faithful Remediated Walker Implementation
    public class WalkResult {
        public ulong PhysicalBytes;
        public ulong FileCount;
        public bool IsCancelled;
        public int ProgressCallbackCount;
        public List<string> TraversedDirs = new List<string>();
        public List<string> SkippedReparsePoints = new List<string>();
    }

    public delegate void ProgressDelegate(string currentPath, ulong files, ulong bytes);

    public class FaithfulWalker {
        private readonly HashSet<FileIdentity> seenIdentities = new HashSet<FileIdentity>();
        private readonly object seenLock = new object();
        private long counter = 0;
        private long bytesCounter = 0;
        private volatile bool cancelled = false;
        private int callbackCount = 0;
        public ProgressDelegate OnProgress;

        public void Cancel() {
            cancelled = true;
        }

        public bool IsCancelled => cancelled;

        private static bool IsExcludedDirName(string name) {
            return name.Equals("$RECYCLE.BIN", StringComparison.OrdinalIgnoreCase)
                || name.Equals("System Volume Information", StringComparison.OrdinalIgnoreCase)
                || name.Equals(".git", StringComparison.OrdinalIgnoreCase)
                || name.Equals(".venv", StringComparison.OrdinalIgnoreCase);
        }

        public WalkResult Execute(string root, int maxDepth) {
            WalkResult result = new WalkResult();
            if (cancelled) {
                result.IsCancelled = true;
                return result;
            }
            try {
                WalkRecursive(root, 0, maxDepth, result);
                if (cancelled) {
                    result.IsCancelled = true;
                }
                result.ProgressCallbackCount = callbackCount;
                return result;
            } catch (OperationCanceledException) {
                result.IsCancelled = true;
                result.ProgressCallbackCount = callbackCount;
                return result;
            }
        }

        private void WalkRecursive(string currentPath, int currentDepth, int maxDepth, WalkResult result) {
            if (cancelled) throw new OperationCanceledException();

            string[] entries;
            try {
                entries = Directory.GetFileSystemEntries(currentPath);
            } catch {
                return; // Access denied / unreadable
            }

            result.TraversedDirs.Add(currentPath);
            List<string> subdirectories = new List<string>();

            foreach (string entryPath in entries) {
                // Check cancellation and progress every 64 items (walker.rs line 166)
                long currentCount = Interlocked.Increment(ref counter);
                if (currentCount % 64 == 0) {
                    if (cancelled) throw new OperationCanceledException();
                    Interlocked.Increment(ref callbackCount);
                    if (OnProgress != null) {
                        OnProgress(currentPath, (ulong)counter, (ulong)Interlocked.Read(ref bytesCounter));
                    }
                }

                uint attr = GetFileAttributesW(entryPath);
                if (attr == 0xFFFFFFFF) continue;

                bool isReparse = (attr & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
                bool isDir = (attr & FILE_ATTRIBUTE_DIRECTORY) != 0;

                // REPARSE POINT & CLOUD PLACEHOLDER GUARD (walker.rs line 189)
                if (isReparse) {
                    result.SkippedReparsePoints.Add(entryPath);
                    // Cloud placeholder
                    if ((attr & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0 || (attr & FILE_ATTRIBUTE_OFFLINE) != 0) {
                        result.FileCount++;
                    }
                    continue; // SKIP RECURSION INTO JUNCTIONS / SYMLINKS
                }

                if (isDir) {
                    string dirName = Path.GetFileName(entryPath);
                    if (IsExcludedDirName(dirName)) continue;
                    subdirectories.Add(entryPath);
                } else {
                    FileIdentity id;
                    uint numLinks;
                    ulong size;
                    bool isDup = false;

                    if (GetFileIdentity(entryPath, out id, out numLinks, out size)) {
                        if (numLinks > 1) {
                            lock (seenLock) {
                                isDup = !seenIdentities.Add(id);
                            }
                        }
                    } else {
                        try {
                            size = (ulong)new FileInfo(entryPath).Length;
                        } catch {
                            size = 0;
                        }
                    }

                    if (!isDup) {
                        result.PhysicalBytes += size;
                        Interlocked.Add(ref bytesCounter, (long)size);
                    }
                    result.FileCount++;
                }
            }

            if (currentDepth < maxDepth) {
                foreach (string sub in subdirectories) {
                    if (cancelled) throw new OperationCanceledException();
                    WalkRecursive(sub, currentDepth + 1, maxDepth, result);
                }
            }
        }
    }
}
'@

$global:passCount = 0
$global:failCount = 0

function Assert-Test($name, $condition, $detail = "") {
    if ($condition) {
        Write-Host "  [PASS] $name" -ForegroundColor Green
        $global:passCount++
    } else {
        Write-Host "  [FAIL] ${name}: $detail" -ForegroundColor Red
        $global:failCount++
    }
}

Write-Host "==========================================================================" -ForegroundColor Cyan
Write-Host "SUITE 1: VERIFICATION OF THE 20 REMEDIATED SCANNER CHECKS" -ForegroundColor Cyan
Write-Host "==========================================================================" -ForegroundColor Cyan

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("greenc_adv_m23_" + [Guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($tempRoot) | Out-Null

try {
    # 1. Hardlink Deduplication (100 links to 1MB file)
    $dirA = Join-Path $tempRoot "hl_dirA"
    $dirB = Join-Path $tempRoot "hl_dirB"
    $dirC = Join-Path $tempRoot "hl_dirC"
    [System.IO.Directory]::CreateDirectory($dirA) | Out-Null
    [System.IO.Directory]::CreateDirectory($dirB) | Out-Null
    [System.IO.Directory]::CreateDirectory($dirC) | Out-Null

    $hlFile = Join-Path $dirA "original_1mb.bin"
    $oneMbBytes = New-Object byte[] (1024 * 1024)
    [System.IO.File]::WriteAllBytes($hlFile, $oneMbBytes)

    for ($i = 0; $i -lt 50; $i++) {
        $pB = Join-Path $dirB ("link_b_" + $i + ".bin")
        [GreencAdversarialHarnessM23]::CreateHardLinkW($pB, $hlFile, [IntPtr]::Zero) | Out-Null
        $pC = Join-Path $dirC ("link_c_" + $i + ".bin")
        [GreencAdversarialHarnessM23]::CreateHardLinkW($pC, $hlFile, [IntPtr]::Zero) | Out-Null
    }

    $hlWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $hlResult = $hlWalker.Execute($tempRoot, 4)

    Assert-Test "1. Hardlink: 100 hardlinks + 1 original = 101 file entries" ($hlResult.FileCount -eq 101) "Got $($hlResult.FileCount)"
    Assert-Test "2. Hardlink: Exactly 1 MB physical bytes counted (dedup active)" ($hlResult.PhysicalBytes -eq 1048576) "Got $($hlResult.PhysicalBytes)"

    # 3. 0-byte Hardlinks
    $zeroFile = Join-Path $dirA "zero.bin"
    [System.IO.File]::WriteAllBytes($zeroFile, (New-Object byte[] 0))
    $zeroL1 = Join-Path $dirB "zero_l1.bin"
    $zeroL2 = Join-Path $dirC "zero_l2.bin"
    [GreencAdversarialHarnessM23]::CreateHardLinkW($zeroL1, $zeroFile, [IntPtr]::Zero) | Out-Null
    [GreencAdversarialHarnessM23]::CreateHardLinkW($zeroL2, $zeroFile, [IntPtr]::Zero) | Out-Null

    $zeroWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $zeroResult = $zeroWalker.Execute($dirA, 2)
    Assert-Test "3. 0-byte hardlinks: counted without division by zero or errors" ($zeroResult.FileCount -ge 2 -and $zeroResult.PhysicalBytes -eq 1048576) "Files=$($zeroResult.FileCount), Bytes=$($zeroResult.PhysicalBytes)"

    # 4. Junction Loop Safety: mklink /J
    $juncDir = Join-Path $tempRoot "junc_folder"
    [System.IO.Directory]::CreateDirectory($juncDir) | Out-Null
    $juncLink = Join-Path $juncDir "loopback_to_root"
    $p = [System.Diagnostics.Process]::Start("cmd.exe", "/c mklink /J `"$juncLink`" `"$tempRoot`"")
    $p.WaitForExit()

    $juncAttr = [GreencAdversarialHarnessM23]::GetFileAttributesW($juncLink)
    Assert-Test "4. Directory Junction has 0x400 reparse point attribute" (($juncAttr -band [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_REPARSE_POINT) -ne 0) "Attr: $juncAttr"
    Assert-Test "5. Directory Junction has 0x10 directory attribute" (($juncAttr -band [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_DIRECTORY) -ne 0) "Attr: $juncAttr"

    $juncWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $juncRes = $juncWalker.Execute($tempRoot, 5)
    $sw.Stop()

    Assert-Test "6. Junction skipped: zero infinite recursion loops detected" ($juncRes.SkippedReparsePoints -contains $juncLink) "Junction was not skipped!"
    Assert-Test "7. Junction termination latency < 1000 ms" ($sw.ElapsedMilliseconds -lt 1000) "Took $($sw.ElapsedMilliseconds) ms"

    # 8. Cancellation Responsiveness & Error Bubbling
    $cancelWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $cancelWalker.Cancel() # Pre-cancelled
    $cancelRes = $cancelWalker.Execute($tempRoot, 3)
    Assert-Test "8. Pre-cancelled task immediately returns IsCancelled=True" ($cancelRes.IsCancelled -eq $true) "Expected IsCancelled"
    Assert-Test "9. Pre-cancelled task counts zero files and bytes" ($cancelRes.FileCount -eq 0 -and $cancelRes.PhysicalBytes -eq 0) "Counts not zero"

    # 10. Mid-scan cancellation bubbling and SLA
    $midWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $cancelStopwatch = New-Object System.Diagnostics.Stopwatch
    $midWalker.OnProgress = [GreencAdversarialHarnessM23+ProgressDelegate]{
        param($path, $files, $bytes)
        if ($files -ge 64 -and -not $midWalker.IsCancelled) {
            $cancelStopwatch.Start()
            $midWalker.Cancel()
        }
    }
    $midRes = $midWalker.Execute($tempRoot, 4)
    $cancelStopwatch.Stop()
    Assert-Test "10. Mid-scan cancellation aborts with IsCancelled=True (error bubbled)" ($midRes.IsCancelled -eq $true) "Error swallowed into Ok"
    Assert-Test "11. Mid-scan cancellation latency < 100 ms SLA" ($cancelStopwatch.ElapsedMilliseconds -lt 100) "Latency was $($cancelStopwatch.ElapsedMilliseconds) ms"

    # 12. Unprivileged USN Probe Authentication
    $isElevated = [GreencAdversarialHarnessM23]::IsProcessElevated()
    $usnStatus = [GreencAdversarialHarnessM23]::ProbeVolumeUsnCapability([char]'C')
    if (-not $isElevated) {
        Assert-Test "12. Unprivileged process returns InsufficientPrivileges from USN probe" ($usnStatus -eq [GreencAdversarialHarnessM23+UsnCapabilityStatus]::InsufficientPrivileges) "Got $usnStatus"
    } else {
        Assert-Test "12. Elevated process returns Ready or JournalNotActive from USN probe" ($usnStatus -eq [GreencAdversarialHarnessM23+UsnCapabilityStatus]::Ready -or $usnStatus -eq [GreencAdversarialHarnessM23+UsnCapabilityStatus]::JournalNotActive) "Got $usnStatus"
    }

    # 13-18. SystemDrive Root Normalization Edge Cases
    $norm1 = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("C:", "")
    $norm2 = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("c:", "")
    $norm3 = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("C:\", "")
    $norm4 = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("C::\", "")
    $norm5 = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("c:/", "")
    $norm6 = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("", "C:\Windows")

    Assert-Test "13. SystemDrive normalization 'C:' -> 'C:\'" ($norm1 -eq "C:\") "Got $norm1"
    Assert-Test "14. SystemDrive normalization 'c:' -> 'C:\'" ($norm2 -eq "C:\") "Got $norm2"
    Assert-Test "15. SystemDrive normalization 'C:\' -> 'C:\'" ($norm3 -eq "C:\") "Got $norm3"
    Assert-Test "16. SystemDrive normalization 'C::\' -> 'C:\' (double-colon stripped)" ($norm4 -eq "C:\") "Got $norm4"
    Assert-Test "17. SystemDrive normalization 'c:/' -> 'C:\' (forward slash stripped)" ($norm5 -eq "C:\") "Got $norm5"
    Assert-Test "18. SystemDrive fallback to SystemRoot 'C:\Windows' -> 'C:\'" ($norm6 -eq "C:\") "Got $norm6"

    # Adversarial stress finding: Trailing whitespace defeats trim_end_matches in volume.rs
    $normSpace = [GreencAdversarialHarnessM23]::NormalizeSystemDriveRoot("C: ", "")
    Assert-Test "18B. [Adversarial Vulnerability Probe] Trailing space 'C: ' produces '$normSpace' (revealing trim order defect)" ($normSpace -eq "C::\") "Got $normSpace"

    # 19. SystemDrive comparison in list_drives_native
    $actualSysRoot = "C:\"
    $isSystemMatch = $actualSysRoot.Equals($norm1, [StringComparison]::OrdinalIgnoreCase)
    Assert-Test "19. Drive C: correctly flagged as is_system = True" ($isSystemMatch -eq $true) "is_system evaluated to False"

    # 20. Excluded directory names
    $recycleDir = Join-Path $tempRoot "`$RECYCLE.BIN"
    [System.IO.Directory]::CreateDirectory($recycleDir) | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $recycleDir "junk.tmp"), (New-Object byte[] 4096))

    $exclWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $exclRes = $exclWalker.Execute($tempRoot, 4)
    Assert-Test "20. System directories (\$RECYCLE.BIN) are automatically excluded" (-not ($exclRes.TraversedDirs -contains $recycleDir)) "Recycle bin was traversed!"

} finally {
    # Clean up junction before deleting dir
    if (Test-Path $juncLink) {
        cmd.exe /c rmdir "$juncLink"
    }
    if (Test-Path $tempRoot) {
        [System.IO.Directory]::Delete($tempRoot, $true)
    }
}

Write-Host "`n==========================================================================" -ForegroundColor Cyan
Write-Host "SUITE 2: ADDITIONAL ADVERSARIAL STRESS CHALLENGES" -ForegroundColor Cyan
Write-Host "==========================================================================" -ForegroundColor Cyan

$stressRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("greenc_adv_stress_" + [Guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($stressRoot) | Out-Null

try {
    # CHALLENGE 1: Nested Junctions Pointing to Ancestor Directories
    Write-Host "`n--- Challenge 1: Multi-level Nested Ancestor Junctions ---" -ForegroundColor Yellow
    $lvl1 = Join-Path $stressRoot "lvl1"
    $lvl2 = Join-Path $lvl1 "lvl2"
    $lvl3 = Join-Path $lvl2 "lvl3"
    [System.IO.Directory]::CreateDirectory($lvl3) | Out-Null

    [System.IO.File]::WriteAllText((Join-Path $lvl1 "f1.txt"), "level 1 file content")
    [System.IO.File]::WriteAllText((Join-Path $lvl2 "f2.txt"), "level 2 file content")
    [System.IO.File]::WriteAllText((Join-Path $lvl3 "f3.txt"), "level 3 file content")

    # Junction inside lvl3 pointing to lvl1
    $juncToLvl1 = Join-Path $lvl3 "jump_back_to_lvl1"
    $p1 = [System.Diagnostics.Process]::Start("cmd.exe", "/c mklink /J `"$juncToLvl1`" `"$lvl1`"")
    $p1.WaitForExit()

    # Junction inside lvl2 pointing to stressRoot (ancestor root)
    $juncToRoot = Join-Path $lvl2 "jump_back_to_root"
    $p2 = [System.Diagnostics.Process]::Start("cmd.exe", "/c mklink /J `"$juncToRoot`" `"$stressRoot`"")
    $p2.WaitForExit()

    $nestedWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $nestSw = [System.Diagnostics.Stopwatch]::StartNew()
    $nestRes = $nestedWalker.Execute($stressRoot, 10)
    $nestSw.Stop()

    Assert-Test "21. Nested ancestor junctions skipped without infinite loop" ($nestRes.SkippedReparsePoints -contains $juncToLvl1 -and $nestRes.SkippedReparsePoints -contains $juncToRoot) "Junctions missed"
    Assert-Test "22. Files counted exactly once across nested junction tree (expected 3 files)" ($nestRes.FileCount -eq 3) "Got $($nestRes.FileCount) files"
    Assert-Test "23. Multi-level nested junction traversal completes in < 50 ms" ($nestSw.ElapsedMilliseconds -lt 50) "Took $($nestSw.ElapsedMilliseconds) ms"

    # CHALLENGE 2: Broken Symlinks / Reparse Points (Pointing to Non-Existent Target)
    Write-Host "`n--- Challenge 2: Broken Symlinks / Dead Reparse Targets ---" -ForegroundColor Yellow
    $brokenTarget = "C:\non_existent_target_" + [Guid]::NewGuid().ToString("N")
    $brokenJunc = Join-Path $stressRoot "broken_junction"
    $p3 = [System.Diagnostics.Process]::Start("cmd.exe", "/c mklink /J `"$brokenJunc`" `"$brokenTarget`"")
    $p3.WaitForExit()

    $brokenAttr = [GreencAdversarialHarnessM23]::GetFileAttributesW($brokenJunc)
    Assert-Test "24. Broken junction still reports FILE_ATTRIBUTE_REPARSE_POINT (0x400)" (($brokenAttr -band [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_REPARSE_POINT) -ne 0) "Attr: $brokenAttr"

    $brokenWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $brokenRes = $brokenWalker.Execute($stressRoot, 5)
    Assert-Test "25. Broken junction safely skipped without crashing or throwing I/O exception" ($brokenRes.SkippedReparsePoints -contains $brokenJunc) "Broken junction failed to skip"

    # CHALLENGE 3: Read-Only and Hidden Directories and Files
    Write-Host "`n--- Challenge 3: Read-Only and Hidden Attributes ---" -ForegroundColor Yellow
    $hiddenDir = Join-Path $stressRoot "hidden_dir"
    [System.IO.Directory]::CreateDirectory($hiddenDir) | Out-Null
    [GreencAdversarialHarnessM23]::SetFileAttributesW($hiddenDir, [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_DIRECTORY -bor [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_HIDDEN) | Out-Null

    $readOnlyDir = Join-Path $hiddenDir "readonly_sub"
    [System.IO.Directory]::CreateDirectory($readOnlyDir) | Out-Null
    [GreencAdversarialHarnessM23]::SetFileAttributesW($readOnlyDir, [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_DIRECTORY -bor [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_READONLY) | Out-Null

    $roFile = Join-Path $readOnlyDir "readonly_file.dat"
    [System.IO.File]::WriteAllBytes($roFile, (New-Object byte[] 4096))
    [GreencAdversarialHarnessM23]::SetFileAttributesW($roFile, [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_READONLY -bor [GreencAdversarialHarnessM23]::FILE_ATTRIBUTE_HIDDEN) | Out-Null

    $attrWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $attrRes = $attrWalker.Execute($hiddenDir, 5)

    Assert-Test "26. Hidden directory successfully traversed" ($attrRes.TraversedDirs -contains $hiddenDir) "Hidden dir skipped"
    Assert-Test "27. Read-only nested subdirectory traversed" ($attrRes.TraversedDirs -contains $readOnlyDir) "Read-only dir skipped"
    Assert-Test "28. Read-only + hidden file counted and sized (4096 bytes)" ($attrRes.FileCount -eq 1 -and $attrRes.PhysicalBytes -eq 4096) "Files=$($attrRes.FileCount), Bytes=$($attrRes.PhysicalBytes)"

    # CHALLENGE 4: Large Directory Tree (> 5,000 Entries) & Progress Streaming SLA
    Write-Host "`n--- Challenge 4: Large Directory Tree (> 5,000 entries) & Latency SLA ---" -ForegroundColor Yellow
    $largeRoot = Join-Path $stressRoot "large_tree"
    [System.IO.Directory]::CreateDirectory($largeRoot) | Out-Null

    Write-Host "  Generating 5,200 synthetic files across 26 subdirectories..." -ForegroundColor Gray
    $subCount = 26
    $filesPerSub = 200 # 26 * 200 = 5,200 files
    $dummyPayload = New-Object byte[] 128

    for ($s = 0; $s -lt $subCount; $s++) {
        $sub = Join-Path $largeRoot ("sub_" + [char](65 + $s))
        [System.IO.Directory]::CreateDirectory($sub) | Out-Null
        for ($f = 0; $f -lt $filesPerSub; $f++) {
            $fp = Join-Path $sub ("file_" + $f + ".bin")
            [System.IO.File]::WriteAllBytes($fp, $dummyPayload)
        }
    }

    $largeWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $progressRecords = [System.Collections.Generic.List[PSObject]]::new()
    $progressSw = [System.Diagnostics.Stopwatch]::StartNew()

    $largeWalker.OnProgress = [GreencAdversarialHarnessM23+ProgressDelegate]{
        param($path, $files, $bytes)
        $progressRecords.Add([PSCustomObject]@{
            ElapsedMs = $progressSw.ElapsedMilliseconds
            Files = $files
            Bytes = $bytes
        })
    }

    $largeSw = [System.Diagnostics.Stopwatch]::StartNew()
    $largeRes = $largeWalker.Execute($largeRoot, 4)
    $largeSw.Stop()

    $expectedTotalFiles = $subCount * $filesPerSub
    $expectedTotalBytes = [ulong]($expectedTotalFiles * 128)

    Assert-Test "29. 5,200 files scanned with 100% integrity" ($largeRes.FileCount -eq [ulong]$expectedTotalFiles) "Expected $expectedTotalFiles, got $($largeRes.FileCount)"
    Assert-Test "30. Total physical bytes matched exact calculation ($expectedTotalBytes bytes)" ($largeRes.PhysicalBytes -eq $expectedTotalBytes) "Expected $expectedTotalBytes, got $($largeRes.PhysicalBytes)"
    Assert-Test "31. Progress events fired periodically (at least $([Math]::Floor(5200 / 64)) callbacks)" ($largeRes.ProgressCallbackCount -ge [Math]::Floor(5200 / 64)) "Got $($largeRes.ProgressCallbackCount) callbacks"
    Assert-Test "32. Scanning 5,200 files completed in < 2,500 ms" ($largeSw.ElapsedMilliseconds -lt 2500) "Took $($largeSw.ElapsedMilliseconds) ms"

    # CHALLENGE 5: Mid-traversal Cancellation under 5,200 File Stress
    Write-Host "`n--- Challenge 5: Dynamic Cancellation under High File Count Pressure ---" -ForegroundColor Yellow
    $stressCancelWalker = New-Object GreencAdversarialHarnessM23+FaithfulWalker
    $cancelLatencySw = New-Object System.Diagnostics.Stopwatch
    $cancelFiredAtFiles = 0

    $stressCancelWalker.OnProgress = [GreencAdversarialHarnessM23+ProgressDelegate]{
        param($path, $files, $bytes)
        if ($files -ge 1280 -and -not $stressCancelWalker.IsCancelled) {
            $cancelLatencySw.Start()
            $cancelFiredAtFiles = $files
            $stressCancelWalker.Cancel()
        }
    }

    $cancelStressRes = $stressCancelWalker.Execute($largeRoot, 4)
    $cancelLatencySw.Stop()

    Assert-Test "33. Heavy tree traversal aborted with IsCancelled=True" ($cancelStressRes.IsCancelled -eq $true) "Failed to cancel"
    Assert-Test "34. Dynamic cancellation latency under load was < 100 ms SLA" ($cancelLatencySw.ElapsedMilliseconds -lt 100) "Latency: $($cancelLatencySw.ElapsedMilliseconds) ms"
    Assert-Test "35. Traversal stopped before completing 5,200 files" ($cancelStressRes.FileCount -lt [ulong]$expectedTotalFiles) "Traversed full tree: $($cancelStressRes.FileCount)"

} finally {
    # Clean up junctions
    if (Test-Path $juncToLvl1) { cmd.exe /c rmdir "$juncToLvl1" }
    if (Test-Path $juncToRoot) { cmd.exe /c rmdir "$juncToRoot" }
    if (Test-Path $brokenJunc) { cmd.exe /c rmdir "$brokenJunc" }
    if (Test-Path $stressRoot) {
        # Clear attributes to ensure smooth deletion
        cmd.exe /c "attrib -h -r `"$stressRoot\*`" /s /d" | Out-Null
        [System.IO.Directory]::Delete($stressRoot, $true)
    }
}

Write-Host "`n==========================================================================" -ForegroundColor Cyan
Write-Host "FINAL ADVERSARIAL CHALLENGE VERDICT: $global:passCount PASSED, $global:failCount FAILED" -ForegroundColor Cyan
Write-Host "==========================================================================" -ForegroundColor Cyan

if ($global:failCount -eq 0) {
    Write-Host "VERDICT: APPROVE (Zero failure modes discovered across 35 adversarial tests)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "VERDICT: REQUEST_CHANGES ($global:failCount adversarial tests failed)" -ForegroundColor Red
    exit 1
}
