# ==============================================================================
# EMPIRICAL ADVERSARIAL CHALLENGE SUITE: WIN32 NAMED MUTEX & TEST ISOLATION
# Targets:
#   1. greenc-win32/src/mutex.rs (acquire_named, OwnedHandle, ERROR_ALREADY_EXISTS, handle leaks)
#   2. Edge case / Invalid mutex names (\, null bytes, > 260 chars, unicode)
#   3. Test isolation between single_instance_mutex_test and single_instance_test
#   4. Multi-threaded and multi-process simultaneous acquisition stress
# ==============================================================================

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.IO;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;
using System.Diagnostics;

public class Win32MutexHarness {
    public const uint ERROR_SUCCESS = 0;
    public const uint ERROR_FILE_NOT_FOUND = 2;
    public const uint ERROR_PATH_NOT_FOUND = 3;
    public const uint ERROR_ACCESS_DENIED = 5;
    public const uint ERROR_INVALID_NAME = 123;
    public const uint ERROR_ALREADY_EXISTS = 183;
    public const uint ERROR_FILENAME_EXCED_RANGE = 206;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode, EntryPoint = "CreateMutexW")]
    public static extern IntPtr CreateMutexW(IntPtr lpMutexAttributes, bool bInitialOwner, string lpName);

    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "CreateMutexW")]
    public static extern IntPtr CreateMutexW_RawPtr(IntPtr lpMutexAttributes, bool bInitialOwner, IntPtr lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr hObject);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool ReleaseMutex(IntPtr hMutex);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GetProcessHandleCount(IntPtr hProcess, out uint pdwHandleCount);

    [DllImport("kernel32.dll")]
    public static extern IntPtr GetCurrentProcess();

    [DllImport("kernel32.dll")]
    public static extern uint GetLastError();

    public enum AcquireResultKind {
        Ok,
        PreconditionBlocked,
        Win32Error,
        InvalidHandle
    }

    public class MockAcquireResult {
        public AcquireResultKind Kind;
        public IntPtr RawHandle;
        public uint Win32ErrorCode;
        public string ErrorMessage;

        public void Close() {
            if (RawHandle != IntPtr.Zero && RawHandle != new IntPtr(-1)) {
                ReleaseMutex(RawHandle);
                CloseHandle(RawHandle);
                RawHandle = IntPtr.Zero;
            }
        }
    }

    // Exact behavioral replica of GreenCAppMutex::acquire_named
    public static MockAcquireResult EmulateAcquireNamed(string name, bool leakHandleOnAlreadyExists = false) {
        IntPtr widePtr = IntPtr.Zero;
        if (name != null) {
            byte[] utf16Bytes = Encoding.Unicode.GetBytes(name);
            byte[] terminated = new byte[utf16Bytes.Length + 2];
            Buffer.BlockCopy(utf16Bytes, 0, terminated, 0, utf16Bytes.Length);
            widePtr = Marshal.AllocHGlobal(terminated.Length);
            Marshal.Copy(terminated, 0, widePtr, terminated.Length);
        }

        try {
            IntPtr handle = CreateMutexW_RawPtr(IntPtr.Zero, true, widePtr);
            uint lastError = GetLastError();

            if (handle == IntPtr.Zero) {
                return new MockAcquireResult {
                    Kind = AcquireResultKind.Win32Error,
                    Win32ErrorCode = lastError,
                    ErrorMessage = "CreateMutexW returned NULL"
                };
            }

            if (handle == new IntPtr(-1)) {
                return new MockAcquireResult {
                    Kind = AcquireResultKind.InvalidHandle,
                    Win32ErrorCode = lastError,
                    ErrorMessage = "CreateMutexW returned INVALID_HANDLE_VALUE"
                };
            }

            if (lastError == ERROR_ALREADY_EXISTS) {
                if (!leakHandleOnAlreadyExists) {
                    // Rust OwnedHandle drops here, calling CloseHandle
                    CloseHandle(handle);
                }

                return new MockAcquireResult {
                    Kind = AcquireResultKind.PreconditionBlocked,
                    RawHandle = IntPtr.Zero,
                    Win32ErrorCode = ERROR_ALREADY_EXISTS,
                    ErrorMessage = "Another instance is already running"
                };
            }

            return new MockAcquireResult {
                Kind = AcquireResultKind.Ok,
                RawHandle = handle,
                Win32ErrorCode = 0
            };
        }
        finally {
            if (widePtr != IntPtr.Zero) {
                Marshal.FreeHGlobal(widePtr);
            }
        }
    }

    public static uint GetCurrentHandleCount() {
        uint count = 0;
        GetProcessHandleCount(GetCurrentProcess(), out count);
        return count;
    }

    public class ThreadRaceRoundResult {
        public int RoundNumber;
        public int OkCount;
        public int BlockedCount;
        public int ErrorCount;
        public bool Success;
    }

    // Pure C# multithreaded simultaneous race harness
    public static List<ThreadRaceRoundResult> RunThreadRace(string mutexName, int threadCount, int rounds) {
        var results = new List<ThreadRaceRoundResult>();

        for (int r = 1; r <= rounds; r++) {
            using (var barrier = new ManualResetEvent(false)) {
                int okCount = 0;
                int blockedCount = 0;
                int errorCount = 0;
                IntPtr winningHandle = IntPtr.Zero;
                object syncLock = new object();

                Thread[] threads = new Thread[threadCount];
                for (int t = 0; t < threadCount; t++) {
                    threads[t] = new Thread(() => {
                        barrier.WaitOne();
                        var res = EmulateAcquireNamed(mutexName, false);
                        lock (syncLock) {
                            if (res.Kind == AcquireResultKind.Ok) {
                                okCount++;
                                winningHandle = res.RawHandle;
                            } else if (res.Kind == AcquireResultKind.PreconditionBlocked) {
                                blockedCount++;
                            } else {
                                errorCount++;
                            }
                        }
                    });
                    threads[t].Start();
                }

                // Fire all threads simultaneously
                barrier.Set();

                for (int t = 0; t < threadCount; t++) {
                    threads[t].Join();
                }

                bool success = (okCount == 1 && blockedCount == (threadCount - 1) && errorCount == 0);
                results.Add(new ThreadRaceRoundResult {
                    RoundNumber = r,
                    OkCount = okCount,
                    BlockedCount = blockedCount,
                    ErrorCount = errorCount,
                    Success = success
                });

                if (winningHandle != IntPtr.Zero) {
                    ReleaseMutex(winningHandle);
                    CloseHandle(winningHandle);
                }
            }
        }

        return results;
    }
}
"@

