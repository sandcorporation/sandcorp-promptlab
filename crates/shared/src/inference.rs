//! 추론 트레잇과 **고정값 한 곳**: 모델(저장소·파일·리비전·sha256)·문맥 길이·샘플링 설정.
//!
//! 모델은 **응시 앱만** 쓴다(AI 도우미, spec #93 5절). 서버·워커에는 모델이 없다 — 채점은 판정기가 한다.
//! 구현은 `promptlab-inference`(llama.cpp), 시험에서는 가짜 LLM을 넣는다. 채팅 템플릿(Qwen2.5 ChatML + 도구 호출)은
//! [`crate::ai`]가 만든다.
//!
//! `scripts/fetch-model.sh --model <id>`는 이 파일의 [`MODELS`] 표를 읽어 내려받고 sha256을 검증한다 —
//! 모델을 더하거나 바꿀 때는 이 표만 고친다(허용 목록·설치·추천이 따라가고, 재현성 시험의 기대 출력은 모델마다 바뀐다).

/// 고정한 모델 하나(후보 표 [`MODELS`]의 한 줄).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPin {
    /// 짧은 이름(`7b`·`1.5b`) — 설정 파일·CLI `--model`·스크립트가 쓴다.
    pub id: &'static str,
    /// 사람에게 보이는 이름.
    pub label: &'static str,
    /// Hugging Face 저장소.
    pub hf_repo: &'static str,
    /// 저장소 안 GGUF 파일 이름(캐시 디렉터리에도 이 이름으로 둔다).
    pub file: &'static str,
    /// 저장소 커밋(리비전). `main`이 아니라 커밋으로 고정한다.
    pub revision: &'static str,
    /// 파일 sha256(소문자 16진).
    pub sha256: &'static str,
    /// 파일 크기(바이트).
    pub size: u64,
    /// 반복 층 수(llama.cpp `n_gpu_layers`는 여기에 출력 층 1을 더해 센다).
    pub layers: u64,
    /// 층마다 K(또는 V)의 폭 = KV 헤드 수 × 헤드 폭(128).
    pub kv_width: u64,
    /// 제한적인 모델(#131 결정 B) — 도구 호출·한국어 답을 자주 못 해 「답을 못 할 수 있음」 표시를 붙인다(점검 화면 추천 줄·
    /// 고르기 목록·고를 때 안내). 추천은 제한적이지 않은 모델을 올릴 수 없는 PC에서만 이것을 고른다.
    pub limited: bool,
}

/// 제한적인 모델([`ModelPin::limited`]) 이름 뒤에 붙는 표시.
pub const LIMITED_NOTE: &str = "제한적 — 답을 못 할 수 있음";

impl ModelPin {
    /// llama.cpp가 `n_gpu_layers`로 세는 층 수(반복 층 + 출력 층).
    pub const fn gpu_layers(&self) -> u32 {
        self.layers as u32 + 1
    }

    /// 내려받는 주소(리비전 고정).
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.hf_repo, self.revision, self.file
        )
    }

    /// 화면·CLI에 보이는 이름 — 제한적이면 「Qwen2.5-Coder 1.5B(제한적 — 답을 못 할 수 있음)」.
    pub fn display_name(&self) -> String {
        if self.limited {
            format!("{}({LIMITED_NOTE})", self.label)
        } else {
            self.label.to_owned()
        }
    }
}

