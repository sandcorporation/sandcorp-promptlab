# Windows 호스트 — 앱 빌드·설치 파일·끝에서 끝 스모크

spec #1 「인프라」·「Testing Decisions」4번, spec #93 「9. Windows 스모크」. 대표의 Windows 데스크톱(호스트)을 Jenkins 에이전트로 쓴다. 예전 Hyper-V VM은 없앴다(2026-11-07) — **노드·계정·도구·Docker Desktop 설치는 사람(대장·대표) 몫**이고, 이 문서는 그때 필요한 것을 모은 체크리스트다. 코드 쪽은 저장소에 있다.

| 무엇 | 어디 |
|---|---|
| 잡(노드 라벨 `windows-promptlab`) | `Jenkinsfile.windows` — 잡 이름 제안 `sandcorp-promptlab-windows`, Pipeline from SCM, Script Path `Jenkinsfile.windows`, 브랜치 `main` |
| 도구 확인·빌드·설치 파일·서명·모델 받기 | `scripts/windows/` (`check-tools`·`build`·`build-installer`·`sign`·`fetch-model` `.ps1`, `installer.iss`) |
| 끝에서 끝 스모크 | `e2e/smoke.ps1` |
| 사람이 하는 끝에서 끝(받아들임) | `docs/e2e-acceptance.md` |

PR 검사(`sandcorp-promptlab-pr`)와 main 배포(`sandcorp-promptlab`)는 이 잡과 무관하다 — Windows 노드가 없어도 둘은 돈다.

## 노드 구성 — 호스트 서비스 에이전트

- Jenkins 에이전트는 **제한 계정 `jenkins-agent`**로 **WinSW 서비스(세션 0)**로 돈다. 대화형 데스크톱이 없다. 그래서 스모크는 **화면 캡처를 하지 않고**(`CopyFromScreen` 없음) 앱 창 확인을 `MainWindowHandle`까지만 본다 — 세션 0에서 WebView2 창이 안 만들어지면 그 단계는 「세션 0이라 건너뜀」 경고가 된다(실패 아님). 앱 화면 자체는 사람이 호스트에서 설치 파일을 직접 실행해 본다(`docs/e2e-acceptance.md`).
- `jenkins-agent`는 `docker-users` 그룹이다(Docker Desktop 엔진 파이프 `\\.\pipe\docker_engine` 접근). 운영 `.env` 등 비밀 파일은 읽기를 막아 둔다.
- 노드: **라벨 `windows-promptlab`**, 실행자 1, 사용법 「이 라벨과 일치하는 잡만」. **VM이 아니라 호스트라 컨트롤러로 아웃바운드 WebSocket(JNLP) 연결**만 쓴다.
- 모델 캐시는 이 계정의 `%LOCALAPPDATA%\promptlab\models`(`fetch-model.ps1`이 둔다).

### 서비스 계정 함정(체크)