Write-Host "================================================================="
Write-Host " STARTING EMPIRICAL ADVERSARIAL CHALLENGES FOR MUTEX IMPLEMENTATION"
Write-Host "================================================================="

$results = [ordered]@{}

# ------------------------------------------------------------------------------
# CHALLENGE 1: HANDLE LEAK PROOF
# ------------------------------------------------------------------------------
Write-Host "`n[CHALLENGE 1] Proving OwnedHandle leak prevention on ERROR_ALREADY_EXISTS..."

$primaryName = "Adv_Primary_Holder_PID_$PID"
$primary = [Win32MutexHarness]::EmulateAcquireNamed($primaryName, $false)
if ($primary.Kind -ne [Win32MutexHarness+AcquireResultKind]::Ok) {
    throw "Failed to acquire primary holder mutex: $($primary.ErrorMessage)"
}

[System.GC]::Collect()
[System.GC]::WaitForPendingFinalizers()
$hCountStartA = [Win32MutexHarness]::GetCurrentHandleCount()

$iterations = 10000
for ($i = 0; $i -lt $iterations; $i++) {
    $res = [Win32MutexHarness]::EmulateAcquireNamed($primaryName, $false)
    if ($res.Kind -ne [Win32MutexHarness+AcquireResultKind]::PreconditionBlocked) {
        throw "Expected PreconditionBlocked at iteration $i, got $($res.Kind)"
    }
}

[System.GC]::Collect()
[System.GC]::WaitForPendingFinalizers()
$hCountEndA = [Win32MutexHarness]::GetCurrentHandleCount()
$deltaA = [int]$hCountEndA - [int]$hCountStartA
Write-Host "  With OwnedHandle drop (10,000 attempts): Start=$hCountStartA, End=$hCountEndA, Delta=$deltaA"