/// 응시 앱 AI 도우미의 **후보 모델 표, 큰 것부터**(#131). 모두 Qwen2.5-Coder Instruct GGUF Q4_K_M, Apache-2.0.
/// 3B는 Qwen Research License(비상업 전용)라 넣지 않았다. 1.5B는 제한적([`ModelPin::limited`]). 앱이 PC 사양으로 하나를 추천하고([`crate::model_choice`])
/// 응시자가 바꿀 수 있다. 문항 시작 때 앱이 고른 파일의 sha256을 신고하고 서버가 허용 목록([`crate::integrity`] —
/// 이 표의 sha256 전부)과 대조해 모델 이름으로 푼다([`model_by_sha256`]).
pub const MODELS: [ModelPin; 2] = [
    ModelPin {
        id: "7b",
        label: "Qwen2.5-Coder 7B",
        hf_repo: "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
        file: "qwen2.5-coder-7b-instruct-q4_k_m.gguf",
        revision: "13fb94bfda8c8cf22497dc57b78f391a9acb426a",
        sha256: "509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c",
        size: 4_683_073_536,
        layers: 28,
        kv_width: 512,
        limited: false,
    },
    ModelPin {
        id: "1.5b",
        label: "Qwen2.5-Coder 1.5B",
        hf_repo: "Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF",
        file: "qwen2.5-coder-1.5b-instruct-q4_k_m.gguf",
        revision: "f86cb2c1fa58255f8052cc32aeede1b7482d4361",
        sha256: "cc324af070c2ecbfd324a30884d2f951a7ff756aba85cb811a6ec436933bb046",
        size: 1_117_320_768,
        layers: 28,
        kv_width: 256,
        // 실제 모델 시험(#131 루프 2): 3/5, Windows 잡 #39에서 JS 빈 틀 두 질문 모두 답 없음.
        limited: true,
    },
];

/// 가장 큰 후보 — 사양을 모를 때·모르는 파일의 기하(층·KV 폭)를 셀 때 쓴다.
pub const LARGEST_MODEL: ModelPin = MODELS[0];

/// 짧은 이름으로 찾는다.
pub fn model_by_id(id: &str) -> Option<&'static ModelPin> {
    MODELS.iter().find(|m| m.id == id)
}

/// sha256(대소문자 무시)으로 찾는다 — 서버가 문항 시작 신고를 모델 이름으로 풀 때.
pub fn model_by_sha256(sha256: &str) -> Option<&'static ModelPin> {
    MODELS
        .iter()
        .find(|m| m.sha256.eq_ignore_ascii_case(sha256))
}

/// 「쓴 모델」 표시 이름(#131): 후보면 그 이름, 아니면(운영자가 허용 목록에 더한 값 등) sha256 앞 12자를 붙인 「알 수 없는 모델」.
pub fn model_name(sha256: &str) -> String {
    match model_by_sha256(sha256) {
        Some(m) => m.label.to_owned(),
        None => {
            let short: String = sha256.chars().take(12).collect();
            format!("알 수 없는 모델 ({short}…)")
        }
    }
}

/// 파일 이름으로 찾는다(캐시 경로의 마지막 조각).
pub fn model_by_file(file: &str) -> Option<&'static ModelPin> {
    MODELS.iter().find(|m| m.file == file)
}

/// 추론 문맥 길이(토큰, spec #93 — 32k). 프롬프트 + 답이 이 안에 들어가야 한다. 넘치기 전에
/// [`crate::ai::Chat`]이 오래된 턴을 요약한다. KV 캐시(f16)는 32k 토큰에 7B 약 1.8GB·1.5B 약 0.9GB다.
pub const CONTEXT_TOKENS: u32 = 32_768;

/// 응시 앱 최소 사양(spec #93): RAM 16GB. 이보다 작으면 응시 전 점검이 경고한다(막지는 않는다).
pub const MIN_RAM_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// 샘플링 설정. 온도 0(탐욕적 선택)·고정 시드·고정 최대 토큰 — 같은 입력이면 같은 답(재현성 시험).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplingParams {
    /// 0이면 탐욕적 선택(argmax). 지금 구현은 0만 받는다.
    pub temperature: f32,
    /// 온도가 0보다 클 때의 난수 시드. 온도 0에서는 쓰이지 않지만 기록을 위해 고정한다.
    pub seed: u32,
    /// 출력 최대 토큰 수(답 하나 — 코드 편집 제안이 들어갈 만큼).
    pub max_tokens: u32,
}

impl Default for SamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            seed: 0,
            max_tokens: 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("모델 파일 문제: {0}")]
    Model(String),
    #[error("입력이 너무 길다: {tokens}토큰 (컨텍스트 {limit})")]
    TooLong { tokens: usize, limit: u32 },
    #[error("지원하지 않는 설정: {0}")]
    Unsupported(String),
    #[error("추론 실패: {0}")]
    Failed(String),
}