1. **rustup은 기본이 사용자별 설치**(`%USERPROFILE%\.rustup`·`.cargo`)다 — 관리자나 대표 계정으로 깔면 `jenkins-agent`의 PATH에 `cargo`가 없다. **`RUSTUP_HOME`·`CARGO_HOME`을 시스템 경로**(예 `C:\rust\rustup`, `C:\rust\cargo`)로 두고 설치하고, 시스템 PATH에 `C:\rust\cargo\bin`을 더한다. `Jenkinsfile.windows`가 잡 안에서 `CARGO_HOME`을 캐시(`C:\promptlab-cache\cargo`)로 덮어쓰지만 `cargo.exe`·`rustc.exe`(rustup 프록시)는 시스템 `bin`에서 찾고 툴체인은 `RUSTUP_HOME`에서 읽는다.
2. **`jenkins-agent`에 쓰기 권한**: `RUSTUP_HOME`(툴체인 판은 `rust-toolchain.toml`이 고정하고 rustup이 첫 빌드 때 받는다)·`CARGO_HOME`·`C:\promptlab-cache`·작업 디렉터리(`C:\jenkins`). 읽기 전용이면 첫 빌드가 `Access is denied`로 죽는다. `check-tools.ps1`이 이 폴더들에 쓰기를 시험한다.
3. **시스템 환경변수·PATH·그룹을 바꾼 뒤에는 에이전트 서비스(WinSW)를 다시 시작해야 한다** — 서비스 프로세스는 시작할 때의 환경·토큰을 평생 쓴다(`docker-users`에 넣은 직후 `Access is denied`가 계속 나는 흔한 원인). 서비스 재시작 뒤 `check-tools.ps1`로 확인한다.
4. 서비스 계정은 로그인하지 않으므로 **Docker Desktop은 호스트에서 이미 켜져 있어야** 엔진에 닿는다(엔진은 Docker Desktop을 실행한 사용자 세션에서 돈다).
5. **서비스(LocalSystem)로 GUI 앱을 띄우지 않는다** — WebView2가 `systemprofile\AppData\Local`에 데이터 폴더를 못 만들어 대표 데스크톱에 「데이터 디렉터리를 만들 수 없습니다」 대화상자가 남는다(#114). 스모크 3단계는 SYSTEM·세션 0이면 건너뛰고, 아니면 `WEBVIEW2_USER_DATA_FOLDER`를 임시 폴더로 주고 지운다. 앱도 시작 전에 쓰기를 시험해 앱 데이터 폴더 `webview`(없으면 `%TEMP%\promptlab-webview`)를 명시한다.

## 설치할 도구

`scripts\windows\check-tools.ps1`이 이 목록을 확인하고 빠진 것과 원인 힌트를 찍는다(잡의 첫 단계, 빠지면 실패 — Vulkan SDK만 경고).

| 도구 | 판 | 경로·환경 | 메모 |
|---|---|---|---|
| Visual Studio 2022 Build Tools | 17.x | `vswhere`가 찾는다 | 워크로드 「C++를 사용한 데스크톱 개발」(MSVC v143 x64, Windows 11 SDK — `signtool`·`dumpbin` 포함) |
| Rustup | 최신 | `RUSTUP_HOME=C:\rust\rustup`, `CARGO_HOME=C:\rust\cargo`(시스템), PATH `C:\rust\cargo\bin` | **기본 호스트 `x86_64-pc-windows-msvc`**(`build.ps1`이 `rustc -vV`로 확인, 아니면 실패). 툴체인 판은 `rust-toolchain.toml`. 위 「서비스 계정 함정」 |
| CMake | 3.28+ | 시스템 PATH | llama.cpp 빌드 |
| LLVM | 18+ | `C:\Program Files\LLVM`, 시스템 환경변수 `LIBCLANG_PATH=C:\Program Files\LLVM\bin` | bindgen(`llama-cpp-sys-2`)이 `libclang.dll`을 찾는다 |
| Git for Windows | 최신 | PATH | 체크아웃 |
| Inno Setup | 6.3+ | `ISCC.exe` 폴더를 시스템 PATH에 | 설치 파일 도구 |
| Docker Desktop | 최신 | `C:\Program Files\Docker\Docker\resources\bin`(PATH), Linux 컨테이너 모드, `docker-users` | 스모크의 러너 이미지 받기·로컬 실행·예제 테스트. `check-tools.ps1`이 `docker version`(서버 쪽까지)으로 확인하고, 안 되면 원인(파이프 권한·엔진 미기동·Windows 컨테이너 모드·docker 없음)을 사람 말로 찍는다 |
| Vulkan SDK(LunarG) | 최신 | 시스템 환경변수 `VULKAN_SDK`(설치가 건다) | **선택** — 없으면 `build.ps1`이 경고만 찍고 CPU 빌드(GPU 가속 빠짐). 아래 「GPU(Vulkan) 빌드」 |
| Ninja | 1.11+ | VS 2022 구성 요소 「C++ CMake tools for Windows」에 딸린 것(`…\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe`) 또는 `ninja.exe`를 시스템 PATH에 | **GPU 빌드만**(`VULKAN_SDK`가 있을 때 필수) — llama.cpp를 Ninja 생성기로 빌드한다(#111, 아래 「GPU(Vulkan) 빌드」). `check-tools.ps1`이 찾는다 |
| WebView2 런타임 | Evergreen | — | Win11엔 기본. 앱(Dioxus/wry)이 쓴다 |
| JDK | 17 | — | Jenkins 에이전트(`agent.jar`) 실행(WinSW가 감싼다) |

`GGML_NATIVE`는 끄고(저장소 설정) 빌드해서, 빌드 호스트와 다른 CPU의 응시자 PC에서도 도는 일반 x64 명령만 쓴다.

**빌드 호스트 사양**: 6코어 이상, RAM 16GB 이상(링크 단계 ~6GB + 모델 시험 시 ~9GB), 여유 디스크 100GB(VS Build Tools ~15GB·cargo 캐시·target ~30GB·모델 4.7GB·러너 이미지 ~1.2GB·Vulkan SDK ~1GB).

## Windows 빌드

**스크립트는 Windows PowerShell 5.1 기준이고, 모든 `.ps1`(·`installer.iss`)은 UTF-8 BOM으로 저장한다** — BOM이 없으면 5.1이 CP949로 읽어 한글 문자열의 따옴표가 깨진다(`.editorconfig`·`.gitattributes`로 고정, `scripts/ci/check.sh`의 `ps1-bom`이 검사). 각 스크립트는 시작에서 콘솔 인코딩을 UTF-8로, 외부 HTTPS 호출 앞에서 TLS 1.2를 켠다. 네이티브 명령은 `Invoke-Native`로 불러 5.1이 stderr 안내를 오류로 던지지 않게 하고 성공은 `$LASTEXITCODE`로만 본다.

`scripts\windows\build.ps1` → `cargo build --release --locked -p promptlab-app --bins` → `dist\app\promptlab-app.exe`(화면)·`promptlab-app-cli.exe`(화면 없는 같은 흐름). 릴리스 화면 바이너리는 콘솔 창 없이(`windows_subsystem = "windows"`) 뜬다. cargo·target 캐시는 `C:\promptlab-cache\{cargo,target}`(`Jenkinsfile.windows`가 `CARGO_HOME`·`CARGO_TARGET_DIR`을 건다). 모델 캐시 경로는 Windows에서 `%LOCALAPPDATA%\promptlab\models`(`promptlab_inference::model_dir()`, `PROMPTLAB_MODEL_DIR`로 덮어씀).

리눅스에서 검증한 범위: 바꾼 Rust의 fmt·clippy(리눅스 대상), PowerShell 문법(docker `mcr.microsoft.com/powershell`의 pwsh 파서 `Parser::ParseFile`), CLI(`--check-docker` 등 스모크가 부르는 인자). **`cargo check --target x86_64-pc-windows-msvc`는 리눅스에서 안 된다** — `llama-cpp-sys-2` 빌드 스크립트가 MSVC cmake 환경을 요구한다. Windows 빌드의 첫 확인은 호스트 노드에서 한다.

## 설치 파일

- 도구: **Inno Setup**(`installer.iss`). 근거: 한 스크립트로 단일 exe가 나오고, 사용자별 설치(`{localappdata}\Programs\promptlab`, 관리자 권한 불필요)·제거 프로그램·무인 설치(`/VERYSILENT`, 스모크가 씀)·**내장 내려받기 페이지의 sha256 검증**이 있어서 모델 받기를 같은 설치 흐름에 넣을 수 있다.
- **AI 모델은 설치 파일에 넣지도, 설치 중에 받지도 않는다(결정, #131).** 모델 크기(7B 4.7GB·1.5B 1.1GB)는 앱이 응시 전 점검에서 이 PC의 그래픽 메모리(llama.cpp Vulkan 장치 메모리 + PDH 사용량)와 여유 RAM으로 추천하고 진행 막대·이어 받기·3번 재시도·sha256 검증과 함께 받는다(`crates/app/src/model_pick.rs`). 그 값은 앱의 llama.cpp 백엔드만 잴 수 있어 설치기(Inno Pascal)에서 같은 추천을 낼 수 없고, 응시자가 크기를 바꿀 수도 있어 받기를 한 곳(앱)에 모았다. 모델은 리비전 커밋·sha256으로 고정돼 있다(`inference.rs`의 `MODELS`). 앱은 모델을 열 때 sha256을 다시 확인한다(`verify_model_file`). 스모크·CI는 `fetch-model.ps1 -All`(curl 이어받기·재시도·sha256 검증)로 후보를 모두 캐시에 둔다. 오프라인 환경이 필요해지면 파일을 `Source:`로 싣는 변형을 후속으로.
- **설치 마법사 안내**: 환영 다음에 「최소 사양」 페이지 — RAM 16GB 이상 권장(앱의 응시 전 점검 `MIN_RAM_BYTES`와 같은 기준, 미만이면 점검이 경고), Docker Desktop(Linux 컨테이너 모드) 필요와 라이선스 문장(「Docker Desktop은 개인 사용은 무료이고, 일정 규모 이상 기업의 업무용 사용은 유료 요금제가 필요합니다.」), 설치 중 모델 받기. 러너 이미지(~350MB 압축 ×4)는 설치가 아니라 앱의 응시 전 점검이 받는다.
- **설치 파일에 들어가는 것**(`build-installer.ps1`이 만들기 전에 확인한다): `promptlab-app.exe`·`promptlab-app-cli.exe`, Vulkan 빌드면 `llama`·`ggml` DLL과 백엔드 모듈 `ggml-vulkan.dll`·`ggml-cpu-*.dll`(둘 중 하나라도 빠지면 실패 — `installer.iss`가 `dist\app\*.dll`을 싣는다. CPU 빌드면 DLL이 없고 경고), **Monaco 편집기 자산은 exe에 구워져 있어**(`include_bytes!`, #97) 따로 싣는 파일이 없다(exe 바이트에서 `vs/editor/editor.main.js` 이름을 찾아 확인, 없으면 경고).
- 서버 주소는 release 앱에 구워져 있다(기본 `https://promptlab.sandcorp.co.kr/api`, 빌드 env `PROMPTLAB_DEFAULT_SERVER_URL`로 바꾼다, 실행 env `PROMPTLAB_SERVER_URL`이 이긴다). 설치 파일은 환경변수를 쓰지 않는다(#62). 로그인은 앱이 첫 API 호출에서 브라우저로 sandcorp-auth를 연다(루프백 PKCE).
- 산출물: `dist\promptlab-app-setup-<버전>.exe`(버전은 워크스페이스 `Cargo.toml`). 잡이 `archiveArtifacts`로 남긴다.

## 코드 서명

`scripts\windows\sign.ps1`. **인증서가 없으면 「코드 서명 건너뜀」을 로그에 크게 찍고 0으로 끝난다**(시범 배포는 서명 없이 — SmartScreen 경고가 뜬다). 있으면 `signtool`(SHA256, RFC3161 타임스탬프)로 앱 exe·CLI·설치 파일을 서명하고 `verify /pa`까지 한다. Jenkins 자격 이름 자리: 파일 `promptlab-codesign-pfx`, 문자열 `promptlab-codesign-pfx-password`(없으면 잡이 서명을 건너뛴다). 인증서 구매는 대표 결정. EV/클라우드 HSM 방식이면 `sign.ps1`만 바꾼다.

## 끝에서 끝 스모크

`e2e\smoke.ps1`(스모크 계정·토큰은 아래). 단계:

| # | 단계 | 실패하면 |
|---|---|---|
| 1 | 설치 파일 무인 설치(`/VERYSILENT`) | 설치 로그 경로를 찍고 실패 |
| 2 | **Docker 확인** — `docker version`(서버 쪽까지) + 앱 CLI `--check-docker`(러너 이미지 받기·빈 틀 실행). 토큰 받기 **앞**이라 이미지 받기가 토큰 수명을 먹지 않는다 | 원인(파이프 권한·엔진 미기동·docker-users·Windows 컨테이너 모드)을 사람 말로 찍고 실패 |
| 3 | 앱(화면) 기동 — 창 핸들 확인(`MainWindowHandle`) | 세션 0이면 「세션 0이라 건너뜀」 경고, 대화형 세션이면 실패 |
| 3b | 서버 주소 기본값이 운영 주소이고 `/auth/app-config`에 닿는다(#62) | 실패 |
| 4 | 가상 카메라(`-Camera`일 때 장치 존재만) | — |
| 5 | 기계 토큰 → 스모크 배정 → CLI로 **스모크 문항의 모범 답안**(스모크 전용 공개 문항 — 저장소 `crates/store/fixtures/smoke/<SMOKE_PROBLEM_ID>.json`의 `reference_solution`, 히든은 꺼내지 않는다, #138)을 `--no-docker` 없이 응시: 로컬 Docker 예제 테스트(`예제 N/N 통과` 확인) → 제출 → **서버 히든 채점 통과 수가 전부 통과**인지 확인 | 예제·히든을 다 못 통과하면 실패 |
| 6 | 제거(`unins000.exe`) | — |

종료 0 = 채점까지 끝. #88의 화면 캡처(`CopyFromScreen`·`dist/screens/*.png` 아카이브)는 세션 0에서 검은 그림이라 지웠다.

- **스모크 계정 = sandcorp-auth 기계 클라이언트**(auth #76, 문서 12절, #28). 운영 클라이언트 「promptlab 스모크」(리소스 `https://promptlab.sandcorp.co.kr/api`, 스코프 `promptlab`). Jenkins 문자열 자격 `promptlab-smoke-client-id`·`promptlab-smoke-client-secret`을 `Jenkinsfile.windows`가 `PROMPTLAB_SMOKE_CLIENT_ID`·`_SECRET`으로 넘긴다.
- **토큰 받기**: `scripts\windows\smoke-token.ps1`의 `Set-SmokeToken`이 `POST https://auth.sandcorp.co.kr/token`(`grant_type=client_credentials`, `client_secret_post`, `resource`·`scope=promptlab`)으로 받아 그 프로세스의 `PROMPTLAB_ACCESS_TOKEN`에만 넣는다. 토큰·시크릿은 찍지 않는다(실패해도 HTTP 상태와 `error` 코드만).
- **수명 600초, refresh 없음** → 설치·Docker 준비·창 확인이 끝난 뒤 **응시 단계 직전에** 받는다. 응시(문항 시작이 모델 파일 sha256을 읽는 수십 초 포함)·예제 테스트·제출·채점 대기가 그 안에 끝나야 한다. 스모크가 「응시·채점 N초 (토큰 수명 600초)」를 찍으니 500초를 넘기 시작하면 제출 뒤 결과 조회 전에 토큰을 다시 받는 단계로 나눈다.
- **서버 쪽**: 그 `client_id`가 서버 env `PROMPTLAB_SMOKE_CLIENT_IDS`에 있어야 한다(없으면 401). 기계 주체는 「스모크」 조직의 응시자로만 행동하고, 배정 목록을 부르면 서버가 스모크 조직·시험·새 배정을 멱등으로 준비한다. 스모크 조직은 대시보드·결과 집계에서 빠진다. 자세히는 `docs/deploy.md` 「스모크 조직」.
- 사람 토큰으로 돌리려면(개발): `PROMPTLAB_SMOKE_CLIENT_ID`를 비우고 `PROMPTLAB_ACCESS_TOKEN`을 직접 넣는다.
- 모델은 스모크 전에 `fetch-model.ps1 -All`로 후보를 모두 캐시에 둔다(설치 파일은 모델을 받지 않는다 — CLI는 추천(또는 설정의 고른 크기) 모델로 문항 시작을 신고하므로 어느 쪽이든 있어야 한다).
- **스모크의 HTTP 호출**(`common.ps1`의 `Invoke-Http`): 5.1 `Invoke-RestMethod`는 전송 실패의 원인을 숨기고·keep-alive로 죽은 연결을 재사용할 수 있고·charset 없는 응답을 ISO-8859-1로 디코드해 한글이 깨진다. 그래서 keep-alive를 끄고 바이트를 UTF-8로 직접 디코드하며, 전송 실패·5xx는 3회 재시도하고 예외 종류·`WebExceptionStatus`·InnerException을 찍는다. 그래도 안 되면 `curl.exe`로 같은 요청(시크릿은 `--config -`로 표준입력에).

## 가상 카메라

호스트 서비스(세션 0)에는 카메라 장치가 없다. 스모크는 기계 주체라 웹캠이 면제되므로(`promptlab-app-cli`) `-Camera`일 때 **장치가 보이는지만** 확인하고(OBS Virtual Camera), 기본은 건너뛴다. 웹캠 흐름(동의·분석)은 사람이 하는 끝에서 끝(`docs/e2e-acceptance.md`)에서 본다.

## Jenkins 노드·잡

- 잡: `sandcorp-promptlab-windows`(위 표). 트리거는 **하루 한 번 KST 04시대**(`Jenkinsfile.windows`의 `cron('TZ=Asia/Seoul\nH 4 * * *')`, #35) + 수동(`SMOKE`·`CAMERA` 매개변수). main 병합마다는 돌리지 않는다. 실패·불안정이면 `post`에서 디스코드로 알린다(#51 — 자격 `discord-deploy-webhook`).
- 자격: `promptlab-smoke-client-id`·`-secret`(문자열, 필수), `promptlab-codesign-pfx`·`-password`(인증서가 생기면), 서버 `.env`에 `PROMPTLAB_SMOKE_CLIENT_IDS`.

## 네트워크 (호스트 → 밖, 아웃바운드만)

| 곳 | 왜 |
|---|---|
| Jenkins 컨트롤러(주소·포트는 대장이 안다) | 에이전트 연결 |
| `github.com` | 체크아웃 |
| `static.rust-lang.org`·`crates.io`·`index.crates.io`·`static.crates.io` | rustup·cargo |
| `huggingface.co`(+`cdn-lfs*.hf.co`) | 모델 받기(앱 응시 전 점검·`fetch-model.ps1`) |
| `promptlab.sandcorp.co.kr`, `auth.sandcorp.co.kr` | 스모크(채점 API·토큰 검증) |
| `ghcr.io` | 스모크의 러너 이미지 받기(앱 CLI 점검이 고정 digest로 받는다) |
| `timestamp.digicert.com` | 서명 타임스탬프(인증서가 생긴 뒤) |

## 체크리스트(대장·대표)

1. [ ] 호스트에 VS 2022 Build Tools(C++·SDK), Rustup(시스템 `RUSTUP_HOME`·`CARGO_HOME`), CMake, LLVM(`LIBCLANG_PATH`), Git, Inno Setup, JDK 17, Docker Desktop(Linux 컨테이너 모드), (선택) Vulkan SDK 설치
2. [ ] `jenkins-agent`: `docker-users` 그룹, `RUSTUP_HOME`·`CARGO_HOME`·`C:\promptlab-cache`·`C:\jenkins` 쓰기 권한
3. [ ] 시스템 환경변수(`RUSTUP_HOME`·`CARGO_HOME`·`LIBCLANG_PATH`·PATH·`VULKAN_SDK`)를 바꿨으면 **에이전트 서비스(WinSW) 재시작**
4. [ ] Docker Desktop이 호스트에서 켜져 있고 엔진이 시작됨
5. [ ] 저장소를 체크아웃해 `powershell scripts\windows\check-tools.ps1`이 모두 `ok`(Vulkan만 경고 가능)
6. [ ] Jenkins 노드(라벨 `windows-promptlab`)가 서비스로 온라인, 자격 등록
7. [ ] 잡 `sandcorp-promptlab-windows` 수동 실행 → 설치 파일 산출물·스모크 통과(첫 실행은 llama.cpp 컴파일·모델·러너 이미지 받기로 오래 걸린다)

## GPU(Vulkan) 빌드(#98)

- 앱은 llama.cpp Vulkan 백엔드를 **동적 모듈**(`GGML_BACKEND_DL`)로 싣는다: `promptlab-app.exe`는 `vulkan-1.dll`을 직접 가져오지 않고, exe 옆의 `ggml-vulkan.dll`만 Vulkan 런타임을 쓴다. Vulkan 런타임(그래픽 드라이버가 깐다)이 없는 PC에서는 그 모듈만 못 읽고 `ggml-cpu-*.dll`로 CPU에서 돈다. `build.ps1`이 `dumpbin /dependents`로 exe에 `vulkan-1.dll`이 없음을 확인한다.
- 빌드에는 **LunarG Vulkan SDK**(`glslc` — 셰이더 컴파일)가 필요하다: 설치 후 시스템 환경변수 `VULKAN_SDK`(설치 프로그램이 건다)를 에이전트가 보게 **에이전트 서비스를 다시 시작**한다. SDK가 없으면 `build.ps1`이 경고만 찍고 CPU 빌드를 한다(Windows 빌드가 깨지지 않는다). `check-tools.ps1`은 경고로 표시한다.
- **생성기는 Ninja다(#111).** Visual Studio 생성기로는 llama.cpp의 `vulkan-shaders-gen`(ExternalProject) 하위 빌드가 깨졌다(`not a CMake build directory (missing CMakeCache.txt)`·`Not a file: …cmake_install.cmake`, windows 잡 #26). 원인: llama.cpp가 `cmake_minimum_required(VERSION 3.14...3.28)`라 정책 CMP0147(CMake 3.27+, NEW)로 VS 사용자 명령에 `BuildInParallel`이 붙고, `llama-cpp-sys-2` build.rs가 MSVC+Vulkan에서 `TrackFileAccess=false`를 거는 상태에서 ExternalProject의 configure·build·install 단계가 겹쳐 돈다(상위 llama.cpp 릴리스 CI의 x64 Vulkan은 VS 생성기지만 `TrackFileAccess`를 끄지 않는다). `llama-cpp-sys-2` 0.1.161도 build.rs·`ggml-vulkan/CMakeLists.txt`가 같아 판 올림으로는 안 고쳐진다. 그래서 `build.ps1`이 GPU 빌드일 때만 ① VS x64 개발자 환경(`vcvars64.bat`)을 들여오고 ② `CMAKE_GENERATOR=Ninja`(cmake 크레이트가 읽는다)·`CC`/`CXX=cl.exe`(안쪽 cmake가 PATH의 gcc·clang을 잡지 않게)로 빌드한다. CPU 빌드는 그대로 VS 생성기다. CMake는 이미 있는 빌드 트리에서 `CMAKE_GENERATOR`를 무시하므로(windows 잡 #27), `build.ps1`은 캐시 target에 VS 생성기로 만든 Vulkan 빌드 트리(`llama-cpp-sys-2-*\out\build\CMakeCache.txt`의 `GGML_VULKAN=ON`·생성기 ≠ Ninja)가 있으면 그 경로를 찍고 `cargo clean --release -p llama-cpp-sys-2`로 지운 뒤 빌드한다.
- **DLL 모으기·의존 검사(#111, windows 잡 #28)**: GPU 빌드의 `llama.dll`·`ggml.dll`·`ggml-base.dll`은 이번 Vulkan 빌드 폴더(`llama-cpp-sys-2-*\out\bin`)에서, 백엔드 모듈은 `out\backends`에서 `dist\app`으로 모은다 — `target\release`의 하드링크는 build.rs가 「이미 있으면 건너뛴다」라 낡았거나 없을 수 있다(잡 #28에서 설치본 exe가 `llama.dll`을 못 찾아 0xC0000135). 끝에 `dumpbin /dependents`로 `dist\app`의 exe·DLL이 가져오는 DLL이 모두 `dist\app` 안이거나 OS DLL(System32·`api-ms-win-*`)인지 검사한다 — VC++ 런타임은 빌드 호스트 System32에 있어도 `dist`에 있어야 하고, `vulkan-1.dll`은 `ggml-vulkan.dll`만 가져와도 된다.
- **VC++ 런타임은 앱 폴더에 함께 둔다(app-local, #111 결정).** Rust exe(/MD)·llama.cpp DLL이 `VCRUNTIME140`·`MSVCP140` 등을, ggml-cpu가 OpenMP `VCOMP140`을 가져온다. `build.ps1`이 VS의 `VC\Redist\MSVC\<판>\x64\Microsoft.VC*.CRT`·`.OpenMP`의 DLL(재배포 허용 파일)을 `dist\app`에 복사한다(CPU 빌드도). `vc_redist.exe`를 설치 중에 돌리지 않는 까닭: 설치 파일은 사용자별(관리자 권한 없음)인데 vc_redist는 관리자 권한을 요구해 UAC 창이 뜨고 무인 설치(스모크)도 막힌다.
- **`vulkan-1.dll`은 설치 파일에 넣지 않는다(#111 결정).** Vulkan 로더는 GPU 드라이버(NVIDIA·AMD·Intel)가 `System32`에 깐다(노드 `C:\Windows\System32\vulkan-1.dll` 확인) — 드라이버가 깐 로더가 그 드라이버(ICD)와 맞는 판이고, 앱 폴더에 옛 로더를 두면 DLL 탐색 순서상 그것이 먼저 잡혀 시스템 로더 갱신을 가린다. 로더가 없는 PC는 Vulkan 드라이버도 없는 PC라 넣어도 GPU로 돌지 못한다(그때는 `ggml-cpu-*`로 돈다). LunarG SDK의 로더는 재배포할 수 있지만(Apache-2.0) 위 까닭으로 싣지 않는다.
- **GPU로 도는지 확인**: `promptlab-app-cli --check-gpu`가 빌드 백엔드(`vulkan(동적 모듈)`|`cpu`)·llama.cpp가 본 장치(백엔드·이름·설명·GPU 여부·메모리)·추론 장치(`[GPU] 추론 장치: GPU <이름>` 또는 `CPU(…)`)를 찍는다(모델·서버 없이). 스모크 2b 단계가 설치본으로 이것을 돌린다: 설치 폴더에 `ggml-vulkan.dll`이 있는데 CPU 빌드라고 하면 실패, GPU 장치가 안 보이면(세션 0 서비스 계정에서 Vulkan 장치가 안 보일 수 있다) 경고만. 사람이 호스트 데스크톱에서 같은 명령으로 RTX를 확인할 수 있다.
- 스모크는 GPU 없이도 돈다(응시 CLI는 모델을 올리지 않는다). 설치 파일에 `ggml-vulkan`·`ggml-cpu-*`가 들어가는지는 `build-installer.ps1`이 확인한다.
