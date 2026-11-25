; Vixeeny installer (Inno Setup 7), itself a 64-bit program. Built by the release workflow:
;   iscc /DAppVersion=0.5.0 /DStageDir=dist\stage /DOutDir=dist packaging\windows\vixeeny.iss
;   (add /ALLUSERS or /CURRENTUSER to skip the choice below).
; For all users (in Program Files, with the administrator's rights; Vixeeny can then have them
; too, see `--admin`), or for the user alone (in their own folders, without them).
; The pages: the install mode, then one page with the folder, the start with Windows and the
; updates (not shown again on an update), the progress and the end. The language is the one of Windows.

#if Ver < EncodeVer(7, 1, 1)
  #error Inno Setup 7.1.1 or later is needed (the 64-bit installer, the windows11 style, dark mode, hidebevels)
#endif

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\stage"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif
; The version without its pre-release part (1.0.0-RC.2 gives 1.0.0), for the file's version
; resource, which takes numbers only.
#if Pos("-", AppVersion) > 0
  #define NumericVersion Copy(AppVersion, 1, Pos("-", AppVersion) - 1)
#else
  #define NumericVersion AppVersion
#endif

[Setup]
AppId={{C3B9682C-A50C-4D2D-A615-DB8FE7EC4EFD}
AppName=Vixeeny
AppVersion={#AppVersion}
AppPublisher=Xantoom
AppPublisherURL=https://github.com/Xantoom/Vixeeny
AppSupportURL=https://github.com/Xantoom/Vixeeny/issues
AppUpdatesURL=https://github.com/Xantoom/Vixeeny/releases
; The entry in Windows' installed apps: the name alone, the version in its own field (by default
; Inno Setup writes "Vixeeny version 1.0.0" as the name). Vixeeny's own updates keep the version
; field in line (see `vixeeny_platform::registry::sync_uninstall_version`).
UninstallDisplayName=Vixeeny
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoDescription=Vixeeny Setup
DefaultDirName={autopf}\Vixeeny
; Only to find the Start menu folder of older versions (see [InstallDelete]): the entry is now
; at the top of the Start menu, without a folder.
DefaultGroupName=Vixeeny
DisableProgramGroupPage=yes
DisableReadyPage=yes
ShowLanguageDialog=no
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
; For all users, the first choice, even over an install for the user alone (which it moves).
UsePreviousPrivileges=no
; The autostart entry and the settings are the user's own, also in an install for all users.
UsedUserAreasWarning=no
SetupArchitecture=x64
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
CloseApplications=yes
RestartApplications=no
; Windows 11's look, light or dark as Windows is.
WizardStyle=modern dynamic windows11 hidebevels
; The logo, centred, at the size that fits the screen's scale.
WizardImageFile=..\icons\vixeeny-128.png,..\icons\vixeeny-256.png
WizardSmallImageFile=..\icons\vixeeny-48.png,..\icons\vixeeny-64.png
WizardImageStretch=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Messages]
english.PrivilegesRequiredOverrideTitle=Vixeeny Setup
english.PrivilegesRequiredOverrideInstruction=Install for
english.PrivilegesRequiredOverrideText1=For all accounts, %1 asks for the administrator's rights and can then capture games started as administrator.
english.PrivilegesRequiredOverrideText2=For all accounts, %1 asks for the administrator's rights and can then capture games started as administrator.
english.PrivilegesRequiredOverrideAllUsers=&All accounts on this PC
english.PrivilegesRequiredOverrideAllUsersRecommended=&All accounts on this PC
english.PrivilegesRequiredOverrideCurrentUser=&My account only
english.PrivilegesRequiredOverrideCurrentUserRecommended=&My account only
english.WizardSelectDir=Install Vixeeny
english.SelectDirDesc=Folder and start-up
english.SelectDirLabel3=Folder
english.DiskSpaceGBLabel=[gb] GB needed.
english.DiskSpaceMBLabel=[mb] MB needed.
english.FinishedHeadingLabel=[name] is installed
english.FinishedLabel=[name] is in the Start menu; once open, it stays in the notification area.
english.FinishedLabelNoIcons=[name] is in the Start menu; once open, it stays in the notification area.
english.ClickFinish=
english.ConfirmUninstall=Uninstall %1?
french.PrivilegesRequiredOverrideTitle=Installation de Vixeeny
french.PrivilegesRequiredOverrideInstruction=Installer pour
french.PrivilegesRequiredOverrideText1=Pour tous les comptes, %1 demande les droits administrateur et peut alors capturer les jeux lancés en administrateur.
french.PrivilegesRequiredOverrideText2=Pour tous les comptes, %1 demande les droits administrateur et peut alors capturer les jeux lancés en administrateur.
french.PrivilegesRequiredOverrideAllUsers=&Tous les comptes de ce PC
french.PrivilegesRequiredOverrideAllUsersRecommended=&Tous les comptes de ce PC
french.PrivilegesRequiredOverrideCurrentUser=&Mon compte uniquement
french.PrivilegesRequiredOverrideCurrentUserRecommended=&Mon compte uniquement
french.WizardSelectDir=Installer Vixeeny
french.SelectDirDesc=Dossier et démarrage
french.SelectDirLabel3=Dossier
french.DiskSpaceGBLabel=[gb] Go nécessaires.
french.DiskSpaceMBLabel=[mb] Mo nécessaires.
french.FinishedHeadingLabel=[name] est installé
french.FinishedLabel=[name] est dans le menu Démarrer ; une fois ouvert, il reste dans la zone de notification.
french.FinishedLabelNoIcons=[name] est dans le menu Démarrer ; une fois ouvert, il reste dans la zone de notification.
french.ClickFinish=
french.ConfirmUninstall=Désinstaller %1 ?

