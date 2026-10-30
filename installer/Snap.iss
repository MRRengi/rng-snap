; Snap - installer (Inno Setup 6).
; Per-user by default (no admin prompt), all users optional.
; Build: iscc /DAppVersion=0.6.2 installer\Snap.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Exe
  #define Exe "..\target\release\Snap.exe"
#endif
#ifndef Icon
  #define Icon "..\target\app.ico"
#endif
#ifndef OutDir
  #define OutDir "..\target\installer"
#endif
#ifndef Art
  #define Art "..\target"
#endif

[Setup]
; Same id as the releases before the rename, so this upgrades them.
AppId={{6B2C7E1A-5D7F-4E0B-9C8A-52454E474953}
AppName=Snap
AppVersion={#AppVersion}
AppVerName=Snap {#AppVersion}
AppPublisher=rng
AppPublisherURL=https://github.com/MRRengi/rng-snap
AppSupportURL=https://github.com/MRRengi/rng-snap/issues
AppUpdatesURL=https://github.com/MRRengi/rng-snap/releases
DefaultDirName={autopf}\Snap
UsePreviousAppDir=no
DefaultGroupName=Snap
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
WizardStyle=modern
WizardSizePercent=100
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\Snap.exe
UninstallDisplayName=Snap
OutputDir={#OutDir}
OutputBaseFilename=Snap-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes
WizardImageFile={#Art}\wizard.bmp
WizardSmallImageFile={#Art}\wizard-small.bmp

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start Snap with Windows"; GroupDescription: "Options:"
Name: "openwith"; Description: "Add ""Open with Snap"" for MP4 videos and PNG images"; GroupDescription: "Options:"
Name: "desktopicon"; Description: "Create a desktop icon"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; DestName: "Snap.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Snap"; Filename: "{app}\Snap.exe"; Comment: "Screenshots, screen recording and quick edits"
Name: "{autodesktop}\Snap"; Filename: "{app}\Snap.exe"; Tasks: desktopicon

[Registry]
; Autostart: the same value the app's own "Start with Windows" switch writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Snap"; ValueData: """{app}\Snap.exe"" --background"; Tasks: autostart; Flags: uninsdeletevalue
; "Open with": the app, its friendly name and the file types it opens.
Root: HKA; Subkey: "Software\Classes\Applications\Snap.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Snap"; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\Snap.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\Snap.exe"" --edit ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\Snap.exe\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\Snap.exe\SupportedTypes"; ValueType: string; ValueName: ".png"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.mp4\OpenWithList\Snap.exe"; ValueType: none; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.png\OpenWithList\Snap.exe"; ValueType: none; Tasks: openwith; Flags: uninsdeletekey
; Tells the app the installer settled autostart, so its first run leaves it alone.
Root: HKCU; Subkey: "Software\Snap"; ValueType: dword; ValueName: "AutostartChosen"; ValueData: 1; Flags: uninsdeletekey

[Run]
Filename: "{app}\Snap.exe"; Description: "Launch Snap"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\Snap.exe"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitSnap"

[UninstallDelete]
; Cut out models downloaded on first use.
Type: filesandordirs; Name: "{localappdata}\Snap\models"
Type: dirifempty; Name: "{localappdata}\Snap"

[Code]
const
  UninstKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{6B2C7E1A-5D7F-4E0B-9C8A-52454E474953}_is1';

function NoSlash(S: String): String;
begin
  Result := Lowercase(RemoveBackslashUnlessRoot(S));
end;

// A version from before the rename lives in "...\Rengi Snap": uninstall it
// first, so nothing of it is left behind.
procedure RemoveOldInstall(Root: Integer);
var
  Loc, Cmd: String;
  Code, I: Integer;
begin
  if not RegQueryStringValue(Root, UninstKey, 'InstallLocation', Loc) then exit;
  if NoSlash(Loc) = NoSlash(ExpandConstant('{app}')) then exit;
  if not RegQueryStringValue(Root, UninstKey, 'UninstallString', Cmd) then exit;
  Cmd := RemoveQuotes(Cmd);
  if not FileExists(Cmd) then exit;
  Exec(Cmd, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, Code);
  // The uninstaller re-launches itself from a temp copy: wait for it to finish.
  for I := 1 to 60 do
  begin
    if not FileExists(Cmd) then break;
    Sleep(500);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
  Old: String;
begin
  Old := ExpandConstant('{app}\Snap.exe');
  if FileExists(Old) then
  begin
    Exec(Old, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Sleep(800);
  end;
  // Copies from before the rename (an older download, an old install) would
  // keep the new one from starting: Snap allows one copy at a time.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM RengiSnap.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  RemoveOldInstall(HKEY_CURRENT_USER);
  if IsAdminInstallMode then RemoveOldInstall(HKEY_LOCAL_MACHINE);
  Result := '';
end;

// The app's own switch may have added the autostart value too: always remove it.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Snap');
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'RengiSnap');
  end;
end;