/// KV 캐시 원소 타입(#116 GPU 메모리). CPU는 언제나 [`KvType::F16`](재현성 시험의 출력이 이 설정에서 고정됐다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvType {
    F16,
    /// 8비트(블록 32개마다 2바이트 배율 — 원소당 34/32바이트). llama.cpp에서 V를 양자화하려면 flash attention이 켜져야 한다.
    Q8_0,
}

impl KvType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F16 => "f16",
            Self::Q8_0 => "q8_0",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "f16" => Some(Self::F16),
            "q8_0" | "q8" => Some(Self::Q8_0),
            _ => None,
        }
    }

    /// 원소 32개의 바이트 수.
    fn bytes_per_32(self) -> u64 {
        match self {
            Self::F16 => 64,
            Self::Q8_0 => 34,
        }
    }
}

/// flash attention 설정. `Auto`는 llama.cpp 기본(백엔드가 받으면 켠다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashAttn {
    Auto,
    On,
    Off,
}

impl FlashAttn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// 추론기의 실행 설정(#116): 문맥 창·KV 타입·flash attention·GPU 레이어 수(`None` = 전부).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub n_ctx: u32,
    pub kv: KvType,
    pub flash_attn: FlashAttn,
    pub gpu_layers: Option<u32>,
}

impl RuntimeConfig {
    /// CPU(GPU 없음)의 설정 — #98 그대로(32k·f16·auto). 재현성 시험의 `EXPECTED`가 이 설정의 출력이다.
    pub const CPU: Self = Self {
        n_ctx: CONTEXT_TOKENS,
        kv: KvType::F16,
        flash_attn: FlashAttn::Auto,
        gpu_layers: Some(0),
    };
}

/// GPU에 모델·KV 말고 남겨 둘 메모리: 계산 버퍼(n_ubatch 512 — 출력 로짓만 152k×512×4 ≈ 0.3GB, FA 켬 기준)와
/// 다른 앱이 쓰다 늘리는 몫. 넘치면 Windows(WDDM)가 조용히 공유 시스템 메모리로 넘겨 PCIe로 돌아 CPU보다 느려진다(#116 측정).
pub const GPU_RESERVE_BYTES: u64 = 1536 * 1024 * 1024;

/// 앱 문맥 창의 하한 — 상태 블록(코드 12,000자 ≈ 4k토큰)·도구 안내·답 자리를 넣고도 대화가 남게.
pub const MIN_CONTEXT_TOKENS: u32 = 16_384;

/// KV 캐시 바이트(층 `layers`개만큼, K와 V, 층마다 폭 `kv_width`).
pub fn kv_bytes(n_ctx: u32, kv: KvType, layers: u64, kv_width: u64) -> u64 {
    n_ctx as u64 * layers * 2 * kv_width * kv.bytes_per_32() / 32
}

/// GPU 여유 메모리(모델을 올리기 전)에 맞춰 설정을 고른다(#116). 차례대로 「모델 + KV + [`GPU_RESERVE_BYTES`]」가
/// 들어가는 첫 것: 32k f16 → 32k q8_0 → 16k q8_0(모두 레이어 전부, FA 켬). 다 안 들어가면 16k q8_0으로 들어가는 만큼만
/// 레이어를 올리고, 한 층도 안 들어가면 [`RuntimeConfig::CPU`]. `free = None`(GPU 없음·모름)도 CPU.
/// 모델 크기·층·KV 폭은 `model`([`MODELS`]의 한 줄)에서 읽는다.
pub fn choose_runtime(free: Option<u64>, model: &ModelPin) -> RuntimeConfig {
    let (model_bytes, layers) = (model.size, model.gpu_layers());
    let Some(free) = free else {
        return RuntimeConfig::CPU;
    };
    let gpu = |n_ctx, kv, gpu_layers| RuntimeConfig {
        n_ctx,
        kv,
        flash_attn: FlashAttn::On,
        gpu_layers,
    };
    for (n_ctx, kv) in [
        (CONTEXT_TOKENS, KvType::F16),
        (CONTEXT_TOKENS, KvType::Q8_0),
        (MIN_CONTEXT_TOKENS, KvType::Q8_0),
    ] {
        if model_bytes + kv_bytes(n_ctx, kv, model.layers, model.kv_width) + GPU_RESERVE_BYTES
            <= free
        {
            return gpu(n_ctx, kv, None);
        }
    }
    let layers = layers.max(1) as u64;
    let per_layer =
        model_bytes / layers + kv_bytes(MIN_CONTEXT_TOKENS, KvType::Q8_0, 1, model.kv_width);
    let fit = free.saturating_sub(GPU_RESERVE_BYTES) / per_layer.max(1);
    if fit == 0 {
        return RuntimeConfig::CPU;
    }
    gpu(
        MIN_CONTEXT_TOKENS,
        KvType::Q8_0,
        Some(fit.min(layers - 1) as u32),
    )
}

