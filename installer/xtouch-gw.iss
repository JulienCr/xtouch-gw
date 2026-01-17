; XTouch GW - Inno Setup Installer Script
; Requires Inno Setup 6+ (https://jrsoftware.org/isinfo.php)
;
; Build: Run build-installer.ps1 or use ISCC.exe directly
; Output: dist/xtouch-gw-{version}-setup.exe

#define MyAppName "XTouch GW"
#define MyAppPublisher "Julien Cr"
#define MyAppURL "https://github.com/JulienCr/xtouch-gw-v3"
#define MyAppExeName "xtouch-gw.exe"

; Version is passed via command line: ISCC.exe /DMyAppVersion=X.Y.Z
; Fallback if not provided
#ifndef MyAppVersion
  #define MyAppVersion "3.0.0"
#endif

[Setup]
; Application identity
AppId={{E2D4F8A3-7B6C-4D9E-8F1A-2C3B5E6D7F8A}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes

; Output settings
OutputDir=..\dist
OutputBaseFilename=xtouch-gw-{#MyAppVersion}-setup
Compression=lzma2
SolidCompression=yes

; Windows compatibility
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; Installer appearance
WizardStyle=modern
WizardSizePercent=100
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
LicenseFile=..\LICENSE

; Privileges
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Types]
Name: "full"; Description: "Full installation"
Name: "compact"; Description: "Compact installation (without Stream Deck plugin)"
Name: "custom"; Description: "Custom installation"; Flags: iscustom

[Components]
Name: "main"; Description: "XTouch GW Application"; Types: full compact custom; Flags: fixed
Name: "streamdeck"; Description: "Stream Deck Plugin"; Types: full

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Main application
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion; Components: main
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion; Components: main
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme; Components: main

; Configuration file - installed to AppData (writable by user)
; onlyifdoesntexist prevents overwriting user customizations on upgrade
Source: "..\config.example.yaml"; DestDir: "{userappdata}\{#MyAppName}"; DestName: "config.yaml"; Flags: ignoreversion onlyifdoesntexist; Components: main

; Stream Deck plugin (copy entire plugin folder, excluding dev files and logs)
Source: "..\streamdeck-plugin\com.juliencr.xtouch-gw.sdPlugin\*"; DestDir: "{userappdata}\Elgato\StreamDeck\Plugins\com.juliencr.xtouch-gw.sdPlugin"; Flags: ignoreversion recursesubdirs createallsubdirs; Components: streamdeck; \
  Excludes: "node_modules,.git,*.ts,tsconfig.json,rollup.config.js,pnpm-lock.yaml,logs,logs\*"

[Dirs]
; Create AppData directory for user configuration and state
Name: "{userappdata}\{#MyAppName}"; Flags: uninsneveruninstall

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Configuration"; Filename: "{userappdata}\{#MyAppName}\config.yaml"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Clean up user data on uninstall (optional - user is prompted)
Type: filesandordirs; Name: "{userappdata}\Elgato\StreamDeck\Plugins\com.juliencr.xtouch-gw.sdPlugin"

[Code]
// Custom code for additional setup logic

function InitializeSetup(): Boolean;
begin
  Result := True;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    // Post-installation tasks can be added here
    // e.g., creating default config, registering services, etc.
  end;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
end;
