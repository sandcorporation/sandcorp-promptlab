# 설치 파일 만들기: build.ps1 산출물 → (서명) → Inno Setup → (서명) → dist\promptlab-app-setup-<버전>.exe
# AI 모델은 넣지도 받지도 않는다(#131) — 앱이 응시 전 점검에서 크기를 추천하고 받는다(docs/windows-host.md 「설치 파일」).
. "$PSScriptRoot\common.ps1"
Set-Location $Root
$version = ([regex]::Match((Get-Content -Raw -Encoding UTF8 'Cargo.toml'), '(?m)^version = "([^"]+)"')).Groups[1].Value
if (-not (Test-Path 'dist\app\promptlab-app.exe')) { throw 'dist\app 가 없다 — 먼저 build.ps1' }

# 설치 파일에 들어갈 것 확인(#101): Vulkan 빌드면 ggml 백엔드 모듈(ggml-vulkan·ggml-cpu-*)이 dist\app 에 다 있어야 한다(installer.iss 가 *.dll 을 싣는다).
$dlls = @(Get-ChildItem 'dist\app' -Filter '*.dll' -File)
# 설치 파일에는 VC++ 런타임(app-local, #111)이 늘 실린다 — GPU(Vulkan) 빌드인지는 llama.dll(dynamic-link) 유무로 본다.
if (Test-Path 'dist\app\llama.dll') {
  if (-not (Test-Path 'dist\app\ggml-vulkan.dll')) { throw 'dist\app 에 ggml-vulkan.dll 이 없다 — Vulkan 빌드가 아니거나 build.ps1 이 모듈을 못 모았다' }
  if (-not (Get-ChildItem 'dist\app' -Filter 'ggml-cpu*.dll')) { throw 'dist\app 에 ggml-cpu-*.dll 이 없다 — Vulkan 이 없는 PC에서 CPU 로 못 돈다' }
  Write-Host "[installer] 설치 파일에 싣는 DLL $($dlls.Count)개: $(($dlls.Name) -join ', ')"
} else { Write-Warning "[installer] llama.dll 없음 — CPU 빌드(GPU 가속 없음)로 설치 파일을 만든다. 싣는 DLL $($dlls.Count)개: $(($dlls.Name) -join ', ')" }
# Monaco(#97)는 exe 에 구워져 있다 — 자산 이름 한 개가 exe 바이트에 보이는지만 확인한다(없으면 경고).
$exeBytes = [IO.File]::ReadAllBytes((Resolve-Path 'dist\app\promptlab-app.exe').Path)
if ([Text.Encoding]::ASCII.GetString($exeBytes).Contains('vs/editor/editor.main.js')) { Write-Host '[installer] Monaco 자산이 exe 에 구워져 있다(확인)' }
else { Write-Warning '[installer] exe 에서 Monaco 자산 이름(vs/editor/editor.main.js)을 못 찾았다 — 구운 방식이 바뀌었으면 이 확인을 고친다' }
$exeBytes = $null
& "$PSScriptRoot\sign.ps1" -Path (Resolve-Path 'dist\app\promptlab-app.exe').Path, (Resolve-Path 'dist\app\promptlab-app-cli.exe').Path
Invoke-Native { iscc.exe "/DAppVersion=$version" "$PSScriptRoot\installer.iss" }
if ($LASTEXITCODE -ne 0) { throw "iscc 실패 ($LASTEXITCODE)" }
$setup = (Resolve-Path "dist\promptlab-app-setup-$version.exe").Path
& "$PSScriptRoot\sign.ps1" -Path $setup
Get-Item $setup | ForEach-Object { Write-Host ("{0}  {1:N1} MB  sha256 {2}" -f $_.Name, ($_.Length / 1MB), (Get-FileHash -Algorithm SHA256 $_).Hash.ToLower()) }
