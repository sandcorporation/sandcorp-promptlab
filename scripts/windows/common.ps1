# scripts/windows/*.ps1 공용: 저장소 루트·모델 고정값 읽기.
$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1: 콘솔·파이프 인코딩을 UTF-8로 (기본은 OEM/CP949라 한글이 깨진다)
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch {}
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
# 5.1 기본은 TLS 1.0/1.1 — 외부 HTTPS(auth·HuggingFace)는 1.2 이상이 필요하다
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

# crates/shared/src/inference.rs 의 `MODELS` 후보 표(#131, 큰 것부터)가 정본 — scripts/fetch-model.sh 와 같은 표를 읽는다.
function Get-ModelPins {
  $src = Get-Content -Raw -Encoding UTF8 (Join-Path $Root 'crates\shared\src\inference.rs')
  $t = [regex]::Match($src, '(?s)pub const MODELS: \[ModelPin; \d+\] = \[(.*?)\n\];')
  if (-not $t.Success) { throw 'inference.rs 에서 MODELS 를 못 읽었다' }
  $pins = @()
  foreach ($b in [regex]::Matches($t.Groups[1].Value, '(?s)ModelPin \{(.*?)\n    \}')) {
    $pin = @{}
    foreach ($f in 'id', 'hf_repo', 'file', 'revision', 'sha256', 'size') {
      $v = [regex]::Match($b.Groups[1].Value, "\b${f}:\s*`"?([0-9A-Za-z_./-]+)`"?,")
      if (-not $v.Success) { throw "MODELS 의 $f 를 못 읽었다" }
      $pin[$f] = if ($f -eq 'size') { $v.Groups[1].Value.Replace('_', '') } else { $v.Groups[1].Value }
    }
    $pins += , $pin
  }
  if ($pins.Count -eq 0) { throw 'MODELS 가 비었다' }
  , $pins
}

# promptlab_inference::model_dir() 과 같은 순서(Windows 몫): PROMPTLAB_MODEL_DIR → XDG_CACHE_HOME → LOCALAPPDATA → ~/.cache
function Get-ModelDir {
  if ($env:PROMPTLAB_MODEL_DIR) { return $env:PROMPTLAB_MODEL_DIR }
  if ($env:XDG_CACHE_HOME) { return Join-Path $env:XDG_CACHE_HOME 'promptlab\models' }
  if ($env:LOCALAPPDATA) { return Join-Path $env:LOCALAPPDATA 'promptlab\models' }
  Join-Path $HOME '.cache\promptlab\models'
}

# cargo 가 만드는 target 디렉터리: Jenkinsfile.windows 가 CARGO_TARGET_DIR 을 걸면 그것, 없으면 $Root\target.
function Get-TargetDir {
  if ($env:CARGO_TARGET_DIR) { return $env:CARGO_TARGET_DIR }
  Join-Path $Root 'target'
}

# VS 2022(C++ x64 도구가 든) 설치 폴더. 없으면 $null.
function Get-VsInstallPath {
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not (Test-Path $vswhere)) { return $null }
  $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if ($vs) { return "$vs".Trim() }
  $null
}

# Ninja 실행 파일: PATH → VS 에 딸린 것(구성 요소 「C++ CMake tools for Windows」). 없으면 $null.
function Find-Ninja {
  $c = Get-Command ninja.exe -ErrorAction SilentlyContinue
  if ($c) { return $c.Source }
  $vs = Get-VsInstallPath
  if ($vs) {
    $n = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
    if (Test-Path $n) { return $n }
  }
  $null
}

# VS x64 개발자 환경(vcvars64.bat)을 이 프로세스 환경변수로 들여온다 — cl.exe·link.exe·INCLUDE·LIB 가 잡힌다.
# 왜(#111): Ninja 로 llama.cpp 를 빌드하면 ExternalProject(vulkan-shaders-gen)의 안쪽 cmake 가 컴파일러를 PATH 에서 찾는다.
function Import-VsDevEnv {
  $vs = Get-VsInstallPath
  if (-not $vs) { throw 'VS 2022 C++ x64 도구를 찾지 못했다(vswhere)' }
  $vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
  if (-not (Test-Path $vcvars)) { throw "vcvars64.bat 없음: $vcvars" }
  $tmp = Join-Path $env:TEMP "promptlab-vcvars-$PID.cmd"
  Set-Content -Path $tmp -Encoding ASCII -Value "@call `"$vcvars`" >nul 2>&1 || exit /b 1`r`n@set"
  try {
    $lines = & cmd.exe /d /c $tmp
    if ($LASTEXITCODE -ne 0) { throw "vcvars64.bat 실패 ($LASTEXITCODE)" }
  } finally { Remove-Item -Force $tmp -ErrorAction SilentlyContinue }
  foreach ($l in $lines) { if ("$l" -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] } }
  if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) { throw 'vcvars64 뒤에도 cl.exe 가 PATH 에 없다' }
  Write-Host "[build] VS 개발자 환경: $vs (VSCMD_VER=$env:VSCMD_VER)"
}

# 네이티브 명령(cargo·rustc·iscc·signtool·curl·앱 CLI …) 호출. Windows PowerShell 5.1 은 `$ErrorActionPreference='Stop'` 일 때
# 네이티브 stderr(rustup 의 `info: syncing channel updates…` 같은 정상 안내)를 NativeCommandError 로 던져 스크립트를 멈춘다.
# 그래서 호출 동안만 Continue 로 두고 stderr 는 줄글로 바꿔 stdout 에 합친다. 성공 여부는 호출한 쪽이 $LASTEXITCODE 로만 판단한다.
#   $out = Invoke-Native { rustc -vV }; if ($LASTEXITCODE -ne 0) { throw ... }
function Invoke-Native([scriptblock]$Command) {
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try {
    & $Command 2>&1 | ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { $_ } }
  } finally { $ErrorActionPreference = $prev }
}

