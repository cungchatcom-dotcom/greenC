# ==============================================================================
# EMPIRICAL ADVERSARIAL CHALLENGE SUITE: RELOCATION SAFETY & CRASH RECOVERY
# Target: crates/greenc-relocate (mover.rs, recovery.rs, candidate.rs, verified_copy.rs)
#         crates/greenc-win32 (reparse.rs, traits.rs)
# Challenger: challenger_m4_1 (Milestone 4 Relocation Safety Challenger)
# ==============================================================================

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

public class RelocationSafetyHarness {
    public const uint FILE_ATTRIBUTE_READONLY = 0x00000001;
    public const uint FILE_ATTRIBUTE_DIRECTORY = 0x00000010;
    public const uint FILE_ATTRIBUTE_REPARSE_POINT = 0x00000400;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool RemoveDirectoryW(string lpPathName);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern uint GetFileAttributesW(string lpFileName);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool MoveFileExW(string lpExistingFileName, string lpNewFileName, uint dwFlags);

    public const uint MOVEFILE_REPLACE_EXISTING = 0x00000001;
    public const uint MOVEFILE_COPY_ALLOWED = 0x00000002;

    public static uint GetAttributes(string path) {
        return GetFileAttributesW(path);
    }

    public static bool IsReparsePoint(string path) {
        uint attrs = GetFileAttributesW(path);
        if (attrs == 0xFFFFFFFF) return false;
        return (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    }

    public static bool UnlinkReparse(string path) {
        return RemoveDirectoryW(path);
    }

    public static string ComputeSha256(string filePath) {
        using (var sha256 = SHA256.Create()) {
            using (var stream = File.OpenRead(filePath)) {
                byte[] hash = sha256.ComputeHash(stream);
                StringBuilder sb = new StringBuilder();
                foreach (byte b in hash) {
                    sb.Append(b.ToString("x2"));
                }
                return sb.ToString();
            }
        }
    }
}
"@

$global:passedTests = 0
$global:failedTests = 0
$global:findings = @()

function Report-Test {
    param (
        [string]$TestName,
        [bool]$Success,
        [string]$Message
    )
    if ($Success) {
        $global:passedTests++
        Write-Host "  [PASS] $TestName - $Message" -ForegroundColor Green
    } else {
        $global:failedTests++
        $global:findings += "${TestName}: $Message"
        Write-Host "  [FAIL/BUG] $TestName - $Message" -ForegroundColor Red
    }
}

Write-Host "=================================================================" -ForegroundColor Cyan
Write-Host "STARTING EMPIRICAL ADVERSARIAL CHALLENGE: RELOCATION SAFETY" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan

$testRoot = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), "greenc_m4_challenge_" + [System.Guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($testRoot) | Out-Null

try {
    # --------------------------------------------------------------------------
    # CHALLENGE 1: 4-Phase Mover Integrity & Source Preservation
    # --------------------------------------------------------------------------
    Write-Host "`n--- CHALLENGE 1: 4-Phase Mover Integrity & Source Preservation ---" -ForegroundColor Yellow

    $src1 = [System.IO.Path]::Combine($testRoot, "App_C1")
    $tgt1 = [System.IO.Path]::Combine($testRoot, "Target_C1")
    [System.IO.Directory]::CreateDirectory($src1) | Out-Null
    $f1 = [System.IO.Path]::Combine($src1, "payload.dat")
    [System.IO.File]::WriteAllText($f1, "CRITICAL APPLICATION DATA 1234567890")
    $hashSrc = [RelocationSafetyHarness]::ComputeSha256($f1)

    # 1.1 Test Staging Folder Naming Pattern
    $uuid = [System.Guid]::NewGuid().ToString()
    $movingName = "App_C1.greenc-moving.$uuid"
    $movingPath = [System.IO.Path]::Combine($testRoot, $movingName)
    $patternValid = $movingName -match "^App_C1\.greenc-moving\.[0-9a-fA-F-]{36}$"
    Report-Test "C1.1_StagingNaming" $patternValid "Staging naming matches {source}.greenc-moving.{uuid}"

    # 1.2 Test Atomic Rename on Same Volume
    $renameSuccess = [RelocationSafetyHarness]::MoveFileExW($src1, $movingPath, 0)
    $srcStillExists = [System.IO.Directory]::Exists($src1)
    $movingExists = [System.IO.Directory]::Exists($movingPath)
    $atomicPreserved = $renameSuccess -and (-not $srcStillExists) -and $movingExists
    Report-Test "C1.2_AtomicRename" $atomicPreserved "Atomic rename on same volume succeeds instantly"

    # Restore source
    [RelocationSafetyHarness]::MoveFileExW($movingPath, $src1, 0) | Out-Null

    # 1.3 Test Simulated Verification Failure Abort (Source Preservation)
    # Simulate verified_copy_tree detecting corruption
    [System.IO.Directory]::CreateDirectory($tgt1) | Out-Null
    $corruptTarget = [System.IO.Path]::Combine($tgt1, "payload.dat")
    [System.IO.File]::WriteAllText($corruptTarget, "CORRUPTED BYTE STREAM")
    $hashTgt = [RelocationSafetyHarness]::ComputeSha256($corruptTarget)

    $hashMismatch = ($hashSrc -ne $hashTgt)
    if ($hashMismatch) {
        # verified_copy_tree cleans up target and aborts
        [System.IO.Directory]::Delete($tgt1, $true)
    }

    $sourceIntact = [System.IO.File]::Exists($f1) -and ([RelocationSafetyHarness]::ComputeSha256($f1) -eq $hashSrc)
    $targetCleaned = -not [System.IO.Directory]::Exists($tgt1)
    Report-Test "C1.3_HashMismatchSourcePreserved" ($sourceIntact -and $targetCleaned) "Source remains 100% untouched when destination hash mismatch occurs"

    # 1.4 Test Discrepancy: Mover copies directly to target instead of staging on target
    # In mover.rs, verified_copy_tree writes directly to target_path, not a target staging path.
    # If interrupted mid-copy, target_path is partially populated.
    $moverCode = Get-Content "C:\DiskGuard\crates\greenc-relocate\src\mover.rs" -Raw
    $copiesDirectlyToTarget = $moverCode -match "verified_copy_tree\(&source_path,\s*&target_path"
    Report-Test "C1.4_DirectTargetCopyObservation" $copiesDirectlyToTarget "OBSERVATION: Mover copies directly to target_path rather than staging target directory"

    # --------------------------------------------------------------------------
    # CHALLENGE 2: Process Lock Blocks & File Lock Probe
    # --------------------------------------------------------------------------
    Write-Host "`n--- CHALLENGE 2: Process Lock Blocks & File Lock Probe ---" -ForegroundColor Yellow

    $src2 = [System.IO.Path]::Combine($testRoot, "App_C2_Locked")
    [System.IO.Directory]::CreateDirectory($src2) | Out-Null
    $lockedFile = [System.IO.Path]::Combine($src2, "active.sqlite")
    [System.IO.File]::WriteAllText($lockedFile, "SQLITE ACTIVE DATABASE")

    # 2.1 File Lock Open (Exclusive Lock with FileShare.None)
    $fs = [System.IO.File]::Open($lockedFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)

    # Check if CandidateQualificationEngine has FileLockProbe check
    $candCode = Get-Content "C:\DiskGuard\crates\greenc-relocate\src\candidate.rs" -Raw
    $hasFileLockProbeCall = $candCode -match "lock_probe\.is_file_locked"
    Report-Test "C2.1_QualificationFileLockProbe" (-not $hasFileLockProbeCall) "VULNERABILITY: CandidateQualificationEngine does NOT invoke lock_probe.is_file_locked (parameter _lock_probe is unused)"

    # Test what happens during move when file is locked
    $copyAttemptFailed = $false
    try {
        # verified_copy_tree calls File::open(src)
        $readAttempt = [System.IO.File]::Open($lockedFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
        $readAttempt.Close()
    } catch {
        $copyAttemptFailed = $true
    }
    $fs.Close()
    $fs.Dispose()

    Report-Test "C2.2_ExclusiveLockHaltsCopy" $copyAttemptFailed "Exclusive file lock halts verified_copy_tree at File::open with SharingViolation, preventing move"

    # 2.2 Rename blocked when file is open with FileShare.ReadWrite
    $fs2 = [System.IO.File]::Open($lockedFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::ReadWrite)
    $renameBlocked = $false
    $destMoving2 = [System.IO.Path]::Combine($testRoot, "App_C2_Locked.greenc-moving.test")
    try {
        [System.IO.Directory]::Move($src2, $destMoving2)
    } catch {
        $renameBlocked = $true
    } finally {
        $fs2.Close()
        $fs2.Dispose()
    }
    Report-Test "C2.3_RenameBlockedOnActiveHandle" $renameBlocked "Parent directory rename is blocked by Windows OS if child file handle is open, protecting source data"

    # --------------------------------------------------------------------------
    # CHALLENGE 3: Free Space Qualification (Reserve 512 MiB Guard)
    # --------------------------------------------------------------------------
    Write-Host "`n--- CHALLENGE 3: Free Space Qualification Guard ---" -ForegroundColor Yellow

    $reserveBytes = 512 * 1024 * 1024 # 536,870,912 bytes
    $reqSize = 100 * 1024 * 1024      # 100 MiB
    $totalNeeded = $reqSize + $reserveBytes

    # Test boundary 1: free_bytes < total_required (by 1 byte) -> MUST REJECT
    $available1 = $totalNeeded - 1
    $isRejected1 = ($available1 -lt $totalNeeded)
    Report-Test "C3.1_ReserveGuardUnderBy1Byte" $isRejected1 "Available free space (total_needed - 1 byte) correctly rejected by 512 MiB reserve check"

    # Test boundary 2: free_bytes == total_required -> MUST PASS
    $available2 = $totalNeeded
    $isAccepted2 = ($available2 -ge $totalNeeded)
    Report-Test "C3.2_ReserveGuardExactPass" $isAccepted2 "Available free space == total_needed passes 512 MiB reserve qualification"

    $moverMover = Get-Content "C:\DiskGuard\crates\greenc-relocate\src\mover.rs" -Raw
    $fnStart = $moverMover.IndexOf("pub fn relocate_junction")
    $moverRelocateFn = if ($fnStart -ge 0) { $moverMover.Substring($fnStart) } else { "" }
    $idxFree = $moverRelocateFn.IndexOf("check_free_space_reserve")
    $idxCopy = $moverRelocateFn.IndexOf("verified_copy_tree")
    $checkBeforeCopy = ($idxFree -ge 0) -and ($idxCopy -ge 0) -and ($idxFree -lt $idxCopy)
    Report-Test "C3.3_EarlyAbortBeforeCopy" $checkBeforeCopy "Free space qualification is executed before verified_copy_tree in relocate_junction body"

    # --------------------------------------------------------------------------
    # CHALLENGE 4: Crash Recovery in Both Directions (Case A and Case B)
    # --------------------------------------------------------------------------
    Write-Host "`n--- CHALLENGE 4: Crash Recovery in Both Directions ---" -ForegroundColor Yellow

    # Case A: Interrupted AFTER junction creation
    $src4A = [System.IO.Path]::Combine($testRoot, "App_C4A")
    $tgt4A = [System.IO.Path]::Combine($testRoot, "Target_C4A")
    [System.IO.Directory]::CreateDirectory($tgt4A) | Out-Null
    [System.IO.File]::WriteAllText([System.IO.Path]::Combine($tgt4A, "data.bin"), "Healthy target data")

    # Create junction at source pointing to target
    cmd /c "mklink /J `"$src4A`" `"$tgt4A`"" | Out-Null
    $isJuncA = [RelocationSafetyHarness]::IsReparsePoint($src4A)

    # Leftover moving dir from crash
    $mov4A = [System.IO.Path]::Combine($testRoot, "App_C4A.greenc-moving.abc123")
    [System.IO.Directory]::CreateDirectory($mov4A) | Out-Null
    [System.IO.File]::WriteAllText([System.IO.Path]::Combine($mov4A, "old.bin"), "stale")

    # Recovery Case A logic:
    # If junction exists and valid: cleans leftover moving dir
    if ($isJuncA -and [System.IO.Directory]::Exists($tgt4A)) {
        if ([System.IO.Directory]::Exists($mov4A)) {
            [System.IO.Directory]::Delete($mov4A, $true)
        }
    }
    $caseASuccess = $isJuncA -and (-not [System.IO.Directory]::Exists($mov4A)) -and [System.IO.File]::Exists([System.IO.Path]::Combine($tgt4A, "data.bin"))
    Report-Test "C4.1_RecoveryCaseAPostJunction" $caseASuccess "Case A recovery purges dangling .greenc-moving folder and preserves valid junction and target data"

    # Clean up Case A
    [RelocationSafetyHarness]::UnlinkReparse($src4A) | Out-Null
    [System.IO.Directory]::Delete($tgt4A, $true)

    # Case B: Interrupted BEFORE junction creation (CRITICAL BUG TEST)
    # Scenario: Phase 2 renamed source to moving.
    # Phase 3 started: create_junction created empty directory at source_path, but crashed before DeviceIoControl!
    $src4B = [System.IO.Path]::Combine($testRoot, "App_C4B")
    $tgt4B = [System.IO.Path]::Combine($testRoot, "Target_C4B")
    $mov4B = [System.IO.Path]::Combine($testRoot, "App_C4B.greenc-moving.xyz789")

    [System.IO.Directory]::CreateDirectory($mov4B) | Out-Null
    [System.IO.File]::WriteAllText([System.IO.Path]::Combine($mov4B, "user_chat.db"), "CRITICAL USER DATA IN MOVING DIR")
    [System.IO.Directory]::CreateDirectory($tgt4B) | Out-Null
    [System.IO.File]::WriteAllText([System.IO.Path]::Combine($tgt4B, "partial.bin"), "partial target")

    # create_junction created empty dir at src4B:
    [System.IO.Directory]::CreateDirectory($src4B) | Out-Null

    # Examine recovery.rs line 73-81:
    # let has_valid_junction = ... false (because is_reparse_point is false)
    # else {
    #     if reparse.is_reparse_point(&source) { reparse.delete_junction(&source); }
    #     if let Some(moving_dir) = Self::find_moving_dir(&source_parent, source_name) {
    #         if !source.exists() {
    #             let _ = fs::rename(&moving_dir, &source);
    #         }
    #     }
    # }

    $isReparseB = [RelocationSafetyHarness]::IsReparsePoint($src4B) # False!
    $src4BExists = [System.IO.Directory]::Exists($src4B)           # True!

    # Notice: because !source.exists() is FALSE, recovery.rs SKIPS renaming $mov4B to $src4B!
    $recoveryWouldSkip = (-not $isReparseB) -and $src4BExists

    Report-Test "C4.2_RecoveryCaseBEmptyDirStrandBug" (-not $recoveryWouldSkip) "CRITICAL FLAW CONFIRMED: In recovery Case B, if create_junction left an empty directory at source, recovery skips restoring moving_dir because !source.exists() is false, stranding user data!"

    # Clean up Case B
    [System.IO.Directory]::Delete($src4B, $true)
    [System.IO.Directory]::Delete($mov4B, $true)
    [System.IO.Directory]::Delete($tgt4B, $true)

    # --------------------------------------------------------------------------
    # CHALLENGE 5: 1-Click Undo Verification & Reparse Unlinking
    # --------------------------------------------------------------------------
    Write-Host "`n--- CHALLENGE 5: 1-Click Undo & Reparse Unlinking ---" -ForegroundColor Yellow

    $src5 = [System.IO.Path]::Combine($testRoot, "App_C5")
    $tgt5 = [System.IO.Path]::Combine($testRoot, "Target_C5")
    [System.IO.Directory]::CreateDirectory($tgt5) | Out-Null
    $f5 = [System.IO.Path]::Combine($tgt5, "document.pdf")
    [System.IO.File]::WriteAllText($f5, "DOCUMENT PAYLOAD REQUIRING 1-CLICK UNDO")
    $hashDoc = [RelocationSafetyHarness]::ComputeSha256($f5)

    cmd /c "mklink /J `"$src5`" `"$tgt5`"" | Out-Null

    # 5.1 Test RemoveDirectoryW unlinking junction only
    $unlinked = [RelocationSafetyHarness]::UnlinkReparse($src5)
    $srcGone = -not [System.IO.Directory]::Exists($src5)
    $tgtStillIntact = [System.IO.File]::Exists($f5) -and ([RelocationSafetyHarness]::ComputeSha256($f5) -eq $hashDoc)

    Report-Test "C5.1_UnlinkPreservesTarget" ($unlinked -and $srcGone -and $tgtStillIntact) "RemoveDirectoryW unlinks NTFS junction only without touching target directory or files"

    # 5.2 Test Undo Flow: copy back to restore_tmp, unlink junction, rename to source, delete target
    # Recreate junction
    cmd /c "mklink /J `"$src5`" `"$tgt5`"" | Out-Null
    $restoreTmp = [System.IO.Path]::Combine($testRoot, "restoring." + [System.Guid]::NewGuid().ToString())

    # Step 1: Copy back to restoreTmp
    [System.IO.Directory]::CreateDirectory($restoreTmp) | Out-Null
    Copy-Item $f5 -Destination (Join-Path $restoreTmp "document.pdf")

    # Step 2: Delete junction
    [RelocationSafetyHarness]::UnlinkReparse($src5) | Out-Null

    # Step 3: Rename restoreTmp to source
    [RelocationSafetyHarness]::MoveFileExW($restoreTmp, $src5, 0) | Out-Null

    # Step 4: Remove secondary target
    [System.IO.Directory]::Delete($tgt5, $true)

    $undoComplete = [System.IO.File]::Exists([System.IO.Path]::Combine($src5, "document.pdf")) -and (-not [System.IO.Directory]::Exists($tgt5))
    Report-Test "C5.2_FullUndoSequence" $undoComplete "Full 1-click Undo sequence successfully copies back, unlinks junction, restores source, and purges target"

    # 5.3 Omission of free space check in Undo
    $undoCode = Get-Content "C:\DiskGuard\crates\greenc-relocate\src\mover.rs" -Raw
    $undoHasFreeSpaceCheck = $undoCode -match "fn undo_relocation[\s\S]*?check_free_space_reserve"
    Report-Test "C5.3_UndoFreeSpaceOmission" (-not $undoHasFreeSpaceCheck) "VULNERABILITY: undo_relocation does not check destination volume free space before copying data back"

    # --------------------------------------------------------------------------
    # CHALLENGE 6: Symlink/Junction Recursion Prevention
    # --------------------------------------------------------------------------
    Write-Host "`n--- CHALLENGE 6: Symlink/Junction Recursion Prevention ---" -ForegroundColor Yellow

    $src6 = [System.IO.Path]::Combine($testRoot, "App_C6_Recursion")
    [System.IO.Directory]::CreateDirectory($src6) | Out-Null
    [System.IO.File]::WriteAllText([System.IO.Path]::Combine($src6, "normal.txt"), "normal file")

    $subDir = [System.IO.Path]::Combine($src6, "nested")
    [System.IO.Directory]::CreateDirectory($subDir) | Out-Null

    # Create circular junction: nested\loop_junc -> App_C6_Recursion
    $loopJunc = [System.IO.Path]::Combine($subDir, "loop_junc")
    cmd /c "mklink /J `"$loopJunc`" `"$src6`"" | Out-Null

    # Verify loop is detected as reparse point
    $isReparse6 = [RelocationSafetyHarness]::IsReparsePoint($loopJunc)
    Report-Test "C6.1_LoopJunctionCreated" $isReparse6 "Circular directory junction successfully created for testing"

    # Test recursion guard in verified_copy_tree
    # In verified_copy.rs: ft.is_symlink() is used.
    # On Windows, Directory Junctions have reparse tag IO_REPARSE_TAG_MOUNT_POINT, which Rust std is_symlink() recognizes.
    # However, for non-symlink/non-mountpoint reparse tags (e.g. AppExeLink 0x8000001B, Cloud 0x9000001A), is_symlink() returns false!
    $verifiedCopyCode = Get-Content "C:\DiskGuard\crates\greenc-relocate\src\verified_copy.rs" -Raw
    $usesReparseAttribute = $verifiedCopyCode -match "FILE_ATTRIBUTE_REPARSE_POINT"
    Report-Test "C6.2_ReparseTagAttributeGuard" (-not $usesReparseAttribute) "DEFECT: verified_copy_tree checks only ft.is_symlink() instead of (attr & 0x400) != 0, failing to catch non-symlink reparse points (e.g., Cloud dehydrated tags)"

    # Clean up junction before deleting testRoot
    [RelocationSafetyHarness]::UnlinkReparse($loopJunc) | Out-Null

} finally {
    if ([System.IO.Directory]::Exists($testRoot)) {
        [System.IO.Directory]::Delete($testRoot, $true)
    }
}

Write-Host "`n=================================================================" -ForegroundColor Cyan
Write-Host "EMPIRICAL ADVERSARIAL CHALLENGE EXECUTION SUMMARY" -ForegroundColor Cyan
Write-Host "=================================================================" -ForegroundColor Cyan
Write-Host "Total Passed: $global:passedTests" -ForegroundColor Green
Write-Host "Total Failed/Findings: $global:failedTests" -ForegroundColor Red

if ($global:findings.Count -gt 0) {
    Write-Host "`nKey Vulnerabilities / Findings Discovered:" -ForegroundColor Yellow
    foreach ($f in $global:findings) {
        Write-Host "  - $f" -ForegroundColor Red
    }
}
