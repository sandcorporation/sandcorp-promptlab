//! llama.cpp(GGUF Q4) 추론 구현. **응시 앱만** 쓴다(AI 도우미, #98) — 서버·워커에는 모델이 없다.
//!
//! - 모델·문맥·샘플링 고정값은 [`promptlab_shared::inference`]에, 채팅 템플릿은 [`promptlab_shared::ai`]에 있다.
//! - **GPU(Vulkan, #98)**: 기능 `vulkan`으로 구우면 llama.cpp를 동적 백엔드(`GGML_BACKEND_DL`)로 빌드한다 — 백엔드가
//!   `ggml-vulkan`·`ggml-cpu-*` 모듈(Windows DLL)로 따로 나오고 [`backend`]가 실행 파일 옆에서 읽어 들인다. Vulkan 런타임
//!   (`vulkan-1.dll`)이 없는 PC에서는 `ggml-vulkan` 모듈만 못 읽고 지나가 CPU로 돈다(실행 파일은 `vulkan-1.dll`에 링크되지
//!   않는다). GPU 장치가 보이면 모든 레이어를 GPU에 올린다. 기능 없이 구우면(리눅스 CI·Vulkan SDK 없는 노드) CPU만.
//! - 추론은 탐욕적 선택(온도 0) — 같은 장치·같은 입력이면 같은 답(재현성 시험). GPU와 CPU의 답은 글자까지 같다고 보장하지 않는다.
//! - 문맥 32k: 긴 접두는 [`BATCH_TOKENS`]씩 나눠 처리한다.
//! - [`LlamaInference::load_with_prefix_cache`](앱)는 **호출 사이에도** 마지막 접두의 KV를 남긴다. 대화는 차례마다 앞이
//!   그대로 접두가 되므로 새 접두가 남긴 접두로 시작하면 늘어난 뒷부분만 처리한다. 요약으로 접두가 바뀌면 그때 한 번 다시 처리한다.

mod os_gpu;

use std::io::Read;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LlamaBackendDeviceType, list_llama_ggml_backend_devices};
use promptlab_shared::inference::{
    FlashAttn, GenStats, Generation, Inference, InferenceError, KvType, LARGEST_MODEL, MODELS,
    ModelPin, RuntimeConfig, SPILL_TOKENS_PER_SEC, SamplingParams, best_probe, choose_runtime,
    next_probe, usable_gpu_memory,
};
use promptlab_shared::model_choice::{GpuMemory, Machine};
use sha2::{Digest, Sha256};

/// 모델 캐시 디렉터리: `PROMPTLAB_MODEL_DIR` → `$XDG_CACHE_HOME/promptlab/models` → (Windows) `%LOCALAPPDATA%\promptlab\models`
/// → `~/.cache/promptlab/models`. `scripts/fetch-model.sh`·`scripts/windows/fetch-model.ps1`도 같은 순서로 고른다.
pub fn model_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PROMPTLAB_MODEL_DIR") {
        return PathBuf::from(dir);
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            // Windows에는 HOME이 없다 — 설치 파일이 모델을 받아 두는 자리.
            cfg!(windows)
                .then(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
                .flatten()
        })
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    cache.join("promptlab").join("models")
}

/// 후보 모델 파일의 캐시 경로(캐시 디렉터리 + [`ModelPin::file`]).
pub fn model_path(model: &ModelPin) -> PathBuf {
    model_dir().join(model.file)
}

/// 캐시에 받아 둔 후보 모델의 id(큰 것부터). 크기만 본다 — sha256은 올릴 때([`verify_model_file`]) 잰다.
pub fn downloaded_models() -> Vec<&'static str> {
    MODELS
        .iter()
        .filter(|m| std::fs::metadata(model_path(m)).is_ok_and(|md| md.len() == m.size))
        .map(|m| m.id)
        .collect()
}

/// 경로의 파일 이름으로 후보 모델을 찾는다.
pub fn model_for_path(path: &Path) -> Option<&'static ModelPin> {
    let name = path.file_name()?.to_str()?;
    promptlab_shared::inference::model_by_file(name)
}

/// 파일 sha256을 재어 후보 표([`MODELS`])의 값인지 본다 — 파일 이름이 후보의 이름이면 바로 그 후보의 값이어야 한다.
/// 4.7GB를 읽으면 수십 초 걸릴 수 있다.
pub fn verify_model_file(path: &Path) -> Result<String, InferenceError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| InferenceError::Model(format!("{}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| InferenceError::Model(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ok = match model_for_path(path) {
        Some(m) => m.sha256 == actual,
        None => MODELS.iter().any(|m| m.sha256 == actual),
    };
    if !ok {
        return Err(InferenceError::Model(format!(
            "sha256 불일치: {} ({}의 고정값이 아니다)",
            actual,
            path.display()
        )));
    }
    Ok(actual)
}

/// llama.cpp 백엔드는 프로세스에 하나만 띄울 수 있다.
/// llama.cpp·ggml 로그를 stderr로 낼지(#116 진단 — `promptlab-app-cli --bench-ai`만 켠다). 백엔드를 처음 올리기 **전에**
/// 불러야 한다(앱은 끈다 — 화면 앱에는 stderr가 없고 로그가 크다).
pub fn set_llama_log(on: bool) {
    LLAMA_LOG.store(on, Ordering::Relaxed);
}

