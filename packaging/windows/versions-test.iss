; The tests of versions.iss: a setup that installs nothing, checks the order of versions as it
; starts and writes the result to the file named by /RESULT= ("ok", or the cases that fail).
;   iscc packaging\windows\versions-test.iss && dist\versions-test.exe /VERYSILENT /RESULT=out.txt

[Setup]
AppName=Vixeeny versions test
AppVersion=0
CreateAppDir=no
Uninstallable=no
OutputDir=..\..\dist
OutputBaseFilename=versions-test
PrivilegesRequired=lowest

[Code]
#include "versions.iss"

var
  Failures: String;

procedure Expect(A, B: String; Order: Integer);
begin
  if CompareVersions(A, B) <> Order then
    Failures := Failures + A + ' vs ' + B + ': ' + IntToStr(CompareVersions(A, B)) + ' instead of ' + IntToStr(Order) + #13#10;
end;

function InitializeSetup(): Boolean;
begin
  Failures := '';
  Expect('1.0.0', '1.0.0', 0);
  Expect('1.0.0', '1.0.1', -1);
  Expect('1.2.0', '1.10.0', -1);
  Expect('2.0.0', '1.99.99', 1);
  Expect('1.0.0-RC.2', '1.0.0', -1);
  Expect('1.0.0', '1.0.0-RC.2', 1);
  Expect('1.0.0-RC.2', '1.0.0-RC.10', -1);
  Expect('1.0.0-RC.2', '1.0.0-RC.2', 0);
  Expect('1.0.0-RC.2', '1.0.0-RC.2.1', -1);
  Expect('1.0.0-RC.1', '0.20.3', 1);
  Expect('1.0.0-alpha', '1.0.0-beta', -1);
  Expect('1.0.0-1', '1.0.0-alpha', -1);
  Expect('1.0.0+build.5', '1.0.0', 0);
  Expect(' 1.0.0 ', '1.0.0', 0);
  if Failures = '' then
    Failures := 'ok';
  SaveStringToFile(ExpandConstant('{param:RESULT}'), Failures, False);
  Result := False;
end;
