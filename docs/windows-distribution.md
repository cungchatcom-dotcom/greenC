# Windows distribution runbook

Status: the Windows installer is **not code-signed yet**. A SignPath Foundation application is pending.
Until it is approved, SmartScreen shows "Windows protected your PC" for `GreenC-Setup-*.exe` downloaded from a browser.

## Every release: reduce SmartScreen friction

1. CI (`publish-release`) attaches `SHA256SUMS.txt` and adds install + SmartScreen notes to the release body automatically.
2. Submit the new installer to Microsoft for analysis (manual, needs a Microsoft account):
   - https://www.microsoft.com/en-us/wdsi/filesubmission → **Software developer**
   - Product: *Microsoft Defender SmartScreen*
   - Upload `GreenC-Setup-<version>.exe` from the GitHub release
   - Detection name: `SmartScreen – unknown publisher warning`
   - Definition / additional info: *"Open-source (Apache-2.0) disk management tool, built from public source by GitHub Actions: https://github.com/cungchatcom-dotcom/greenC. SHA256: `<hash>`. Requesting reputation review; code signing via SignPath Foundation is pending."*
   - Select "Incorrectly detected as malware/unwanted" (or "warning") and submit. Results typically arrive in 1–3 business days by email.

The built-in updater (`commands/updater.rs`) downloads with reqwest, so the file has no Mark-of-the-Web and auto-updates do not hit SmartScreen.

## winget (`CungChat.GreenC`)

`winget install` downloads without Mark-of-the-Web, so users do not see the SmartScreen prompt.

### First submission (one time, manual)

Manifests for 2.1.1 are in `packaging/winget/manifests/c/CungChat/GreenC/2.1.1/`.

From Linux/macOS, use [komac](https://github.com/russellbanks/Komac) (classic PAT with `public_repo` scope):

```bash
komac token update            # stores the PAT once
komac submit packaging/winget/manifests/c/CungChat/GreenC/2.1.1
```

From a Windows machine:

```powershell
winget install Microsoft.WingetCreate
winget validate --manifest packaging\winget\manifests\c\CungChat\GreenC\2.1.1
winget install --manifest packaging\winget\manifests\c\CungChat\GreenC\2.1.1   # local smoke test (needs: winget settings --enable LocalManifestFiles)
wingetcreate submit --token <GITHUB_PAT> packaging\winget\manifests\c\CungChat\GreenC\2.1.1
```

This opens a PR on `microsoft/winget-pkgs`. Watch it for validation bot comments; first reviews take a few days.

### Automatic updates for later releases

After the first PR is merged:

1. Fork `microsoft/winget-pkgs` into the `cungchatcom-dotcom` account (the action pushes to that fork).
2. Create a **classic** PAT with `public_repo` scope → repo secret `WINGET_TOKEN`.
3. Repo variable `WINGET_ENABLED=true`.

The `publish-winget` job then opens a winget-pkgs PR on every `v*` tag.
When the installer becomes signed, the hash changes; nothing else needs to change.

## Microsoft Store

There are two supported Microsoft Store paths:

### Path A — MSIX package (recommended now, no external code-signing certificate needed)

Microsoft Store signs and distributes MSIX packages directly, so this path does **not** wait for SignPath Foundation.

- Build uses Cargo feature `--features store` (`greenc-app` and `greenc-cli`):
  - Disables the GitHub Releases updater completely (updates come from Microsoft Store only).
  - Uses `desktop:StartupTask` (`Windows.ApplicationModel.StartupTask`) instead of `HKCU\...\Run`.
  - Isolates app data under `%LOCALAPPDATA%\GreenC-Store` (MSIX virtualized) without touching or migrating any standalone EXE installation.
  - Uses `runFullTrust` only (no `unvirtualizedResources`), blocks NTFS junctions inside virtualized AppData and Known Folder HKCU redirection in Store builds, and stages atomic cache operations outside AppData (`%USERPROFILE%\.greenc-store-staging`).
- Manifest template & packaging scripts live in `packaging/msix/` (`AppxManifest.xml.in`, `generate-assets.ps1`, `build-msix.ps1`, `build-bundle.ps1`, `smoke-test.ps1`).
- CI builds both `x64` (`windows-latest`) and native `arm64` (`windows-11-arm`) in `build-msix` (uploading separate per-architecture `GreenC-MSIX-x64` and `GreenC-MSIX-arm64` artifacts), bundles them in `bundle-msix` into `GreenC-MSIX-Store` (`GreenC-<ver>.msixbundle` + `GreenC-<ver>.msixupload`), and smoke-tests the bundle on both runners in `smoke-bundle` without auto-submitting. Runs on `workflow_dispatch`, `feat/microsoft-store`, `feat/store-arm64`, and `v*` tags once `STORE_IDENTITY_NAME` (or `PACKAGEIDENTITYNAME`) is configured as a repository variable.
- Full step-by-step registration, Partner Center setup, listing copy, privacy policy, and `runFullTrust` certification notes:
  - [`docs/microsoft-store.md`](microsoft-store.md)
  - [`docs/store-listing/vi.md`](store-listing/vi.md) & [`docs/store-listing/en.md`](store-listing/en.md)
  - [`docs/privacy.md`](privacy.md)

### Path B — Win32 EXE/MSI submission (optional after SignPath signing is live)

Once SignPath code signing is approved, the Store also accepts the signed Inno Setup `.exe` directly (per-user silent switches `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-`, x64, Windows 10 1903+), which keeps the built-in GitHub updater and unvirtualized AppData/HKCU behavior. Until signing is live, use **Path A (MSIX)**.