# ---- HTTP 호출(스모크의 auth·API 호출 공용, #35) ----
# 왜: Windows PowerShell 5.1 의 Invoke-RestMethod 는 (a) 연결·TLS 예외의 원인을 버리면 「HTTP 0」만 남고(스모크 #5),
#   (b) keep-alive 로 죽은 연결을 재사용하다 요청이 서버에 닿기 전에 실패하기도 하고, (c) Content-Type 에 charset 이 없으면
#   ISO-8859-1 로 디코드해 한글이 깨진다. 그래서 ① -DisableKeepAlive 로 바이트를 받아 UTF-8 로 직접 디코드하고
#   ② 전송 실패(응답 없음)·5xx 는 지수 대기로 다시 시도하고 ③ 그래도 안 되면 curl.exe(Windows 내장)로 같은 요청을 한다.
# 진단 출력(Format-HttpError)은 예외 종류·메시지·WebExceptionStatus·InnerException 만 싣는다 — 요청 본문·헤더·응답 본문은
#   싣지 않는다(시크릿·토큰 보호).
# 반환: @{ Status = <HTTP 코드>; Text = <UTF-8 본문> }. 4xx·5xx 도 돌려준다(호출한 쪽이 판단). 전송 자체가 끝내 실패하면 throw.

function Format-HttpError($Err) {
  $e = if ($Err -is [System.Management.Automation.ErrorRecord]) { $Err.Exception } else { $Err }
  $parts = @()
  $n = 0
  while ($e -and $n -lt 5) {
    $s = "$($e.GetType().FullName): $($e.Message)"
    if ($e -is [System.Net.WebException]) { $s += " [WebExceptionStatus=$($e.Status)]" }
    $parts += $s
    $e = $e.InnerException
    $n++
  }
  $parts -join ' <- '
}

function ConvertTo-FormBody([hashtable]$Form) {
  (($Form.GetEnumerator() | ForEach-Object { [uri]::EscapeDataString("$($_.Key)") + '=' + [uri]::EscapeDataString("$($_.Value)") }) -join '&')
}

function Invoke-HttpOnce($Method, $Uri, $Headers, $Body, $TimeoutSec) {
  $args2 = @{ Method = $Method; Uri = $Uri; UseBasicParsing = $true; DisableKeepAlive = $true; TimeoutSec = $TimeoutSec }
  if ($Headers -and $Headers.Count) { $args2.Headers = $Headers }
  if ($null -ne $Body) { $args2.Body = $Body; $args2.ContentType = 'application/x-www-form-urlencoded' }
  $stream = $null; $status = 0
  try {
    $r = Invoke-WebRequest @args2
    $status = [int]$r.StatusCode
    $stream = $r.RawContentStream
  } catch {
    $resp = $_.Exception.Response
    if (-not $resp) { throw }          # 응답 없음 = 전송 실패 — 호출한 쪽이 다시 시도
    $status = [int]$resp.StatusCode
    $stream = $resp.GetResponseStream()
  }
  $ms = New-Object System.IO.MemoryStream
  if ($stream) { try { if ($stream.CanSeek) { $stream.Position = 0 }; $stream.CopyTo($ms) } catch {} }
  @{ Status = $status; Text = (New-Object System.Text.UTF8Encoding $false).GetString($ms.ToArray()) }
}

