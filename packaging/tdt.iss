; Optional wizard installer. packaging/build-installer.ps1 ships TDT-Setup.exe
; as a self-contained stub and does not require Inno Setup.
#define AppName "TDT"
#define AppLongName "TDT - Talk Don't Type"
#ifndef AppVersion
  #define AppVersion "0.1.5"
#endif
#define AppPublisher "Hi9841"
#define AppURL "https://github.com/Hi9841/tdt"
#define AppIdStr "{{E7B2C1A0-4D8F-4B6A-9C31-7A1B2C3D4E5F}"

[Setup]
AppId={#AppIdStr}
AppName={#AppName}
AppVerName={#AppLongName} {#AppVersion}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
DefaultDirName={localappdata}\TDT
DefaultGroupName=TDT
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
InfoBeforeFile=..\THIRD_PARTY_NOTICES.md
OutputDir=..\dist
OutputBaseFilename=TDT-Setup
Compression=lzma2
SolidCompression=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
UninstallDisplayIcon={app}\TDT.exe
UninstallDisplayName={#AppLongName}
CloseApplications=yes
RestartApplications=no
WizardStyle=modern
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts"; Flags: unchecked
Name: "startup"; Description: "Start TDT when I sign in"; GroupDescription: "Startup"; Flags: unchecked

[Files]
Source: "stage\TDT.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "stage\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "stage\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "stage\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "stage\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "stage\models\sensevoice\*"; DestDir: "{app}\models\sensevoice"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\TDT"; Filename: "{app}\TDT.exe"; Comment: "Talk Don't Type"
Name: "{group}\Uninstall TDT"; Filename: "{uninstallexe}"
Name: "{userdesktop}\TDT"; Filename: "{app}\TDT.exe"; Tasks: desktopicon
Name: "{userstartup}\TDT"; Filename: "{app}\TDT.exe"; Tasks: startup

[Run]
Filename: "{app}\TDT.exe"; Description: "Open TDT"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\models"
