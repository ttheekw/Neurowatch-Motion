; Compile with /DVersion=... /DBundleDir=... /DOutputDir=...
[Setup]
AppId=NeuroWatchMotion
AppName=NeuroWatch Motion
AppVersion={#Version}
AppPublisher=NeuroWatch Project
AppPublisherURL=https://github.com/ttheekw/Neurowatch-Motion
DefaultDirName={localappdata}\Programs\NeuroWatch Motion
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
DisableProgramGroupPage=yes
LicenseFile={#BundleDir}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=nw-motion-{#Version}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\bin\nw-motion.exe

[Files]
Source: "{#BundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{userstartup}\NeuroWatch Motion"; Filename: "{app}\bin\nw-motion-launcher.exe"; Comment: "Start local motion detection for NeuroWatch"

[Run]
Filename: "{app}\bin\nw-motion-launcher.exe"; Flags: waituntilterminated

[Code]
procedure StopInstalledDaemon();
var
  Locator, Services, Processes, Process: Variant;
  Index: Integer;
  Target: String;
begin
  Target := ExpandConstant('{app}\bin\nw-motion.exe');
  if not FileExists(Target) then
    Exit;
  Locator := CreateOleObject('WbemScripting.SWbemLocator');
  Services := Locator.ConnectServer('', 'root\CIMV2');
  Processes := Services.ExecQuery('SELECT * FROM Win32_Process WHERE Name = ''nw-motion.exe''');
  for Index := 0 to Processes.Count - 1 do
  begin
    Process := Processes.ItemIndex(Index);
    if not VarIsNull(Process.ExecutablePath) then
      if CompareText(Process.ExecutablePath, Target) = 0 then
        if Process.Terminate(0) <> 0 then
          RaiseException('Could not stop the installed motion daemon. Close it and try again.');
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  try
    StopInstalledDaemon();
  except
    Result := GetExceptionMessage();
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopInstalledDaemon();
end;
