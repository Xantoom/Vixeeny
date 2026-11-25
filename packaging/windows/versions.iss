// SPDX-License-Identifier: GPL-3.0-or-later
// The order of Vixeeny's versions, for the installer's [Code] (included by vixeeny.iss and by
// versions-test.iss, which CI runs).

// The text before the first `Sep` of `S`, which keeps the rest (all of it, then '').
function CutAt(var S: String; Sep: String): String;
var
  P: Integer;
begin
  P := Pos(Sep, S);
  if P = 0 then
  begin
    Result := S;
    S := '';
  end
  else
  begin
    Result := Copy(S, 1, P - 1);
    S := Copy(S, P + 1, Length(S));
  end;
end;

function IsNumber(S: String): Boolean;
var
  I: Integer;
begin
  Result := S <> '';
  for I := 1 to Length(S) do
    if (S[I] < '0') or (S[I] > '9') then
      Result := False;
end;

function Sign(N: Int64): Integer;
begin
  if N < 0 then
    Result := -1
  else if N > 0 then
    Result := 1
  else
    Result := 0;
end;

// Two identifiers of a pre-release: numbers by value and before words, words as text.
function CompareIdentifiers(A, B: String): Integer;
begin
  if IsNumber(A) and IsNumber(B) then
    Result := Sign(StrToInt64Def(A, 0) - StrToInt64Def(B, 0))
  else if IsNumber(A) then
    Result := -1
  else if IsNumber(B) then
    Result := 1
  else
    Result := Sign(CompareStr(A, B));
end;

// -1, 0 or 1 as version A is older than, the same as or newer than B, the way semantic
// versioning orders them (as Vixeeny's updates do): 1.0.0-RC.2 < 1.0.0-RC.10 < 1.0.0 < 1.0.1.
function CompareVersions(A, B: String): Integer;
var
  CoreA, CoreB, PreA, PreB, Rest: String;
  I: Integer;
begin
  // The build metadata (`+…`) does not count. (`X := CutAt(X, …)` would give '': the result
  // and the cut variable are the same one in Pascal Script.)
  Rest := Trim(A);
  PreA := CutAt(Rest, '+');
  Rest := Trim(B);
  PreB := CutAt(Rest, '+');
  CoreA := CutAt(PreA, '-');
  CoreB := CutAt(PreB, '-');
  Result := 0;
  for I := 1 to 3 do
    if Result = 0 then
      Result := Sign(StrToInt64Def(CutAt(CoreA, '.'), 0) - StrToInt64Def(CutAt(CoreB, '.'), 0));
  if Result <> 0 then
    Exit;
  // A pre-release comes before its release.
  if (PreA = '') and (PreB = '') then
    Exit;
  if PreA = '' then
  begin
    Result := 1;
    Exit;
  end;
  if PreB = '' then
  begin
    Result := -1;
    Exit;
  end;
  while (Result = 0) and ((PreA <> '') or (PreB <> '')) do
  begin
    // The one with fewer identifiers, all equal so far, comes first.
    if PreA = '' then
      Result := -1
    else if PreB = '' then
      Result := 1
    else
      Result := CompareIdentifiers(CutAt(PreA, '.'), CutAt(PreB, '.'));
  end;
end;
