# 빌드 에이전트(호스트 서비스 계정 jenkins-agent, 세션 0)에 필요한 도구가 다 있는지 본다(없으면 목록과 원인 힌트를 찍고 실패). docs/windows-host.md 의 설치 목록과 같다.
# 함정: rustup 은 기본이 사용자별 설치(%USERPROFILE%\.cargo)라 서비스 계정 PATH 에 안 보인다 — RUSTUP_HOME·CARGO_HOME 을 시스템 경로에 두고 jenkins-agent 에 쓰기 권한을 준다.
#       시스템 환경변수·PATH 를 바꾼 뒤에는 에이전트 서비스(WinSW)를 다시 시작해야 서비스가 새 값을 본다.
$ErrorActionPreference = 'Continue'
# Windows PowerShell 5.1: 콘솔·파이프 인코딩을 UTF-8로 (기본은 OEM/CP949라 한글이 깨진다)
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch {}
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
$missing = @()
function Need($name, $cmd, $hint) {
  $c = Get-Command $cmd -ErrorAction SilentlyContinue
  if ($c) { Write-Host ("ok   {0,-14} {1}" -f $name, $c.Source) } else { Write-Host ("없음 {0,-14} {1}" -f $name, $hint); $script:missing += $name }
}
Need 'git' 'git.exe' 'Git for Windows'
Need 'rustup' 'rustup.exe' 'https://rustup.rs (기본 호스트 x86_64-pc-windows-msvc)'
Need 'cargo' 'cargo.exe' 'rustup 과 함께'
Need 'cmake' 'cmake.exe' 'CMake 3.28+ (PATH 에 추가)'
Need 'iscc' 'iscc.exe' 'Inno Setup 6.3+ (ISCC.exe 폴더를 PATH 에)'
Need 'curl' 'curl.exe' 'Windows 10+ 기본'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
  $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if ($vs) { Write-Host "ok   MSVC           $vs" } else { Write-Host '없음 MSVC           VS 2022 Build Tools + C++ x64 도구·Windows 11 SDK'; $missing += 'MSVC' }
} else { Write-Host '없음 MSVC           VS 2022 Build Tools'; $missing += 'MSVC' }
# bindgen(llama-cpp-sys-2)이 libclang.dll 을 찾는다.
$lib = $env:LIBCLANG_PATH
if (-not $lib) { $lib = 'C:\Program Files\LLVM\bin' }
if (Test-Path (Join-Path $lib 'libclang.dll')) { Write-Host "ok   libclang       $lib" } else { Write-Host '없음 libclang       LLVM 18+ 설치 후 LIBCLANG_PATH=C:\Program Files\LLVM\bin'; $missing += 'LLVM' }
$wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
if ($wv) { Write-Host "ok   WebView2       $($wv.pv)" } else { Write-Host '없음 WebView2       Evergreen 런타임(앱 실행·스모크용)'; $missing += 'WebView2' }
# 선택: Vulkan SDK(glslc) — 없으면 build.ps1 이 CPU 빌드로 간다(실패하지 않는다).
if ($env:VULKAN_SDK -and (Test-Path (Join-Path $env:VULKAN_SDK 'Bin\glslc.exe'))) {
  Write-Host "ok   VulkanSDK      $env:VULKAN_SDK"
  # GPU 빌드는 Ninja 생성기로 돈다(#111 — VS 생성기는 vulkan-shaders-gen 단계가 겹쳐 깨진다). VS 에 딸린 것이어도 된다.
  . (Join-Path $PSScriptRoot 'common.ps1'); $ErrorActionPreference = 'Continue'
  $nj = Find-Ninja
  if ($nj) { Write-Host "ok   Ninja          $nj" } else { Write-Host '없음 Ninja          GPU 빌드에 필요 — VS 2022 구성 요소 「C++ CMake tools for Windows」(Ninja 포함) 또는 ninja.exe 를 시스템 PATH 에(에이전트 재시작)'; $missing += 'Ninja' }
} else { Write-Warning '선택 VulkanSDK      없음 — CPU 빌드(GPU 가속 빠짐, 실패 아님). LunarG Vulkan SDK 설치 후 시스템 환경변수 VULKAN_SDK 와 에이전트 서비스 재시작' }
# Docker Desktop(호스트 네이티브, Linux 컨테이너 모드) — 서비스 계정에서 엔진(서버 쪽)까지 닿아야 스모크의 로컬 실행·예제 테스트가 돈다.
. (Join-Path $PSScriptRoot 'common.ps1')
$ErrorActionPreference = 'Continue'   # common.ps1 이 Stop 으로 바꾼다
$dk = Test-DockerEngine
if ($dk.Ok) { Write-Host "ok   Docker         $($dk.Detail)" } else { Write-Host "없음 Docker         $($dk.Detail)"; $missing += 'Docker' }
# 쓰기 권한: cargo 캐시·target(Jenkinsfile.windows 의 C:\promptlab-cache)과 RUSTUP_HOME·CARGO_HOME 에 jenkins-agent 가 쓸 수 있어야 한다.
foreach ($d in @($env:CARGO_HOME, $env:RUSTUP_HOME, $env:CARGO_TARGET_DIR) | Where-Object { $_ }) {
  if (Test-Path $d) {
    $probe = Join-Path $d ".write-probe-$PID"
    try { Set-Content -Path $probe -Value 'x' -ErrorAction Stop; Remove-Item -Force $probe; Write-Host "ok   쓰기 권한      $d" }
    catch { Write-Host "없음 쓰기 권한      $d — 계정 '$svc' 에 쓰기 권한을 준다"; $missing += "쓰기:$d" }
  }
}
if ($missing) { Write-Host "`n빠진 것: $($missing -join ', ')"; exit 1 }
