# Tasks 004 — Store Arm64

Design: build per architecture on a native runner, bundle in a third job.

- [x] T1 `packaging/msix/AppxManifest.xml.in`: `ProcessorArchitecture="@ARCH@"`.
- [x] T2 `packaging/msix/build-msix.ps1`: `-Architecture x64|arm64` (ValidateSet, default x64); default exe
      paths `target\<triple>\release\` (`x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`); read the PE
      header of both exes and throw on mismatch; stage in `dist\msix\stage-<arch>`; output
      `GreenC-<ver>-<arch>.msix`; stop creating a per-architecture `.msixupload`.
- [x] T3 new `packaging/msix/build-bundle.ps1`: input directory; require exactly one x64 + one arm64 msix;
      unpack each and compare Identity; `makeappx bundle /d <dir holding only the two msix> /p <out> /bv <ver> /o`;
      `makeappx unbundle` and check `AppxMetadata\AppxBundleManifest.xml` (two `Package Type="application"`
      entries, x64 and arm64, same version); write `.msixupload` (ZIP with only the bundle). Check every
      native exit code.
- [ ] T4 `packaging/msix/smoke-test.ps1`: parameters for the package/bundle path and expected architecture;
      take the Publisher from the built manifest without assuming a fixed stage folder; assert
      `Get-AppxPackage` `.Architecture` equals the expected one; keep all current assertions; pick a
      `signtool.exe` that runs on the host.
- [ ] T5 `.github/workflows/ci-release.yml`: `build-msix` matrix (x64 → `windows-latest`, arm64 →
      `windows-11-arm`), `rustup target add`, `cargo build --release --target <triple> ... --features store`,
      store-feature tests on arm64, package, smoke test, upload `GreenC-MSIX-<arch>`; new job `bundle-msix`
      (needs build-msix) producing `GreenC-MSIX-Store`; new job `smoke-bundle` matrix on both runners.
      Add `refs/heads/feat/store-arm64` to the run condition. Keep `msix_version` and tag version handling,
      and use the same version in all jobs. `release-*` and `publish-*` jobs must not change.
- [ ] T6 Check Arm64 build needs of native dependencies (`aws-lc-sys` through reqwest/rustls needs a C
      compiler that targets aarch64-msvc, usually clang; also `libsqlite3-sys`, `zstd-sys`, WebView2 loader)
      against the `windows-11-arm` runner image; add explicit setup steps where needed.
- [ ] T7 `docs/microsoft-store.md` (Vietnamese): bundle upload, x64 + Arm64, version must be new
      (2.2.2.0 is taken; 4th part stays 0), bundle cannot be reverted to single msix.
- [ ] T8 Update any other reference to the old file names or the `stage` folder (`git grep`).