static LLAMA_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn backend() -> Result<&'static LlamaBackend, InferenceError> {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    static INIT: Mutex<()> = Mutex::new(());
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    // 두 스레드가 동시에 처음 올리면 두 번째 init이 `BackendAlreadyInitialized`로 실패한다 — 한 줄로 세운다.
    let _guard = INIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    load_dynamic_backends();
    let mut b = LlamaBackend::init().map_err(|e| InferenceError::Failed(e.to_string()))?;
    // 끄지 않으면 llama.cpp 기본 로거가 stderr로 낸다: 장치(ggml_vulkan: fp16·int dot·matrix cores)·레이어 배치·
    // 버퍼 크기·graph splits(#116 진단).
    if !LLAMA_LOG.load(Ordering::Relaxed) {
        b.void_logs();
    }
    Ok(BACKEND.get_or_init(|| b))
}

/// 동적 백엔드(기능 `vulkan`)면 백엔드 모듈을 읽어 들인다: 실행 파일 옆(설치본) → 빌드 때 자리(`cargo run`·시험).
/// 모듈 하나가 못 읽히면(예: `vulkan-1.dll` 없음) llama.cpp가 그 모듈만 건너뛴다 — 남은 CPU 모듈로 돈다.
#[cfg(feature = "vulkan")]
fn load_dynamic_backends() {
    // SAFETY: 장치 수 조회는 읽기만 한다(레지스트리는 llama.cpp가 처음 부를 때 만든다).
    let count = || unsafe { llama_cpp_sys_2::ggml_backend_dev_count() };
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        llama_cpp_2::llama_backend::load_backends_from_path(&dir);
    }
    if count() == 0 {
        llama_cpp_2::llama_backend::load_backends();
    }
}

#[cfg(not(feature = "vulkan"))]
fn load_dynamic_backends() {}

/// 이 PC에서 추론이 쓸 장치(응시 전 점검의 「AI 속도」 행).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeInfo {
    /// 레이어를 올릴 GPU 이름(Vulkan 장치 설명). 없으면 CPU로 돈다.
    pub gpu: Option<String>,
    /// 시스템 메모리 합(바이트, CPU 장치가 알려 준 값). 모르면 `None`.
    pub ram_bytes: Option<u64>,
}

/// 백엔드를 올리고 장치를 본다(모델은 올리지 않는다 — 빠르다).
pub fn compute_info() -> Result<ComputeInfo, InferenceError> {
    backend()?;
    let devices = list_llama_ggml_backend_devices();
    let gpu = devices
        .iter()
        .find(|d| {
            matches!(
                d.device_type,
                LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu
            )
        })
        .map(|d| {
            if d.description.is_empty() {
                d.name.clone()
            } else {
                d.description.clone()
            }
        });
    let ram_bytes = devices
        .iter()
        .find(|d| d.device_type == LlamaBackendDeviceType::Cpu)
        .map(|d| d.memory_total as u64)
        .filter(|&b| b > 0);
    Ok(ComputeInfo { gpu, ram_bytes })
}

/// 이 바이너리에 GPU(Vulkan) 백엔드 모듈이 구워졌나(기능 `vulkan`). 아니면 CPU만 쓰는 빌드다.
pub const GPU_BUILD: bool = cfg!(feature = "vulkan");

/// llama.cpp가 본 장치 하나 — 설치본이 GPU로 도는지 확인하는 출력용(`promptlab-app-cli --check-gpu`, #111).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// ggml 백엔드 이름(예: `Vulkan`·`CPU`).
    pub backend: String,
    /// 장치 이름(예: `Vulkan0`).
    pub name: String,
    /// 장치 설명(예: GPU 모델 이름).
    pub description: String,
    /// 레이어를 올릴 수 있는 GPU(외장·내장)인가 — [`compute_info`]가 고르는 기준과 같다.
    pub gpu: bool,
    /// 장치 메모리 합(바이트, 모르면 0).
    pub memory_total: u64,
}

/// 백엔드를 올리고(동적 모듈 읽기 포함) llama.cpp가 본 장치를 모두 돌려준다. 모델은 올리지 않는다.
pub fn devices() -> Result<Vec<DeviceInfo>, InferenceError> {
    backend()?;
    Ok(list_llama_ggml_backend_devices()
        .into_iter()
        .map(|d| DeviceInfo {
            gpu: matches!(
                d.device_type,
                LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu
            ),
            backend: d.backend,
            name: d.name,
            description: d.description,
            memory_total: d.memory_total as u64,
        })
        .collect())
}

/// 레이어를 올릴 GPU의 (여유, 전체) 메모리 바이트(#116 — 진단·[`choose_runtime`]). GPU가 없거나 모르면 `None`.
pub fn gpu_memory() -> Result<Option<(u64, u64)>, InferenceError> {
    backend()?;
    Ok(list_llama_ggml_backend_devices()
        .into_iter()
        .find(|d| {
            matches!(
                d.device_type,
                LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu
            )
        })
        .map(|d| (d.memory_free as u64, d.memory_total as u64))
        .filter(|&(_, total)| total > 0))
}

