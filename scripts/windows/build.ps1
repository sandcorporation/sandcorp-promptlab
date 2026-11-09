# Windows x64 릴리스 빌드: promptlab-app(화면) + promptlab-app-cli(화면 없는 같은 흐름, 스모크용).
# 산출물: dist\app\promptlab-app.exe, dist\app\promptlab-app-cli.exe (+ Vulkan 빌드면 llama·ggml DLL과 ggml 백엔드 모듈)
# GPU(#98): VULKAN_SDK 가 있으면 `--features vulkan`(llama.cpp Vulkan 백엔드를 동적 모듈로) — 없으면 경고 찍고 CPU 빌드.
. "$PSScriptRoot\common.ps1"
Set-Location $Root
if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin' }
# rust-toolchain.toml 이 판을 고정한다(rustup 이 알아서 받는다). 호스트가 MSVC 인지 확인.
$rustcInfo = Invoke-Native { rustc -vV }
if ($LASTEXITCODE -ne 0) { throw "rustc -vV 실패 ($LASTEXITCODE)" }
$host_triple = (($rustcInfo | Where-Object { "$_" -match '^host:' } | Select-Object -First 1).ToString()).Split(' ')[1]
if ($host_triple -ne 'x86_64-pc-windows-msvc') { throw "호스트 $host_triple — x86_64-pc-windows-msvc 이어야 한다" }

$vulkan = $env:VULKAN_SDK -and (Test-Path (Join-Path $env:VULKAN_SDK 'Bin\glslc.exe'))
if ($vulkan) {
  Write-Host "[build] Vulkan SDK $env:VULKAN_SDK — GPU(Vulkan) 백엔드를 동적 모듈로 굽는다(GPU 빌드)"
  # 함정(#111): Visual Studio 생성기로 빌드하면 llama.cpp 의 vulkan-shaders-gen(ExternalProject) 단계가 깨진다 —
  #   llama.cpp 가 cmake_minimum_required(…3.28)라 CMP0147(NEW, CMake 3.27+)로 사용자 명령에 BuildInParallel 이 붙고,
  #   llama-cpp-sys-2 build.rs 가 TrackFileAccess=false 를 건 상태에서 configure·build·install 단계가 겹쳐 돈다
  #   (「not a CMake build directory (missing CMakeCache.txt)」·「Not a file: …cmake_install.cmake」).
  #   그래서 MSBuild 를 빼고 Ninja 로 빌드한다 — cmake 크레이트가 CMAKE_GENERATOR 를 읽는다. 안쪽 cmake 가 컴파일러를
  #   PATH·CC/CXX 로 찾도록 VS 개발자 환경을 들여오고 CC/CXX=cl 로 못 박는다(PATH 의 gcc·clang 을 잡지 않게).
  Import-VsDevEnv
  $ninja = Find-Ninja
  if (-not $ninja) { throw 'Ninja 가 없다 — GPU 빌드에 필요하다. VS 2022 구성 요소 「C++ CMake tools for Windows」(Ninja 포함)를 깔거나 ninja.exe 를 PATH 에 둔다(docs/windows-host.md)' }
  $env:PATH = "$(Split-Path $ninja);$env:PATH"
  $env:CMAKE_GENERATOR = 'Ninja'
  $env:CC = 'cl.exe'
  $env:CXX = 'cl.exe'
  $ninjaVer = Invoke-Native { & $ninja --version }
  $cmakeVer = (Invoke-Native { cmake --version } | Select-Object -First 1)
  Write-Host "[build] 생성기 Ninja $ninjaVer ($ninja), $cmakeVer"
  # 함정(#111, windows 잡 #27): CMake 는 이미 있는 빌드 트리에서 CMAKE_GENERATOR 환경변수를 무시한다. 캐시(target)에
  #   VS 생성기로 만든 llama.cpp Vulkan 빌드 트리가 남아 있으면 Ninja 가 먹지 않고 같은 실패가 되풀이된다.
  #   그런 트리(GGML_VULKAN=ON 이고 생성기가 Ninja 가 아님)가 보이면 llama-cpp-sys-2 산출물을 지워 build.rs 부터 다시 돌린다 —
  #   out\build 만 지우면 cargo 가 「최신」으로 보고 build.rs 를 다시 안 돌릴 수 있어 cargo clean -p 로 지문까지 지운다.
  $stale = @(Get-ChildItem (Join-Path (Get-TargetDir) 'release\build') -Directory -Filter 'llama-cpp-sys-2-*' -ErrorAction SilentlyContinue |
    ForEach-Object { Join-Path $_.FullName 'out\build\CMakeCache.txt' } | Where-Object { Test-Path $_ } | Where-Object {
      $c = Get-Content -Encoding UTF8 $_
      $gen = ($c | Where-Object { $_ -match '^CMAKE_GENERATOR:INTERNAL=' } | Select-Object -First 1) -replace '^CMAKE_GENERATOR:INTERNAL=', ''
      ($c -match '^GGML_VULKAN:BOOL=ON$') -and ($gen -ne 'Ninja')
    })
  if ($stale) {
    foreach ($f in $stale) { Write-Host "[build] 낡은 llama.cpp 빌드 트리(생성기가 Ninja 아님): $f" }
    Write-Host '[build] cargo clean --release -p llama-cpp-sys-2 — Ninja 로 처음부터 다시 구성한다'
    Invoke-Native { cargo clean --release -p llama-cpp-sys-2 }
    if ($LASTEXITCODE -ne 0) { throw "cargo clean -p llama-cpp-sys-2 실패 ($LASTEXITCODE)" }
    foreach ($f in $stale) { if (Test-Path $f) { throw "낡은 빌드 트리가 남았다: $f" } }
  } else { Write-Host '[build] 남은 VS 생성기 llama.cpp 빌드 트리 없음' }
  Invoke-Native { cargo build --release --locked -p promptlab-app --bins --features vulkan }
} else {
  Write-Warning '[build] VULKAN_SDK(glslc) 없음 — GPU 없이 CPU 빌드. Vulkan SDK 를 깔면 다음 빌드부터 GPU 가속이 들어간다(docs/windows-host.md).'
  Invoke-Native { cargo build --release --locked -p promptlab-app --bins }
}
if ($LASTEXITCODE -ne 0) { throw "cargo build 실패 ($LASTEXITCODE)" }

