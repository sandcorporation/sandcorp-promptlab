# 스모크 기계 토큰(#28): sandcorp-auth `/token` client_credentials 로 액세스 토큰을 받아 이 프로세스의
# $env:PROMPTLAB_ACCESS_TOKEN 에 넣는다. 규칙은 auth 문서 12절(acct=machine, sub=client:<id>, 수명 600초, refresh 없음).
#   . scripts\windows\smoke-token.ps1; Set-SmokeToken
# 입력(수동 스모크 호출자가 환경 변수로 전달):
#   PROMPTLAB_SMOKE_CLIENT_ID · PROMPTLAB_SMOKE_CLIENT_SECRET
#   PROMPTLAB_AUTH_URL(기본 https://auth.sandcorp.co.kr) · PROMPTLAB_RESOURCE(기본 https://promptlab.sandcorp.co.kr/api)
# 토큰·시크릿은 찍지 않는다(오류에도 응답 본문의 error 코드만 싣는다). 전송 실패는 재시도 뒤 curl.exe 로 대체(common.ps1).

# Windows PowerShell 5.1: 콘솔·파이프 인코딩을 UTF-8로 (기본은 OEM/CP949라 한글이 깨진다)
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch {}
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
# 5.1 기본은 TLS 1.0/1.1 — 외부 HTTPS(auth·HuggingFace)는 1.2 이상이 필요하다
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

# 재시도·진단·curl 대체 경로는 common.ps1 의 Invoke-Http
. (Join-Path $PSScriptRoot 'common.ps1')

function Set-SmokeToken {
  if (-not $env:PROMPTLAB_SMOKE_CLIENT_ID -or -not $env:PROMPTLAB_SMOKE_CLIENT_SECRET) {
    throw 'PROMPTLAB_SMOKE_CLIENT_ID·SECRET 이 없다 — PROMPTLAB_SMOKE_CLIENT_ID·SECRET 환경 변수 (docs/windows-host.md 「스모크 계정」)'
  }
  $auth = if ($env:PROMPTLAB_AUTH_URL) { $env:PROMPTLAB_AUTH_URL.TrimEnd('/') } else { 'https://auth.sandcorp.co.kr' }
  $resource = if ($env:PROMPTLAB_RESOURCE) { $env:PROMPTLAB_RESOURCE } else { 'https://promptlab.sandcorp.co.kr/api' }
  $body = @{
    grant_type    = 'client_credentials'
    client_id     = $env:PROMPTLAB_SMOKE_CLIENT_ID
    client_secret = $env:PROMPTLAB_SMOKE_CLIENT_SECRET
    resource      = $resource
    scope         = 'promptlab'
  }
  $resp = Invoke-Http -Method Post -Uri "$auth/token" -Form $body -Label '스모크 토큰 받기'
  $r = $null
  try { $r = $resp.Text | ConvertFrom-Json } catch {}
  if ($resp.Status -ne 200) {
    $code = if ($r -and $r.error) { $r.error } else { '' }   # 응답 본문 중 error 코드만 싣는다
    throw "스모크 토큰 받기 실패 (HTTP $($resp.Status) $code)"
  }
  if (-not $r.access_token) { throw '스모크 토큰 응답에 access_token 이 없다' }
  $env:PROMPTLAB_ACCESS_TOKEN = $r.access_token
  Write-Host "스모크 기계 토큰 받음 (수명 $($r.expires_in)초, 스코프 $($r.scope))"
}
