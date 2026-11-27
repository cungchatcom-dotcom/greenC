# Spec 004 — Native Arm64 package for the Microsoft Store edition

## Why
The Store package is x64 only. On Windows 11 on Arm it runs under x64 emulation; on Windows 10 on Arm it
cannot run at all (that OS only emulates x86). Partner Center recommends Arm64 (AArch64) packages.
GreenC ships no AArch32 code, so the AArch32 deprecation itself does not affect the published package.

## Goal
One Store upload (`.msixupload` holding one `.msixbundle`) with an x64 package and a native Arm64 package
built from the same commit, with the same Identity and the same version `a.b.c.0`.

## Requirements
- R1. `AppxManifest.xml.in` takes the architecture from the build; accepted values are exactly `x64` and `arm64`.
- R2. Each package contains `GreenC.exe` and `greenc-cli.exe` whose PE machine type matches the manifest
  (x64 = 0x8664, arm64 = 0xAA64). The build fails otherwise. This is the check that stops an x64 binary
  being shipped inside an "arm64" package.
- R3. Both packages are built with `--features store` and keep every existing manifest invariant
  (MinVersion 10.0.18362.0, only `runFullTrust`, StartupTask disabled, execution alias, all assets).
- R4. The bundle step fails unless it receives exactly one x64 and one arm64 package with identical
  Identity Name, Publisher and Version, and the produced bundle manifest lists exactly those two.
- R5. The final artifact `GreenC-MSIX-Store` contains `GreenC-<ver>.msixbundle` and
  `GreenC-<ver>.msixupload` (a ZIP holding only that bundle). Per-architecture `.msix` files are published
  as separate artifacts that cannot be mistaken for the upload file.
- R6. CI builds, tests (store feature) and smoke-tests the Arm64 package on a real Arm64 Windows runner
  (`windows-11-arm`). The existing x64 smoke test keeps running. The smoke test asserts the installed
  package architecture.
- R7. The bundle is smoke-tested by installing it on both an x64 and an Arm64 runner and asserting that
  Windows picked the matching architecture.
- R8. A failure of either architecture fails the workflow and produces no upload file.
- R9. `docs/microsoft-store.md` describes the bundle upload, the Arm64 requirement, and that once a bundle
  has been published every later submission must also be a bundle.

## Out of scope
- The standalone EXE installer, winget, macOS, Linux (no change).
- Merging main / 2.2.3 / 2.3.0 into the Store branch.
- Changing the version in `Cargo.toml`, publishing, or submitting to Partner Center.
- Any change to Rust behaviour, unless Arm64 compilation or tests require a minimal fix.

## Acceptance
CI on the branch is green; artifacts contain the bundle and upload file; logs show both architectures
installed and passing the clean + Vault restore smoke test.
