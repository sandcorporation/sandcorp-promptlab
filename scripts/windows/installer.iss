; PromptLab(응시 앱) 설치 파일(Inno Setup 6.3+). AI 모델은 넣지도 받지도 않는다(#131) — 앱이 응시 전 점검에서 이 PC의
; 그래픽 메모리·RAM으로 크기(7B·1.5B)를 추천하고 진행 막대와 함께 받는다(추천에 쓰는 Vulkan 장치 메모리·PDH 사용량은 앱의 llama.cpp 백엔드만 잰다).
; 스모크(무인 설치)는 fetch-model.ps1 로 캐시에 먼저 둔다.
; 빌드: iscc /DAppVersion=0.1.0 installer.iss  (build-installer.ps1 이 값을 채운다)
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B0A7E2C4-5C1D-4E8A-9F3B-6D2A1C7E9F40}
AppName=PromptLab
AppVersion={#AppVersion}
AppPublisher=Sandcorp
DefaultDirName={localappdata}\Programs\promptlab
DefaultGroupName=PromptLab
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=promptlab-app-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\promptlab-app.exe
; 서명은 build-installer.ps1 이 sign.ps1 로 따로 한다(인증서 없으면 건너뜀).

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"

[Files]
Source: "..\..\dist\app\promptlab-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\app\promptlab-app-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
; DLL: VC++ 런타임(app-local, #111 — 늘)과 Vulkan 빌드(#98)의 llama·ggml DLL·백엔드 모듈(ggml-cpu-*·ggml-vulkan). build.ps1 이 dumpbin 으로 의존 닫힘을, build-installer.ps1 이 목록을 확인한다.
; Monaco 편집기 자산은 exe 에 구워져 있어(include_bytes!, #97) 따로 싣는 파일이 없다.
Source: "..\..\dist\app\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\PromptLab"; Filename: "{app}\promptlab-app.exe"
Name: "{autodesktop}\PromptLab"; Filename: "{app}\promptlab-app.exe"

; 서버 주소는 앱 바이너리에 구워져 있다(release 기본값 = 운영 주소, crates/app/src/client.rs). 레지스트리 env는 쓰지 않는다 —
; 쓰면 ChangesEnvironment 없이는 설치 직후 앱이 못 보고(#62), 쓰고 나면 제거해도 남는다. 개발·스모크는 실행 env PROMPTLAB_SERVER_URL로 덮는다.

[Run]
Filename: "{app}\promptlab-app.exe"; Description: "PromptLab 실행"; Flags: nowait postinstall skipifsilent

[Code]
var
  InfoPage: TOutputMsgWizardPage;

procedure InitializeWizard;
begin
  // 최소 사양 안내(spec #93) — 앱의 응시 전 점검과 같은 기준이다(RAM은 점검이 경고만, Docker는 시험에서 막는다).
  InfoPage := CreateOutputMsgPage(wpWelcome, '최소 사양', '설치 전에 확인하세요',
    '- 메모리(RAM) 16GB 이상 권장. 이보다 작으면 응시 전 점검이 경고합니다.' + #13#10#13#10 +
    '- AI 모델은 앱의 응시 전 점검에서 이 PC의 그래픽 메모리·메모리에 맞는 크기(약 4.4GB 또는 1.0GB)를 추천받아 내려받습니다. 네트워크 연결이 필요합니다.' + #13#10#13#10 +
    '- Docker Desktop(Linux 컨테이너 모드)이 필요합니다. 코드를 내 PC에서 실행해 보는 데 씁니다. 앱이 응시 전 점검에서 확인하고, 없으면 설치 안내를 보여 줍니다.' + #13#10#13#10 +
    '- Docker Desktop은 개인 사용은 무료이고, 일정 규모 이상 기업의 업무용 사용은 유료 요금제가 필요합니다.');
end;
