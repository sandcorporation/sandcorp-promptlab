//! 모델 크기 추천(#131 결정 B) 경계값: 기본은 7B(GPU 전부·일부 층·CPU), 7B를 못 올리는 RAM에서만 1.5B(제한적),
//! 그래픽 메모리 8GB(여유 7.5·6·5GB)·4GB, GPU 없음 + RAM 넉넉·모자람, 이미 받은 모델, 직접 고른 1.5B.

use promptlab_shared::inference::{
    CONTEXT_TOKENS, LIMITED_NOTE, MODELS, RuntimeConfig, choose_runtime, model_by_id,
};
use promptlab_shared::model_choice::{
    GpuMemory, Machine, SLOW_CPU_MODEL_BYTES, cpu_ram_need, download_text, fits, plan, ram_need,
    recommend,
};

const GIB: u64 = 1024 * 1024 * 1024;

fn gpu(total: u64, usable: u64, free_ram: u64) -> Machine {
    Machine {
        gpu: Some(GpuMemory { total, usable }),
        free_ram,
    }
}

fn cpu(free_ram: u64) -> Machine {
    Machine {
        gpu: None,
        free_ram,
    }
}

#[test]
fn 후보는_큰_것부터_7b_15b이고_15b만_제한적이다() {
    let ids: Vec<_> = MODELS.iter().map(|m| m.id).collect();
    assert_eq!(ids, ["7b", "1.5b"]);
    assert!(MODELS.windows(2).all(|w| w[0].size > w[1].size));
    let (big, small) = (model_by_id("7b").unwrap(), model_by_id("1.5b").unwrap());
    assert!(!big.limited && small.limited);
    assert_eq!(big.display_name(), "Qwen2.5-Coder 7B");
    assert_eq!(
        small.display_name(),
        format!("Qwen2.5-Coder 1.5B({LIMITED_NOTE})")
    );
    assert_eq!(LIMITED_NOTE, "제한적 — 답을 못 할 수 있음");
    assert!(big.size > SLOW_CPU_MODEL_BYTES && small.size <= SLOW_CPU_MODEL_BYTES);
}

#[test]
fn 큰_7b를_cpu로만_돌리는_몫은_약_7_1gb다() {
    let need = cpu_ram_need(model_by_id("7b").unwrap());
    assert!((7 * GIB..7 * GIB + GIB / 5).contains(&need), "{need}");
    // 층 전부 GPU면 RAM 몫은 남길 몫뿐이다.
    let m = model_by_id("7b").unwrap();
    let full = choose_runtime(Some(8 * GIB), m);
    assert!(full.gpu_layers.is_none());
    assert_eq!(ram_need(m, &full), GIB);
}

#[test]
fn 빈_8gb_카드는_7b를_gpu에_전부_올린다() {
    let p = recommend(&gpu(8 * GIB, 7 * GIB + GIB / 2, 12 * GIB), &[]);
    assert_eq!(p.model.id, "7b");
    assert!(p.full_gpu());
    assert_eq!(p.runtime.n_ctx, CONTEXT_TOKENS);
    assert!(
        p.reason.contains("그래픽 메모리 8.0GB 중 여유 7.5GB"),
        "{}",
        p.reason
    );
    assert!(p.reason.contains("받기 4.4GB"), "{}", p.reason);
    assert!(!p.reason.contains("느릴"), "{}", p.reason);
}

#[test]
fn 카드_8gb_중_여유_5gb면_7b의_일부_층만_gpu에_올린다() {
    let p = recommend(&gpu(8 * GIB, 5 * GIB, 12 * GIB), &[]);
    assert_eq!(p.model.id, "7b");
    let n = p.runtime.gpu_layers.expect("일부 층");
    assert!((15..p.model.gpu_layers()).contains(&n), "{n}");
    assert_eq!(p.runtime, RuntimeConfig::partial(n));
    assert!(
        p.reason.starts_with(&format!(
            "그래픽 메모리 8.0GB 중 여유 5.0GB · Qwen2.5-Coder 7B의 {n}/29층만 GPU, 나머지는 CPU(답이 다소 느릴 수 있음)"
        )),
        "{}",
        p.reason
    );
    assert!(!p.reason.contains("닫으세요"), "{}", p.reason);
    // 다른 앱이 써서 여유 6GB여도(루프 1까지는 1.5B) 7B다.
    assert_eq!(
        recommend(&gpu(8 * GIB, 6 * GIB, 12 * GIB), &[]).model.id,
        "7b"
    );
}

#[test]
fn 일부_층_7b의_경계는_cpu에_남는_몫과_여유_ram이다() {
    let m = model_by_id("7b").unwrap();
    let rt = choose_runtime(Some(5 * GIB), m);
    let need = ram_need(m, &rt);
    assert!(need < cpu_ram_need(m) / 2, "{need}");
    assert_eq!(recommend(&gpu(8 * GIB, 5 * GIB, need), &[]).model.id, "7b");
    let p = recommend(&gpu(8 * GIB, 5 * GIB, need - 1), &[]);
    assert_eq!(p.model.id, "1.5b");
    assert!(p.full_gpu());
    assert!(
        p.reason
            .contains("로는 Qwen2.5-Coder 7B를 올릴 수 없음(필요"),
        "{}",
        p.reason
    );
}