/// 모델 크기 추천의 입력(#131 — [`promptlab_shared::model_choice::recommend`]): GPU의 전체·쓸 수 있는 메모리
/// (올릴 때 [`choose_runtime`]이 쓰는 값과 같다 — llama.cpp 여유와 OS가 센 사용량 중 작은 쪽)와 여유 RAM.
pub fn machine() -> Result<Machine, InferenceError> {
    backend()?;
    let memory = if compute_info()?.gpu.is_some() {
        gpu_memory()?
    } else {
        None
    };
    let os_used = memory.and(os_gpu::dedicated_usage_bytes());
    let free_ram = list_llama_ggml_backend_devices()
        .into_iter()
        .find(|d| d.device_type == LlamaBackendDeviceType::Cpu)
        .map_or(0, |d| d.memory_free as u64);
    Ok(Machine {
        gpu: memory.map(|(free, total)| GpuMemory {
            total,
            usable: usable_gpu_memory(free, total, os_used),
        }),
        free_ram,
    })
}

fn vram_mib() -> Option<(u64, u64)> {
    gpu_memory()
        .ok()
        .flatten()
        .map(|(free, total)| (free >> 20, total >> 20))
}

/// 속도 재기(#116 루프 3): 길게 이어지는 답을 내는 짧은 프롬프트, 생성 토큰 수, 앞에서 빼는 준비 토큰 수.
const PROBE_PROMPT: &str = "<|im_start|>user\nCount from 1 to 100, separated by commas.<|im_end|>\n<|im_start|>assistant\n";
const PROBE_TOKENS: u32 = 24;
const PROBE_WARMUP: usize = 4;

/// 한 번에 decode하는 토큰 수(긴 접두는 이만큼씩 나눈다). llama.cpp 물리 배치(n_ubatch) 기본값과 같다.
pub const BATCH_TOKENS: usize = 512;

/// 고정 모델을 올린 llama.cpp 추론기.
pub struct LlamaInference {
    /// 접두 KV 캐시(앱만). **`model`보다 먼저 선언한다** — 컨텍스트가 모델을 가리키므로 먼저 버려져야 한다.
    cache: Option<Mutex<Option<PrefixCache>>>,
    /// 상자에 담아 구조체를 옮겨도 모델 주소가 변하지 않게 한다([`PrefixCache`]의 안전 근거).
    model: Box<LlamaModel>,
    threads: i32,
    /// 문맥 창·KV 타입·flash attention(#116 — GPU 메모리에 맞춰 고른다, CPU는 [`RuntimeConfig::CPU`]).
    runtime: RuntimeConfig,
    /// 올리기 전 GPU 여유·전체(MiB)와 고른 까닭 — 진단 출력.
    load_note: String,
    /// GPU에 올린 레이어 수 / 전체(출력 층 포함)·장치 이름 — 속도 진단(#116).
    gpu_layers: u32,
    layers: u32,
    backend: String,
    /// 자동 설정이 처음 고른 GPU 설정이 잰 속도로는 그래픽 메모리가 모자랐다(넘침 — 레이어를 줄였다) — 앱이 안내한다.
    gpu_short: bool,
    /// 토큰 누계(입력, 출력) — [`Inference::token_totals`].
    input_total: AtomicU64,
    output_total: AtomicU64,
}

/// 앞 호출이 남긴 접두의 KV 캐시: 컨텍스트 하나와 그 안에 처리돼 있는 접두 토큰들.
struct PrefixCache {
    tokens: Vec<LlamaToken>,
    ctx: LlamaContext<'static>,
}

impl LlamaInference {
    /// 모델 파일을 올린다. 파일 해시는 보지 않는다 — 필요하면 먼저 [`verify_model_file`].
    pub fn load(path: &Path) -> Result<Self, InferenceError> {
        Self::load_inner(path, false)
    }

    /// [`load`](Self::load) + 호출 사이 접두 KV 캐시(응시 앱용, 모듈 문서 참고).
    pub fn load_with_prefix_cache(path: &Path) -> Result<Self, InferenceError> {
        Self::load_inner(path, true)
    }

    fn load_inner(path: &Path, prefix_cache: bool) -> Result<Self, InferenceError> {
        Self::load_with(path, prefix_cache, None)
    }

