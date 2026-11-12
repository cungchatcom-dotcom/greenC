; Inno Setup script for Green C:\
; Built by scripts\build.ps1, which passes /DAppVersion=X.Y.Z (read from diskguard/__init__.py).

#ifndef AppVersion
  #error AppVersion is not defined. Build with scripts\build.ps1.
#endif

#define AppName "Green C:\"
#define AppShortName "Green C"
#define AppExe "GreenC.exe"
#define AppPublisher "Cùng Chat"
#define AppUrl "https://cungchat.com"
#define RepoRoot ".."

[Setup]
; Never change AppId: upgrades and uninstall depend on it.
AppId={{6F1C2B7A-3E4D-4C5B-9A8E-7D2F1B0C9E51}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}
AppUpdatesURL=https://github.com/cungchatcom-dotcom/greenC/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoCompany={#AppPublisher}
; Per-user install into %LOCALAPPDATA%\Programs: no admin prompt, matches the per-user data the app manages.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppShortName}
DefaultGroupName={#AppShortName}
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile={#RepoRoot}\build\packaging\greenc.ico
WizardSmallImageFile={#RepoRoot}\build\packaging\wizard_small.png
WizardStyle=modern
OutputDir={#RepoRoot}\dist
OutputBaseFilename=GreenC-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Same name as SINGLE_INSTANCE_MUTEX in main.py: setup asks the user to close the running app.
AppMutex=GreenCAppMutex
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "vi"; MessagesFile: "Vietnamese.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
vi.LaunchApp=Mở {#AppName}
en.LaunchApp=Launch {#AppName}
vi.DataKept=Dữ liệu của Green C:\ (Vault, cài đặt) được giữ lại ở %LOCALAPPDATA%\Green C và trên ổ dữ liệu bạn đã chọn. Các thư mục đã chuyển sang ổ khác vẫn hoạt động bình thường.
en.DataKept=Green C:\ data (Vault, settings) is kept in %LOCALAPPDATA%\Green C and on your chosen data drive. Relocated folders keep working.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; Remove files from the previous version so stale Qt/Python modules never linger.
Type: filesandordirs; Name: "{app}\_internal"

[Files]
Source: "{#RepoRoot}\dist\GreenC\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppShortName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppShortName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    MsgBox(CustomMessage('DataKept'), mbInformation, MB_OK);
end;
