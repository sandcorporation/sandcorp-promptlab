//! 시험 이음매 ③: 응시 세션 상태 기계(UI·네트워크·모델 없이).

use std::time::{Duration, Instant};

use promptlab_app::session::{ExamItemRef, Phase, Session, SessionError, SessionEvent, Submission};
use promptlab_shared::api::{SubmissionState, SubmissionView};
use promptlab_shared::coding::{
    DEFAULT_CASE_TIME_MS, Language, Limits, Param, PublicCodingProblem, Signature, TestCase,
    ValueType, templates,
};
use promptlab_shared::grading::GradeReport;
use serde_json::json;

fn problem() -> PublicCodingProblem {
    let signature = Signature {
        name: "add_one".into(),
        params: vec![Param {
            name: "n".into(),
            ty: ValueType::Int,
        }],
        returns: ValueType::Int,
    };
    PublicCodingProblem {
        id: "p-1".into(),
        title: "하나 더하기".into(),
        description: "n에 1을 더하라".into(),
        templates: templates(&signature),
        signature,
        examples: vec![TestCase {
            args: vec![json!(1)],
            expected: json!(2),
        }],
        limits: Limits {
            case_time_ms: DEFAULT_CASE_TIME_MS,
        },
    }
}

const CODE: &str = "def add_one(n):\n    return n + 1\n";

fn submit(at: Instant) -> SessionEvent {
    SessionEvent::Submit {
        at,
        language: Language::Python,
        code: CODE.into(),
    }
}

/// 응시 중까지 데려간 세션과 시작 시각.
fn in_progress() -> (Session, Instant) {
    let mut s = Session::new();
    let t0 = Instant::now();
    s.apply(SessionEvent::Start).unwrap();
    s.apply(SessionEvent::ChecksPassed { problem: problem() })
        .unwrap();
    s.apply(SessionEvent::ConsentGiven { at: t0 }).unwrap();
    (s, t0)
}

fn view(state: SubmissionState) -> SubmissionView {
    SubmissionView {
        id: "sub-1".into(),
        problem_id: "p-1".into(),
        org_id: None,
        state,
        score: None,
    }
}

fn report() -> GradeReport {
    GradeReport::from_cases("p-1", Language::Python, &[true, false, true])
}

#[test]
fn 시작하면_점검으로_간다() {
    let mut s = Session::new();
    s.apply(SessionEvent::Start).unwrap();
    assert_eq!(s.phase(), &Phase::Checking);
}

#[test]
fn 점검_전에는_제출할_수_없다() {
    let mut s = Session::new();
    let err = s.apply(submit(Instant::now())).unwrap_err();
    assert!(matches!(err, SessionError::InvalidTransition { .. }));
    assert_eq!(s.phase(), &Phase::NotStarted);
}

#[test]
fn 점검에_실패하면_알림을_남기고_다시_시작할_수_있다() {
    let mut s = Session::new();
    s.apply(SessionEvent::Start).unwrap();
    s.apply(SessionEvent::ChecksFailed {
        reason: "서버에 닿지 못했다".into(),
    })
    .unwrap();
    assert_eq!(s.phase(), &Phase::NotStarted);
    assert_eq!(s.notice(), Some("서버에 닿지 못했다"));
    s.apply(SessionEvent::Start).unwrap();
    assert_eq!(s.notice(), None);
}

#[test]
fn 제출에는_언어_코드_소요_시간이_담긴다() {
    let (mut s, t0) = in_progress();
    s.apply(submit(t0 + Duration::from_secs(95))).unwrap();
    let Phase::Submitting { submission } = s.phase() else {
        panic!("제출 중이어야 한다");
    };
    assert_eq!(
        submission,
        &Submission {
            language: Language::Python,
            code: CODE.into(),
            elapsed_secs: 95,
        }
    );
}

#[test]
fn 빈_코드는_제출하지_않는다() {
    let (mut s, t0) = in_progress();
    assert_eq!(
        s.apply(SessionEvent::Submit {
            at: t0,
            language: Language::Python,
            code: "  
"
            .into(),
        }),
        Err(SessionError::EmptyCode)
    );
    assert_eq!(s.phase(), &Phase::InProgress);
}

#[test]
fn 제출이_접수되면_채점을_기다리고_끝나면_점수를_보인다() {
    let (mut s, t0) = in_progress();
    s.apply(submit(t0)).unwrap();
    s.apply(SessionEvent::SubmitAccepted {
        view: view(SubmissionState::Queued),
    })
    .unwrap();
    assert_eq!(
        s.phase(),
        &Phase::Grading {
            submission_id: "sub-1".into()
        }
    );

    s.apply(SessionEvent::StatusPolled {
        view: view(SubmissionState::Grading),
    })
    .unwrap();
    assert!(matches!(s.phase(), Phase::Grading { .. }));

    s.apply(SessionEvent::StatusPolled {
        view: view(SubmissionState::Done { report: report() }),
    })
    .unwrap();
    assert_eq!(
        s.phase(),
        &Phase::Graded {
            submission_id: "sub-1".into(),
            report: report(),
            score: None
        }
    );
    // 채점이 끝난 뒤에는 다시 낼 수 없다.
    assert!(s.apply(submit(t0)).is_err());
}

