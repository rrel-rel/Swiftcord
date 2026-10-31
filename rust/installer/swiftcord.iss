; '08cord installer (Inno Setup 6). Build with tools\build-installer.ps1, which compiles the release exes
; for x64 and ARM64 first; this installs whichever matches the PC.
;
; It installs per user, without administrator rights, to %LOCALAPPDATA%\Programs\'08cord (or where an earlier
; version went: it was Swiftcord). Settings, the sign-in and caches live in %LOCALAPPDATA%\Swiftcord, under the
; app's first name, and are kept when uninstalling.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppName "'08cord"
#define ExeName "swiftcord.exe"
; Must match APP_ID in crates/app/src/platform/mod.rs, so notifications show under '08cord.
#define AppUserModelId "Swiftcord.Rust"

[Setup]
AppId={{6F3C2B1E-9A4D-4E8B-B2C7-5D1A0F7E3C91}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion} (test build)
AppPublisher='08cord
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible or arm64
ArchitecturesInstallIn64BitMode=x64compatible or arm64
MinVersion=10.0.17763
OutputDir=..\target\installer
OutputBaseFilename=08cord-Setup-{#AppVersion}
SetupIconFile=..\crates\app\assets\icon.ico
InfoBeforeFile=TESTERS.txt
UninstallDisplayIcon={app}\{#ExeName}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
CloseApplicationsFilter={#ExeName}
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
; The native build for the PC: ARM64 on ARM devices, x64 everywhere else.
Source: "..\target\aarch64-pc-windows-msvc\release\{#ExeName}"; DestDir: "{app}"; Check: IsArm64; Flags: ignoreversion
Source: "..\target\x86_64-pc-windows-msvc\release\{#ExeName}"; DestDir: "{app}"; Check: not IsArm64; Flags: ignoreversion

[InstallDelete]
; The shortcuts from when it was called Swiftcord.
Type: files; Name: "{autoprograms}\Swiftcord.lnk"
Type: files; Name: "{autodesktop}\Swiftcord.lnk"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#ExeName}"; AppUserModelID: "{#AppUserModelId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#ExeName}"; AppUserModelID: "{#AppUserModelId}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#ExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// Sign-in and the video window use Microsoft Edge WebView2, which Windows 11 includes and most Windows 10 PCs
// have; warn (but carry on) if it's missing.
function WebView2Installed: Boolean;
var
  Version: String;
begin
  Result :=
    RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version) or
    RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version) or
    RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version);
  Result := Result and (Version <> '') and (Version <> '0.0.0.0');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and not WebView2Installed and not WizardSilent then
    MsgBox('Microsoft Edge WebView2 doesn''t seem to be installed. ''08cord will still work, but signing in ' +
      'through the browser and playing YouTube videos need it. You can get it from ' +
      'https://developer.microsoft.com/microsoft-edge/webview2/ (the "Evergreen Bootstrapper").', mbInformation, MB_OK);
end;
