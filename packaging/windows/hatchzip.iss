; HatchZip-x64-setup.exe, built with Inno Setup 6 from a folder holding the
; files of the CLI release zip plus hatchzip-gui.exe (see build-setup.ps1):
;
;   iscc /DPayload=C:\path\to\payload /DAppVersion=0.1.0 packaging\windows\hatchzip.iss
;
; It installs for the current user only and needs no administrator rights.
; It uses the same folder as install.ps1 and puts it on PATH the same way,
; so either one updates what the other installed.

#ifndef Payload
  #define Payload "..\..\dist\payload"
#endif
#ifndef AppVersion
  #define AppVersion GetStringFileInfo(Payload + "\hatchzip-gui.exe", "ProductVersion")
#endif

[Setup]
; Never change: Windows finds the installed copy to update or remove by it.
AppId=io.github.xevrion.hatchzip
AppName=HatchZip
AppVersion={#AppVersion}
AppVerName=HatchZip {#AppVersion}
AppPublisher=Yash Bavadiya
AppPublisherURL=https://github.com/xevrion/hatchzip
AppSupportURL=https://github.com/xevrion/hatchzip/issues
AppUpdatesURL=https://github.com/xevrion/hatchzip/releases
AppComments=Extract huge archives in place
DefaultDirName={localappdata}\Programs\HatchZip
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
UsedUserAreasWarning=no
; ARM64 Windows runs the x64 build through its built-in emulation.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\..\dist
OutputBaseFilename=HatchZip-x64-setup
SetupIconFile=..\..\assets\icon\hatchzip.ico
WizardStyle=modern
UninstallDisplayIcon={app}\hatchzip-gui.exe
UninstallDisplayName=HatchZip
Compression=lzma2/max
SolidCompression=yes
; Tells Explorer and new terminals that PATH changed, on install and on
; uninstall.
ChangesEnvironment=yes
; Also lets Explorer pick up the "Open with" entries without a sign-out.
ChangesAssociations=yes

[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; The short name for the CLI. A copy, since Windows has no symlinks without
; developer mode; it's a few MB.
Source: "{#Payload}\hatchzip.exe"; DestDir: "{app}"; DestName: "hz.exe"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\HatchZip"; Filename: "{app}\hatchzip-gui.exe"; WorkingDir: "{userdocs}"; Comment: "Extract huge archives in place"

[Registry]
; "Open with" for each archive type, without taking over the default app.
Root: HKCU; Subkey: "Software\Classes\HatchZip.Archive"; ValueType: string; ValueData: "Archive"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\HatchZip.Archive\DefaultIcon"; ValueType: string; ValueData: """{app}\hatchzip-gui.exe"",0"
Root: HKCU; Subkey: "Software\Classes\HatchZip.Archive\shell\open\command"; ValueType: string; ValueData: """{app}\hatchzip-gui.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\Applications\hatchzip-gui.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "HatchZip"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\.zip\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.7z\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tar\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tgz\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.gz\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.001\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.rar\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.r00\OpenWithProgids"; ValueType: string; ValueName: "HatchZip.Archive"; ValueData: ""; Flags: uninsdeletevalue

[Run]
; The right-click menu lives in the user's registry hive and is written by
; the app itself, so the installer and the app's settings switch agree.
Filename: "{app}\hatchzip-gui.exe"; Parameters: "--register-shell"; StatusMsg: "Adding HatchZip to the right-click menu..."; Flags: runhidden waituntilterminated
Filename: "{app}\hatchzip-gui.exe"; Description: "Open HatchZip"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\hatchzip-gui.exe"; Parameters: "--unregister-shell"; RunOnceId: "UnregisterShell"; Flags: runhidden waituntilterminated

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Code]
const
  EnvKey = 'Environment';

{ True if Dir is one of the entries of the user's PATH, ignoring case. }
function PathHas(Path, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') > 0;
end;

procedure AddToPath(Dir: String);
var
  Path: String;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Path) then
    Path := '';
  if PathHas(Path, Dir) then
    exit;
  if (Path <> '') and (Path[Length(Path)] <> ';') then
    Path := Path + ';';
  { Expandable, like the value Windows creates, so entries such as
    %USERPROFILE%\bin keep working. }
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Path + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Path: String;
  P: Integer;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Path) then
    exit;
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P = 0 then
    exit;
  Delete(Path, P, Length(Dir) + 1);
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Copy(Path, 2, Length(Path) - 2));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
