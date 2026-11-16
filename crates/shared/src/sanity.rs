//! 잰 값의 범위 검증(#26). 응시 앱이 llama.cpp(동적 백엔드 모듈)·OS에서 읽는 장치 이름·메모리·생성 속도는 FFI 너머의
//! 값이라 드라이버·모듈 판이 어긋나면 주소 같은 거대값·빈 이름·제어 문자가 그대로 올 수 있다(Windows 11·외장 GPU에서
//! 점검 화면에 `70190099686700`이 보였다). 말이 안 되는 값은 화면에 그대로 내지 않고 [`MEASURE_FAILED`]로 보이고,
//! 모델 추천([`crate::model_choice`])에는 「모름」으로 넣고, 서버는 신고값([`crate::log::AiSpeed`])을 저장하지 않는다.
//!
//! 모두 UI 없는 순수 함수다 — 시험 `crates/shared/tests/sanity.rs`.

/// 말이 안 되는 값 대신 보이는 말.
pub const MEASURE_FAILED: &str = "측정 실패";

/// 시스템 메모리(RAM)로 받아들이는 범위(바이트): 256MiB ~ 16TiB.
pub const RAM_BYTES: std::ops::RangeInclusive<u64> = (256 << 20)..=(16 << 40);

/// GPU 전용 메모리 전체로 받아들이는 범위(바이트): 128MiB ~ 1TiB.
pub const GPU_BYTES: std::ops::RangeInclusive<u64> = (128 << 20)..=(1 << 40);

/// 생성 속도(토큰/초)의 상한 — 이 앱의 후보 모델(1.5B·7B)은 데스크톱 GPU에서 수백을 넘지 않는다(#116 측정: 1.5B 전 층 204).
pub const MAX_TOKENS_PER_SEC: f64 = 2_000.0;

/// 모델 층 수 상한(후보 모델은 29층 안팎).
pub const MAX_LAYERS: u32 = 1_024;

/// 생성 토큰 누계 상한(한 응시 문항 안 — 넉넉히).
pub const MAX_GENERATED_TOKENS: u64 = 100_000_000;

/// 생성 시간 누계 상한(밀리초, 7일 — 문항 제한 시간보다 훨씬 길다).
pub const MAX_GENERATE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// 장치 이름 길이 상한(글자).
pub const MAX_DEVICE_NAME_CHARS: usize = 128;

/// RAM 바이트가 범위([`RAM_BYTES`]) 안이면 그대로.
pub fn ram_bytes(bytes: u64) -> Option<u64> {
    RAM_BYTES.contains(&bytes).then_some(bytes)
}

/// GPU (여유, 전체) 바이트: 전체가 [`GPU_BYTES`] 안이고 여유가 전체 이하일 때만.
pub fn gpu_memory(free: u64, total: u64) -> Option<(u64, u64)> {
    (GPU_BYTES.contains(&total) && free <= total).then_some((free, total))
}

/// OS가 센 GPU 사용량: 전체 이하일 때만(넘으면 잘못 읽은 값 — 버리고 llama.cpp 여유만 쓴다).
pub fn gpu_used(used: u64, total: u64) -> Option<u64> {
    (used <= total).then_some(used)
}

/// 생성 속도(토큰/초): 유한하고 0보다 크고 [`MAX_TOKENS_PER_SEC`] 이하일 때만.
pub fn tokens_per_sec(v: f64) -> Option<f64> {
    (v.is_finite() && v > 0.0 && v <= MAX_TOKENS_PER_SEC).then_some(v)
}

/// 화면에 낼 장치 이름: 앞뒤 공백을 걷고, 비었거나·너무 길거나·제어 문자가 있거나·글자가 하나도 없으면(숫자·기호만 —
/// 주소처럼 생긴 값) `None`.
pub fn device_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_DEVICE_NAME_CHARS
        && !name.chars().any(char::is_control)
        && name.chars().any(char::is_alphabetic)
        && !looks_like_hex_address(name);
    ok.then(|| name.to_owned())
}

/// `0x3fd667f5652c`처럼 16진 주소로만 된 이름.
fn looks_like_hex_address(name: &str) -> bool {
    let hex = name
        .strip_prefix("0x")
        .or_else(|| name.strip_prefix("0X"))
        .unwrap_or(name);
    hex.len() >= 8 && hex.chars().all(|c| c.is_ascii_hexdigit())
}
