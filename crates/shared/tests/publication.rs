//! 발행 전 점검(spec #93): 모범 답안이 예제 + 히든을 모두 통과해야 발행할 수 있다.

use promptlab_shared::authoring::{PreviewState, PreviewView};
use promptlab_shared::coding::Language;
use promptlab_shared::publication::{CheckResult, TemplateCheck};

fn templates(compiled: impl Fn(Language) -> bool) -> Vec<TemplateCheck> {
    Language::ALL
        .into_iter()
        .map(|language| TemplateCheck {
            language,
            compiled: compiled(language),
        })
        .collect()
}

fn view(state: PreviewState, result: Option<CheckResult>) -> PreviewView {
    let mut v = PreviewView {
        id: "c".into(),
        problem_id: "org-1".into(),
        state,
        created_at: 0,
        current: true,
        example_count: 1,
        hidden_count: 5,
        result,
        publishable: true,
        reason: Some("남은 값".into()),
        queue_ahead: None,
        failure_reason: None,
    };
    v.judge();
    v
}

#[test]
fn 모두_통과해야_발행할_수_있다() {
    let ok = CheckResult {
        passed: 6,
        total: 6,
        compiled: true,
        templates: templates(|_| true),
    };
    assert!(ok.all_passed());
    let v = view(PreviewState::Done, Some(ok));
    assert!(v.publishable);
    assert_eq!(v.reason, None);
}

#[test]
fn 하나라도_틀리거나_컴파일되지_않으면_이유를_보인다() {
    let partial = CheckResult {
        passed: 5,
        total: 6,
        compiled: true,
        templates: templates(|_| true),
    };
    let v = view(PreviewState::Done, Some(partial));
    assert!(!v.publishable);
    assert!(v.reason.as_deref().unwrap().contains("5 / 6"));

    let broken = CheckResult {
        passed: 0,
        total: 6,
        compiled: false,
        templates: templates(|_| true),
    };
    assert!(!broken.all_passed());
    assert!(broken.reason().unwrap().contains("컴파일"));
    assert!(
        !CheckResult {
            passed: 0,
            total: 0,
            compiled: true,
            templates: templates(|_| true),
        }
        .all_passed()
    );
}

#[test]
fn 끝나기_전에는_발행할_수_없고_이유도_없다() {
    for state in [
        PreviewState::Queued,
        PreviewState::Running,
        PreviewState::Failed,
    ] {
        let v = view(state, None);
        assert!(!v.publishable);
        assert_eq!(v.reason, None);
    }
}

#[test]
fn 틀_컴파일이_하나라도_안_되면_발행할_수_없다() {
    let java_broken = CheckResult {
        passed: 6,
        total: 6,
        compiled: true,
        templates: templates(|l| l != Language::Java),
    };
    assert!(!java_broken.all_passed());
    let reason = java_broken.reason().unwrap();
    assert!(reason.contains("Java") && reason.contains("틀"));
    let v = view(PreviewState::Done, Some(java_broken));
    assert!(!v.publishable);

    // 옛 점검(틀 결과 없음)은 다시 점검해야 한다.
    let old = CheckResult {
        passed: 6,
        total: 6,
        compiled: true,
        templates: Vec::new(),
    };
    assert!(!old.all_passed());
}
