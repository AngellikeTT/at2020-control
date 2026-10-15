; Inno Setup script (used by CI and by scripts\package.ps1 when Inno Setup is installed).
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\dist\at2020-control.exe"
#endif
#ifndef OutDir
  #define OutDir "..\dist"
#endif

[Setup]
AppId={{6C3B9F6A-3D4E-4B7E-9C0F-5A2020C0DE01}
AppName=AT2020USB-X Control Panel
AppVersion={#AppVersion}
AppPublisher=AT2020 Control Panel
DefaultDirName={autopf}\AT2020 Control Panel
DefaultGroupName=AT2020 Control Panel
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutDir}
OutputBaseFilename=AT2020-Control-{#AppVersion}-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\at2020-control.exe
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\app\assets\fonts\Inter-OFL.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\crates\app\assets\fonts\JetBrainsMono-OFL.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{group}\AT2020 Control Panel"; Filename: "{app}\at2020-control.exe"
Name: "{autodesktop}\AT2020 Control Panel"; Filename: "{app}\at2020-control.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Run]
Filename: "{app}\at2020-control.exe"; Description: "Launch AT2020 Control Panel"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "taskkill"; Parameters: "/im at2020-control.exe /f"; Flags: runhidden; RunOnceId: "KillApp"