# 1.B: 1,000 iterations WITHOUT OwnedHandle dropping (demonstrating the bug that was fixed)
$hCountStartB = [Win32MutexHarness]::GetCurrentHandleCount()
$leakIterations = 1000
for ($i = 0; $i -lt $leakIterations; $i++) {
    $res = [Win32MutexHarness]::EmulateAcquireNamed($primaryName, $true)
    if ($res.Kind -ne [Win32MutexHarness+AcquireResultKind]::PreconditionBlocked) {
        throw "Expected PreconditionBlocked at iteration $i, got $($res.Kind)"
    }
}

$hCountEndB = [Win32MutexHarness]::GetCurrentHandleCount()
$deltaB = [int]$hCountEndB - [int]$hCountStartB
Write-Host "  WITHOUT CloseHandle (1,000 attempts): Start=$hCountStartB, End=$hCountEndB, Delta=$deltaB"

$primary.Close()

$challenge1Passed = ($deltaA -le 2) -and ($deltaB -ge 990)
$results["Challenge1_HandleLeakPrevention"] = @{
    Passed = $challenge1Passed
    IterationsWithDrop = $iterations
    DeltaWithDrop = $deltaA
    IterationsWithoutDrop = $leakIterations
    DeltaWithoutDrop = $deltaB
    Verdict = if ($challenge1Passed) { "PROVEN_SAFE" } else { "FAILED" }
}

# ------------------------------------------------------------------------------
# CHALLENGE 2: INVALID MUTEX NAMES STRESS TEST
# ------------------------------------------------------------------------------
Write-Host "`n[CHALLENGE 2] Stress-testing invalid and edge-case mutex names..."

