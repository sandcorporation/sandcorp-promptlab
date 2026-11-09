//! AI 도우미 모델 크기 추천(#131): 이 PC의 그래픽 메모리·여유 RAM과 이미 받은 모델로 후보 표
//! [`MODELS`](크기 순)에서 하나를 고르고 실행 설정과 사람 말 이유를 준다. 화면·CLI(`--recommend-model`)가 같은 함수를 쓴다.
//!
//! 규칙(#131 결정 B — 대표 2026-11-09):
//! 1. **기본은 가장 큰 모델(7B)** — GPU에 다 들어가면 전부, 모자라면 들어가는 층만 GPU에 올리고 나머지는 CPU,
//!    GPU가 없으면 CPU로. 층 수·문맥은 [`choose_runtime`](「모델 + KV + [`crate::inference::GPU_RESERVE_BYTES`]」)이
//!    정하고, 쓸 수 있는 그래픽 메모리는 [`crate::inference::usable_gpu_memory`](다른 앱 몫을 뺀 값) 그대로 받는다.
//! 2. 그 실행 설정에서 **CPU에 남는 몫([`ram_need`] = CPU 층의 가중치 + CPU 층의 KV + [`RAM_RESERVE_BYTES`])이 여유
//!    RAM에 들어가야** 올릴 수 있다. 7B: CPU 전부면 4.4GB + 32k f16 KV 1.75GB + 1GB ≈ 7.1GB, 그래픽 8GB 중 여유 5GB면
//!    21/29층이 GPU라 ≈ 2.4GB. 7B를 못 올리면 다음으로 큰 후보(1.5B)를 고른다 — 그것도 안 들어가면 가장 작은 것을
//!    고르고 「다른 앱을 닫으라」를 붙인다(막지 않는다 — RAM은 응시 전 점검이 따로 경고한다).
//! 3. 제한적인 모델([`crate::inference::ModelPin::limited`] — 1.5B)은 추천이든 직접 고른 것이든 「제한적 — 답을 못 할 수
//!    있음」 표시를 단다(`display_name`). 1.5B는 빠르지만(#116: 204 tok/s) 도구 호출·한국어 답을 자주 못 했다
//!    (#131 루프 2 실제 모델 3/5, Windows 잡 #39 JS 빈 틀 두 질문 모두 답 없음) — 느려도 답하는 7B가 낫다는 결정이다.
//!
//! 받기 크기·예상 시간은 고르는 데 쓰지 않고 이유에만 싣는다(이미 받은 모델이면 0) — 한 번 받으면 끝이다.

use crate::inference::{MODELS, ModelPin, RuntimeConfig, choose_runtime, kv_bytes};

/// 이보다 큰 모델을 CPU(또는 일부 층만 GPU)로 돌리면 이유에 「답이 느릴 수 있음」을 붙인다 — 7B(4.4GB)는 CPU 몫이
/// 있으면 초당 5토큰 안팎(#116 측정: 넘친 4060 Ti에서 0층 5.3·10층 7.9 tok/s), 1.5B(1.0GB)는 CPU로도 빠르다.
pub const SLOW_CPU_MODEL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// CPU 몫 말고 남길 RAM(계산 버퍼·앱 자신·웹뷰).
pub const RAM_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;

/// 받기 예상 시간에 쓰는 속도(100Mbps = 초당 12.5MB). 실제 속도는 받는 동안 진행 막대가 보여 준다.
pub const ASSUMED_DOWNLOAD_BYTES_PER_SEC: u64 = 12_500_000;

/// 그래픽 메모리(바이트).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMemory {
    /// 전용 메모리 전체.
    pub total: u64,
    /// 지금 쓸 수 있는 몫([`crate::inference::usable_gpu_memory`]).
    pub usable: u64,
}

/// 추천의 입력: 이 PC의 사양.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    /// 레이어를 올릴 GPU(없거나 모르면 `None`).
    pub gpu: Option<GpuMemory>,
    /// 지금 여유 RAM(바이트).
    pub free_ram: u64,
}

/// 모델 하나를 이 PC에서 돌리는 계획.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub model: &'static ModelPin,
    /// [`choose_runtime`]이 고를 실행 설정(앱이 올릴 때 다시 잰 메모리로 한 번 더 고른다).
    pub runtime: RuntimeConfig,
    /// 더 받아야 하는 바이트(받아 둔 모델이면 0).
    pub download_bytes: u64,
    /// 사람 말 이유(한 줄).
    pub reason: String,
}

impl Plan {
    /// 층 전부를 GPU에 올리는가.
    pub fn full_gpu(&self) -> bool {
        self.runtime.gpu_layers.is_none()
    }
}

