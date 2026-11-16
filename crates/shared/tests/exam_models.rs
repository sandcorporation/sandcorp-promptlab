//! 시험 설정의 허용 AI 모델(#5): 검증·허용 판정·안내 문구·AI 속도 표시.

use promptlab_shared::exam::{
    ExamSettings, all_models, model_required_message, required_models_note,
};
use promptlab_shared::inference::{MODELS, model_by_id};
use promptlab_shared::log::AiSpeed;

fn with_models(models: &[&str]) -> ExamSettings {
    ExamSettings {
        allowed_models: models.iter().map(|m| (*m).to_owned()).collect(),
        ..ExamSettings::default()
    }
}

#[test]
fn 기본은_후보_전부를_허용한다() {
    let s = ExamSettings::default();
    assert_eq!(s.allowed_models, vec!["7b".to_owned(), "1.5b".to_owned()]);
    assert_eq!(all_models().len(), MODELS.len());
    assert!(s.validate().is_ok());
    assert!(s.allows_all_models());
    // 설정 JSON에 칸이 없으면(옛 요청) 기본값 = 전부.
    let parsed: ExamSettings = serde_json::from_str(r#"{"score_weight": 50}"#).unwrap();
    assert_eq!(parsed.allowed_models, all_models());
}

#[test]
fn 빈_목록_모르는_id_중복은_거부한다() {
    assert!(with_models(&[]).validate().is_err());
    assert!(with_models(&["7b", "3b"]).validate().is_err());
    assert!(with_models(&["7b", "7b"]).validate().is_err());
    assert!(with_models(&["1.5b"]).validate().is_ok());
}

#[test]
fn 신고한_sha256이_허용_모델인지_본다() {
    let big = model_by_id("7b").unwrap();
    let small = model_by_id("1.5b").unwrap();
    let only_big = with_models(&["7b"]);
    assert!(only_big.model_allowed(big.sha256));
    assert!(only_big.model_allowed(&big.sha256.to_ascii_uppercase()));
    assert!(!only_big.model_allowed(small.sha256));
    assert!(!only_big.model_allowed("00"));
    // 전부 허용이면 후보 밖 값(운영자 허용 목록이 거른다)도 막지 않는다.
    assert!(ExamSettings::default().model_allowed(small.sha256));
}

#[test]
fn 필요한_모델_안내는_전부가_아닐_때만_있다() {
    assert_eq!(required_models_note(&all_models()), None);
    let note = required_models_note(&["7b".to_owned()]).unwrap();
    assert!(note.contains("Qwen2.5-Coder 7B 모델만"), "{note}");
    assert_eq!(
        model_required_message(&["7b".to_owned()]),
        "이 시험은 Qwen2.5-Coder 7B 모델이 필요합니다 — 응시 전 점검에서 AI 모델을 바꿔 주세요"
    );
}

#[test]
fn ai_속도는_초당_토큰과_gpu_층으로_보인다() {
    let s = AiSpeed {
        generated_tokens: 246,
        generate_ms: 20_000,
        gpu_layers: 21,
        total_layers: 29,
    };
    assert_eq!(s.tokens_per_sec_label(), "12.3");
    assert_eq!(s.label(), "12.3 토큰/초 · GPU 21/29층");
    let cpu = AiSpeed { gpu_layers: 0, ..s };
    assert_eq!(cpu.gpu_label(), "CPU");
    let all = AiSpeed {
        gpu_layers: 29,
        ..s
    };
    assert_eq!(all.gpu_label(), "GPU");
    let none = AiSpeed {
        generated_tokens: 0,
        generate_ms: 0,
        ..s
    };
    assert_eq!(none.tokens_per_sec(), None);
    assert_eq!(none.label(), "생성 없음 · GPU 21/29층");
}
