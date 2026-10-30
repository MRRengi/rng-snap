; Snap by rng - installer (Inno Setup 6).
; Per-user by default (no admin prompt), all users optional.
; Build: iscc /DAppVersion=0.6.0 /DExe=..\target\release\RengiSnap.exe /DIcon=..\target\app.ico installer\RengiSnap.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Exe
  #define Exe "..\target\release\RengiSnap.exe"
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
AppId={{6B2C7E1A-5D7F-4E0B-9C8A-52454E474953}
AppName=Snap by rng
AppVersion={#AppVersion}
AppVerName=Snap by rng {#AppVersion}
AppPublisher=rng
AppPublisherURL=https://github.com/MRRengi/rng-snap
AppSupportURL=https://github.com/MRRengi/rng-snap/issues
AppUpdatesURL=https://github.com/MRRengi/rng-snap/releases
DefaultDirName={autopf}\Rengi Snap
DefaultGroupName=Snap by rng
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
UninstallDisplayIcon={app}\RengiSnap.exe
UninstallDisplayName=Snap by rng
OutputDir={#OutDir}
OutputBaseFilename=RengiSnap-Setup-{#AppVersion}
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
Source: "{#Exe}"; DestDir: "{app}"; DestName: "RengiSnap.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Snap by rng"; Filename: "{app}\RengiSnap.exe"; Comment: "Screenshots, screen recording and quick edits"
Name: "{autodesktop}\Snap by rng"; Filename: "{app}\RengiSnap.exe"; Tasks: desktopicon

[Registry]
; Autostart: the same value the app's own "Start with Windows" switch writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "RengiSnap"; ValueData: """{app}\RengiSnap.exe"" --background"; Tasks: autostart; Flags: uninsdeletevalue
; "Open with": the app, its friendly name and the file types it opens.
Root: HKA; Subkey: "Software\Classes\Applications\RengiSnap.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Snap by rng"; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\RengiSnap.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\RengiSnap.exe"" --edit ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\RengiSnap.exe\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\RengiSnap.exe\SupportedTypes"; ValueType: string; ValueName: ".png"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.mp4\OpenWithList\RengiSnap.exe"; ValueType: none; Tasks: openwith; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.png\OpenWithList\RengiSnap.exe"; ValueType: none; Tasks: openwith; Flags: uninsdeletekey

; Tells the app the installer settled autostart, so its first run leaves it alone.
Root: HKCU; Subkey: "Software\Rengi Snap"; ValueType: dword; ValueName: "AutostartChosen"; ValueData: 1; Flags: uninsdeletekey

[Run]
Filename: "{app}\RengiSnap.exe"; Description: "Launch Snap"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\RengiSnap.exe"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitSnap"

[Code]
// Updating over a running copy: ask it to quit first so the exe can be replaced.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
  Old: String;
begin
  Old := ExpandConstant('{app}\RengiSnap.exe');
  if FileExists(Old) then
  begin
    Exec(Old, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Sleep(800);
  end;
  // Any other copy (an older download, a portable copy) would keep the new
  // one from starting: Snap allows one copy at a time.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM RengiSnap.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;

// The app's own switch may have added the autostart value too: always remove it.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'RengiSnap');
end;
