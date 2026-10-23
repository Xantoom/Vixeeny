; Vixeeny installer (Inno Setup 6). Built by the release workflow:
;   iscc /DAppVersion=0.5.0 /DStageDir=dist\stage /DOutDir=dist packaging\windows\vixeeny.iss
;   (add /ALLUSERS or /CURRENTUSER to skip the choice below).
; For all users (in Program Files, with the administrator's rights; Vixeeny can then have them
; too, see `--admin`), or for the user alone (in their own folders, without them).

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
DefaultDirName={autopf}\Vixeeny
DefaultGroupName=Vixeeny
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
; For all users, the first choice, even over an install for the user alone (which it moves).
UsePreviousPrivileges=no
; The autostart entry and the settings are the user's own, also in an install for all users.
UsedUserAreasWarning=no
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
; For all users: the user's task that starts Vixeeny with the administrator's rights.
Filename: "{app}\vixeeny-app.exe"; Parameters: "--admin register"; Flags: runhidden waituntilterminated; Check: IsAdminInstallMode
; Started as the user (without the rights: the task gives them).
Filename: "{app}\Vixeeny.exe"; Description: "Vixeeny"; Flags: nowait postinstall skipifsilent runasoriginaluser
; Moved from the user's folders by the settings' button (a silent install): it starts again.
Filename: "{app}\Vixeeny.exe"; Flags: nowait runasoriginaluser; Check: MovedSilently

[UninstallRun]
Filename: "{app}\vixeeny-app.exe"; Parameters: "--admin remove"; RunOnceId: "RemoveTasks"; Flags: runhidden waituntilterminated; Check: IsAdminInstallMode
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
const
  // Where the install for the user alone is recorded (the AppId, then `_is1`).
  UserInstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{C3B9682C-A50C-4D2D-A615-DB8FE7EC4EFD}_is1';

var
  OldFiles: TStringList;
  // An install for the user alone that this one, for all users, replaces ('' if none), and
  // its Start menu folder.
  UserDir: String;
  UserGroup: String;

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

// The install for the user alone, when this one is for all users: it moves here.
procedure FindUserInstall();
var
  Dir: String;
begin
  UserDir := '';
  UserGroup := '';
  if not IsAdminInstallMode then
    Exit;
  if not RegQueryStringValue(HKCU, UserInstallKey, 'Inno Setup: App Path', Dir) then
    Exit;
  Dir := RemoveBackslashUnlessRoot(Dir);
  // Only a Vixeeny folder, and never this install's.
  if (Dir <> '') and FileExists(Dir + '\vixeeny-app.exe') and not SameText(Dir, ExpandConstant('{app}')) then
  begin
    UserDir := Dir;
    RegQueryStringValue(HKCU, UserInstallKey, 'Inno Setup: Icon Group', UserGroup);
  end;
end;

// Stops the Vixeeny of the user's folders, as `StopVixeeny` does; what does not stop is ended
// (its files are removed next).
procedure StopUserVixeeny();
var
  Code: Integer;
begin
  Exec(UserDir + '\vixeeny-app.exe', '--update quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM Vixeeny.exe /IM vixeeny-app.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

// Removes the install for the user alone: its folder, its Start menu entry and its uninstall
// entry. Its settings stay (they are not in its folder); the autostart entry follows the
// program on its next start.
procedure RemoveUserInstall();
begin
  DelTree(UserDir, True, True, True);
  if UserGroup <> '' then
    DelTree(ExpandConstant('{userprograms}\') + UserGroup, True, True, True);
  RegDeleteKeyIncludingSubkeys(HKCU, UserInstallKey);
end;

function MovedSilently(): Boolean;
begin
  Result := WizardSilent and (UserDir <> '');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopVixeeny();
  FindUserInstall();
  if UserDir <> '' then
    StopUserVixeeny();
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
  else if (CurStep = ssPostInstall) and (UserDir <> '') then
    RemoveUserInstall();
  if (CurStep = ssPostInstall) and (OldFiles <> nil) then
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