[CustomMessages]
english.StartWithWindows=Start Vixeeny with Windows
english.OpenVixeeny=Open Vixeeny
english.KeepSettingsTitle=Keep your settings?
english.KeepSettingsText=Your settings, shortcuts and Vixeeny's logs can stay for a later install. Your captures and recordings are never removed.
english.KeepSettingsYes=Keep my settings
english.KeepSettingsNo=Remove everything
english.MaintenanceTitle=Vixeeny is already installed
english.MaintenanceOlder=Version %1 is installed. What do you want to do?
english.MaintenanceSame=This version (%1) is installed. What do you want to do?
english.MaintenanceNewer=A newer version (%1) is installed. What do you want to do?
english.ActionUpdate=Update to version %1
english.ActionRepair=Repair: install the files of version %1 again
english.ActionDowngrade=Install version %1 over it
english.ActionUninstall=Uninstall Vixeeny
english.StoppingVixeeny=Closing Vixeeny
english.StoppingVixeenyText=A recording in progress ends and is saved first.
english.AutoUpdate=Install updates automatically
english.AutoUpdateNote=Vixeeny connects to github.com only to check for and download its updates. Nothing else leaves your PC.
french.StartWithWindows=Démarrer Vixeeny avec Windows
french.OpenVixeeny=Ouvrir Vixeeny
french.KeepSettingsTitle=Conserver vos paramètres ?
french.KeepSettingsText=Vos paramètres, vos raccourcis et les journaux de Vixeeny peuvent rester pour une prochaine installation. Vos captures et vos enregistrements ne sont jamais supprimés.
french.KeepSettingsYes=Conserver mes paramètres
french.KeepSettingsNo=Tout supprimer
french.MaintenanceTitle=Vixeeny est déjà installé
french.MaintenanceOlder=La version %1 est installée. Que voulez-vous faire ?
french.MaintenanceSame=Cette version (%1) est installée. Que voulez-vous faire ?
french.MaintenanceNewer=Une version plus récente (%1) est installée. Que voulez-vous faire ?
french.ActionUpdate=Mettre à jour vers la version %1
french.ActionRepair=Réparer : réinstaller les fichiers de la version %1
french.ActionDowngrade=Installer la version %1 à la place
french.ActionUninstall=Désinstaller Vixeeny
french.StoppingVixeeny=Fermeture de Vixeeny
french.StoppingVixeenyText=Un enregistrement en cours se termine et est d'abord enregistré.
french.AutoUpdate=Installer les mises à jour automatiquement
french.AutoUpdateNote=Vixeeny se connecte à github.com uniquement pour vérifier et télécharger ses mises à jour. Rien d'autre ne quitte votre PC.

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; The same identity as the program's windows: Windows groups them under this entry.
Name: "{autoprograms}\Vixeeny"; Filename: "{app}\Vixeeny.exe"; AppUserModelID: "Xantoom.Vixeeny"