impl RuntimeConfig {
    /// 레이어 `n`개만 GPU에 올리는 설정(16k q8_0·FA 켬 — [`choose_runtime`]의 일부 레이어와 같다). `0`이면 [`Self::CPU`].
    pub fn partial(n: u32) -> Self {
        if n == 0 {
            return Self::CPU;
        }
        Self {
            n_ctx: MIN_CONTEXT_TOKENS,
            kv: KvType::Q8_0,
            flash_attn: FlashAttn::On,
            gpu_layers: Some(n),
        }
    }
}

/// 실제로 쓸 수 있는 GPU 메모리(#116 루프 3): llama.cpp가 읽는 여유(`free`, Vulkan 메모리 예산 — Windows에서는
/// 다른 프로세스가 쓰는 몫을 빼지 못했다: 다른 앱이 3.3GB를 쓰는데 7181MiB로 읽었다)와 OS 전체 사용량
/// (`os_used`, Windows 작업 관리자의 「전용 GPU 메모리」 = PDH `\GPU Adapter Memory(*)\Dedicated Usage`)으로
/// 계산한 여유(`total − os_used`) 중 작은 쪽. OS 값을 모르면 `free` 그대로.
pub fn usable_gpu_memory(free: u64, total: u64, os_used: Option<u64>) -> u64 {
    match os_used {
        Some(used) => free.min(total.saturating_sub(used)),
        None => free,
    }
}

/// 올린 직후 잰 생성 속도가 이보다 느리면 그래픽 메모리가 넘친 것으로 본다(#116 루프 3: 4060 Ti에서 7B 가중치가
/// 시스템 메모리로 내려가 PCIe로 돌면 2.7 tok/s, 제대로 올라가면 40 이상 — 1.5B 전 층은 204).
pub const SPILL_TOKENS_PER_SEC: f64 = 10.0;

/// 속도 재기 횟수 상한(올리기 + 짧은 생성 — 한 번에 수 초).
pub const PROBE_MAX_TRIES: usize = 4;

/// 다음에 잴 GPU 레이어 수(`tried` = 지금까지 잰 (레이어 수, tok/s), 처음 것이 고른 설정). 마지막 것이
/// [`SPILL_TOKENS_PER_SEC`] 이상이거나 CPU(0)까지 쟀거나 [`PROBE_MAX_TRIES`]번이면 `None`(그만 — [`best_probe`]를 쓴다).
/// 사다리는 처음 레이어 수의 2/3 → 1/3 → 0(넘친 4060 Ti 측정: 0층 5.3·10층 7.9·18층 5.1·24층 3.6 tok/s — 넘칠 때는
/// 일부만 올리는 쪽이 빠르고 어디가 가장 빠른지는 재 봐야 안다).
pub fn next_probe(tried: &[(u32, f64)]) -> Option<u32> {
    let (&(first, _), &(last, speed)) = (tried.first()?, tried.last()?);
    if speed >= SPILL_TOKENS_PER_SEC || last == 0 || tried.len() >= PROBE_MAX_TRIES {
        return None;
    }
    [first * 2 / 3, first / 3, 0]
        .into_iter()
        .find(|&n| n < last)
}