    /// 설정을 정해 올린다(`None` = 자동: GPU에 실제로 쓸 수 있는 메모리로 고르고([`choose_runtime`]·
    /// [`usable_gpu_memory`]), GPU에 올렸으면 짧게 생성 속도를 재어 그래픽 메모리가 넘쳤으면 레이어를 줄여 가며 가장
    /// 빠른 설정을 쓴다(#116 루프 3 — [`next_probe`]). 지정하면 재지 않는다. `promptlab-app-cli --bench-ai`가 비교에 쓴다.
    pub fn load_with(
        path: &Path,
        prefix_cache: bool,
        runtime: Option<RuntimeConfig>,
    ) -> Result<Self, InferenceError> {
        if !path.is_file() {
            return Err(InferenceError::Model(format!(
                "{} 이 없다 — scripts/fetch-model.sh 로 받는다",
                path.display()
            )));
        }
        let gpu = compute_info()?.gpu;
        let memory = if gpu.is_some() { gpu_memory()? } else { None };
        // 다른 앱이 쓰는 몫까지 OS가 센 사용량(Windows만 — 리눅스·macOS는 `None`이라 llama.cpp 값 그대로).
        let os_used = memory.and(os_gpu::dedicated_usage_bytes());
        let usable = memory.map(|(free, total)| usable_gpu_memory(free, total, os_used));
        let model_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        // 후보 모델이면 그 기하(층·KV 폭), 모르는 파일(`--bench-ai <파일>`)이면 가장 큰 후보의 기하에 실제 크기.
        let pin = model_for_path(path).copied().unwrap_or(ModelPin {
            size: model_bytes,
            ..LARGEST_MODEL
        });
        let chosen = runtime.unwrap_or_else(|| choose_runtime(usable, &pin));
        let mib = |b: Option<u64>| b.map_or("-".to_owned(), |b| (b >> 20).to_string());
        let note = match memory {
            Some((free, total)) => format!(
                "vram_free_mib={} vram_total_mib={} os_used_mib={} usable_mib={} model_mib={} chosen=n_ctx:{}/kv:{}/fa:{}/layers:{}{}",
                free >> 20,
                total >> 20,
                mib(os_used),
                mib(usable),
                model_bytes >> 20,
                chosen.n_ctx,
                chosen.kv.as_str(),
                chosen.flash_attn.as_str(),
                chosen
                    .gpu_layers
                    .map_or("all".to_owned(), |l| l.to_string()),
                if runtime.is_some() { " (지정)" } else { "" }
            ),
            None => format!(
                "vram=- (GPU 없음) chosen=n_ctx:{}/kv:{}/fa:{}",
                chosen.n_ctx,
                chosen.kv.as_str(),
                chosen.flash_attn.as_str()
            ),
        };
        let first = Self::load_once(path, prefix_cache, chosen, gpu.as_deref(), note)?;
        if runtime.is_some() || first.gpu_layers == 0 {
            return Ok(first);
        }
        // 올린 직후 속도 재기: 느리면(넘침) 레이어를 줄여 다시 올려 잰다. 앞 모델을 놓은 **뒤에** 다음을 올린다(VRAM).
        let mut tried = Vec::new();
        let mut current = first;
        loop {
            let speed = match current.probe_speed() {
                Ok(s) => s,
                Err(e) => {
                    current.load_note.push_str(&format!(" probe_failed={e}"));
                    0.0
                }
            };
            tried.push((current.gpu_layers, speed));
            let Some(next) = next_probe(&tried) else {
                break;
            };
            let note = std::mem::take(&mut current.load_note);
            drop(current);
            current = Self::load_once(
                path,
                prefix_cache,
                RuntimeConfig::partial(next),
                gpu.as_deref(),
                note,
            )?;
        }
        let (best, speed) = best_probe(&tried).unwrap_or((current.gpu_layers, 0.0));
        if best != current.gpu_layers {
            let note = std::mem::take(&mut current.load_note);
            drop(current);
            current = Self::load_once(
                path,
                prefix_cache,
                RuntimeConfig::partial(best),
                gpu.as_deref(),
                note,
            )?;
        }
        current.gpu_short = tried[0].1 < SPILL_TOKENS_PER_SEC;
        let tried_text: Vec<String> = tried.iter().map(|(l, s)| format!("{l}:{s:.1}")).collect();
        current.load_note.push_str(&format!(
            " probe_layers_tok_s={} picked_layers={best} picked_tok_s={speed:.1} gpu_short={}",
            tried_text.join(","),
            current.gpu_short
        ));
        Ok(current)
    }

    /// 설정 하나로 모델을 올리고 문맥이 열리는지 본다(안 열리면 FA·KV를 차례로 물린다).
    fn load_once(
        path: &Path,
        prefix_cache: bool,
        chosen: RuntimeConfig,
        gpu: Option<&str>,
        mut load_note: String,
    ) -> Result<Self, InferenceError> {
        let load = |layers: u32| {
            let params = LlamaModelParams::default().with_n_gpu_layers(layers);
            LlamaModel::load_from_file(backend()?, path, &params)
                .map_err(|e| InferenceError::Model(e.to_string()))
        };
        // 고른 만큼 레이어를 올린다(None = 전부, 999는 「전부」). GPU 메모리가 모자라 실패하면 CPU로.
        let want = chosen.gpu_layers.unwrap_or(999);
        let (model, mut runtime) = match &gpu {
            Some(_) if want > 0 => match load(want) {
                Ok(m) => (m, chosen),
                Err(e) => {
                    load_note.push_str(&format!(" gpu_load_failed={e}"));
                    (load(0)?, RuntimeConfig::CPU)
                }
            },
            _ => (
                load(0)?,
                // 지정했으면 CPU에서도 그 설정(비교용), 아니면 고른 것(GPU 없으면 CPU 설정).
                RuntimeConfig {
                    gpu_layers: Some(0),
                    ..chosen
                },
            ),
        };
        let layers = model.n_layer() + 1;
        let on_layers = match runtime.gpu_layers {
            None => layers,
            Some(n) => n.min(layers),
        };
        let backend = match (&gpu, on_layers > 0) {
            (Some(name), true) => format!("GPU {name}"),
            (Some(name), false) => format!("CPU (GPU {name} 안 씀)"),
            (None, _) => "CPU".to_owned(),
        };
        // 스레드 수는 속도에만 영향을 준다(ggml CPU 연산은 출력 원소마다 한 스레드가 같은 순서로 더한다).
        // 8을 넘기면 메모리 대역폭에 막혀 빨라지지 않는다.
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(8) as i32;
        // 문맥을 한 번 열어 본다: 고른 설정이 안 열리면(예: 백엔드가 양자화 KV·FA를 못 받음) 차례로 물러난다.
        if runtime != RuntimeConfig::CPU {
            let fallbacks = [
                runtime,
                RuntimeConfig {
                    flash_attn: FlashAttn::Auto,
                    ..runtime
                },
                RuntimeConfig {
                    kv: KvType::F16,
                    flash_attn: FlashAttn::Auto,
                    ..runtime
                },
            ];
            let mut opened = None;
            for cfg in fallbacks {
                match open_context(&model, cfg, threads) {
                    Ok(_) => {
                        opened = Some(cfg);
                        break;
                    }
                    Err(e) => load_note.push_str(&format!(
                        " ctx_failed(kv:{}/fa:{})={e}",
                        cfg.kv.as_str(),
                        cfg.flash_attn.as_str()
                    )),
                }
            }
            runtime = opened
                .ok_or_else(|| InferenceError::Failed(format!("문맥을 열 수 없다: {load_note}")))?;
        }
        Ok(Self {
            cache: prefix_cache.then(|| Mutex::new(None)),
            model: Box::new(model),
            threads,
            runtime,
            load_note,
            gpu_layers: on_layers,
            layers,
            backend,
            gpu_short: false,
            input_total: AtomicU64::new(0),
            output_total: AtomicU64::new(0),
        })
    }
}

