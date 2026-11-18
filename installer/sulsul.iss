; 술술 Windows 설치 파일 (Inno Setup 6)
;
; 직접 컴파일하지 말고 installer\build.ps1을 쓰세요. 릴리스 빌드와 넣을 파일 모으기를 한 뒤
; 버전(AppVersion)과 모은 폴더(StageDir)를 넘겨 이 스크립트를 컴파일합니다.
;
; - 사용자별 설치: 관리자 권한 없이 %LOCALAPPDATA%\Programs\sulsul에 설치합니다.
; - 설치·업데이트·지우기 전에 설치한 폴더에서 실행 중인 술술을 끝냅니다.
; - 지울 때 자동 실행 등록을 지우고, 설정·기록·모델도 지울지 묻습니다(기본은 남김).

#ifndef AppVersion
  #error "AppVersion을 넘겨 주세요 (예: /DAppVersion=0.1.0)."
#endif
#ifndef StageDir
  #error "StageDir을 넘겨 주세요 (installer\build.ps1이 모은 폴더)."
#endif

#define AppName "술술"
#define AppExe "sulsul.exe"
#define AppUrl "https://github.com/YubinMoon/sulsul"

[Setup]
; 업데이트할 때 같은 설치로 알아보는 값이라 바꾸지 않습니다.
AppId={{BAEE309B-F693-47B4-9A78-4D2BBCA80177}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=YubinMoon
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} 설치 프로그램

PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

DefaultDirName={autopf}\sulsul
DisableProgramGroupPage=yes
ShowLanguageDialog=auto
WizardStyle=modern dynamic
SetupIconFile=..\crates\sulsul-app\assets\icon\sulsul.ico
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
; 실행 중인 술술은 [Code]에서 직접 끝냅니다 (트레이 앱이라 재시작 관리자로는 잘 닫히지 않음).
CloseApplications=no

OutputBaseFilename=sulsul-{#AppVersion}-windows-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
korean.DeleteUserData=설정, 받아쓰기 기록, 내려받은 모델도 함께 지울까요?%n%n다시 설치해서 그대로 쓰려면 [아니요]를 누르세요.
english.DeleteUserData=Also delete your settings, dictation history, and downloaded models?%n%nChoose No to keep them for a future reinstall.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\sulsul-llm-runner.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

{ 설치 폴더 경로(소문자, 끝에 \) }
function AppDirPrefix(): String;
begin
  Result := Lowercase(AddBackslash(ExpandConstant('{app}')));
end;

{ 설치한 폴더에서 실행 중인 술술과 글 다듬기 실행기를 끝냅니다.
  다른 폴더에서 실행 중인 술술(개발 빌드 등)은 건드리지 않습니다. }
procedure StopRunningApp();
var
  Dir, Params: String;
  ResultCode: Integer;
begin
  Dir := AppDirPrefix();
  StringChangeEx(Dir, '''', '''''', True);
  Params := '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' +
    'Get-Process -Name sulsul,sulsul-llm-runner -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -and $_.Path.ToLower().StartsWith(''' + Dir + ''') } | ' +
    'Stop-Process -Force"';
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'), Params, '',
    SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

{ 앱이 HKCU\...\Run에 넣은 자동 실행 값 가운데 이 설치 폴더의 술술을 가리키는 것만 지웁니다.
  값 이름은 sulsul, 설정 폴더를 따로 정했으면 sulsul-<해시>입니다(crates/sulsul-app/src/autostart.rs). }
procedure RemoveAutostart();
var
  Names: TArrayOfString;
  Value: String;
  I: Integer;
begin
  if not RegGetValueNames(HKCU, RunKey, Names) then
    Exit;
  for I := 0 to GetArrayLength(Names) - 1 do
    if (Pos('sulsul', Lowercase(Names[I])) = 1) and
       RegQueryStringValue(HKCU, RunKey, Names[I], Value) and
       (Pos(AppDirPrefix(), Lowercase(Value)) > 0) then
      RegDeleteValue(HKCU, RunKey, Names[I]);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopRunningApp();
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  case CurUninstallStep of
    usUninstall:
      begin
        StopRunningApp();
        RemoveAutostart();
      end;
    usPostUninstall:
      { 설정은 %APPDATA%\sulsul, 기록·모델·로그는 %LOCALAPPDATA%\sulsul에 있습니다(sulsul-core paths.rs).
        설치 폴더(%LOCALAPPDATA%\Programs\sulsul)와는 다른 폴더입니다. }
      if (not UninstallSilent) and
         (MsgBox(CustomMessage('DeleteUserData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
      begin
        DelTree(ExpandConstant('{userappdata}\sulsul'), True, True, True);
        DelTree(ExpandConstant('{localappdata}\sulsul'), True, True, True);
      end;
  end;
end;