$out = Join-Path $Root 'dist\app'
if (Test-Path $out) { Get-ChildItem $out -File | Remove-Item -Force }
New-Item -ItemType Directory -Force $out | Out-Null
$release = Join-Path (Get-TargetDir) 'release'
foreach ($n in 'promptlab-app', 'promptlab-app-cli') {
  Copy-Item -Force (Join-Path $release "$n.exe") $out
}
$vs = Get-VsInstallPath
if ($vulkan) {
  # dynamic-link(#98·#111): llama.dll·ggml.dll·ggml-base.dll 은 llama-cpp-sys-2 빌드의 OUT_DIR\bin 에, 백엔드 모듈(ggml-cpu-*·ggml-vulkan)은
  # OUT_DIR\backends 에 있다. target\release 의 하드링크는 build.rs 가 「이미 있으면 건너뛴다」라 낡았거나 없을 수 있다(windows 잡 #28:
  # 설치본 exe 가 llama.dll 을 못 찾아 0xC0000135) — 그래서 이번 Vulkan 빌드 폴더에서 직접 모은다. 앱은 exe 옆에서 백엔드를 읽는다.
  $sys = Get-ChildItem (Join-Path $release 'build') -Directory -Filter 'llama-cpp-sys-2-*' |
    Where-Object { Test-Path (Join-Path $_.FullName 'out\backends\ggml-vulkan.dll') } |
    Sort-Object { (Get-Item (Join-Path $_.FullName 'out\backends\ggml-vulkan.dll')).LastWriteTime } -Descending | Select-Object -First 1
  if (-not $sys) { throw 'ggml-vulkan.dll 이 든 llama-cpp-sys-2 빌드 폴더(out\backends)를 찾지 못했다' }
  $bin = Join-Path $sys.FullName 'out\bin'
  $backends = Join-Path $sys.FullName 'out\backends'
  Write-Host "[build] llama.cpp DLL 출처: $bin · $backends"
  if (-not (Test-Path $bin)) { throw "llama.cpp DLL 폴더가 없다: $bin" }
  Get-ChildItem $bin -Filter '*.dll' -File | Copy-Item -Destination $out -Force
  Get-ChildItem $backends -Filter '*.dll' -File | Copy-Item -Destination $out -Force
  foreach ($d in 'llama.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-vulkan.dll') {
    if (-not (Test-Path (Join-Path $out $d))) { throw "$d 가 dist\app 에 없다" }
  }
  if (-not (Get-ChildItem $out -Filter 'ggml-cpu*.dll')) { throw 'ggml-cpu 백엔드 모듈이 없다 — CPU 대체가 안 된다' }
}

