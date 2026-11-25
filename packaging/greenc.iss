; Inno Setup 6 installer script for GreenC v2 (Rust + Tauri v2) — Task T068
; Built by scripts\build.ps1, which passes /DAppVersion=X.Y.Z (read from Cargo.toml).

#ifndef AppVersion
  #define AppVersion "2.1.0"
#endif

#define AppName "GreenC"
#define AppShortName "GreenC"
#define AppExe "GreenC.exe"
#define CliExe "greenc-cli.exe"
#define AppPublisher "Cùng Chat"
#define AppUrl "https://cungchat.com"
#define RepoRoot ".."

[Setup]
; Never change AppId: seamless in-place upgrades from Python GreenC v2.0 depend on this GUID.
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
; Per-user install into %LOCALAPPDATA%\Programs\GreenC: zero UAC Admin prompt required.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppShortName}
DefaultGroupName={#AppShortName}
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile={#RepoRoot}\crates\greenc-app\icons\icon.ico
WizardStyle=modern
OutputDir={#RepoRoot}\dist
OutputBaseFilename=GreenC-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.18362
; No AppMutex directive: it makes silent installs (winget upgrade) abort while GreenC sits in the tray.
; Restart Manager closes the app instead; PrepareToInstall refuses while GreenC is working on files.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "vi"; MessagesFile: "Vietnamese.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
vi.LaunchApp=Mở {#AppName}
en.LaunchApp=Launch {#AppName}
vi.AppBusy=GreenC đang dọn hoặc chuyển dữ liệu. Hãy chờ xong rồi cài lại.
en.AppBusy=GreenC is cleaning or moving data. Wait for it to finish, then run setup again.
vi.CloseApp=GreenC đang chạy. Hãy thoát GreenC ở khay hệ thống rồi bấm OK.
en.CloseApp=GreenC is running. Exit it from the system tray, then click OK.
vi.DataKept=Dữ liệu của GreenC (Kho dự phòng Vault, quy tắc hạn mức, cài đặt) được giữ nguyên ở %LOCALAPPDATA%\GreenC và trên ổ dữ liệu bạn đã chọn. Các thư mục đã di chuyển bằng NTFS Junction vẫn hoạt động bình thường.
en.DataKept=GreenC data (Quarantine Vault, quota rules, settings) is preserved in %LOCALAPPDATA%\GreenC and on your chosen data drive. Relocated NTFS Junction folders continue working normally.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; Clean up legacy PyInstaller/PySide6 ~65 MB runtime directory when upgrading from Python GreenC v2.0.
Type: filesandordirs; Name: "{app}\_internal"
Type: files; Name: "{app}\*.pyd"
Type: files; Name: "{app}\python*.dll"

[Files]
Source: "{#RepoRoot}\dist\GreenC\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\dist\GreenC\{#CliExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\dist\GreenC\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\{#AppShortName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppShortName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent
; A silent upgrade closes the tray app, so start it again if it was running.
Filename: "{app}\{#AppExe}"; Flags: nowait skipifnotsilent; Check: AppWasRunning

[Code]
var
  WasRunning: Boolean;

function AppWasRunning(): Boolean;
begin
  Result := WasRunning;
end;

function InitializeSetup(): Boolean;
begin
  Result := True;
  WasRunning := CheckForMutexes('GreenCAppMutex');
  if not IsWin64 then
  begin
    MsgBox('GreenC requires 64-bit Windows 10 (1903+) or Windows 11.', mbCriticalError, MB_OK);
    Result := False;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  // Matches BUSY_MUTEX_NAME in greenc-core.
  if CheckForMutexes('GreenCBusyMutex') then
    Result := CustomMessage('AppBusy');
end;

function InitializeUninstall(): Boolean;
begin
  Result := True;
  // Matches SINGLE_INSTANCE_MUTEX_NAME in greenc-core.
  while Result and CheckForMutexes('GreenCAppMutex') do
    Result := SuppressibleMsgBox(CustomMessage('CloseApp'), mbError, MB_OKCANCEL, IDCANCEL) = IDOK;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    MsgBox(CustomMessage('DataKept'), mbInformation, MB_OK);
end;
