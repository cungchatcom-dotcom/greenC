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

Manifests for 2.1.0 are in `packaging/winget/manifests/c/CungChat/GreenC/2.1.0/`. From a Windows machine:

```powershell
winget install Microsoft.WingetCreate
winget validate --manifest packaging\winget\manifests\c\CungChat\GreenC\2.1.0
winget install --manifest packaging\winget\manifests\c\CungChat\GreenC\2.1.0   # local smoke test (needs: winget settings --enable LocalManifestFiles)
wingetcreate submit --token <GITHUB_PAT> packaging\winget\manifests\c\CungChat\GreenC\2.1.0
```

This opens a PR on `microsoft/winget-pkgs`. Watch it for validation bot comments; first reviews take a few days.

### Automatic updates for later releases

After the first PR is merged:

1. Fork `microsoft/winget-pkgs` into the `cungchatcom-dotcom` account (the action pushes to that fork).
2. Create a **classic** PAT with `public_repo` scope → repo secret `WINGET_TOKEN`.
3. Repo variable `WINGET_ENABLED=true`.

The `publish-winget` job then opens a winget-pkgs PR on every `v*` tag.
When the installer becomes signed, the hash changes; nothing else needs to change.

## Microsoft Store (after signing)

The Store accepts our Inno Setup `.exe` directly (Win32 EXE/MSI submission). No MSIX repackaging is needed.
The Store does **not** re-sign EXE installers, though. They must be signed by a CA in the Microsoft Trusted Root Program, so this step waits for SignPath.

When signing is live:

1. Register an individual developer account at https://storedeveloper.microsoft.com (free for individuals) and reserve the name **GreenC**.
2. New product → *EXE or MSI app*. Package URL: the versioned GitHub release URL of the signed installer.
3. Silent install switches: `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-`
4. Architecture x64, per-user install (no admin), Windows 10 1903+.
5. Listing: reuse the README text and `docs/screenshots/`. Privacy policy URL is required: host one on greenc.cungchat.com (the app processes data locally only).
6. For each release, update the package URL to the new versioned installer. The EXE path allows the app to keep its own updater.