# VC++ 런타임(#111): Rust exe(/MD)와 llama.cpp DLL 이 VCRUNTIME140·MSVCP140 등을, ggml-cpu 가 OpenMP(VCOMP140)를 가져온다.
# 응시자 PC에 VC++ 재배포 패키지가 없을 수 있어 앱 폴더에 함께 둔다(app-local 배포 — VS 의 Redist\MSVC\<판>\x64\Microsoft.VC*.CRT·
# .OpenMP 는 재배포 허용 파일). vc_redist.exe 를 설치 중에 돌리지 않는 까닭: 설치 파일은 사용자별(관리자 권한 없음)인데
# vc_redist 는 관리자 권한을 요구해 UAC 창이 뜨고 무인 설치(스모크)도 막힌다.
$crt = @()
if ($vs) {
  $redist = Get-ChildItem "$vs\VC\Redist\MSVC" -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^\d+\.\d+' } |
    Sort-Object { [version]($_.Name -replace '[^\d.].*$', '') } -Descending | Select-Object -First 1
  if ($redist) {
    $crt = @(Get-ChildItem (Join-Path $redist.FullName 'x64') -Directory -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match '^Microsoft\.VC\d+\.(CRT|OpenMP)$' } | ForEach-Object { Get-ChildItem $_.FullName -Filter '*.dll' -File })
  }
}
if (-not $crt) { throw 'VC++ 재배포 DLL(VS\VC\Redist\MSVC\<판>\x64\Microsoft.VC*.CRT)을 찾지 못했다 — VS Build Tools 에 「C++ x64 도구」(Redist 포함)가 있어야 한다' }
$crt | Copy-Item -Destination $out -Force
Write-Host "[build] VC++ 런타임 app-local $($crt.Count)개: $(($crt.Name) -join ', ')"

# 함정: exe 가 vulkan-1.dll 을 직접 가져오면 Vulkan 런타임이 없는 PC에서 앱이 아예 안 뜬다.
# vulkan-1 은 ggml-vulkan.dll(동적으로 읽는 모듈)만 가져와야 한다 — exe 의 의존 DLL 목록을 검사한다.
# 그리고(#111) dist\app 의 exe·DLL 이 가져오는 DLL 이 모두 ① dist\app 안에 있거나 ② OS DLL(System32·api-ms-win-*/ext-ms-*)인지 본다.
# VC++ 런타임(vcruntime·msvcp·concrt·vccorlib·vcomp)은 빌드 호스트 System32 에 있어도 응시자 PC엔 없을 수 있어 dist 에 있어야 한다.
# vulkan-1.dll 은 ggml-vulkan.dll 만 가져와도 되고 GPU 드라이버가 깐다(없으면 그 모듈만 못 읽고 CPU 로 돈다 — docs/windows-host.md).
$dumpbin = $null
if ($vs) { $dumpbin = Get-ChildItem "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1 }
if (-not $dumpbin) { throw 'dumpbin 을 찾지 못했다 — exe·DLL 의존 검사를 할 수 없다(VS C++ x64 도구)' }
$present = @{}
Get-ChildItem $out -File | ForEach-Object { $present[$_.Name.ToLowerInvariant()] = $true }
$sys32 = Join-Path $env:SystemRoot 'System32'
$bad = @()
foreach ($f in Get-ChildItem $out -File | Where-Object { $_.Extension -in '.exe', '.dll' }) {
  $deps = @(Invoke-Native { & $dumpbin.FullName /nologo /dependents $f.FullName } | ForEach-Object { if ("$_" -match '^\s+(\S+\.dll)\s*$') { $Matches[1].ToLowerInvariant() } })
  if ($LASTEXITCODE -ne 0) { throw "dumpbin /dependents $($f.Name) 실패 ($LASTEXITCODE)" }
  foreach ($d in $deps) {
    if ($d -eq 'vulkan-1.dll') {
      if ($f.Extension -eq '.exe') { throw "$($f.Name) 가 vulkan-1.dll 에 직접 링크됐다 — Vulkan 없는 PC에서 안 뜬다" }
      continue
    }
    if ($present.ContainsKey($d)) { continue }
    if ($d -match '^(api|ext)-ms-') { continue }
    $isCrt = $d -match '^(vcruntime|msvcp|concrt|vccorlib|vcomp)\d'
    if (-not $isCrt -and (Test-Path (Join-Path $sys32 $d))) { continue }
    $bad += "$($f.Name) → $d"
  }
}
if ($bad) { throw "dist\app 에서 못 찾는 의존 DLL(설치본이 0xC0000135 로 안 뜬다):`n  $($bad -join "`n  ")" }
Write-Host '[build] exe·DLL 의존 DLL 이 모두 dist\app 또는 OS 에 있다(vulkan-1.dll 은 ggml-vulkan 만, 직접 링크 없음 — 확인)'
Get-ChildItem $out | ForEach-Object { Write-Host ("{0}  {1:N1} MB" -f $_.Name, ($_.Length / 1MB)) }
