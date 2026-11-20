; Inno Setup script for m3u8 studio (per-user, fully self-contained).
;
; Build locally:
;   ISCC.exe /DMyAppVersion=1.3.0 installer\m3u8-studio.iss
;
; The installer puts the executable, the bundled browser extension, settings
; and downloads inside one writable folder:
;   %LOCALAPPDATA%\Programs\m3u8 studio
; No admin rights and no VC++ redistributable are required.

#define MyAppName "m3u8 studio"
#define MyAppPublisher "faroukserk"
#define MyAppURL "https://github.com/faroukserk/m3u8-studio"
#define MyAppExeName "m3u8-studio.exe"

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0-dev"
#endif

[Setup]
; Keep this AppId stable: it is how Windows recognises upgrades of the app.
AppId={{7C4A1E3B-9D6F-4A2C-8B1E-5F3D2A9C7E41}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Per-user install: the install folder stays writable, so the app can keep
; settings, downloads and the unpacked extension next to the executable.
PrivilegesRequired=lowest
OutputDir=..
OutputBaseFilename=m3u8-studio-{#MyAppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\{#MyAppExeName}
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; Ready-to-load extension. The app refreshes this folder itself when the
; version changes, so the stamp file is deliberately not shipped.
Source: "..\extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: ".installed-version"

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Runtime data that can be regenerated. Downloads are user content: the
; uninstaller asks before touching them (see [Code] below).
Type: filesandordirs; Name: "{app}\extension"
Type: filesandordirs; Name: "{app}\data"

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DownloadsDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DownloadsDir := ExpandConstant('{app}\downloads');
    if DirExists(DownloadsDir) and (not UninstallSilent) then
    begin
      if MsgBox('Also delete the downloaded videos in:' + #13#10 + DownloadsDir + '?',
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DownloadsDir, True, True, True);
    end;
  end;
end;