#[test]
fn 채점에_실패하면_이유를_보인다() {
    let (mut s, t0) = in_progress();
    s.apply(submit(t0)).unwrap();
    s.apply(SessionEvent::SubmitAccepted {
        view: view(SubmissionState::Failed {
            reason: "추론 실패".into(),
        }),
    })
    .unwrap();
    assert_eq!(
        s.phase(),
        &Phase::GradingFailed {
            reason: "추론 실패".into()
        }
    );
}

#[test]
fn 제출을_보내지_못하면_응시_중으로_돌아가_다시_낸다() {
    let (mut s, t0) = in_progress();
    s.apply(submit(t0)).unwrap();
    s.apply(SessionEvent::SubmitRejected {
        reason: "서버에 닿지 못했다".into(),
    })
    .unwrap();
    assert_eq!(s.phase(), &Phase::InProgress);
    assert_eq!(s.notice(), Some("서버에 닿지 못했다"));
    s.apply(submit(t0)).unwrap();
    assert!(matches!(s.phase(), Phase::Submitting { .. }));
}

fn exam_item() -> ExamItemRef {
    ExamItemRef {
        assignment_id: "as-1".into(),
        position: 0,
    }
}

/// 배정된 시험 문항을 응시 중까지(마감 `deadline`).
fn exam_in_progress(deadline: Instant) -> Session {
    let mut s = Session::new();
    s.apply(SessionEvent::Start).unwrap();
    s.apply(SessionEvent::ExamReady {
        problem: problem(),
        item: exam_item(),
        allowed_languages: vec![Language::Python, Language::Java],
        deadline,
    })
    .unwrap();
    assert_eq!(s.phase(), &Phase::AwaitingConsent);
    s.apply(SessionEvent::ConsentGiven { at: Instant::now() })
        .unwrap();
    s
}

#[test]
fn 시험_문항은_마감까지_남은_시간을_보이고_마감_뒤에도_제출은_된다() {
    let now = Instant::now();
    let s = exam_in_progress(now + Duration::from_secs(90));
    assert_eq!(s.exam_item(), Some(&exam_item()));
    assert_eq!(s.remaining_secs(now), Some(90));
    assert!(!s.time_up(now));
    assert!(s.time_up(now + Duration::from_secs(90)));
    assert_eq!(s.remaining_secs(now + Duration::from_secs(120)), Some(0));

    // 마감 뒤에도 제출은 된다(화면이 바로 낸다 — 서버가 여유 안에서 받는다).
    let mut late = exam_in_progress(now - Duration::from_secs(1));
    late.apply(submit(Instant::now())).unwrap();
    assert!(matches!(late.phase(), Phase::Submitting { .. }));
}

#[test]
fn 시험은_허용_언어만_낼_수_있다() {
    let mut s = exam_in_progress(Instant::now() + Duration::from_secs(600));
    assert_eq!(
        s.allowed_languages(),
        vec![Language::Python, Language::Java]
    );
    assert_eq!(
        s.apply(SessionEvent::Submit {
            at: Instant::now(),
            language: Language::Cpp,
            code: "int add_one(long long n) { return n + 1; }".into(),
        }),
        Err(SessionError::LanguageNotAllowed(Language::Cpp))
    );
    assert_eq!(s.phase(), &Phase::InProgress);
    s.apply(submit(Instant::now())).unwrap();
    assert!(matches!(s.phase(), Phase::Submitting { .. }));
}

#[test]
fn 개인_연습은_네_언어를_모두_낼_수_있다() {
    let (s, _) = in_progress();
    assert_eq!(s.allowed_languages(), Language::ALL.to_vec());
}

#[test]
fn 연습_응시에는_마감이_없다() {
    let (s, t0) = in_progress();
    assert_eq!(s.exam_item(), None);
    assert_eq!(s.remaining_secs(t0), None);
    assert!(!s.time_up(t0 + Duration::from_secs(100_000)));
}

#[test]
fn 결과를_공개하지_않는_시험은_응시_완료로_끝난다() {
    let mut s = exam_in_progress(Instant::now() + Duration::from_secs(600));
    s.apply(submit(Instant::now())).unwrap();
    s.apply(SessionEvent::SubmitAccepted {
        view: view(SubmissionState::Completed),
    })
    .unwrap();
    assert_eq!(s.phase(), &Phase::Completed);
    assert_eq!(s.phase().name(), "응시 완료");
}

#[test]
fn 서버_마감은_서버_시각_차이로_앱_시계에_옮긴다() {
    let t = Instant::now();
    // PC 시계와 상관없이 서버가 준 now와 deadline의 차이만 쓴다.
    assert_eq!(
        promptlab_app::exam::local_deadline(1_000_600, 1_000_100, t),
        t + Duration::from_secs(500)
    );
    assert_eq!(promptlab_app::exam::local_deadline(100, 200, t), t);
}
