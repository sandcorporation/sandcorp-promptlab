//! 잰 값 범위 검증(#26): 정상·0·음수·NaN·무한대·주소 같은 거대값(`70190099686700` = `0x3fd667f5652c`),
//! 추천 입력의 「측정 실패」, AI 속도 신고값.

use promptlab_shared::inference::model_by_id;
use promptlab_shared::log::AiSpeed;
use promptlab_shared::model_choice::{Machine, recommend};
use promptlab_shared::sanity::{
    MEASURE_FAILED, device_name, gpu_memory, gpu_used, ram_bytes, tokens_per_sec,
};

const GIB: u64 = 1024 * 1024 * 1024;
/// 대표 PC 점검 화면에 보인 값.
const ADDRESS_LIKE: u64 = 70_190_099_686_700;

#[test]
fn ram은_범위_안만_받는다() {
    assert_eq!(ram_bytes(16 * GIB), Some(16 * GIB));
    assert_eq!(ram_bytes(512 << 20), Some(512 << 20));
    assert_eq!(ram_bytes(0), None);
    assert_eq!(ram_bytes(1), None);
    assert_eq!(ram_bytes(ADDRESS_LIKE * 1024), None);
    assert_eq!(ram_bytes(u64::MAX), None);
}

#[test]
fn gpu_메모리는_전체가_범위_안이고_여유가_전체_이하일_때만() {
    assert_eq!(gpu_memory(6 * GIB, 8 * GIB), Some((6 * GIB, 8 * GIB)));
    assert_eq!(gpu_memory(0, 8 * GIB), Some((0, 8 * GIB)));
    assert_eq!(gpu_memory(0, 0), None);
    assert_eq!(gpu_memory(9 * GIB, 8 * GIB), None, "여유 > 전체");
    assert_eq!(gpu_memory(ADDRESS_LIKE, ADDRESS_LIKE), None);
    assert_eq!(gpu_memory(GIB, u64::MAX), None);
    assert_eq!(gpu_used(3 * GIB, 8 * GIB), Some(3 * GIB));
    assert_eq!(gpu_used(ADDRESS_LIKE, 8 * GIB), None);
}

#[test]
fn 속도는_유한하고_0보다_크고_상한_이하일_때만() {
    assert_eq!(tokens_per_sec(42.5), Some(42.5));
    assert_eq!(tokens_per_sec(0.0), None);
    assert_eq!(tokens_per_sec(-3.0), None);
    assert_eq!(tokens_per_sec(f64::NAN), None);
    assert_eq!(tokens_per_sec(f64::INFINITY), None);
    assert_eq!(tokens_per_sec(f64::NEG_INFINITY), None);
    assert_eq!(tokens_per_sec(ADDRESS_LIKE as f64), None);
}

#[test]
fn 장치_이름은_글자가_있고_제어_문자가_없을_때만() {
    assert_eq!(
        device_name("  NVIDIA GeForce RTX 4060 Ti "),
        Some("NVIDIA GeForce RTX 4060 Ti".to_owned())
    );
    assert_eq!(
        device_name("AMD Radeon(TM) Graphics"),
        Some("AMD Radeon(TM) Graphics".into())
    );
    assert_eq!(device_name(""), None);
    assert_eq!(device_name("   "), None);
    assert_eq!(device_name("70190099686700"), None, "숫자만");
    assert_eq!(device_name("0x3fd667f5652c"), None, "16진 주소");
    assert_eq!(device_name("3fd667f5652c"), None, "0x 없는 16진 주소");
    assert_eq!(device_name("GPU\u{1}\u{2}"), None, "제어 문자");
    assert_eq!(device_name(&"A".repeat(500)), None, "너무 김");
}