/// 실행 설정 `runtime`으로 `model`을 올릴 때 CPU(RAM)에 남는 몫: GPU에 안 올린 층의 가중치와 KV + [`RAM_RESERVE_BYTES`].
/// 층 전부를 GPU에 올리면 [`RAM_RESERVE_BYTES`]뿐이다.
pub fn ram_need(model: &ModelPin, runtime: &RuntimeConfig) -> u64 {
    let total = model.gpu_layers() as u64;
    let on_gpu = runtime.gpu_layers.map_or(total, |n| (n as u64).min(total));
    let cpu_layers = total - on_gpu;
    let weights = model.size / total * cpu_layers;
    let kv = kv_bytes(
        runtime.n_ctx,
        runtime.kv,
        cpu_layers.min(model.layers),
        model.kv_width,
    );
    weights + kv + RAM_RESERVE_BYTES
}

/// CPU로만 돌릴 때 드는 RAM(모델 + 32k f16 KV + [`RAM_RESERVE_BYTES`]).
pub fn cpu_ram_need(model: &ModelPin) -> u64 {
    ram_need(model, &RuntimeConfig::CPU)
}

/// 이 PC에서 `model`을 올릴 수 있나 — 층 전부가 GPU에 들어가거나, CPU에 남는 몫이 여유 RAM에 들어간다.
pub fn fits(model: &ModelPin, machine: &Machine) -> bool {
    let runtime = choose_runtime(machine.gpu.map(|g| g.usable), model);
    runtime.gpu_layers.is_none() || ram_need(model, &runtime) <= machine.free_ram
}

/// 이 PC에 맞는 모델을 추천한다(위 규칙). `downloaded` = 캐시에 이미 있는 모델의 [`ModelPin::id`].
pub fn recommend(machine: &Machine, downloaded: &[&str]) -> Plan {
    let model = MODELS
        .iter()
        .find(|m| fits(m, machine))
        .unwrap_or(&MODELS[MODELS.len() - 1]);
    plan(model, machine, downloaded, true)
}

/// 고른 모델(추천이든 응시자가 고른 것이든)의 계획과 이유. `recommended`면 더 큰 모델을 왜 못 올리는지도 적는다.
pub fn plan(
    model: &'static ModelPin,
    machine: &Machine,
    downloaded: &[&str],
    recommended: bool,
) -> Plan {
    let usable = machine.gpu.map(|g| g.usable);
    let runtime = choose_runtime(usable, model);
    let download_bytes = if downloaded.contains(&model.id) {
        0
    } else {
        model.size
    };
    let mut parts = Vec::new();
    match machine.gpu {
        Some(g) => parts.push(format!(
            "그래픽 메모리 {} 중 여유 {}",
            gb(g.total),
            gb(g.usable)
        )),
        None => parts.push("GPU 없음".to_owned()),
    }
    if recommended {
        for bigger in MODELS.iter().take_while(|m| m.id != model.id) {
            let need = ram_need(bigger, &choose_runtime(usable, bigger));
            parts.push(format!(
                "여유 RAM {}로는 {}를 올릴 수 없음(필요 {})",
                gb(machine.free_ram),
                bigger.label,
                gb(need)
            ));
        }
    }
    let slow = model.size > SLOW_CPU_MODEL_BYTES;
    match runtime.gpu_layers {
        None => parts.push(format!("{}를 GPU에 전부 올립니다", model.label)),
        Some(0) => parts.push(format!(
            "{}를 CPU로 돌립니다{}",
            model.label,
            if slow {
                "(답이 느릴 수 있음)"
            } else {
                ""
            }
        )),
        Some(n) => parts.push(format!(
            "{}의 {}/{}층만 GPU, 나머지는 CPU{}",
            model.label,
            n,
            model.gpu_layers(),
            if slow {
                "(답이 다소 느릴 수 있음)"
            } else {
                ""
            }
        )),
    }
    if !fits(model, machine) {
        parts.push(format!(
            "여유 RAM {}로는 빠듯합니다 — 다른 앱을 닫으세요",
            gb(machine.free_ram)
        ));
    }
    parts.push(download_text(download_bytes));
    Plan {
        model,
        runtime,
        download_bytes,
        reason: parts.join(" · "),
    }
}

/// 「받기 1.1GB(100Mbps에서 약 2분)」 또는 「받아 둠」.
pub fn download_text(bytes: u64) -> String {
    if bytes == 0 {
        return "받아 둠".to_owned();
    }
    let secs = bytes.div_ceil(ASSUMED_DOWNLOAD_BYTES_PER_SEC);
    format!("받기 {}(100Mbps에서 약 {})", gb(bytes), duration(secs))
}

/// 바이트를 「6.0GB」(GiB, 소수 한 자리 — Windows 작업 관리자와 같은 단위)로.
pub fn gb(bytes: u64) -> String {
    format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}초")
    } else {
        format!("{}분", secs.div_ceil(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 시간_문구() {
        assert_eq!(duration(5), "5초");
        assert_eq!(duration(61), "2분");
    }
}