/// 잰 것 중 가장 빠른 (레이어 수, tok/s). 같으면 먼저 잰 것(레이어가 많은 쪽).
pub fn best_probe(tried: &[(u32, f64)]) -> Option<(u32, f64)> {
    tried.iter().copied().fold(None, |best, t| match best {
        Some((_, s)) if s >= t.1 => best,
        _ => Some(t),
    })
}

/// 추론 하나: 접두 뒤에 이어 붙일 조각들로 답을 만든다. 같은 (prefix, suffix, params)면 같은 출력을 내야 한다.
///
/// AI 도우미는 대화 전체를 [`crate::ai::Prompt`]로 나눠(`prefix` = 앞 메시지들, `suffix` = 새 메시지 + assistant 차례)
/// 넘긴다 — 구현이 앞 호출의 접두 KV를 남겨 두면 늘어난 뒷부분만 처리한다.
pub trait Inference {
    /// `prefix`를 한 번 처리하고 `suffixes` 각각을 그 뒤에 이어 생성한다(접두 KV 재사용). 답은 `suffixes` 순서대로.
    fn complete(
        &self,
        prefix: &str,
        suffixes: &[&str],
        params: &SamplingParams,
    ) -> Result<Vec<String>, InferenceError>;

    /// 글의 토큰 수(템플릿을 덧붙이지 않고 그대로, 특수 토큰은 하나로).
    fn count_tokens(&self, text: &str) -> Result<usize, InferenceError>;

    /// 문맥 창(토큰). 실제 추론기는 GPU 메모리에 맞춰 [`CONTEXT_TOKENS`]보다 작게 열 수 있다([`choose_runtime`]) — 앱의
    /// 요약 예산이 이 값을 따른다.
    fn context_tokens(&self) -> u32 {
        CONTEXT_TOKENS
    }

    /// 지금까지 처리한 (입력, 출력) 토큰 누계. 모르면 `None`.
    fn token_totals(&self) -> Option<(u64, u64)> {
        None
    }

    /// 접두 + suffix 하나로 답을 만들며 **토큰마다** `on_text(지금까지의 글, 생성 토큰 수)`를 부른다(#116 스트리밍).
    /// 글은 늘 온전한 UTF-8 앞부분이다(여러 토큰에 걸친 글자는 다 나올 때까지 기다린다). `on_text`가 `false`를 돌려주면
    /// 생성을 멈추고 그때까지의 글을 `stopped = true`로 돌려준다. 같은 입력이면 끝까지 간 답은 [`complete`](Self::complete)와 같다.
    ///
    /// 기본 구현은 `complete`로 한꺼번에 만들고 한 번만 알린다(시험의 가짜 추론).
    fn stream(
        &self,
        prefix: &str,
        suffix: &str,
        params: &SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<Generation, InferenceError> {
        let text = self
            .complete(prefix, &[suffix], params)?
            .pop()
            .unwrap_or_default();
        on_text(&text, 0);
        Ok(Generation {
            text,
            stopped: false,
            stats: None,
        })
    }
}

/// [`Inference::stream`]의 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct Generation {
    /// 답 원문(멈췄으면 그때까지).
    pub text: String,
    /// 응시자가 멈췄다(`on_text`가 `false`).
    pub stopped: bool,
    /// 속도 진단(#116 — 실제 모델만).
    pub stats: Option<GenStats>,
}