[Registry]
; Written by the app as the user who installs (see [Run]), kept in line with the `autostart`
; setting by the daemon; removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Vixeeny"; Flags: uninsdeletevalue
; The `vixeeny://` links of the Windows notifications of older versions, which nothing opens any more.
Root: HKCU; Subkey: "Software\Classes\vixeeny"; ValueType: none; Flags: deletekey uninsdeletekey

[Run]
; For all users: the user's task that starts Vixeeny with the administrator's rights.
Filename: "{app}\vixeeny-app.exe"; Parameters: "--admin register"; Flags: runhidden waituntilterminated; Check: IsAdminInstallMode
; A first install: the choices of the folder's page go into the settings and the registry of the
; user who installs (in an install for all users, the administrator who gave the rights may be
; another account), before the first start.
Filename: "{app}\vixeeny-app.exe"; Parameters: "--first-install {code:FirstInstallChoices}"; Flags: runhidden waituntilterminated runasoriginaluser; Check: IsFirstInstall
; The start with Windows of the user who installs points at this folder (an update, a move).
Filename: "{app}\vixeeny-app.exe"; Parameters: "--autostart-refresh"; Flags: runhidden waituntilterminated runasoriginaluser
; Windows' Snipping Tool no longer opens with the Print Screen key over Vixeeny's capture, in the
; user's own settings (Settings, Accessibility, Keyboard). Vixeeny also does it once at its start.
Filename: "{app}\vixeeny-app.exe"; Parameters: "--print-screen take"; Flags: runhidden waituntilterminated runasoriginaluser
; Started as the user (without the rights: the task gives them).
Filename: "{app}\Vixeeny.exe"; Description: "{cm:OpenVixeeny}"; Flags: nowait postinstall skipifsilent runasoriginaluser
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
; The Start menu folder of older versions (the entry is now at the top, without a folder).
Type: files; Name: "{group}\Vixeeny.lnk"
Type: dirifempty; Name: "{group}"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\.update-staged"
Type: filesandordirs; Name: "{app}\.update-backup"
Type: filesandordirs; Name: "{app}\.update-download"
Type: files; Name: "{app}\.update-lock"
Type: files; Name: "{app}\.update-download-turn"

[Code]
const
  // Where an install is recorded (the AppId, then `_is1`): under HKLM for all users, HKCU for
  // the user alone.
  InstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{C3B9682C-A50C-4D2D-A615-DB8FE7EC4EFD}_is1';

var
  OldFiles: TStringList;
  // An install for the user alone that this one, for all users, replaces ('' if none), and
  // its Start menu folder.
  UserDir: String;
  UserGroup: String;
  // The entry of an install for the user alone whose folder no longer exists.
  StaleUserEntry: Boolean;
  // On the folder's page: the start with Windows and the updates.
  AutostartBox: TNewCheckBox;
  UpdateBox: TNewCheckBox;
  UpdateNote: TNewStaticText;
  // The install of the same kind already there: its version and its uninstaller ('' if none).
  PrevVersion: String;
  PrevUninstaller: String;
  // What to do with it, when there is one.
  MaintenancePage: TInputOptionWizardPage;
  // The setup ends without installing (the user chose to uninstall).
  Leaving: Boolean;
  // Uninstall: the user's settings stay, and the files the installed version listed.
  KeepSettings: Boolean;
  ListedFiles: TStringList;