impl Inference for LlamaInference {
    fn complete(
        &self,
        prefix: &str,
        suffixes: &[&str],
        params: &SamplingParams,
    ) -> Result<Vec<String>, InferenceError> {
        check_params(params)?;
        let vocab = self.model.vocab();
        let prefix_tokens = vocab.tokenize(prefix.as_bytes(), false, true);
        let suffix_tokens: Vec<Vec<LlamaToken>> = suffixes
            .iter()
            .map(|s| vocab.tokenize(s.as_bytes(), false, true))
            .collect();
        for suffix in &suffix_tokens {
            if suffix.is_empty() {
                return Err(InferenceError::Unsupported("빈 suffix".into()));
            }
            let needed = prefix_tokens.len() + suffix.len() + params.max_tokens as usize;
            if needed > self.runtime.n_ctx as usize {
                return Err(InferenceError::TooLong {
                    tokens: needed,
                    limit: self.runtime.n_ctx,
                });
            }
        }
        if suffix_tokens.is_empty() {
            return Ok(Vec::new());
        }

        match &self.cache {
            None => {
                let mut ctx = self.new_context()?;
                self.run(&mut ctx, &prefix_tokens, 0, &suffix_tokens, params)
            }
            Some(slot) => self.run_cached(slot, &prefix_tokens, &suffix_tokens, params),
        }
    }

    fn count_tokens(&self, text: &str) -> Result<usize, InferenceError> {
        // complete와 같은 방식(특수 토큰을 토큰으로, BOS 없음).
        Ok(self
            .model
            .vocab()
            .tokenize(text.as_bytes(), false, true)
            .len())
    }

    fn stream(
        &self,
        prefix: &str,
        suffix: &str,
        params: &SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<Generation, InferenceError> {
        check_params(params)?;
        let vocab = self.model.vocab();
        let mut prompt = vocab.tokenize(prefix.as_bytes(), false, true);
        let suffix_tokens = vocab.tokenize(suffix.as_bytes(), false, true);
        if suffix_tokens.is_empty() {
            return Err(InferenceError::Unsupported("빈 suffix".into()));
        }
        prompt.extend(suffix_tokens);
        let needed = prompt.len() + params.max_tokens as usize;
        if needed > self.runtime.n_ctx as usize {
            return Err(InferenceError::TooLong {
                tokens: needed,
                limit: self.runtime.n_ctx,
            });
        }
        match &self.cache {
            None => {
                let mut ctx = self.new_context()?;
                self.generate(&mut ctx, &prompt, 0, params, on_text)
                    .map(|(g, _)| g)
            }
            Some(slot) => {
                let mut guard = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // 마지막 토큰은 로짓을 얻으려 늘 다시 처리하므로 그 앞까지만 겹침을 본다.
                let (mut ctx, common) = self.reuse(guard.take(), &prompt[..prompt.len() - 1])?;
                let (generation, out) =
                    self.generate(&mut ctx, &prompt, common, params, on_text)?;
                // 접두·suffix·답까지 KV에 그대로 있다 — 다음 차례의 접두는 이 대화에 답 하나를 더 붙인 것이라 거의 다 다시 쓴다.
                prompt.extend(out);
                *guard = Some(PrefixCache {
                    tokens: prompt,
                    ctx,
                });
                Ok(generation)
            }
        }
    }

    fn context_tokens(&self) -> u32 {
        self.runtime.n_ctx
    }

    fn token_totals(&self) -> Option<(u64, u64)> {
        Some((
            self.input_total.load(Ordering::Relaxed),
            self.output_total.load(Ordering::Relaxed),
        ))
    }
}

impl LlamaInference {
    /// 접두 중 앞 `already`개는 `ctx`에 이미 처리돼 있다고 보고 나머지 접두 → suffix마다 생성한다.
    fn run(
        &self,
        ctx: &mut LlamaContext<'_>,
        prefix_tokens: &[LlamaToken],
        already: usize,
        suffix_tokens: &[Vec<LlamaToken>],
        params: &SamplingParams,
    ) -> Result<Vec<String>, InferenceError> {
        let failed = |e: &dyn std::fmt::Display| InferenceError::Failed(e.to_string());
        let mut batch = LlamaBatch::new(BATCH_TOKENS, 1);
        // 1단계: 접두 — 한 번만. 로짓은 필요 없다(다음 토큰은 suffix 끝에서 고른다).
        if prefix_tokens.len() > already {
            decode_chunks(ctx, &mut batch, &prefix_tokens[already..], already, false)?;
        }
        let keep = u32::try_from(prefix_tokens.len()).map_err(|e| failed(&e))?;
        let mut outputs = Vec::with_capacity(suffix_tokens.len());
        let mut input = prefix_tokens.len().saturating_sub(already) as u64;
        let mut output = 0u64;
        for suffix in suffix_tokens {
            // 2단계: 접두 뒤(앞 suffix와 그 답)를 지우고 이 suffix부터.
            ctx.clear_kv_cache_seq(Some(0), Some(keep), None)
                .map_err(|e| failed(&e))?;
            decode_chunks(ctx, &mut batch, suffix, prefix_tokens.len(), true)?;
            let (out, _, _) = self.sample_until_end(
                ctx,
                &mut batch,
                prefix_tokens.len() + suffix.len(),
                params,
                &mut |_, _| true,
            )?;
            input += suffix.len() as u64;
            output += out.len() as u64;
            outputs.push(self.detokenize(&out));
        }
        // 실제로 처리한 토큰(접두는 한 번, 캐시에서 이어받은 부분은 빼고).
        self.input_total.fetch_add(input, Ordering::Relaxed);
        self.output_total.fetch_add(output, Ordering::Relaxed);
        Ok(outputs)
    }

