//! GPU 메모리에 맞춘 추론 실행 설정(#116): RTX 4060 Ti(8GB)에서 32k f16 KV가 넘쳐 공유 메모리로 돌아
//! CPU보다 느렸다(처리 34 tok/s·생성 1.4 tok/s). 여유 메모리에 들어가는 설정을 고른다.

use promptlab_shared::ai::Budget;
use promptlab_shared::inference::{
    CONTEXT_TOKENS, FlashAttn, GPU_RESERVE_BYTES, KvType, LARGEST_MODEL, MIN_CONTEXT_TOKENS,
    ModelPin, RuntimeConfig, best_probe, choose_runtime, kv_bytes, next_probe, usable_gpu_memory,
};

const GIB: u64 = 1024 * 1024 * 1024;
const MODEL: ModelPin = LARGEST_MODEL;
const LAYERS: u32 = MODEL.gpu_layers();
const MODEL_LAYERS: u64 = MODEL.layers;
const W: u64 = MODEL.kv_width;

#[test]
fn kv_크기는_7b_32k_f16이_약_1_75gib이고_q8_0은_약_절반이다() {
    let f16 = kv_bytes(CONTEXT_TOKENS, KvType::F16, MODEL_LAYERS, W);
    assert_eq!(f16, 32_768 * 28 * 2 * 512 * 2);
    let q8 = kv_bytes(CONTEXT_TOKENS, KvType::Q8_0, MODEL_LAYERS, W);
    assert_eq!(q8 * 64, f16 * 34);
}

#[test]
fn gpu가_없으면_cpu_설정_그대로다() {
    assert_eq!(choose_runtime(None, &MODEL), RuntimeConfig::CPU);
    assert_eq!(RuntimeConfig::CPU.n_ctx, CONTEXT_TOKENS);
    assert_eq!(RuntimeConfig::CPU.kv, KvType::F16);
    assert_eq!(RuntimeConfig::CPU.flash_attn, FlashAttn::Auto);
}

#[test]
fn 넉넉하면_32k_f16_레이어_전부_fa_켬() {
    let c = choose_runtime(Some(12 * GIB), &MODEL);
    assert_eq!(
        c,
        RuntimeConfig {
            n_ctx: CONTEXT_TOKENS,
            kv: KvType::F16,
            flash_attn: FlashAttn::On,
            gpu_layers: None,
        }
    );
}

#[test]
fn 빈_8gb_카드는_32k_q8_0으로_전부_올린다() {
    // 4060 Ti 8GB가 거의 비었을 때(여유 7.5GiB): 모델 4.36GiB + 32k f16 1.75GiB + 여유 1.5GiB는 넘친다.
    let c = choose_runtime(Some(7 * GIB + GIB / 2), &MODEL);
    assert_eq!(c.n_ctx, CONTEXT_TOKENS);
    assert_eq!(c.kv, KvType::Q8_0);
    assert_eq!(c.flash_attn, FlashAttn::On);
    assert_eq!(c.gpu_layers, None);
}

#[test]
fn 조금_모자라면_16k_q8_0으로_줄인다() {
    let need = MODEL.size
        + kv_bytes(MIN_CONTEXT_TOKENS, KvType::Q8_0, MODEL_LAYERS, W)
        + GPU_RESERVE_BYTES;
    let c = choose_runtime(Some(need), &MODEL);
    assert_eq!(
        (c.n_ctx, c.kv, c.gpu_layers),
        (MIN_CONTEXT_TOKENS, KvType::Q8_0, None)
    );
}

#[test]
fn 모델도_안_들어가면_들어가는_만큼만_레이어를_올린다() {
    // 대장 PC 측정: 8GB 중 3.4GB를 다른 앱이 써 여유 약 4.6GB — 모델(4.68GB)조차 안 들어간다.
    let free = 4_600_000_000;
    let c = choose_runtime(Some(free), &MODEL);
    assert_eq!((c.n_ctx, c.kv), (MIN_CONTEXT_TOKENS, KvType::Q8_0));
    let layers = c.gpu_layers.expect("일부만");
    assert!((1..LAYERS).contains(&layers), "{layers}");
    // 올린 층의 무게 + KV + 여유가 여유 메모리 안이다.
    let used = MODEL.size / LAYERS as u64 * layers as u64
        + kv_bytes(MIN_CONTEXT_TOKENS, KvType::Q8_0, layers as u64, W)
        + GPU_RESERVE_BYTES;
    assert!(used <= free, "{used} > {free}");
}

