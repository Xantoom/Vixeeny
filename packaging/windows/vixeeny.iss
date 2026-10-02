; Vixeeny installer (Inno Setup 6). Built by the release workflow:
;   iscc /DAppVersion=0.5.0 /DStageDir=dist\stage /DOutDir=dist packaging\windows\vixeeny.iss
; Per-user, no administrator rights (plan 10.1).

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
OutputDir={#OutDir}
OutputBaseFilename=Vixeeny-{#AppVersion}-setup
Compression=lzma2/ultra
SolidCompression=yes
UninstallDisplayIcon={app}\vixeeny-daemon.exe
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
Name: "contextmenu"; Description: "{cm:ContextMenu}"; GroupDescription: "{cm:IntegrationGroup}"; Flags: unchecked

[CustomMessages]
english.CreateStartMenu=Create a Start menu entry
english.StartWithWindows=Start Vixeeny when I sign in
english.ContextMenu=Add "Convert with Vixeeny" to the Explorer menu of images
english.ShortcutsGroup=Shortcuts
english.IntegrationGroup=Integration
english.KeepSettings=Keep your settings and the update information?
french.CreateStartMenu=Créer une entrée dans le menu Démarrer
french.StartWithWindows=Lancer Vixeeny à l'ouverture de session
french.ContextMenu=Ajouter « Convertir avec Vixeeny » au menu contextuel des images
french.ShortcutsGroup=Raccourcis
french.IntegrationGroup=Intégration
french.KeepSettings=Conserver vos paramètres et les informations de mise à jour ?

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Vixeeny"; Filename: "{app}\vixeeny-daemon.exe"; Tasks: startmenu

[Registry]
; The daemon keeps this entry in line with the `autostart` setting afterwards.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Vixeeny"; ValueData: """{app}\vixeeny-daemon.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\vixeeny-app.exe"; Parameters: "--install-menu"; Tasks: contextmenu; Flags: runhidden
Filename: "{app}\vixeeny-daemon.exe"; Description: "Vixeeny"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\vixeeny-app.exe"; Parameters: "--uninstall-menu"; RunOnceId: "RemoveMenu"; Flags: runhidden skipifdoesntexist
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM vixeeny-daemon.exe /IM vixeeny-app.exe /IM vixeeny-updater.exe"; RunOnceId: "StopVixeeny"; Flags: runhidden

[UninstallDelete]
Type: filesandordirs; Name: "{app}\.update-staged"
Type: filesandordirs; Name: "{app}\.update-backup"

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Dir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    Dir := ExpandConstant('{userappdata}\Vixeeny');
    if DirExists(Dir) and (MsgBox(CustomMessage('KeepSettings'), mbConfirmation, MB_YESNO) = IDNO) then
      DelTree(Dir, True, True, True);
  end;
end;