    /// [`run`](Self::run)을 앞 호출이 남긴 접두 KV 위에서 돌린다: 새 접두가 남긴 접두로 시작하면 늘어난 뒷부분만 처리한다.
    /// 끝나면 접두까지만 남기고(suffix·답은 지운다) 다음 호출에 넘긴다. 오류가 나면 캐시를 버린다.
    fn run_cached(
        &self,
        slot: &Mutex<Option<PrefixCache>>,
        prefix_tokens: &[LlamaToken],
        suffix_tokens: &[Vec<LlamaToken>],
        params: &SamplingParams,
    ) -> Result<Vec<String>, InferenceError> {
        let failed = |e: &dyn std::fmt::Display| InferenceError::Failed(e.to_string());
        let mut guard = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut ctx, common) = self.reuse(guard.take(), prefix_tokens)?;
        let outputs = self.run(&mut ctx, prefix_tokens, common, suffix_tokens, params)?;
        let keep = u32::try_from(prefix_tokens.len()).map_err(|e| failed(&e))?;
        if ctx.clear_kv_cache_seq(Some(0), Some(keep), None).is_ok() {
            *guard = Some(PrefixCache {
                tokens: prefix_tokens.to_vec(),
                ctx,
            });
        }
        Ok(outputs)
    }

    /// 남긴 KV에서 `prompt`와 겹치는 앞부분만 남긴 컨텍스트와 그 길이(없거나 지우지 못하면 새 컨텍스트, 0).
    fn reuse(
        &self,
        cached: Option<PrefixCache>,
        prompt: &[LlamaToken],
    ) -> Result<(LlamaContext<'static>, usize), InferenceError> {
        let Some(mut cached) = cached else {
            return Ok((self.new_context_static()?, 0));
        };
        let common = cached
            .tokens
            .iter()
            .zip(prompt)
            .take_while(|(a, b)| a == b)
            .count();
        let from = u32::try_from(common).map_err(|e| InferenceError::Failed(e.to_string()))?;
        // 겹치는 앞부분만 남긴다(새 접두가 이어지지 않으면 0 = 전부 지운다).
        match cached.ctx.clear_kv_cache_seq(Some(0), Some(from), None) {
            Ok(_) => Ok((cached.ctx, common)),
            Err(_) => Ok((self.new_context_static()?, 0)),
        }
    }

    /// `prompt` 중 앞 `already`개가 `ctx`에 처리돼 있다고 보고 나머지를 처리한 뒤 토큰마다 `on_text`로 알리며 생성한다.
    /// 답 원문과 생성 토큰(모두 KV에 들어가 있다)을 돌려준다.
    fn generate(
        &self,
        ctx: &mut LlamaContext<'_>,
        prompt: &[LlamaToken],
        already: usize,
        params: &SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<(Generation, Vec<LlamaToken>), InferenceError> {
        // 마지막 토큰의 로짓이 있어야 첫 토큰을 고른다 — 프롬프트 전체가 남아 있어도 마지막 하나는 다시 처리한다.
        let already = already.min(prompt.len().saturating_sub(1));
        let mut batch = LlamaBatch::new(BATCH_TOKENS, 1);
        ctx.reset_timings();
        let t0 = std::time::Instant::now();
        decode_chunks(ctx, &mut batch, &prompt[already..], already, true)?;
        // GPU decode는 비동기로 끝날 수 있다 — 로짓을 읽어(동기화) 프롬프트 처리 시간에 넣는다(#116: 안 그러면 생성 시간에 섞였다).
        let _ = ctx.get_logits_ith(batch.n_tokens() - 1);
        let prompt_ms = t0.elapsed().as_millis() as u64;
        let t1 = std::time::Instant::now();
        let (out, stopped, split) =
            self.sample_until_end(ctx, &mut batch, prompt.len(), params, on_text)?;
        let timings = ctx.timings();
        let generate_ms = t1.elapsed().as_millis() as u64;
        self.input_total
            .fetch_add((prompt.len() - already) as u64, Ordering::Relaxed);
        self.output_total
            .fetch_add(out.len() as u64, Ordering::Relaxed);
        let stats = GenStats {
            prompt_tokens: prompt.len(),
            reused_tokens: already,
            prompt_ms,
            generated_tokens: out.len(),
            generate_ms,
            n_ctx: ctx.n_ctx(),
            n_batch: ctx.n_batch(),
            n_ubatch: ctx.n_ubatch(),
            flash_attn: self.runtime.flash_attn.as_str(),
            kv: self.runtime.kv.as_str(),
            decode_ms: split.decode_ms,
            sample_ms: split.sample_ms,
            text_ms: split.text_ms,
            llama_prompt_ms: timings.t_p_eval_ms(),
            llama_prompt_tokens: timings.n_p_eval().max(0) as usize,
            llama_eval_ms: timings.t_eval_ms(),
            llama_eval_tokens: timings.n_eval().max(0) as usize,
            vram_mib: if self.gpu_layers > 0 {
                vram_mib()
            } else {
                None
            },
            gpu_layers: self.gpu_layers,
            layers: self.layers,
            backend: self.backend.clone(),
            threads: self.threads,
        };
        let generation = Generation {
            text: self.detokenize(&out),
            stopped,
            stats: Some(stats),
        };
        Ok((generation, out))
    }

    fn new_context_static(&self) -> Result<LlamaContext<'static>, InferenceError> {
        let ctx = self.new_context()?;
        // SAFETY: 컨텍스트는 `self.model`(상자 안, 주소가 바뀌지 않는다)만 가리키고, 이 컨텍스트를 담는 `cache`는
        // 구조체에서 `model`보다 먼저 선언돼 모델보다 먼저 버려진다. 상자 밖으로 내보내지 않는다.
        Ok(unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(ctx) })
    }
}

