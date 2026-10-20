; Vixeeny installer (Inno Setup 6). Built by the release workflow:
;   iscc /DAppVersion=0.5.0 /DStageDir=dist\stage /DOutDir=dist packaging\windows\vixeeny.iss
; Per-user, no administrator rights.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\stage"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif

[Setup]
AppId={{C3B9682C-A50C-4D2D-A615-DB8FE7EC4EFD}
AppName=Vixeeny
AppVersion={#AppVersion}
AppPublisher=Xantoom
AppPublisherURL=https://github.com/Xantoom/Vixeeny
DefaultDirName={localappdata}\Programs\Vixeeny
DefaultGroupName=Vixeeny
PrivilegesRequired=lowest
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible
; Windows 10 1903: the first with the screen capture Vixeeny uses.
MinVersion=10.0.18362
OutputDir={#OutDir}
OutputBaseFilename=Vixeeny-{#AppVersion}-setup
Compression=lzma2/ultra
SolidCompression=yes
UninstallDisplayIcon={app}\Vixeeny.exe
SetupIconFile=..\icons\vixeeny.ico
LicenseFile={#StageDir}\LICENSE
CloseApplications=yes
RestartApplications=no
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "startmenu"; Description: "{cm:CreateStartMenu}"; GroupDescription: "{cm:ShortcutsGroup}"
Name: "autostart"; Description: "{cm:StartWithWindows}"; GroupDescription: "{cm:ShortcutsGroup}"

[CustomMessages]
english.CreateStartMenu=Create a Start menu entry
english.StartWithWindows=Start Vixeeny when I sign in
english.ShortcutsGroup=Shortcuts
english.KeepSettings=Keep your settings and the update information?
french.CreateStartMenu=Créer une entrée dans le menu Démarrer
french.StartWithWindows=Lancer Vixeeny à l'ouverture de session
french.ShortcutsGroup=Raccourcis
french.KeepSettings=Conserver vos paramètres et les informations de mise à jour ?

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; The same identity as the program's windows: Windows groups them under this entry.
Name: "{group}\Vixeeny"; Filename: "{app}\Vixeeny.exe"; AppUserModelID: "Xantoom.Vixeeny"; Tasks: startmenu

[Registry]
; The daemon keeps this entry in line with the `autostart` setting afterwards.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Vixeeny"; ValueData: """{app}\Vixeeny.exe"""; Tasks: autostart; Flags: uninsdeletevalue
; Removed on uninstall even when the setting, not this task, wrote it.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Vixeeny"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\Vixeeny.exe"; Description: "Vixeeny"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Only if Vixeeny did not stop when asked (see StopVixeeny). Hidden: no window shows.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM Vixeeny.exe /IM vixeeny-daemon.exe /IM vixeeny-app.exe"; RunOnceId: "StopVixeeny"; Flags: runhidden

[InstallDelete]
; Files of versions older than the list `files.txt` (see CurStepChanged): the console updater
; (before 0.9.2), the daemon under its old name and FFmpeg's device library (before 0.9.4).
Type: files; Name: "{app}\vixeeny-updater.exe"
Type: files; Name: "{app}\vixeeny-daemon.exe"
Type: files; Name: "{app}\avdevice-*.dll"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\.update-staged"
Type: filesandordirs; Name: "{app}\.update-backup"
Type: filesandordirs; Name: "{app}\.update-download"
Type: files; Name: "{app}\.update-lock"

[Code]
var
  OldFiles: TStringList;

// Stops a running Vixeeny the way an update does: a recording ends first, and nothing is
// killed. The program of the installed version asks; a version too old to know how does
// nothing, and Windows' own closing (CloseApplications) follows.
procedure StopVixeeny();
var
  App: String;
  Code: Integer;
begin
  App := ExpandConstant('{app}\vixeeny-app.exe');
  if FileExists(App) then
    Exec(App, '--update quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopVixeeny();
  Result := '';
end;

// The files the previous version listed (files.txt) and this one no longer has are removed,
// as an update does; files of no list (settings, portable.flag) stay.
procedure CurStepChanged(CurStep: TSetupStep);
var
  NewFiles: TStringList;
  I: Integer;
  Name: String;
begin
  if CurStep = ssInstall then
  begin
    OldFiles := TStringList.Create;
    if FileExists(ExpandConstant('{app}\files.txt')) then
      OldFiles.LoadFromFile(ExpandConstant('{app}\files.txt'));
  end
  else if (CurStep = ssPostInstall) and (OldFiles <> nil) then
  begin
    NewFiles := TStringList.Create;
    try
      NewFiles.LoadFromFile(ExpandConstant('{app}\files.txt'));
      for I := 0 to OldFiles.Count - 1 do
      begin
        Name := Trim(OldFiles[I]);
        // Only a path inside the folder.
        if (Name <> '') and (NewFiles.IndexOf(Name) < 0) then
          if (Pos('..', Name) = 0) and (Pos(':', Name) = 0) and (Pos('/', Name) <> 1) then
          begin
            StringChangeEx(Name, '/', '\', True);
            DeleteFile(ExpandConstant('{app}\') + Name);
          end;
      end;
    finally
      NewFiles.Free;
      OldFiles.Free;
      OldFiles := nil;
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Dir: String;
begin
  if CurUninstallStep = usUninstall then
    StopVixeeny()
  else if CurUninstallStep = usPostUninstall then
  begin
    Dir := ExpandConstant('{userappdata}\Vixeeny');
    // Kept in a silent uninstall.
    if DirExists(Dir) and (SuppressibleMsgBox(CustomMessage('KeepSettings'), mbConfirmation, MB_YESNO, IDYES) = IDNO) then
      DelTree(Dir, True, True, True);
  end;
end;
