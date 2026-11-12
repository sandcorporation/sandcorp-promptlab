# 끝에서 끝 스모크(Windows 호스트 서비스 에이전트): 무인 설치 → Docker 확인 → 추론 장치(GPU) 확인 → 앱 창 확인 → 응시(모범 답안·로컬 Docker 예제 테스트)·제출·서버 히든 채점(CLI) → 제거.
#   .\e2e\smoke.ps1 -Setup dist\promptlab-app-setup-0.1.0.exe
# 환경변수: PROMPTLAB_SMOKE_CLIENT_ID·PROMPTLAB_SMOKE_CLIENT_SECRET(스모크 기계 클라이언트 — 응시 단계 직전에
#            client_credentials 로 토큰을 받는다, #28) 또는 PROMPTLAB_ACCESS_TOKEN(이미 받아 둔 토큰). 둘 다 로그에 찍지 않는다.
#          PROMPTLAB_SERVER_URL(기본 운영 https://promptlab.sandcorp.co.kr/api)
# 기계 토큰은 「스모크」 조직의 응시자다: 배정 목록을 부르면 서버가 새 스모크 배정을 준비하고, 그 문항을 응시·제출한다.
# 토큰 수명은 600초(refresh 없음)라 설치·Docker 준비·창 확인을 마친 뒤 응시 직전에 받는다 — 응시·채점 대기가 그 안에 끝나야 한다.
# 종료 0 = 채점까지 끝남. 설치 파일은 모델을 받지 않는다(#131) — 응시 신고에 쓸 모델은 호스트 캐시(%LOCALAPPDATA%\promptlab\models)에 미리 둔다(아래 수동 스모크 절차).
# 에이전트는 세션 0(대화형 데스크톱 없음)에서 돈다 — 화면 캡처는 하지 않고 창 확인은 MainWindowHandle 까지만 본다(#101).
param(
  [Parameter(Mandatory)][string]$Setup,
  [switch]$Camera,            # 가상 카메라 단계(앱에 웹캠이 생긴 뒤 — #13). 지금은 장치 존재만 확인한다.
  [int]$GuiSeconds = 15
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\scripts\windows\common.ps1')
# Windows PowerShell 5.1: 콘솔·파이프 인코딩을 UTF-8로 (기본은 OEM/CP949라 한글이 깨진다)
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch {}
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
$useMachine = [bool]($env:PROMPTLAB_SMOKE_CLIENT_ID -and $env:PROMPTLAB_SMOKE_CLIENT_SECRET)
if (-not $useMachine -and -not $env:PROMPTLAB_ACCESS_TOKEN) { throw 'PROMPTLAB_SMOKE_CLIENT_ID·SECRET 또는 PROMPTLAB_ACCESS_TOKEN 이 없다 (docs/windows-host.md 「스모크 계정」)' }
. (Join-Path $PSScriptRoot '..\scripts\windows\smoke-token.ps1')
if (-not $env:PROMPTLAB_SERVER_URL) { $env:PROMPTLAB_SERVER_URL = 'https://promptlab.sandcorp.co.kr/api' }
$installDir = Join-Path $env:TEMP "promptlab-smoke-$PID"
function Step($n) { Write-Host "`n=== $n ===" }

Step '1. 무인 설치'
$p = Start-Process -FilePath $Setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=$installDir", "/LOG=$env:TEMP\promptlab-setup.log" -Wait -PassThru
# ExitCode 가 null 로 남는 서비스 세션 함정이 있어 null 은 실패로 보지 않는다(아래에서 설치 결과 exe 존재를 따로 확인한다).
if ($null -ne $p.ExitCode -and $p.ExitCode -ne 0) { throw "설치 실패 (종료 $($p.ExitCode)) — $env:TEMP\promptlab-setup.log" }
foreach ($f in 'promptlab-app.exe', 'promptlab-app-cli.exe') { if (-not (Test-Path (Join-Path $installDir $f))) { throw "설치 결과에 $f 없음" } }

try {
  Step '2. Docker 확인 — 서비스 계정에서 호스트 Docker Desktop 엔진에 닿고, 러너 이미지로 빈 틀이 돌아야 한다'
  $dk = Test-DockerEngine
  if (-not $dk.Ok) { throw "Docker 를 쓸 수 없다: $($dk.Detail)" }
  Write-Host "docker $($dk.Detail)"
  $cli = Join-Path $installDir 'promptlab-app-cli.exe'
  $sol = Get-SmokeSolution
  # 앱 CLI 의 Docker 점검(이미지 받기 — 고정 digest — 와 빈 틀 실행)만 먼저 돈다. 받기가 오래 걸려도 토큰 수명(600초)을 먹지 않게 토큰 받기 앞에 둔다.
  $dockerClock = [Diagnostics.Stopwatch]::StartNew()
  $savedTok = $env:PROMPTLAB_ACCESS_TOKEN
  Remove-Item Env:PROMPTLAB_ACCESS_TOKEN -ErrorAction SilentlyContinue
  try { Invoke-Native { & $cli --check-docker --language $sol.Language } | ForEach-Object { Write-Host $_ } } finally { $env:PROMPTLAB_ACCESS_TOKEN = $savedTok }
  if ($LASTEXITCODE -ne 0) { throw "앱의 Docker 점검 실패 (종료 $LASTEXITCODE) — 위 [Docker] 줄이 원인" }
  Write-Host ("Docker 준비 {0:N0}초" -f $dockerClock.Elapsed.TotalSeconds)

  Step '2b. 추론 장치 확인 — 설치본이 GPU(Vulkan) 백엔드를 싣고, 이 세션에서 어느 장치로 추론하는지 찍는다(#111)'
  # 설치 폴더에 ggml-vulkan.dll 이 있으면 GPU 빌드여야 한다(아니면 결함). GPU 장치가 안 보이는 것은 실패가 아니라 경고다 —
  # 세션 0(서비스)에서는 Vulkan 로더가 장치를 못 볼 수 있고, 그때 앱은 CPU 로 돈다.
  $vkModule = Test-Path (Join-Path $installDir 'ggml-vulkan.dll')
  $gpuLines = @(Invoke-Native { & $cli --check-gpu } | ForEach-Object { Write-Host $_; "$_" })
  if ($LASTEXITCODE -ne 0) { throw "앱의 추론 장치 확인 실패 (종료 $LASTEXITCODE) — 백엔드를 못 올렸다" }
  $gpuBuild = [bool]($gpuLines | Where-Object { $_ -match '빌드 백엔드: vulkan' })
  if ($vkModule -and -not $gpuBuild) { throw '설치 폴더에 ggml-vulkan.dll 이 있는데 CLI 가 CPU 빌드라고 한다 — 빌드·설치가 섞였다' }
  if (-not $vkModule) { Write-Warning '설치 폴더에 ggml-vulkan.dll 없음 — CPU 빌드(GPU 가속 없음)' }
  $dev = $gpuLines | Where-Object { $_ -match '추론 장치: (.+)$' } | Select-Object -Last 1
  if ($dev -match '추론 장치: GPU (.+)$') { Write-Host "GPU 추론 확인: $($Matches[1]) (세션 $([Diagnostics.Process]::GetCurrentProcess().SessionId))" }
  else { Write-Warning "GPU 장치가 보이지 않는다 — 이 세션(세션 $([Diagnostics.Process]::GetCurrentProcess().SessionId))에서는 CPU 로 추론한다. 서비스 세션 0이면 Vulkan 장치가 안 보일 수 있다(실패 아님, 사람이 호스트에서 CLI --check-gpu 로 확인)" }

  Step '2c. AI 모델 크기 추천(#131) — 이 세션에서 고를 모델을 찍는다(응시 신고에 쓰는 모델과 같다, 호출자가 후보를 모두 캐시에 둔다)'
  $recLines = @(Invoke-Native { & $cli --recommend-model } | ForEach-Object { Write-Host $_; "$_" })
  if ($LASTEXITCODE -ne 0) { throw "앱의 모델 추천 실패 (종료 $LASTEXITCODE)" }
  if (-not ($recLines | Where-Object { $_ -match '\[모델\] 쓸 모델: ' })) { throw '모델 추천 출력에 「쓸 모델」 줄이 없다' }

  Step '3. 앱(화면) 기동 — 창이 뜨고 N초 동안 살아 있어야 한다(세션 0이면 창 확인은 건너뜀)'
  # 에이전트가 세션 0(서비스)이면 대화형 데스크톱이 없어 WebView2 창이 안 만들어질 수 있다 — 그때는 실패가 아니라 경고다.
  # 대화형 세션이면 창이 없다는 것은 결함이다(WebView2 런타임·DLL).
  $session0 = ([Diagnostics.Process]::GetCurrentProcess().SessionId -eq 0)
  $isSystem = [Security.Principal.WindowsIdentity]::GetCurrent().IsSystem
  # 서비스(LocalSystem)로 띄우면 WebView2 가 systemprofile 에 데이터 폴더를 못 만들어 대표 데스크톱에 오류 대화상자가
  # 뜬다(#114). 그래서 SYSTEM 이거나 세션 0 이면 GUI 기동을 건너뛰고, 아니어도 데이터 폴더를 쓰기 가능한 임시 폴더로 준다.
  if ($isSystem -or $session0) {
    Write-Warning "GUI 기동을 건너뜀 — SYSTEM 계정($isSystem)이거나 세션 0($session0)이라 대화형 데스크톱이 없다(WebView2 오류 대화상자 방지, #114). 창 확인은 사람이 호스트에서 설치 파일을 직접 실행해 본다(docs/e2e-acceptance.md)"
  } else {
  $wvDir = Join-Path ([IO.Path]::GetTempPath()) "promptlab-smoke-webview-$PID"
  $savedWv = $env:WEBVIEW2_USER_DATA_FOLDER
  $env:WEBVIEW2_USER_DATA_FOLDER = $wvDir
  try {
  $gui = Start-Process -FilePath (Join-Path $installDir 'promptlab-app.exe') -PassThru
  Start-Sleep -Seconds $GuiSeconds
  $gui.Refresh()
  if ($gui.HasExited) {
    if ($session0) { Write-Warning "세션 0이라 건너뜀 — 앱이 바로 종료됐다(종료 $($gui.ExitCode)). 대화형 데스크톱이 없는 서비스 세션에서는 WebView2 창을 못 만든다. 창 확인은 사람이 호스트에서 설치 파일을 직접 실행해 본다(docs/e2e-acceptance.md)" }
    else { throw "앱이 바로 종료됨 (종료 $($gui.ExitCode)) — WebView2 런타임·DLL 확인" }
  } else {
    if ($gui.MainWindowHandle -eq 0) {
      if ($session0) { Write-Warning '세션 0이라 건너뜀 — 앱은 떠 있으나 창 핸들이 없다(대화형 데스크톱 없음).' }
      else { Stop-Process -Id $gui.Id -Force; throw '앱은 떠 있는데 창이 없다' }
    } else { Write-Host "창 제목: $($gui.MainWindowTitle)" }
    Stop-Process -Id $gui.Id -Force
  }
  } finally {
    if ($null -eq $savedWv) { Remove-Item Env:WEBVIEW2_USER_DATA_FOLDER -ErrorAction SilentlyContinue } else { $env:WEBVIEW2_USER_DATA_FOLDER = $savedWv }
    if (Test-Path -LiteralPath $wvDir) { Remove-Item -LiteralPath $wvDir -Recurse -Force -ErrorAction SilentlyContinue }
  }
  }

  Step '3b. 서버 주소 기본값 — env 없이 설치된 앱의 기본값이 운영 서버에 닿아야 한다(#62)'
  # 앱 CLI 를 PROMPTLAB_SERVER_URL 없이 돌려 로그인 설정(/auth/app-config, 토큰 없이 되는 공개 경로)을 받는다.
  $savedUrl = $env:PROMPTLAB_SERVER_URL; $savedTok = $env:PROMPTLAB_ACCESS_TOKEN
  Remove-Item Env:PROMPTLAB_SERVER_URL -ErrorAction SilentlyContinue
  Remove-Item Env:PROMPTLAB_ACCESS_TOKEN -ErrorAction SilentlyContinue
  try {
    $out = & $cli --print-server-url 2>&1 | Out-String
  } finally { $env:PROMPTLAB_SERVER_URL = $savedUrl; $env:PROMPTLAB_ACCESS_TOKEN = $savedTok }
  if ($out.Trim() -ne 'https://promptlab.sandcorp.co.kr/api') { throw "env 없는 앱의 기본 서버 주소가 운영 주소가 아니다: $out" }
  $cfg = Invoke-Http -Uri "$($out.Trim())/auth/app-config" -Headers @{} -Label '앱 기본 주소 app-config'
  if ($cfg.Status -ne 200) { throw "기본 주소로 /auth/app-config 에 닿지 못했다 (HTTP $($cfg.Status))" }

  Step '4. 가상 카메라'
  if ($Camera) {
    # TODO(#13): 앱에 웹캠 분석이 생기면 — 가상 카메라(OBS Virtual Camera)에 녹화 영상을 흘리고(obs-cmd/obs-websocket),
    # 앱을 같은 흐름으로 돌려 카메라 신호(얼굴 없음·여럿 등)가 과정 로그에 남는지 본다. 지금은 장치가 보이는지만 확인한다.
    $cam = Get-PnpDevice -Class Camera, Image -Status OK -ErrorAction SilentlyContinue | Where-Object FriendlyName -Match 'OBS|Virtual'
    if (-not $cam) { throw '가상 카메라 장치가 없다 (OBS Virtual Camera 설치·시작 확인)' }
    Write-Host "가상 카메라 장치: $($cam.FriendlyName -join ', ') — 앱 연동 검증은 #13 이후"
  } else { Write-Host '건너뜀 (-Camera 없음)' }

  Step '5. 응시·제출·서버 히든 채점 (화면 없는 같은 흐름 — 모범 답안, 로컬 Docker 예제 테스트)'
  if ($useMachine) { Set-SmokeToken }
  $clock = [Diagnostics.Stopwatch]::StartNew()
  # 내 시험(스모크 배정) 중 시작 전 문항 하나. 기계 토큰이면 이 호출이 새 배정을 준비한다.
  $headers = @{ Authorization = "Bearer $env:PROMPTLAB_ACCESS_TOKEN" }
  # 바이트로 받아 UTF-8 로 디코드(5.1 Invoke-RestMethod 는 charset 없으면 ISO-8859-1 이라 한글 조직 이름이 깨진다 — #35). 재시도·진단은 Invoke-Http.
  $resp = Invoke-Http -Uri "$($env:PROMPTLAB_SERVER_URL.TrimEnd('/'))/me/assignments" -Headers $headers -Label '배정 조회'
  if ($resp.Status -ne 200) { throw "배정 조회 실패 (HTTP $($resp.Status))" }
  $assignments = @($resp.Text | ConvertFrom-Json | ForEach-Object { $_ })
  $pick = $null
  foreach ($a in $assignments) {
    $item = $a.items | Where-Object { $_.state.status -eq 'not_started' } | Select-Object -First 1
    if ($item) { $pick = @{ id = $a.id; item = $item.position; org = $a.org_name }; break }
  }
  if (-not $pick) {
    # 원인이 보이게: 받은 배정 목록 요약(배정 id·문항 상태만).
    foreach ($a in $assignments) {
      $states = @($a.items | ForEach-Object { "$($_.position):$($_.state.status)" }) -join ','
      Write-Host "  받은 배정 $($a.id) 문항 [$states]"
    }
    Write-Host "  받은 배정 $($assignments.Count)개"
    throw '시작 전 문항이 있는 배정이 없다 (스모크 조직: 서버 PROMPTLAB_SMOKE_CLIENT_IDS 확인)'
  }
  Write-Host "배정 $($pick.id) ($($pick.org)) 문항 $($pick.item) — 문제 $($sol.Id)의 모범 답안($($sol.Language))을 낸다"
  # 모범 답안 코드를 임시 파일로(BOM 없는 UTF-8). 히든 테스트는 어디에도 내지 않는다.
  $ext = @{ python = 'py'; javascript = 'js'; java = 'java'; cpp = 'cpp' }[$sol.Language]
  $codeFile = Join-Path $env:TEMP "promptlab-smoke-$PID-solution.$ext"
  [IO.File]::WriteAllText($codeFile, $sol.Code, (New-Object System.Text.UTF8Encoding $false))
  $lines = @(Invoke-Native { & $cli --assignment $pick.id --item $pick.item --language $sol.Language --code-file $codeFile } | ForEach-Object { Write-Host $_; $_ })
  $code = $LASTEXITCODE
  Remove-Item -Force -ErrorAction SilentlyContinue $codeFile
  Write-Host ("응시·채점 {0:N0}초 (토큰 수명 600초)" -f $clock.Elapsed.TotalSeconds)
  if ($code -ne 0) { throw "응시 흐름 실패 (종료 $code)" }
  # 로컬 Docker 예제 테스트가 모범 답안으로 전부 통과했나, 서버 히든 채점이 전부 통과했나.
  $ex = $lines | Where-Object { $_ -match '\[예제 테스트\]\s+예제\s+(\d+)/(\d+)\s+통과' } | Select-Object -First 1
  if (-not $ex) { throw '예제 테스트 결과 줄이 없다 — 로컬 Docker 실행이 돌지 않았다(위 [예제 테스트] 줄 확인)' }
  $m = [regex]::Match($ex, '예제\s+(\d+)/(\d+)\s+통과')
  if ($m.Groups[1].Value -ne $m.Groups[2].Value) { throw "모범 답안이 예제 테스트를 다 통과하지 못했다: $ex" }
  $gr = $lines | Where-Object { $_ -match '\[결과\].*통과\s+(\d+)/(\d+)' } | Select-Object -First 1
  if ($gr) {
    $g = [regex]::Match($gr, '통과\s+(\d+)/(\d+)')
    if ($g.Groups[1].Value -ne $g.Groups[2].Value) { throw "모범 답안이 서버 히든 채점을 다 통과하지 못했다: $gr" }
  } else { Write-Warning '결과 줄에 통과 수가 없다(점수를 공개하지 않는 시험이면 「응시 완료」만 나온다) — 서버 채점 통과 수는 확인하지 못했다' }
  Write-Host "`n스모크 통과"
} finally {
  Step '6. 제거'
  $un = Join-Path $installDir 'unins000.exe'
  if (Test-Path $un) { Start-Process -FilePath $un -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES' -Wait }
}