fn check_params(params: &SamplingParams) -> Result<(), InferenceError> {
    if params.temperature != 0.0 {
        return Err(InferenceError::Unsupported(format!(
            "온도 {} — 재현성을 위해 0만 받는다",
            params.temperature
        )));
    }
    Ok(())
}

/// `tokens`를 위치 `start`부터 [`BATCH_TOKENS`]씩 decode한다. `logits_last`면 마지막 토큰만 로짓을 낸다
/// (배치에는 마지막 조각이 남는다 — 이어서 그 로짓으로 고른다).
fn decode_chunks(
    ctx: &mut LlamaContext<'_>,
    batch: &mut LlamaBatch,
    tokens: &[LlamaToken],
    start: usize,
    logits_last: bool,
) -> Result<(), InferenceError> {
    let chunks = tokens.chunks(BATCH_TOKENS).count();
    for (i, chunk) in tokens.chunks(BATCH_TOKENS).enumerate() {
        batch.clear();
        add_tokens(
            batch,
            chunk,
            start + i * BATCH_TOKENS,
            logits_last && i + 1 == chunks,
        )?;
        ctx.decode(batch)
            .map_err(|e| InferenceError::Failed(e.to_string()))?;
    }
    Ok(())
}

/// `tokens`를 위치 `start`부터 배치에 싣는다. `logits_last`면 마지막 토큰만 로짓을 낸다.
fn add_tokens(
    batch: &mut LlamaBatch,
    tokens: &[LlamaToken],
    start: usize,
    logits_last: bool,
) -> Result<(), InferenceError> {
    let failed = |e: &dyn std::fmt::Display| InferenceError::Failed(e.to_string());
    let last = tokens.len().saturating_sub(1);
    for (i, token) in tokens.iter().enumerate() {
        let pos = i32::try_from(start + i).map_err(|e| failed(&e))?;
        batch
            .add(*token, pos, &[0], logits_last && i == last)
            .map_err(|e| failed(&e))?;
    }
    Ok(())
}

