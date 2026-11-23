; urakata installer (Inno Setup 6.3 or newer).
;
; Installs per user (no admin) into %LOCALAPPDATA%\Programs\urakata, so
; updates run without UAC. Only the SudoVDA driver step asks for admin: it
; runs `urakata driver ...` elevated.
;
; Build: iscc /DAppVersion=0.1.0 /DBinDir=..\target\release
;             /DDriverDir=<folder from fetch-driver.ps1> urakata.iss

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif
#ifndef DriverDir
  #error Pass /DDriverDir=<folder with the SudoVDA package>
#endif
#define DriverVersion "1.10.9.289"

[Setup]
AppId={{E42FB385-4C0B-4A76-818D-FB7D8C79A74F}
AppName=urakata
AppVersion={#AppVersion}
AppVerName=urakata {#AppVersion}
AppPublisherURL=https://github.com/Tokahiro/urakata
AppSupportURL=https://github.com/Tokahiro/urakata/issues
AppUpdatesURL=https://github.com/Tokahiro/urakata/releases
DefaultDirName={autopf}\urakata
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
LicenseFile=..\LICENSE
OutputDir=output
OutputBaseFilename=urakata-setup-{#AppVersion}
SetupIconFile=..\assets\logo\urakata.ico
UninstallDisplayIcon={app}\urakata.exe
UninstallDisplayName=urakata
WizardStyle=modern
; The watcher is stopped in [Code]; the Restart Manager is not needed.
CloseApplications=no
RestartApplications=no

[Files]
Source: "{#BinDir}\urakata.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\urakata-watch.exe"; DestDir: "{app}"; Flags: ignoreversion
; Settings window. Skipped until the build has it.
Source: "{#BinDir}\urakata-app.exe"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#DriverDir}\SudoVDA.inf"; DestDir: "{app}\driver"; Flags: ignoreversion
Source: "{#DriverDir}\sudovda.cat"; DestDir: "{app}\driver"; Flags: ignoreversion
Source: "{#DriverDir}\SudoVDA.dll"; DestDir: "{app}\driver"; Flags: ignoreversion
Source: "{#DriverDir}\sudovda.cer"; DestDir: "{app}\driver"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "THIRD-PARTY.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\urakata"; Filename: "{app}\urakata-app.exe"; Check: AppWindowInstalled

[Tasks]
Name: autostart; Description: "Start the urakata watcher at each logon (recommended)"

[Code]
const
  HealthOk = 0;
  HealthMissing = 10;
  HealthBroken = 11;

var
  DriverPage: TInputOptionWizardPage;
  DriverHealth: Integer;
  DriverCommands: array of String;

{ Runs `urakata.exe driver status` from the temp folder. Returns the exit
  code (0 ok, 10 not installed, 11 needs repair, other: error) and the
  output in Text. }
function ReadDriverStatus(var Text: String): Integer;
var
  Output: TExecOutput;
  Code, I: Integer;
begin
  ExtractTemporaryFile('urakata.exe');
  Text := '';
  if not ExecAndCaptureOutput(ExpandConstant('{tmp}\urakata.exe'), 'driver status', '',
      SW_HIDE, ewWaitUntilTerminated, Code, Output) then
  begin
    Result := -1;
    Text := SysErrorMessage(Code);
    Exit;
  end;
  for I := 0 to GetArrayLength(Output.StdOut) - 1 do
    Text := Text + Output.StdOut[I] + #13#10;
  for I := 0 to GetArrayLength(Output.StdErr) - 1 do
    Text := Text + Output.StdErr[I] + #13#10;
  Result := Code;
end;

procedure AddChoice(const Caption, Command: String);
var
  N: Integer;
begin
  DriverPage.Add(Caption);
  N := GetArrayLength(DriverCommands);
  SetArrayLength(DriverCommands, N + 1);
  DriverCommands[N] := Command;
end;

procedure InitializeWizard;
var
  Status: String;
begin
  DriverHealth := ReadDriverStatus(Status);
  DriverPage := CreateInputOptionPage(wpSelectTasks,
    'SudoVDA driver',
    'Steam creates its virtual display with the SudoVDA driver.',
    Status + #13#10 + 'Changing the driver needs admin rights once. ' +
    'Do not change it while a stream runs.',
    True, False);
  { The first choice is the default. Silent runs skip the driver step. }
  case DriverHealth of
    HealthOk:
      begin
        AddChoice('Keep the installed driver', '');
        AddChoice('Reinstall the bundled driver {#DriverVersion}', 'reinstall');
      end;
    HealthMissing:
      begin
        AddChoice('Install the bundled driver {#DriverVersion}', 'install');
        AddChoice('Do not install a driver now', '');
      end;
    HealthBroken:
      begin
        AddChoice('Repair the driver', 'repair');
        AddChoice('Reinstall the bundled driver {#DriverVersion}', 'reinstall');
        AddChoice('Leave the driver as it is', '');
      end;
  else
    AddChoice('Leave the driver as it is (its state could not be read)', '');
  end;
  DriverPage.SelectedValueIndex := 0;
end;

function AppWindowInstalled: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\urakata-app.exe'));
end;

function SelectedDriverCommand: String;
begin
  Result := DriverCommands[DriverPage.SelectedValueIndex];
end;

{ Stops this user's watcher and settings window, so their files can be
  replaced. }
procedure StopWatcher;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'),
    '/F /IM urakata-watch.exe /IM urakata-app.exe /FI "USERNAME eq ' +
    GetUserNameString + '"', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

procedure RunDriverCommand(const Command: String);
var
  Code: Integer;
  Log: String;
  Lines: TArrayOfString;
  Text: String;
  I: Integer;
begin
  Log := ExpandConstant('{app}\driver.log');
  DeleteFile(Log);
  WizardForm.StatusLabel.Caption := 'Setting up the SudoVDA driver (admin)...';
  { urakata.exe itself is elevated, so the UAC prompt names it. --log
    writes its output, because its console is not visible. }
  if not ShellExec('runas', ExpandConstant('{app}\urakata.exe'),
      'driver ' + Command + ' --log "' + Log + '"',
      '', SW_HIDE, ewWaitUntilTerminated, Code) then
  begin
    SuppressibleMsgBox('The driver was not changed (' + SysErrorMessage(Code) + '). ' +
      'Run "urakata driver ' + Command + '" from an admin prompt later.',
      mbError, MB_OK, IDOK);
    Exit;
  end;
  if Code <> 0 then
  begin
    Text := '';
    if LoadStringsFromFile(Log, Lines) then
      for I := 0 to GetArrayLength(Lines) - 1 do
        Text := Text + Lines[I] + #13#10;
    { A line must not start with #: ISPP reads it as a directive. }
    SuppressibleMsgBox('The driver step ended with code ' + IntToStr(Code) + ':' + #13#10 +
      '' + #13#10 + Text, mbError, MB_OK, IDOK);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep = ssInstall then
    StopWatcher
  else if CurStep = ssPostInstall then
  begin
    { A silent run is an update (`urakata update`): it never changes the
      driver and never shows a UAC prompt. }
    if (not WizardSilent) and (SelectedDriverCommand <> '') then
      RunDriverCommand(SelectedDriverCommand);
    if WizardIsTaskSelected('autostart') then
    begin
      Exec(ExpandConstant('{app}\urakata.exe'), 'watch --install', '', SW_HIDE,
        ewWaitUntilTerminated, Code);
      Exec(ExpandConstant('{app}\urakata-watch.exe'), '', '', SW_HIDE, ewNoWait, Code);
    end
    else
      Exec(ExpandConstant('{app}\urakata.exe'), 'watch --uninstall', '', SW_HIDE,
        ewWaitUntilTerminated, Code);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  Exec(ExpandConstant('{app}\urakata.exe'), 'watch --uninstall', '', SW_HIDE,
    ewWaitUntilTerminated, Code);
  StopWatcher;
  { Default No: Apollo or another tool may use the same driver. }
  if SuppressibleMsgBox('Remove the SudoVDA driver and its certificate too? ' +
      'Steam cannot create virtual displays without it.',
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
    ShellExec('runas', ExpandConstant('{app}\urakata.exe'), 'driver uninstall', '',
      SW_HIDE, ewWaitUntilTerminated, Code);
end;

[UninstallDelete]
Type: files; Name: "{app}\driver.log"