#[test]
fn 여유가_예약분도_안_되면_cpu() {
    assert_eq!(
        choose_runtime(Some(GPU_RESERVE_BYTES), &MODEL),
        RuntimeConfig::CPU
    );
}

#[test]
fn 요약_예산은_실제_문맥_창을_따른다() {
    let b = Budget::app();
    assert_eq!(b.capped(CONTEXT_TOKENS), b);
    let small = b.capped(MIN_CONTEXT_TOKENS);
    assert_eq!(small.context, MIN_CONTEXT_TOKENS as usize);
    assert_eq!(
        small.compact_at(),
        MIN_CONTEXT_TOKENS as usize - b.reply - b.margin
    );
}

// ── #116 루프 3: 다른 앱이 쓰는 그래픽 메모리·올린 뒤 속도 재기 ──

#[test]
fn os_사용량을_알면_llama_cpp_여유와_전체_빼기_사용량_중_작은_쪽이다() {
    // 4060 Ti 실측: llama.cpp는 7181MiB로 읽었지만 다른 앱이 3.3GB를 써서 실제 여유는 약 4.8GB.
    let mib = 1024 * 1024;
    let usable = usable_gpu_memory(7181 * mib, 8188 * mib, Some(3380 * mib));
    assert_eq!(usable, 4808 * mib);
    // 그 여유로는 32k·16k 전 층이 들어가지 않아 일부 레이어만 고른다.
    let c = choose_runtime(Some(usable), &MODEL);
    assert!(matches!(c.gpu_layers, Some(n) if n < LAYERS), "{c:?}");
    // OS 값을 모르면(리눅스·macOS) llama.cpp 값 그대로, 사용량이 전체를 넘어도 0에서 멈춘다.
    assert_eq!(usable_gpu_memory(5 * GIB, 8 * GIB, None), 5 * GIB);
    assert_eq!(usable_gpu_memory(5 * GIB, 8 * GIB, Some(9 * GIB)), 0);
}

#[test]
fn 빠르면_한_번만_재고_그대로_쓴다() {
    assert_eq!(next_probe(&[(29, 45.0)]), None);
    assert_eq!(best_probe(&[(29, 45.0)]), Some((29, 45.0)));
}

#[test]
fn 넘쳐서_느리면_레이어를_줄여_가며_재고_가장_빠른_것을_고른다() {
    // 4060 Ti 실측 모양: 전 층 2.7, 19층 5.1, 9층 7.9, CPU 5.3.
    let mut tried = vec![(29, 2.7)];
    assert_eq!(next_probe(&tried), Some(19));
    tried.push((19, 5.1));
    assert_eq!(next_probe(&tried), Some(9));
    tried.push((9, 7.9));
    assert_eq!(next_probe(&tried), Some(0));
    tried.push((0, 5.3));
    assert_eq!(next_probe(&tried), None, "CPU까지 쟀으면 그만");
    assert_eq!(best_probe(&tried), Some((9, 7.9)));
}

#[test]
fn 줄이다_빨라지면_거기서_멈추고_사다리는_늘_줄어든다() {
    assert_eq!(next_probe(&[(29, 2.7), (19, 30.0)]), None);
    // 처음부터 일부 레이어(예: 12)였으면 8 → 4 → 0.
    assert_eq!(next_probe(&[(12, 3.0)]), Some(8));
    assert_eq!(next_probe(&[(12, 3.0), (8, 3.0)]), Some(4));
    // 한 층이면 바로 CPU.
    assert_eq!(next_probe(&[(1, 3.0)]), Some(0));
    assert_eq!(next_probe(&[]), None);
}

#[test]
fn 일부_레이어_설정은_16k_q8_0이고_0이면_cpu다() {
    assert_eq!(RuntimeConfig::partial(0), RuntimeConfig::CPU);
    let p = RuntimeConfig::partial(9);
    assert_eq!(
        (p.n_ctx, p.kv, p.flash_attn, p.gpu_layers),
        (MIN_CONTEXT_TOKENS, KvType::Q8_0, FlashAttn::On, Some(9))
    );
}