# curl.exe 대체 경로. 시크릿(폼 본문·Authorization)은 명령줄 인자에 두지 않는다 — 프로세스 목록(`-u`·`-d`·`-H` 인자)에 보이기 때문.
# 요청 전체를 `--config -`(표준입력)로 넘긴다. 임시 파일을 안 쓰니 디스크에도 남지 않는다.
function Invoke-HttpCurl($Method, $Uri, $Headers, $Body, $TimeoutSec) {
  $q = { param($s) '"' + ($s -replace '\\', '\\' -replace '"', '\"' -replace "`r", '\r' -replace "`n", '\n') + '"' }
  $cfg = @("url = $(& $q $Uri)", "request = $(& $q $Method)")
  if ($Headers) { foreach ($k in $Headers.Keys) { $cfg += "header = $(& $q "${k}: $($Headers[$k])")" } }
  if ($null -ne $Body) {
    $cfg += "header = $(& $q 'Content-Type: application/x-www-form-urlencoded')"
    $cfg += "data = $(& $q $Body)"
  }
  $out = [IO.Path]::GetTempFileName()
  try {
    # -s -S: 진행표시 없이 오류만. -w 는 http 코드만(본문은 -o 파일). 오류 메시지에 요청 내용은 들지 않는다.
    $code = $cfg | & curl.exe --silent --show-error --max-time $TimeoutSec --config - --output $out --write-out '%{http_code}' 2>&1
    $exit = $LASTEXITCODE
    if ($exit -ne 0) { throw "curl.exe 종료 $exit ($(($code | Out-String).Trim()))" }
    @{ Status = [int]("$code".Trim()); Text = (New-Object System.Text.UTF8Encoding $false).GetString([IO.File]::ReadAllBytes($out)) }
  } finally { Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue }
}

function Invoke-Http {
  param(
    [Parameter(Mandatory)][string]$Uri,
    [string]$Method = 'GET',
    [hashtable]$Headers,
    [hashtable]$Form,
    [int]$TimeoutSec = 30,
    [int]$Attempts = 3,
    [string]$Label = $Uri
  )
  $body = if ($Form) { ConvertTo-FormBody $Form } else { $null }
  $last = $null
  for ($i = 1; $i -le $Attempts; $i++) {
    try {
      $r = Invoke-HttpOnce $Method $Uri $Headers $body $TimeoutSec
      if ($r.Status -lt 500) { return $r }
      $last = "HTTP $($r.Status)"
      Write-Host "[http] $Label 시도 $i/$Attempts 서버 오류 $($r.Status)"
    } catch {
      $last = Format-HttpError $_
      Write-Host "[http] $Label 시도 $i/$Attempts 전송 실패: $last"
    }
    if ($i -lt $Attempts) { Start-Sleep -Seconds ([math]::Pow(2, $i)) }   # 2초, 4초
  }
  Write-Host "[http] $Label — .NET 경로가 $Attempts 번 실패, curl.exe 로 다시 시도"
  try {
    $r = Invoke-HttpCurl $Method $Uri $Headers $body $TimeoutSec
    Write-Host "[http] $Label curl.exe 로 성공 (HTTP $($r.Status))"
    return $r
  } catch {
    throw "$Label 요청 실패 — .NET: $last / curl: $(Format-HttpError $_)"
  }
}