#[test]
fn 추천은_말이_안_되는_gpu_메모리를_모름으로_다룬다() {
    // GPU 메모리가 주소 같은 값 → GPU 없음처럼 CPU 기준(RAM 넉넉 → 7B CPU), 이유에 「측정 실패」.
    let m = Machine::measured(Some((ADDRESS_LIKE, ADDRESS_LIKE)), None, Some(16 * GIB));
    assert!(m.gpu.is_none());
    assert!(m.unmeasured.gpu && !m.unmeasured.ram);
    let plan = recommend(&m, &[]);
    assert!(plan.reason.contains(MEASURE_FAILED), "{}", plan.reason);
    assert!(!plan.reason.contains("GPU 없음"), "{}", plan.reason);
    assert_eq!(plan.runtime.gpu_layers, Some(0), "CPU로 돌린다");
    // 거대한 GPU 메모리를 믿었으면 7B를 GPU에 전부 올린다고 했을 것이다.
    assert!(!plan.reason.contains("전부 올립니다"), "{}", plan.reason);
}

#[test]
fn 추천은_말이_안_되는_여유_ram을_모름으로_다뤄_큰_모델을_고르지_않는다() {
    let m = Machine::measured(None, None, Some(ADDRESS_LIKE * 1024));
    assert!(m.unmeasured.ram);
    assert_eq!(m.free_ram, 0);
    let plan = recommend(&m, &[]);
    assert_eq!(plan.model.id, "1.5b");
    assert!(
        plan.reason.contains(&format!("여유 RAM {MEASURE_FAILED}")),
        "{}",
        plan.reason
    );
    assert!(!plan.reason.contains("0.0GB"), "{}", plan.reason);
}

#[test]
fn 추천은_os_사용량이_전체를_넘으면_버리고_llama_여유를_쓴다() {
    let m = Machine::measured(Some((7 * GIB, 8 * GIB)), Some(ADDRESS_LIKE), Some(16 * GIB));
    let gpu = m.gpu.expect("GPU 메모리는 말이 된다");
    assert_eq!(gpu.usable, 7 * GIB);
    assert_eq!(m.unmeasured, Default::default());
    // 정상 값은 그대로.
    let ok = Machine::measured(Some((7 * GIB, 8 * GIB)), Some(3 * GIB), Some(16 * GIB));
    assert_eq!(ok.gpu.unwrap().usable, 5 * GIB);
    assert_eq!(ok.free_ram, 16 * GIB);
    assert_eq!(recommend(&ok, &[]).model.id, model_by_id("7b").unwrap().id);
}

fn speed(generated_tokens: u64, generate_ms: u64, gpu_layers: u32, total_layers: u32) -> AiSpeed {
    AiSpeed {
        generated_tokens,
        generate_ms,
        gpu_layers,
        total_layers,
    }
}

#[test]
fn ai_속도_신고값은_말이_될_때만_받는다() {
    assert!(speed(300, 60_000, 0, 29).is_plausible());
    assert!(speed(0, 0, 29, 29).is_plausible(), "올렸지만 생성 없음");
    assert!(speed(5_000, 50_000, 21, 29).is_plausible());
    // 0에 가까운 시간으로 나눔·시간 0.
    assert!(!speed(300, 0, 0, 29).is_plausible());
    assert!(!speed(ADDRESS_LIKE, 1, 0, 29).is_plausible());
    // 주소 같은 누계.
    assert!(!speed(300, ADDRESS_LIKE, 0, 29).is_plausible());
    // 층 수.
    assert!(!speed(300, 60_000, 30, 29).is_plausible(), "GPU 층 > 전체");
    assert!(!speed(300, 60_000, 0, 0).is_plausible(), "전체 0층");
    assert!(!speed(300, 60_000, 0, u32::MAX).is_plausible());
}

#[test]
fn ai_속도_문구는_말이_안_되는_속도를_측정_실패로() {
    let bad = speed(ADDRESS_LIKE, 1, 0, 29);
    assert_eq!(bad.tokens_per_sec(), None);
    assert_eq!(bad.tokens_per_sec_label(), MEASURE_FAILED);
    assert!(bad.label().contains(MEASURE_FAILED), "{}", bad.label());
    assert!(!bad.label().contains(&ADDRESS_LIKE.to_string()));
    assert_eq!(speed(0, 0, 0, 29).tokens_per_sec_label(), "—");
    assert_eq!(speed(300, 60_000, 0, 29).tokens_per_sec_label(), "5.0");
}
