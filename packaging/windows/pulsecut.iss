; Inno Setup script for PulseCut (per-user install, no admin rights).
[Setup]
AppName=PulseCut
AppVersion=0.1.0
DefaultDirName={localappdata}\Programs\PulseCut
PrivilegesRequired=lowest
OutputDir=..\..\target\dist
OutputBaseFilename=PulseCut-setup-x64
ArchitecturesInstallIn64BitMode=x64compatible
[Files]
Source: "..\..\target\dist\PulseCut\*"; DestDir: "{app}"; Flags: recursesubdirs
[Icons]
Name: "{userprograms}\PulseCut"; Filename: "{app}\PulseCut.exe"
[Tasks]
Name: "autostart"; Description: "Start PulseCut when I sign in"; Flags: unchecked
[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "PulseCut"; ValueData: """{app}\PulseCut.exe"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue
