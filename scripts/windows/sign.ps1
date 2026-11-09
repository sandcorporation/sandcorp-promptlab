# 코드 서명(선택). 인증서가 있으면 서명하고, 없으면 「건너뜀」을 분명히 찍고 0 으로 끝난다(시범 배포는 서명 없이).
#   -Path       서명할 파일들(exe)
#   -PfxPath    인증서 .pfx (Jenkins 자격 `promptlab-codesign-pfx`(파일)가 넘긴다). 비어 있으면 건너뜀.
#   -PfxPassword 인증서 비밀번호(자격 `promptlab-codesign-pfx-password`). 로그에 찍지 않는다.
param(
  [Parameter(Mandatory)][string[]]$Path,
  [string]$PfxPath = $env:CODESIGN_PFX,
  [string]$PfxPassword = $env:CODESIGN_PFX_PASSWORD,
  [string]$TimestampUrl = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\common.ps1"
# Windows PowerShell 5.1: 콘솔·파이프 인코딩을 UTF-8로 (기본은 OEM/CP949라 한글이 깨진다)
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch {}
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
if (-not $PfxPath -or -not (Test-Path $PfxPath)) {
  Write-Host '########## 코드 서명 건너뜀 — 인증서 없음 (SmartScreen 경고가 뜬다. 시범 배포 한정) ##########'
  exit 0
}
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
  Sort-Object FullName -Descending | Select-Object -First 1
if (-not $signtool) { throw 'signtool.exe 없음 — Windows SDK 가 필요하다' }
foreach ($p in $Path) {
  Invoke-Native { & $signtool.FullName sign /f $PfxPath /p $PfxPassword /fd SHA256 /tr $TimestampUrl /td SHA256 $p }
  if ($LASTEXITCODE -ne 0) { throw "서명 실패: $p" }
  Invoke-Native { & $signtool.FullName verify /pa $p } | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "서명 검증 실패: $p" }
  Write-Host "서명함: $p"
}