#[test]
fn 카드_4gb도_ram이_있으면_7b_일부_층_없으면_15b를_gpu에() {
    let p = recommend(&gpu(4 * GIB, 3 * GIB + GIB / 2, 8 * GIB), &[]);
    assert_eq!(p.model.id, "7b");
    assert!(p.runtime.gpu_layers.is_some_and(|n| n > 0));
    let p = recommend(&gpu(4 * GIB, 3 * GIB + GIB / 2, 3 * GIB), &[]);
    assert_eq!(p.model.id, "1.5b");
    assert!(p.full_gpu());
}

#[test]
fn gpu가_없고_ram이_넉넉하면_7b를_cpu로() {
    let p = recommend(&cpu(12 * GIB), &[]);
    assert_eq!(p.model.id, "7b");
    assert_eq!(p.runtime, RuntimeConfig::CPU);
    assert!(
        p.reason
            .starts_with("GPU 없음 · Qwen2.5-Coder 7B를 CPU로 돌립니다(답이 느릴 수 있음)"),
        "{}",
        p.reason
    );
    assert!(!p.reason.contains("닫으세요"), "{}", p.reason);
    let need = cpu_ram_need(model_by_id("7b").unwrap());
    assert_eq!(recommend(&cpu(need), &[]).model.id, "7b");
    assert_eq!(recommend(&cpu(need - 1), &[]).model.id, "1.5b");
}

#[test]
fn ram이_모자라면_15b를_제한적으로_고르고_까닭을_말한다() {
    let p = recommend(&cpu(4 * GIB), &[]);
    assert_eq!(p.model.id, "1.5b");
    assert!(p.model.display_name().contains(LIMITED_NOTE));
    assert!(
        p.reason.starts_with(
            "GPU 없음 · 여유 RAM 4.0GB로는 Qwen2.5-Coder 7B를 올릴 수 없음(필요 7.1GB) · Qwen2.5-Coder 1.5B를 CPU로 돌립니다"
        ),
        "{}",
        p.reason
    );
    assert!(!p.reason.contains("느릴"), "{}", p.reason);
    // 1.5B도 빠듯하면 그래도 1.5B, 닫으라고 한다.
    let need = cpu_ram_need(model_by_id("1.5b").unwrap());
    assert!(!recommend(&cpu(need), &[]).reason.contains("닫으세요"));
    let p = recommend(&cpu(need - 1), &[]);
    assert_eq!(p.model.id, "1.5b");
    assert!(!fits(p.model, &cpu(need - 1)));
    assert!(p.reason.contains("다른 앱을 닫으세요"), "{}", p.reason);
}

#[test]
fn 이미_받은_모델은_받기_0이고_추천을_바꾸지_않는다() {
    let p = recommend(&cpu(4 * GIB), &["1.5b"]);
    assert_eq!(p.download_bytes, 0);
    assert!(p.reason.ends_with("받아 둠"), "{}", p.reason);
    // 1.5B만 받아 뒀어도 7B를 올릴 수 있으면 추천은 7B다(받기 크기는 고르는 데 쓰지 않는다).
    let p = recommend(&gpu(8 * GIB, 5 * GIB, 12 * GIB), &["1.5b"]);
    assert_eq!(p.model.id, "7b");
    assert!(
        p.reason.ends_with("받기 4.4GB(100Mbps에서 약 7분)"),
        "{}",
        p.reason
    );
    assert_eq!(download_text(0), "받아 둠");
}

#[test]
fn 직접_고른_15b는_7b를_올릴_수_있어도_제한적_표시가_남는다() {
    let rich = gpu(8 * GIB, 7 * GIB + GIB / 2, 12 * GIB);
    let p = plan(model_by_id("1.5b").unwrap(), &rich, &[], false);
    assert_eq!(p.model.id, "1.5b");
    assert!(p.full_gpu());
    assert_eq!(
        p.model.display_name(),
        "Qwen2.5-Coder 1.5B(제한적 — 답을 못 할 수 있음)"
    );
    // 고른 것이라 더 큰 모델을 왜 못 올리는지는 적지 않는다.
    assert!(!p.reason.contains("올릴 수 없음"), "{}", p.reason);
}

#[test]
fn 응시자가_고른_7b가_ram에_빠듯하면_닫으라고_한다() {
    let p = plan(model_by_id("7b").unwrap(), &cpu(4 * GIB), &["7b"], false);
    assert_eq!(p.runtime, RuntimeConfig::CPU);
    assert!(p.reason.contains("느릴 수 있음"), "{}", p.reason);
    assert!(p.reason.contains("다른 앱을 닫으세요"), "{}", p.reason);
    assert_eq!(p.download_bytes, 0);
}