impl LlamaInference {
    fn new_context(&self) -> Result<LlamaContext<'_>, InferenceError> {
        open_context(&self.model, self.runtime, self.threads)
    }

    /// 실행 설정(문맥 창·KV·FA·GPU 레이어).
    pub fn runtime(&self) -> RuntimeConfig {
        self.runtime
    }

    /// 그래픽 메모리가 모자라 처음 고른 GPU 설정이 느렸다(다른 앱이 VRAM을 쓰는 중 — 레이어를 줄여 가장 빠른 것을 쓴다).
    pub fn gpu_short(&self) -> bool {
        self.gpu_short
    }

    /// GPU에 올린 레이어 수와 전체(출력 층 포함).
    pub fn gpu_layers(&self) -> (u32, u32) {
        (self.gpu_layers, self.layers)
    }

    /// 짧게 생성해 속도(tok/s)를 잰다 — 실제 문맥(KV 포함)을 열고, 첫 [`PROBE_WARMUP`]토큰(그래프·셰이더 준비)은 빼고
    /// 나머지 decode 간격으로 잰다. 토큰 누계에는 넣지 않는다.
    pub fn probe_speed(&self) -> Result<f64, InferenceError> {
        let mut ctx = self.new_context()?;
        let prompt = self
            .model
            .vocab()
            .tokenize(PROBE_PROMPT.as_bytes(), false, true);
        let params = SamplingParams {
            max_tokens: PROBE_TOKENS,
            ..SamplingParams::default()
        };
        let mut marks: Vec<std::time::Instant> = Vec::new();
        let result = self.generate(&mut ctx, &prompt, 0, &params, &mut |_, _| {
            marks.push(std::time::Instant::now());
            true
        });
        self.input_total.store(0, Ordering::Relaxed);
        self.output_total.store(0, Ordering::Relaxed);
        result?;
        let (Some(from), Some(to)) = (marks.get(PROBE_WARMUP - 1), marks.last()) else {
            return Err(InferenceError::Failed(
                "속도를 잴 만큼 생성하지 않았다".into(),
            ));
        };
        let secs = to.duration_since(*from).as_secs_f64();
        let tokens = (marks.len() - PROBE_WARMUP) as f64;
        if tokens < 1.0 || secs <= 0.0 {
            return Err(InferenceError::Failed(
                "속도를 잴 만큼 생성하지 않았다".into(),
            ));
        }
        Ok(tokens / secs)
    }

    /// 올릴 때의 GPU 메모리·고른 설정·물러난 까닭(진단 한 줄).
    pub fn load_note(&self) -> &str {
        &self.load_note
    }

    /// 방금 decode한 배치의 마지막 로짓부터 탐욕적으로 고른다(끝 토큰·최대 토큰까지). `next_pos` = 다음 토큰 위치.
    /// 토큰마다 `on_text(지금까지의 온전한 UTF-8 글, 토큰 수)`를 부르고 `false`면 멈춘다(`true` = 멈춤).
    fn sample_until_end(
        &self,
        ctx: &mut LlamaContext<'_>,
        batch: &mut LlamaBatch,
        next_pos: usize,
        params: &SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<(Vec<LlamaToken>, bool, GenSplit), InferenceError> {
        let failed = |e: &dyn std::fmt::Display| InferenceError::Failed(e.to_string());
        let vocab = self.model.vocab();
        let mut sampler = LlamaSampler::greedy();
        let mut out = Vec::new();
        let mut split = GenSplit::default();
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
        let start = i32::try_from(next_pos).map_err(|e| failed(&e))?;
        for pos in (start..).take(params.max_tokens as usize) {
            // sample()이 고른 토큰을 샘플러에 accept까지 한다.
            let t = std::time::Instant::now();
            let token = sampler.sample(ctx, batch.n_tokens() - 1);
            split.sample_ms += ms(t);
            if vocab.is_eog(token) {
                break;
            }
            out.push(token);
            let t = std::time::Instant::now();
            batch.clear();
            batch.add(token, pos, &[0], true).map_err(|e| failed(&e))?;
            ctx.decode(batch).map_err(|e| failed(&e))?;
            // 로짓을 읽어 GPU 계산이 끝나기를 기다린다 — decode 시간과 샘플링 시간을 가른다(#116 진단).
            let _ = ctx.get_logits_ith(batch.n_tokens() - 1);
            split.decode_ms += ms(t);
            let t = std::time::Instant::now();
            // 전체를 다시 풀어 온전한 글자까지만 알린다(토큰 하나씩 풀면 여러 토큰에 걸친 한글이 깨진다). 1024토큰이면 가볍다.
            let bytes = vocab.detokenize(&out, false, true);
            let text = match std::str::from_utf8(&bytes) {
                Ok(t) => t,
                Err(e) => std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or_default(),
            };
            let go_on = on_text(text, out.len());
            split.text_ms += ms(t);
            if !go_on {
                return Ok((out, true, split));
            }
        }
        Ok((out, false, split))
    }

    /// 토큰 하나씩이 아니라 한꺼번에 풀어야 여러 토큰에 걸친 UTF-8(한글 등)이 깨지지 않는다.
    /// 최대 토큰에서 끊겨 글자 중간이 잘리면 그 조각만 U+FFFD로 바뀐다(결정적).
    fn detokenize(&self, tokens: &[LlamaToken]) -> String {
        // unparse_special: `<tool_call>` 같은 특수(추가) 토큰도 글로 풀어야 도구 호출을 읽는다(#98).
        let bytes = self.model.vocab().detokenize(tokens, false, true);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// 설정대로 문맥을 연다. CPU 설정([`RuntimeConfig::CPU`])은 #98과 같은 파라미터(KV·FA는 llama.cpp 기본)를 넘겨 출력이 같다.
fn open_context(
    model: &LlamaModel,
    runtime: RuntimeConfig,
    threads: i32,
) -> Result<LlamaContext<'_>, InferenceError> {
    let mut ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(runtime.n_ctx))
        .with_n_batch(BATCH_TOKENS as u32)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    if runtime != RuntimeConfig::CPU {
        let kv = match runtime.kv {
            KvType::F16 => KvCacheType::F16,
            KvType::Q8_0 => KvCacheType::Q8_0,
        };
        let fa = match runtime.flash_attn {
            FlashAttn::Auto => llama_cpp_sys_2::LLAMA_FLASH_ATTN_TYPE_AUTO,
            FlashAttn::On => llama_cpp_sys_2::LLAMA_FLASH_ATTN_TYPE_ENABLED,
            FlashAttn::Off => llama_cpp_sys_2::LLAMA_FLASH_ATTN_TYPE_DISABLED,
        };
        // KV(와 KQV 연산)를 GPU 레이어와 같은 장치에 둔다 — llama.cpp 기본이지만 진단을 위해 못 박는다.
        ctx_params = ctx_params
            .with_type_k(kv)
            .with_type_v(kv)
            .with_flash_attention_policy(fa)
            .with_offload_kqv(true);
    }
    model
        .new_context(backend()?, ctx_params)
        .map_err(|e| InferenceError::Failed(e.to_string()))
}

/// 생성 시간 가르기(밀리초): decode(GPU 동기화까지)·샘플링·글 풀기와 알림.
#[derive(Debug, Default, Clone, Copy)]
struct GenSplit {
    decode_ms: f64,
    sample_ms: f64,
    text_ms: f64,
}