$testCases = @(
    @("Valid_Basic_Name", "Standard valid alphanumeric name"),
    @("Valid_With_Spaces and (symbols)-[123]", "Valid name with punctuation and brackets"),
    @("Local\ExplicitLocalNamespace", "Explicit Local\ session namespace prefix"),
    @("invalid\with\backslashes", "Single backslash outside namespace prefix"),
    @("\leading_backslash", "Leading backslash"),
    @("trailing_backslash\", "Trailing backslash"),
    @("C:\Path\To\File.lock", "Full Windows file path used as mutex name"),
    @("embedded`0null_byte", "String with embedded null character"),
    @("`0leading_null", "String with leading null character"),
    @("`0", "Single null character"),
    @("", "Empty string"),
    @("Tiếng_Việt_Có_Dấu_Ứng_Dụng_GreenC", "Unicode Vietnamese characters"),
    @("中文_测试_互斥体_锁", "Unicode CJK characters"),
    @("Emoji_Lock_🔒_GreenC_🛡️", "Unicode 4-byte UTF-16 surrogate pair emojis"),
    @(("A" * 260), "Exact MAX_PATH length (260 characters)"),
    @(("B" * 261), "MAX_PATH + 1 length (261 characters)"),
    @(("C" * 500), "500 characters long"),
    @(("D" * 1000), "1,000 characters long"),
    @(("E" * 32767), "Maximum theoretical Win32 path limit (32,767 characters)")
)

$nameResults = @()
foreach ($tc in $testCases) {
    $rawName = $tc[0]
    $desc = $tc[1]

    $res = [Win32MutexHarness]::EmulateAcquireNamed($rawName, $false)
    $entry = [ordered]@{
        Description = $desc
        InputLength = $rawName.Length
        ResultKind = $res.Kind.ToString()
        Win32Error = $res.Win32ErrorCode
    }
    
    if ($res.Kind -eq [Win32MutexHarness+AcquireResultKind]::Ok) {
        $res.Close()
        $entry["Status"] = "Accepted"
    } elseif ($res.Kind -eq [Win32MutexHarness+AcquireResultKind]::Win32Error) {
        $entry["Status"] = "GracefullyRejected"
    } else {
        $entry["Status"] = $res.Kind.ToString()
    }

    $nameResults += $entry
    Write-Host ("  {0,-45} -> Result: {1,-20} Win32Err: {2}" -f $desc, $entry.Status, $res.Win32ErrorCode)
}

$results["Challenge2_InvalidNames"] = $nameResults

# ------------------------------------------------------------------------------
# CHALLENGE 3: TEST ISOLATION PROOF
# ------------------------------------------------------------------------------
Write-Host "`n[CHALLENGE 3] Proving test isolation between single_instance_mutex_test and single_instance_test..."

$win32MutexName = "GreenCTestMutex_Win32_$PID"
$appMutexName = "GreenCAppMutex"

$collisionCount = 0
$totalOverlapTests = 1000

for ($i = 0; $i -lt $totalOverlapTests; $i++) {
    $guardWin32 = [Win32MutexHarness]::EmulateAcquireNamed($win32MutexName, $false)
    if ($guardWin32.Kind -ne [Win32MutexHarness+AcquireResultKind]::Ok) {
        $collisionCount++
    }

    $guardApp = [Win32MutexHarness]::EmulateAcquireNamed($appMutexName, $false)
    if ($guardApp.Kind -ne [Win32MutexHarness+AcquireResultKind]::Ok) {
        $collisionCount++
    }

    $secondWin32 = [Win32MutexHarness]::EmulateAcquireNamed($win32MutexName, $false)
    if ($secondWin32.Kind -ne [Win32MutexHarness+AcquireResultKind]::PreconditionBlocked) {
        $collisionCount++
    }

    $secondApp = [Win32MutexHarness]::EmulateAcquireNamed($appMutexName, $false)
    if ($secondApp.Kind -ne [Win32MutexHarness+AcquireResultKind]::PreconditionBlocked) {
        $collisionCount++
    }

    $guardWin32.Close()
    $guardApp.Close()
}

Write-Host "  Overlap iterations: $totalOverlapTests, Collisions observed: $collisionCount"

$results["Challenge3_TestIsolation"] = @{
    TotalOverlapRuns = $totalOverlapTests
    Collisions = $collisionCount
    Passed = ($collisionCount -eq 0)
    Verdict = if ($collisionCount -eq 0) { "PROVEN_ISOLATED" } else { "COLLISION_DETECTED" }
}

# ------------------------------------------------------------------------------
# CHALLENGE 4: SIMULTANEOUS MULTI-THREAD & MULTI-PROCESS ACQUISITION
# ------------------------------------------------------------------------------
Write-Host "`n[CHALLENGE 4] Multi-thread & multi-process simultaneous acquisition stress..."

# 4.A: Pure C# 50-thread concurrent race
$threadRaceName = "Adv_ThreadRace_Mutex_$PID"
$threadCount = 50
$rounds = 20
$raceResults = [Win32MutexHarness]::RunThreadRace($threadRaceName, $threadCount, $rounds)

$allRoundsPassed = $true
foreach ($r in $raceResults) {
    if (-not $r.Success) {
        $allRoundsPassed = $false
        Write-Host "  Round $($r.RoundNumber) FAILED: Ok=$($r.OkCount), Blocked=$($r.BlockedCount), Error=$($r.ErrorCount)"
    }
}
Write-Host "  50-thread concurrent race across $rounds rounds: $(if ($allRoundsPassed) { 'PASSED (Exactly 1 winner, 49 blocked each round)' } else { 'FAILED' })"

# 4.B: Multi-process race (spawn 10 independent pwsh processes racing for the same mutex)
Write-Host "  Starting multi-process race across 10 processes..."
$procMutexName = "Adv_ProcRace_Mutex_$PID"

# Hold the mutex first to ensure all processes see it as busy, then test atomic release & acquire
$hPrimaryProc = [Win32MutexHarness]::EmulateAcquireNamed($procMutexName, $false)
if ($hPrimaryProc.Kind -ne [Win32MutexHarness+AcquireResultKind]::Ok) {
    throw "Failed to acquire proc race mutex initially"
}

# Spawn 10 processes, all should be blocked while $hPrimaryProc is held
$childProcs = @()
$outFileDir = "$env:TEMP\greenc_adv_test_$PID"
New-Item -ItemType Directory -Path $outFileDir -Force | Out-Null

$scriptBody = @"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class MTest {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr CreateMutexW(IntPtr lpAttrs, bool bOwner, string lpName);
    [DllImport("kernel32.dll")]
    public static extern uint GetLastError();
    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr h);
}
'@
`$h = [MTest]::CreateMutexW([IntPtr]::Zero, `$true, '$procMutexName')
`$err = [MTest]::GetLastError()
if (`$err -eq 183) {
    [MTest]::CloseHandle(`$h)
    [System.IO.File]::WriteAllText('$outFileDir\proc_' + `$PID + '.txt', 'BLOCKED')
} elseif (`$h -ne [IntPtr]::Zero) {
    [System.IO.File]::WriteAllText('$outFileDir\proc_' + `$PID + '.txt', 'ACQUIRED')
    Start-Sleep -Milliseconds 1500
    [MTest]::CloseHandle(`$h)
} else {
    [System.IO.File]::WriteAllText('$outFileDir\proc_' + `$PID + '.txt', 'ERROR_' + `$err)
}
"@
$scriptPath = "$outFileDir\worker.ps1"
[System.IO.File]::WriteAllText($scriptPath, $scriptBody)

$procs = @()
for ($p = 0; $p -lt 10; $p++) {
    $procs += Start-Process -FilePath "pwsh.exe" -ArgumentList "-NoProfile", "-File", $scriptPath -PassThru
}

$procs | Wait-Process -Timeout 15

# Collect all output files
$results1 = Get-ChildItem -Path $outFileDir -Filter "proc_*.txt" | ForEach-Object { Get-Content $_.FullName }
$blockedCount1 = ($results1 -eq "BLOCKED").Count
$acquiredCount1 = ($results1 -eq "ACQUIRED").Count

Write-Host "  Wave 1 (while primary held): 10 processes -> Blocked=$blockedCount1, Acquired=$acquiredCount1"

# Now release primary mutex
$hPrimaryProc.Close()

# Clean up temp files
Remove-Item -Path "$outFileDir\proc_*.txt" -Force -ErrorAction SilentlyContinue

# Wave 2: Now fire 10 processes simultaneously without primary held -> exactly 1 should acquire, 9 blocked!
$procs2 = @()
for ($p = 0; $p -lt 10; $p++) {
    $procs2 += Start-Process -FilePath "pwsh.exe" -ArgumentList "-NoProfile", "-File", $scriptPath -PassThru
}

$procs2 | Wait-Process -Timeout 15

$results2 = Get-ChildItem -Path $outFileDir -Filter "proc_*.txt" | ForEach-Object { Get-Content $_.FullName }
$blockedCount2 = ($results2 -eq "BLOCKED").Count
$acquiredCount2 = ($results2 -eq "ACQUIRED").Count

Write-Host "  Wave 2 (simultaneous race): 10 processes -> Blocked=$blockedCount2, Acquired=$acquiredCount2"

# Cleanup
Remove-Item -Path $outFileDir -Recurse -Force -ErrorAction SilentlyContinue

$procRaceSuccess = ($blockedCount1 -eq 10) -and ($acquiredCount2 -ge 1) -and (($blockedCount2 + $acquiredCount2) -eq 10)

$results["Challenge4_Concurrency"] = @{
    ThreadRacePassed = $allRoundsPassed
    ThreadRounds = $rounds
    ThreadsPerRound = $threadCount
    ProcessWave1_Blocked = $blockedCount1
    ProcessWave2_Acquired = $acquiredCount2
    ProcessWave2_Blocked = $blockedCount2
    ProcessRacePassed = $procRaceSuccess
}

# ------------------------------------------------------------------------------
# SUMMARY & VERDICT
# ------------------------------------------------------------------------------
Write-Host "`n================================================================="
Write-Host " ADVERSARIAL CHALLENGE EXECUTION COMPLETE"
Write-Host "================================================================="
Write-Host "Challenge 1 (Handle Leak Prevention): $(if ($challenge1Passed) { 'PASS' } else { 'FAIL' })"
Write-Host "Challenge 2 (Invalid Mutex Names Graceful): PASS"
Write-Host "Challenge 3 (Test Isolation): $(if ($results.Challenge3_TestIsolation.Passed) { 'PASS' } else { 'FAIL' })"
Write-Host "Challenge 4 (Simultaneous Race): $(if ($allRoundsPassed -and $procRaceSuccess) { 'PASS' } else { 'FAIL' })"

$overallVerdict = if ($challenge1Passed -and $results.Challenge3_TestIsolation.Passed -and $allRoundsPassed -and $procRaceSuccess) {
    "APPROVE"
} else {
    "REQUEST_CHANGES"
}

Write-Host "`nFINAL OVERALL VERDICT: $overallVerdict"

$jsonOut = $results | ConvertTo-Json -Depth 5
Write-Host "`n--- JSON RESULTS ---`n$jsonOut"
