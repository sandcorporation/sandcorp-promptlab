# 후보 모델(GGUF)을 받아 sha256을 검증하고 캐시 디렉터리에 둔다(scripts/fetch-model.sh 의 PowerShell 판, #131).
# 쓰기: fetch-model.ps1 [-Model <id>] [-All]   (기본 = 가장 큰 후보, id 예: 7b · 1.5b)
# 이미 있고 해시가 맞으면 받지 않는다. 받은 모델마다 경로를 한 줄씩 찍는다.
param([string]$Model = '', [switch]$All)
. "$PSScriptRoot\common.ps1"
$pins = Get-ModelPins
if ($All) { $picked = $pins }
elseif ($Model) {
  $picked = @($pins | Where-Object { $_.id -eq $Model })
  if ($picked.Count -eq 0) { throw "후보에 없는 모델 $Model (후보: $(($pins | ForEach-Object { $_.id }) -join ', '))" }
}
else { $picked = @($pins[0]) }
$dir = Get-ModelDir
New-Item -ItemType Directory -Force $dir | Out-Null

foreach ($pin in $picked) {
  $dest = Join-Path $dir $pin.file
  if ((Test-Path $dest) -and ((Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower() -eq $pin.sha256)) {
    Write-Host "fetch-model: 캐시에 있음 ($($pin.id) $($pin.file))"
    Write-Output $dest; continue
  }
  $url = "https://huggingface.co/$($pin.hf_repo)/resolve/$($pin.revision)/$($pin.file)"
  $part = "$dest.part"
  Write-Host "fetch-model: 내려받기 $($pin.id) $($pin.hf_repo) $($pin.file)"
  # curl.exe 는 Windows 10+ 기본 포함 — 끊겼으면 이어 받는다(-C -). 한 번에 5번 재시도하고, 그래도 끊기면 전체를 3번 되풀이한다.
  for ($try = 1; $try -le 3; $try++) {
    Invoke-Native { curl.exe -fsSL --retry 5 --retry-delay 3 -C - -o $part $url } | ForEach-Object { Write-Host $_ }
    if ($LASTEXITCODE -eq 0) { break }
    Write-Host "fetch-model: 내려받기 실패(curl 종료 $LASTEXITCODE) — $try/3"
    if ($try -eq 3) { throw "내려받기 실패 (curl 종료 $LASTEXITCODE)" }
    Start-Sleep -Seconds (5 * $try)
  }
  if ((Get-FileHash -Algorithm SHA256 $part).Hash.ToLower() -ne $pin.sha256) {
    Remove-Item -Force $part
    throw "sha256 불일치 — 받은 파일을 버렸다 ($($pin.file))"
  }
  Move-Item -Force $part $dest
  Write-Host "fetch-model: 검증 끝 ($($pin.id) $($pin.file))"
  Write-Output $dest
}