/// 생성 하나의 속도 진단(#116): 앱 로그(`<앱 데이터>/ai-perf.log`)와 `promptlab-app-cli --bench-ai`가 찍는다.
#[derive(Debug, Clone, PartialEq)]
pub struct GenStats {
    /// 프롬프트(접두 + suffix) 토큰 수.
    pub prompt_tokens: usize,
    /// 그중 앞 호출이 남긴 KV를 다시 쓴 토큰 수(처리하지 않았다).
    pub reused_tokens: usize,
    /// 새로 처리한 프롬프트 토큰의 처리 시간(밀리초).
    pub prompt_ms: u64,
    /// 생성한 토큰 수.
    pub generated_tokens: usize,
    /// 생성 시간(밀리초, 첫 토큰부터 끝까지).
    pub generate_ms: u64,
    /// 문맥 창(n_ctx).
    pub n_ctx: u32,
    /// 한 번에 decode하는 토큰 수(n_batch)·물리 배치(n_ubatch).
    pub n_batch: u32,
    pub n_ubatch: u32,
    /// flash attention 설정(`auto`·`on`·`off`).
    pub flash_attn: &'static str,
    /// KV 캐시 타입(`f16`·`q8_0`).
    pub kv: &'static str,
    /// 생성 시간 가르기(밀리초, #116): 토큰마다 decode(GPU 계산 끝까지)·샘플링·글 풀기와 화면 알림의 합.
    pub decode_ms: f64,
    pub sample_ms: f64,
    pub text_ms: f64,
    /// llama.cpp가 잰 프롬프트 처리·생성 decode 시간과 토큰 수(`llama_perf_context`).
    pub llama_prompt_ms: f64,
    pub llama_prompt_tokens: usize,
    pub llama_eval_ms: f64,
    pub llama_eval_tokens: usize,
    /// 생성이 끝난 뒤 GPU 여유·전체 메모리(MiB, GPU가 없으면 `None`).
    pub vram_mib: Option<(u64, u64)>,
    /// GPU에 올린 레이어 수 / 전체(출력 층 포함).
    pub gpu_layers: u32,
    pub layers: u32,
    /// 레이어를 올린 장치(예: `Vulkan: NVIDIA GeForce RTX 4060 Ti`, 없으면 `CPU`).
    pub backend: String,
    /// CPU 스레드 수.
    pub threads: i32,
}

impl GenStats {
    /// 초당 토큰(시간이 0이면 0).
    fn rate(tokens: usize, ms: u64) -> f64 {
        if ms == 0 {
            0.0
        } else {
            tokens as f64 * 1000.0 / ms as f64
        }
    }

    /// 새로 처리한 프롬프트 토큰의 초당 처리 수.
    pub fn prompt_tok_s(&self) -> f64 {
        Self::rate(
            self.prompt_tokens.saturating_sub(self.reused_tokens),
            self.prompt_ms,
        )
    }

    /// 초당 생성 토큰.
    pub fn generate_tok_s(&self) -> f64 {
        Self::rate(self.generated_tokens, self.generate_ms)
    }

    /// 로그 한 줄(`key=value` 공백 구분).
    pub fn line(&self) -> String {
        format!(
            "prompt_tokens={} reused={} prompt_ms={} prompt_tok_s={:.1} generated={} generate_ms={} generate_tok_s={:.1} \
n_ctx={} n_batch={} n_ubatch={} flash_attn={} kv={} gpu_layers={}/{} threads={} backend={:?} vram_free_mib={} vram_total_mib={} \
decode_ms_per_tok={:.1} sample_ms_per_tok={:.2} text_ms_per_tok={:.2} llama_pp={}tok/{:.0}ms llama_tg={}tok/{:.0}ms",
            self.prompt_tokens,
            self.reused_tokens,
            self.prompt_ms,
            self.prompt_tok_s(),
            self.generated_tokens,
            self.generate_ms,
            self.generate_tok_s(),
            self.n_ctx,
            self.n_batch,
            self.n_ubatch,
            self.flash_attn,
            self.kv,
            self.gpu_layers,
            self.layers,
            self.threads,
            self.backend,
            self.vram_mib.map_or("-".to_owned(), |v| v.0.to_string()),
            self.vram_mib.map_or("-".to_owned(), |v| v.1.to_string()),
            self.per_token(self.decode_ms),
            self.per_token(self.sample_ms),
            self.per_token(self.text_ms),
            self.llama_prompt_tokens,
            self.llama_prompt_ms,
            self.llama_eval_tokens,
            self.llama_eval_ms,
        )
    }

    /// 생성 토큰 하나당 밀리초(생성이 없으면 0).
    pub fn per_token(&self, ms: f64) -> f64 {
        if self.generated_tokens == 0 {
            0.0
        } else {
            ms / self.generated_tokens as f64
        }
    }
}