# ---- Docker Desktop(호스트 네이티브) 확인 — check-tools.ps1 과 스모크가 같이 쓴다(#101) ----
# 서비스 계정(jenkins-agent, 세션 0)에서 엔진에 닿는지 `docker version`(서버 쪽까지)으로 본다. 반환 @{ Ok; Detail }:
# Ok 면 Detail = 「서버 <판> (<os>)」, 아니면 원인을 사람 말로(파이프 권한·엔진 미기동·Windows 컨테이너 모드·docker 없음).
function Find-Docker {
  $c = Get-Command docker.exe -ErrorAction SilentlyContinue
  if ($c) { return $c.Source }
  $p = Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe'
  if (Test-Path $p) { return $p }
  $null
}
function Test-DockerEngine {
  $docker = Find-Docker
  if (-not $docker) { return @{ Ok = $false; Detail = 'docker.exe 가 없다 — 호스트에 Docker Desktop 을 설치하고(Linux 컨테이너 모드) 서비스 환경의 PATH 에 C:\Program Files\Docker\Docker\resources\bin 이 있게 한 뒤 에이전트 서비스를 다시 시작한다' } }
  $so = [IO.Path]::GetTempFileName(); $se = [IO.Path]::GetTempFileName()
  try {
    $p = Start-Process -FilePath $docker -ArgumentList 'version', '--format', '"{{.Server.Version}} {{.Server.Os}}"' -NoNewWindow -PassThru -RedirectStandardOutput $so -RedirectStandardError $se
    if (-not $p.WaitForExit(30000)) {
      try { $p.Kill() } catch {}
      return @{ Ok = $false; Detail = 'docker version 이 30초 안에 끝나지 않았다 — Docker Desktop 엔진이 시작 중이거나 멈췄다. 호스트에서 Docker Desktop 상태를 본다' }
    }
    $out = (Get-Content -Raw -Encoding UTF8 $so); $err = (Get-Content -Raw -Encoding UTF8 $se)
    if ($null -eq $out) { $out = '' }; if ($null -eq $err) { $err = '' }
    # 판정은 종료 코드가 아니라 stdout 으로 한다: 서비스 세션에서 Start-Process -PassThru + 출력 리다이렉트를 쓰면 Process.ExitCode 가 null 로 남는다(#101 windows 잡 #24).
    # 엔진이 닿으면 stdout 이 `<판> <os>` 한 줄이고, 닿지 않으면 stdout 은 비고 stderr 에 원인이 나온다.
    $v = $out.Trim().Trim('"').Trim()
    if ($v -match '^\S+\s+(linux|windows)$') {
      if ($Matches[1] -eq 'windows') { return @{ Ok = $false; Detail = "엔진이 Windows 컨테이너 모드다($v) — Docker Desktop 트레이 메뉴에서 「Linux 컨테이너로 전환」" } }
      return @{ Ok = $true; Detail = "서버 $v" }
    }
    $who = (& whoami.exe 2>$null)
    $inGroup = [bool]((& whoami.exe /groups 2>$null) -match '(?i)docker-users')
    if ($err -match '(?i)access is denied|permission denied') {
      $why = if ($inGroup) { '계정은 docker-users 그룹이지만 엔진 파이프(\\.\pipe\docker_engine)가 거부한다 — Docker Desktop 설정의 접근 권한 확인' }
             else { "계정($who)이 docker-users 그룹에 없다. 그룹에 넣은 뒤에는 토큰이 서비스 시작 때 정해지므로 에이전트 서비스(WinSW)를 다시 시작해야 한다" }
      return @{ Ok = $false; Detail = "Docker 엔진 파이프 권한이 없다 — $why" }
    }
    if ($err -match '(?i)cannot find the file|error during connect|daemon running|failed to connect|pipe') {
      return @{ Ok = $false; Detail = "Docker 엔진이 꺼져 있다 — 호스트에서 Docker Desktop 이 떠 있는지(엔진이 시작됐는지) 본다. 서비스 계정은 로그인하지 않으므로 엔진은 Docker Desktop 이 이미 켜져 있어야 닿는다. (docker-users 그룹: $(if ($inGroup) { '있음' } else { '없음' }))" }
    }
    $first = ($err -split "`r?`n" | Where-Object { $_.Trim() } | Select-Object -First 1)
    return @{ Ok = $false; Detail = "docker version 이 서버 판을 못 읽었다(stdout: '$v'): $first" }
  } finally { Remove-Item -Force -ErrorAction SilentlyContinue $so, $se }
}

# 스모크 문항(crates/store/src/catalog.rs 의 SMOKE_PROBLEM_ID — 스모크 전용 공개 문항, #138)의 모범 답안 코드와 언어.
# 파일은 저장소의 공개 픽스처(crates/store/fixtures/smoke/<id>.json)라 비공개 데이터 저장소 없이 돈다.
# 히든 테스트가 같은 파일에 있으니 이 함수는 reference_solution·reference_language 만 꺼내고 나머지는 어디에도 싣지 않는다.
function Get-SmokeSolution {
  $src = Get-Content -Raw -Encoding UTF8 (Join-Path $Root 'crates\store\src\catalog.rs')
  $m = [regex]::Match($src, 'SMOKE_PROBLEM_ID:\s*&str\s*=\s*"([^"]+)"')
  if (-not $m.Success) { throw 'catalog.rs 에서 SMOKE_PROBLEM_ID 를 못 읽었다' }
  $id = $m.Groups[1].Value
  $file = Join-Path $Root "crates\store\fixtures\smoke\$id.json"
  $json = Get-Content -Raw -Encoding UTF8 $file | ConvertFrom-Json
  $lang = if ($json.reference_language) { "$($json.reference_language)".ToLower() } else { 'python' }
  @{ Id = $id; Language = $lang; Code = [string]$json.reference_solution }
}