#include "versions.iss"

// ---- Stopping Vixeeny ----------------------------------------------------------------------

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
  StaleUserEntry := False;
  if not IsAdminInstallMode then
    Exit;
  if not RegQueryStringValue(HKCU, InstallKey, 'Inno Setup: App Path', Dir) then
    Exit;
  Dir := RemoveBackslashUnlessRoot(Dir);
  // Its folder is gone (removed by hand): only its entry in the installed apps is left, removed after the install.
  StaleUserEntry := not FileExists(Dir + '\vixeeny-app.exe');
  // Only a Vixeeny folder, and never this install's.
  if (Dir <> '') and FileExists(Dir + '\vixeeny-app.exe') and not SameText(Dir, ExpandConstant('{app}')) then
  begin
    UserDir := Dir;
    RegQueryStringValue(HKCU, InstallKey, 'Inno Setup: Icon Group', UserGroup);
    // A folder of the Start menu, by its name: never a path out of it.
    if (Pos('..', UserGroup) <> 0) or (Pos(':', UserGroup) <> 0) or (Pos('/', UserGroup) <> 0)
      or (Pos('\', UserGroup) = 1) then
      UserGroup := '';
  end;
end;

// Stops the Vixeeny of the user's folders, as `StopVixeeny` does; what does not stop is ended
// (its files are removed next). Its program runs with the user's own rights: its folder is
// the user's to change, and nothing from there gets the administrator's.
procedure StopUserVixeeny();
var
  Code: Integer;
begin
  ExecAsOriginalUser(UserDir + '\vixeeny-app.exe', '--update quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM Vixeeny.exe /IM vixeeny-app.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

// ---- Files ---------------------------------------------------------------------------------

// Whether a line of a list (files.txt) names a file inside the folder: a relative path that
// never goes up.
function InsideFolder(Name: String): Boolean;
begin
  Result := (Name <> '') and (Pos('..', Name) = 0) and (Pos(':', Name) = 0) and (Pos('/', Name) <> 1)
    and (Pos('\', Name) <> 1);
end;

// The lines of `Dir`'s list (files.txt) that name a file inside it, with Windows' separators;
// empty without one.
function LoadList(Dir: String): TStringList;
var
  Lines: TStringList;
  I: Integer;
  Name: String;
begin
  // Names compared without case, as Windows does.
  Result := TStringList.Create;
  if not FileExists(Dir + '\files.txt') then
    Exit;
  Lines := TStringList.Create;
  try
    try
      Lines.LoadFromFile(Dir + '\files.txt');
    except
      Exit;
    end;
    for I := 0 to Lines.Count - 1 do
    begin
      Name := Trim(Lines[I]);
      StringChangeEx(Name, '/', '\', True);
      if InsideFolder(Name) then
        Result.Add(Name);
    end;
  finally
    Lines.Free;
  end;
end;

// Removes the folders under `Dir` (and `Dir` itself) that are empty: the user's own files stay.
procedure RemoveEmptyFolders(Dir: String);
begin
  if DirExists(Dir) then
    DelTree(Dir, True, False, True);
end;

// What updates leave in a folder: the staged version, the backup, the download, the locks.
procedure RemoveUpdateLeftovers(Dir: String);
begin
  DelTree(Dir + '\.update-staged', True, True, True);
  DelTree(Dir + '\.update-backup', True, True, True);
  DelTree(Dir + '\.update-download', True, True, True);
  DeleteFile(Dir + '\.update-lock');
  DeleteFile(Dir + '\.update-download-turn');
end;

// Removes the files of the install for the user alone: those its list (files.txt) names, its
// uninstaller and what its updates left; without a list (an old version), its programs and
// libraries. Its folders go once empty: what the user put there stays.
procedure RemoveUserFiles();
var
  Files: TStringList;
  I: Integer;
begin
  if FileExists(UserDir + '\files.txt') then
  begin
    Files := LoadList(UserDir);
    try
      for I := 0 to Files.Count - 1 do
        DeleteFile(UserDir + '\' + Files[I]);
    finally
      Files.Free;
    end;
    DeleteFile(UserDir + '\files.txt');
  end
  else
  begin
    DelTree(UserDir + '\*.exe', False, True, False);
    DelTree(UserDir + '\*.dll', False, True, False);
  end;
  DelTree(UserDir + '\unins*.*', False, True, False);
  RemoveUpdateLeftovers(UserDir);
  RemoveEmptyFolders(UserDir);
end;

// Removes the install for the user alone: its files, its Start menu entry and its uninstall
// entry. Its settings stay (they are not in its folder); the autostart entry follows the
// program on its next start.
procedure RemoveUserInstall();
begin
  RemoveUserFiles();
  DeleteFile(ExpandConstant('{userprograms}\Vixeeny.lnk'));
  if UserGroup <> '' then
    DelTree(ExpandConstant('{userprograms}\') + UserGroup, True, True, True);
  RegDeleteKeyIncludingSubkeys(HKCU, InstallKey);
end;

// ---- The wizard ----------------------------------------------------------------------------

// The install of the same kind (all users, or this user alone) already there.
procedure FindPreviousInstall();
var
  Uninstaller: String;
begin
  PrevVersion := '';
  PrevUninstaller := '';
  if not RegQueryStringValue(HKA, InstallKey, 'DisplayVersion', PrevVersion) then
    PrevVersion := '';
  if RegQueryStringValue(HKA, InstallKey, 'UninstallString', Uninstaller) then
    PrevUninstaller := RemoveQuotes(Uninstaller);
  // An entry whose uninstaller is gone (its folder removed by hand) is no install: this one takes its place.
  if (PrevUninstaller = '') or not FileExists(PrevUninstaller) then
  begin
    PrevVersion := '';
    PrevUninstaller := '';
  end;
end;

// The page that says what is installed and what this setup does with it: update an older
// version, repair this one, put this one over a newer one, or uninstall.
procedure CreateMaintenancePage();
var
  Order: Integer;
  Text: String;
  Action: String;
begin
  Order := CompareVersions(PrevVersion, '{#AppVersion}');
  if Order < 0 then
  begin
    Text := FmtMessage(CustomMessage('MaintenanceOlder'), [PrevVersion]);
    Action := FmtMessage(CustomMessage('ActionUpdate'), ['{#AppVersion}']);
  end
  else if Order = 0 then
  begin
    Text := FmtMessage(CustomMessage('MaintenanceSame'), [PrevVersion]);
    Action := FmtMessage(CustomMessage('ActionRepair'), ['{#AppVersion}']);
  end
  else
  begin
    Text := FmtMessage(CustomMessage('MaintenanceNewer'), [PrevVersion]);
    Action := FmtMessage(CustomMessage('ActionDowngrade'), ['{#AppVersion}']);
  end;
  MaintenancePage := CreateInputOptionPage(wpWelcome, CustomMessage('MaintenanceTitle'), Text, '', True, False);
  MaintenancePage.Add(Action);
  MaintenancePage.Add(CustomMessage('ActionUninstall'));
  MaintenancePage.SelectedValueIndex := 0;
end;

// The folder's page only says where: the start with Windows goes under the folder, in the
// place of the text about the buttons.
procedure InitializeWizard();
var
  Shift: Integer;
begin
  Shift := WizardForm.DirEdit.Top - WizardForm.SelectDirBrowseLabel.Top;
  WizardForm.SelectDirBrowseLabel.Visible := False;
  WizardForm.DirEdit.Top := WizardForm.DirEdit.Top - Shift;
  WizardForm.DirBrowseButton.Top := WizardForm.DirBrowseButton.Top - Shift;
  AutostartBox := TNewCheckBox.Create(WizardForm);
  AutostartBox.Parent := WizardForm.SelectDirPage;
  AutostartBox.Caption := CustomMessage('StartWithWindows');
  AutostartBox.Checked := True;
  AutostartBox.Left := WizardForm.DirEdit.Left;
  AutostartBox.Top := WizardForm.DirEdit.Top + WizardForm.DirEdit.Height + ScaleY(20);
  AutostartBox.Width := WizardForm.SelectDirPage.Width - AutostartBox.Left;
  AutostartBox.Height := ScaleY(17);
  // The only data Vixeeny sends on its own, said where it can be turned off.
  UpdateBox := TNewCheckBox.Create(WizardForm);
  UpdateBox.Parent := WizardForm.SelectDirPage;
  UpdateBox.Caption := CustomMessage('AutoUpdate');
  UpdateBox.Checked := True;
  UpdateBox.Left := AutostartBox.Left;
  UpdateBox.Top := AutostartBox.Top + AutostartBox.Height + ScaleY(10);
  UpdateBox.Width := AutostartBox.Width;
  UpdateBox.Height := AutostartBox.Height;
  UpdateNote := TNewStaticText.Create(WizardForm);
  UpdateNote.Parent := WizardForm.SelectDirPage;
  UpdateNote.Caption := CustomMessage('AutoUpdateNote');
  UpdateNote.AutoSize := False;
  UpdateNote.WordWrap := True;
  UpdateNote.Left := UpdateBox.Left + ScaleX(18);
  UpdateNote.Top := UpdateBox.Top + UpdateBox.Height + ScaleY(4);
  UpdateNote.Width := UpdateBox.Width - ScaleX(18);
  UpdateNote.Height := ScaleY(30);
  UpdateNote.Enabled := False;
  FindPreviousInstall();
  if PrevVersion <> '' then
    CreateMaintenancePage();
end;

// The first install only: on an update or a repair (the folder's page is not shown) the
// settings stay as they are.
function IsFirstInstall(): Boolean;
begin
  Result := WizardForm.PrevAppDir = '';
end;

// The choices of the folder's page, for `vixeeny-app --first-install`.
function FirstInstallChoices(Param: String): String;
begin
  if AutostartBox.Checked then
    Result := 'autostart=on'
  else
    Result := 'autostart=off';
  if UpdateBox.Checked then
    Result := Result + ' updates=on'
  else
    Result := Result + ' updates=off';
end;

function MovedSilently(): Boolean;
begin
  Result := WizardSilent and (UserDir <> '');
end;

// Runs the uninstaller of the version installed (it asks about the settings itself), then
// ends this setup. `True` when it ran.
function RunPreviousUninstaller(): Boolean;
var
  Code: Integer;
begin
  Result := (PrevUninstaller <> '')
    and Exec(PrevUninstaller, '', '', SW_SHOWNORMAL, ewWaitUntilTerminated, Code);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (MaintenancePage <> nil) and (CurPageID = MaintenancePage.ID) and (MaintenancePage.SelectedValueIndex = 1) then
  begin
    Result := False;
    if RunPreviousUninstaller() then
    begin
      Leaving := True;
      WizardForm.Close;
    end;
  end;
end;

// Leaving after an uninstall is not a cancel to confirm.
procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  if Leaving then
    Confirm := False;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Page: TOutputMarqueeProgressWizardPage;
begin
  Page := CreateOutputMarqueeProgressPage(CustomMessage('StoppingVixeeny'), CustomMessage('StoppingVixeenyText'));
  Page.Show;
  try
    Page.Animate;
    StopVixeeny();
    FindUserInstall();
    if UserDir <> '' then
      StopUserVixeeny();
  finally
    Page.Hide;
  end;
  Result := '';
end;

// The files the previous version listed (files.txt) and this one no longer has are removed,
// as an update does, with what updates left and the folders left empty; files of no list
// (settings, portable.flag) stay.
procedure CurStepChanged(CurStep: TSetupStep);
var
  NewFiles: TStringList;
  I: Integer;
begin
  if CurStep = ssInstall then
    OldFiles := LoadList(ExpandConstant('{app}'))
  else if CurStep = ssPostInstall then
  begin
    if UserDir <> '' then
      RemoveUserInstall()
    else if StaleUserEntry then
      RegDeleteKeyIncludingSubkeys(HKCU, InstallKey);
    if OldFiles <> nil then
    begin
      NewFiles := LoadList(ExpandConstant('{app}'));
      try
        // Without the new list (it cannot be read), nothing is taken for old.
        if NewFiles.Count > 0 then
          for I := 0 to OldFiles.Count - 1 do
            if NewFiles.IndexOf(OldFiles[I]) < 0 then
              DeleteFile(ExpandConstant('{app}\') + OldFiles[I]);
      finally
        NewFiles.Free;
        OldFiles.Free;
        OldFiles := nil;
      end;
    end;
    RemoveUpdateLeftovers(ExpandConstant('{app}'));
    RemoveEmptyFolders(ExpandConstant('{app}'));
  end;
end;

// ---- Uninstall -----------------------------------------------------------------------------

// Asked before anything is removed. A silent uninstall keeps them.
function InitializeUninstall(): Boolean;
begin
  KeepSettings := True;
  if not UninstallSilent then
    KeepSettings := TaskDialogMsgBox(CustomMessage('KeepSettingsTitle'), CustomMessage('KeepSettingsText'),
      mbConfirmation, MB_YESNO, [CustomMessage('KeepSettingsYes'), CustomMessage('KeepSettingsNo')], 0) <> IDNO;
  Result := True;
end;

// Removes a folder of the user's whole.
procedure RemoveFolder(Dir: String);
begin
  if (Dir <> '') and DirExists(Dir) then
    DelTree(Dir, True, True, True);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  App: String;
  Code: Integer;
  I: Integer;
begin
  App := ExpandConstant('{app}');
  if CurUninstallStep = usUninstall then
  begin
    UninstallProgressForm.StatusLabel.Caption := CustomMessage('StoppingVixeeny');
    StopVixeeny();
    // The Print Screen key goes back to Windows' Snipping Tool when Vixeeny took it from it.
    if FileExists(App + '\vixeeny-app.exe') then
      Exec(App + '\vixeeny-app.exe', '--print-screen give', '', SW_HIDE, ewWaitUntilTerminated, Code);
    // Read before the uninstaller removes it: the files updates brought, which it does not know.
    ListedFiles := LoadList(App);
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    if ListedFiles <> nil then
    begin
      for I := 0 to ListedFiles.Count - 1 do
        DeleteFile(App + '\' + ListedFiles[I]);
      ListedFiles.Free;
      ListedFiles := nil;
    end;
    RemoveUpdateLeftovers(App);
    RemoveEmptyFolders(App);
    // The installer an update downloaded (to move Vixeeny to Program Files).
    RemoveFolder(AddBackslash(GetEnv('TEMP')) + 'Vixeeny-setup');
    // The replay's seconds kept on the disk, when a replay did not end.
    RemoveFolder(AddBackslash(GetEnv('TEMP')) + 'Vixeeny-replay');
    DeleteFile(AddBackslash(GetEnv('TEMP')) + 'Vixeeny-setup.turn');
    if not KeepSettings then
    begin
      // The settings (roaming), then the logs, the crash reports and the caches (local).
      RemoveFolder(ExpandConstant('{userappdata}\Vixeeny'));
      RemoveFolder(ExpandConstant('{localappdata}\Vixeeny'));
    end;
  end;
end;
