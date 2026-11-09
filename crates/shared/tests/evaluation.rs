//! 기준표·근거·평가 입력·최종 점수 규칙(#99) 이음매 ①.

use promptlab_shared::activity::{Activity, AfterAccept, Decision, block_present};
use promptlab_shared::coding::{Language, PublicCodingProblem};
use promptlab_shared::evaluation::{
    Evaluation, EvaluationStatus, Evidence, FinalCode, Graded, HiddenPassCount, RawCriterion,
    RawEvaluation, ScoreBasis, accept, evaluation_input, exam_evaluation_score, exam_final_score,
    item_score, rubric,
};
use promptlab_shared::exam::RubricItem;
use promptlab_shared::log::{EditRange, LocalRunKind, LogEvent, ToolCall};

fn eval(score: f64) -> Evaluation {
    Evaluation {
        criteria: Vec::new(),
        score,
        model: "m".into(),
        dropped_evidence: 0,
    }
}

#[test]
fn 기준표는_공통_4항목_뒤에_추가_항목() {
    let extra = [RubricItem {
        name: "협업".into(),
        description: "설명".into(),
    }];
    let r = rubric(&extra);
    let keys: Vec<&str> = r.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "instruction",
            "verification",
            "critique",
            "efficiency",
            "extra_1"
        ]
    );
    let names: Vec<&str> = r.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names[..4], ["지시 품질", "검증", "AI 제안 비판", "효율"]);
    assert_eq!(r[4].name, "협업");
}

#[test]
fn 최종_점수는_통과율_곱하기_w_더하기_ai_활용_곱하기_나머지() {
    let s = item_score(
        Graded::Done(0.8),
        50,
        EvaluationStatus::Done,
        Some(eval(60.0)),
    );
    assert_eq!(s.basis, ScoreBasis::Weighted);
    assert!((s.final_score.unwrap() - 70.0).abs() < 1e-9);
    assert_eq!(
        s.formula,
        "최종 70.0 = 통과율 80.0 × 50% + AI 활용 60.0 × 50%"
    );

    let s = item_score(
        Graded::Done(1.0),
        30,
        EvaluationStatus::Done,
        Some(eval(40.0)),
    );
    assert!(
        (s.final_score.unwrap() - 58.0).abs() < 1e-9,
        "100×0.3 + 40×0.7"
    );
}

#[test]
fn 평가가_없으면_통과율만이고_까닭을_드러낸다() {
    for status in [
        EvaluationStatus::Disabled,
        EvaluationStatus::Smoke,
        EvaluationStatus::Practice,
        EvaluationStatus::Failed,
        EvaluationStatus::NotEvaluated,
    ] {
        let s = item_score(Graded::Done(0.4), 50, status, None);
        assert_eq!(s.basis, ScoreBasis::HiddenOnly, "{status:?}");
        assert_eq!(s.final_score, Some(40.0));
        assert!(s.formula.contains(status.label()), "{}", s.formula);
    }
    assert_eq!(EvaluationStatus::Disabled.label(), "AI 활용 평가 꺼짐");
}

#[test]
fn 평가를_기다리면_최종_점수가_없고_w_100이면_기다리지_않는다() {
    let s = item_score(Graded::Done(0.4), 50, EvaluationStatus::Queued, None);
    assert_eq!((s.basis, s.final_score), (ScoreBasis::Pending, None));
    assert_eq!(s.hidden_score, Some(40.0));
    let s = item_score(Graded::Done(0.4), 100, EvaluationStatus::Queued, None);
    assert_eq!((s.basis, s.final_score), (ScoreBasis::Weighted, Some(40.0)));
}

#[test]
fn 채점_실패는_0점_채점_전은_없음() {
    let s = item_score(Graded::Failed, 50, EvaluationStatus::NotEvaluated, None);
    assert_eq!(
        (s.basis, s.final_score),
        (ScoreBasis::GradingFailed, Some(0.0))
    );
    let s = item_score(Graded::Pending, 50, EvaluationStatus::Queued, None);
    assert_eq!((s.basis, s.final_score), (ScoreBasis::Pending, None));
}

#[test]
fn 시험_점수는_문항_최종의_평균이고_하나라도_없으면_없다() {
    let a = item_score(
        Graded::Done(1.0),
        50,
        EvaluationStatus::Done,
        Some(eval(50.0)),
    );
    let b = item_score(Graded::Failed, 50, EvaluationStatus::NotEvaluated, None);
    let c = item_score(Graded::Done(1.0), 50, EvaluationStatus::Queued, None);
    assert_eq!(exam_final_score([Some(&a), Some(&b)]), Some(37.5));
    assert_eq!(exam_final_score([Some(&a), Some(&c)]), None);
    assert_eq!(exam_final_score([Some(&a), None]), None, "안 낸 문항");
    assert_eq!(exam_final_score(std::iter::empty()), None);
}

#[test]
fn ai_활용_평균은_평가된_문항만() {
    let a = item_score(
        Graded::Done(1.0),
        50,
        EvaluationStatus::Done,
        Some(eval(80.0)),
    );
    let b = item_score(
        Graded::Done(1.0),
        50,
        EvaluationStatus::Done,
        Some(eval(40.0)),
    );
    let c = item_score(Graded::Done(0.5), 50, EvaluationStatus::Failed, None);
    assert_eq!(
        exam_evaluation_score([Some(&a), Some(&b), Some(&c), None]),
        Some(60.0)
    );
    assert_eq!(exam_evaluation_score([Some(&c)]), None);
}

// ---- 기록(Activity): 턴·제안·결정·수락 뒤 수정 ----

fn range(start: u32, end: u32) -> EditRange {
    EditRange {
        start_line: start,
        start_column: 1,
        end_line: end,
        end_column: 1,
    }
}

fn snap(code: &str) -> LogEvent {
    LogEvent::CodeSnapshot { code: code.into() }
}

fn msg(text: &str) -> LogEvent {
    LogEvent::AiMessage { text: text.into() }
}

fn reply(text: &str) -> LogEvent {
    LogEvent::AiReply {
        text: text.into(),
        tool_calls: vec![ToolCall {
            name: "propose_edit".into(),
            arguments: serde_json::json!({}),
        }],
        stopped: false,
    }
}

fn propose(n: u32, text: &str) -> LogEvent {
    LogEvent::EditProposed {
        proposal: n,
        range: range(2, 2),
        text: text.into(),
    }
}

fn build(events: &[LogEvent], final_code: Option<&str>) -> Activity {
    Activity::from_events(
        events
            .iter()
            .enumerate()
            .map(|(i, e)| (i as u64, i as i64 * 1000, e)),
        final_code,
    )
}

const BEFORE: &str = "def f(a):\n    pass\n";
const ACCEPTED: &str = "def f(a):\n    return a + 1\n";

#[test]
fn 응시자_메시지마다_턴이_열리고_답과_제안이_그_턴에_든다() {
    let a = build(
        &[
            LogEvent::LanguageChosen {
                language: Language::Python,
            },
            snap(BEFORE),
            msg("하나 더해 줘"),
            reply("이렇게 바꾸세요"),
            propose(1, "    return a + 1"),
            LogEvent::EditRejected { proposal: 1 },
            msg("다시"),
            reply("다시 제안"),
            propose(2, "    return a + 1"),
            LogEvent::EditAccepted { proposal: 2 },
            LogEvent::LocalRun {
                kind: LocalRunKind::Examples,
                passed: Some(2),
                total: Some(3),
                summary: "예제 2/3 통과".into(),
            },
        ],
        Some(ACCEPTED),
    );
    let turns: Vec<(u32, &str, usize)> = a
        .turns
        .iter()
        .map(|t| (t.turn, t.message.as_str(), t.replies.len()))
        .collect();
    assert_eq!(turns, [(1, "하나 더해 줘", 1), (2, "다시", 1)]);
    let props: Vec<(u32, Option<u32>, Decision)> = a
        .proposals
        .iter()
        .map(|p| (p.proposal, p.turn, p.decision))
        .collect();
    assert_eq!(
        props,
        [
            (1, Some(1), Decision::Rejected),
            (2, Some(2), Decision::Accepted)
        ]
    );
    assert_eq!(
        a.proposals[0].after_accept, None,
        "거절한 제안은 판정하지 않는다"
    );
    assert_eq!(a.proposals[1].after_accept, Some(AfterAccept::Kept));
    assert_eq!((a.runs.len(), a.runs[0].turn), (1, Some(2)));
    assert_eq!(a.languages, [Language::Python]);
    assert_eq!(a.decision_counts(), (1, 0, 1));
}

#[test]
fn 수락_뒤_그_영역을_고치면_처음_달라진_스냅숏을_가리킨다() {
    let events = [
        snap(BEFORE),
        msg("고쳐 줘"),
        propose(1, "    return a + 1\n"),
        LogEvent::EditAccepted { proposal: 1 },
        snap(ACCEPTED),
        snap("def f(a):\n    return a + 1  \n"), // 줄 끝 공백만 — 그대로로 본다
        snap("def f(a):\n    return a + 2\n"),   // seq 6: 고쳤다
        snap(ACCEPTED),                          // 되돌려도 처음 고친 곳이 남는다
    ];
    let a = build(&events, Some(ACCEPTED));
    assert_eq!(
        a.proposals[0].after_accept,
        Some(AfterAccept::Modified { seq: Some(6) })
    );
    assert_eq!(a.decision_counts(), (1, 1, 0));
}

#[test]
fn 스냅숏에선_그대로여도_최종_코드에서_없어지면_고친_것이다() {
    let events = [
        msg("고쳐 줘"),
        propose(1, "    return a + 1"),
        LogEvent::EditAccepted { proposal: 1 },
        snap(ACCEPTED),
    ];
    let a = build(&events, Some("def f(a):\n    return a * 2\n"));
    assert_eq!(
        a.proposals[0].after_accept,
        Some(AfterAccept::Modified { seq: None })
    );
    // 수락 뒤 스냅숏도 최종 코드도 없으면 판정하지 않는다.
    let a = build(&events[..3], None);
    assert_eq!(a.proposals[0].after_accept, None);
}

#[test]
fn 여러_줄_블록은_연속해_있어야_그대로다() {
    let block = "\n  x = 1\n  y = 2\n\n";
    assert!(block_present("a\n  x = 1\r\n  y = 2\nb", block));
    assert!(!block_present("  x = 1\nz\n  y = 2", block));
    assert!(
        block_present("anything", "\n\n"),
        "지우기 제안(빈 글)은 늘 그대로"
    );
}

#[test]
fn 결정은_처음_것만_남고_없는_번호의_결정은_버린다() {
    let a = build(
        &[
            msg("q"),
            propose(1, "x"),
            LogEvent::EditAccepted { proposal: 1 },
            LogEvent::EditRejected { proposal: 1 },
            LogEvent::EditRejected { proposal: 9 },
            propose(2, "y"),
        ],
        None,
    );
    assert_eq!(a.proposals[0].decision, Decision::Accepted);
    assert_eq!(a.proposals[1].decision, Decision::Undecided);
}

// ---- 근거 검사 ----

fn raw(evidence: Vec<Evidence>, score: i64) -> RawEvaluation {
    RawEvaluation {
        criteria: rubric(&[])
            .iter()
            .map(|c| RawCriterion {
                key: c.key.clone(),
                score,
                reason: "근거".into(),
                evidence: evidence.clone(),
            })
            .collect(),
    }
}

#[test]
fn 기록에_없는_턴_제안_근거만_버리고_점수는_남긴다() {
    let a = build(&[msg("q"), propose(3, "x")], None);
    let r = raw(
        vec![
            Evidence::Turn(1),
            Evidence::Turn(2),
            Evidence::Proposal(3),
            Evidence::Proposal(1),
            Evidence::Turn(1),
        ],
        4,
    );
    let e = accept(&r, &rubric(&[]), &a, "m").expect("형식 맞음");
    assert_eq!(
        e.criteria[0].evidence,
        [Evidence::Turn(1), Evidence::Proposal(3)]
    );
    assert_eq!(e.dropped_evidence, 2 * 4, "항목마다 없는 근거 2개");
    assert!((e.score - 80.0).abs() < 1e-9);
    assert_eq!(Evidence::Proposal(3).label(), "제안 #3");
    assert_eq!(Evidence::Turn(2).label(), "턴 2");
}

#[test]
fn 항목이_빠지거나_점수가_범위_밖이면_형식_오류() {
    let a = Activity::default();
    assert!(accept(&raw(vec![], 6), &rubric(&[]), &a, "m").is_none());
    assert!(accept(&raw(vec![], -1), &rubric(&[]), &a, "m").is_none());
    let mut r = raw(vec![], 3);
    r.criteria.pop();
    assert!(accept(&r, &rubric(&[]), &a, "m").is_none());
}

#[test]
fn 근거는_kind와_n으로_읽고_쓴다() {
    let e: Vec<Evidence> =
        serde_json::from_str(r#"[{"kind":"turn","n":2},{"kind":"proposal","n":5}]"#).unwrap();
    assert_eq!(e, [Evidence::Turn(2), Evidence::Proposal(5)]);
    assert_eq!(
        serde_json::to_string(&Evidence::Turn(2)).unwrap(),
        r#"{"kind":"turn","n":2}"#
    );
}

// ---- 평가 입력 ----

fn problem() -> PublicCodingProblem {
    serde_json::from_value(serde_json::json!({
        "id": "p", "title": "하나 더하기", "description": "a에 1을 더한다",
        "signature": {"name": "f", "params": [{"name": "a", "type": "int"}], "returns": "int"},
        "examples": [{"args": [1], "expected": 2}],
        "limits": {"case_time_ms": 2000},
        "templates": []
    }))
    .expect("공개 문제")
}

#[test]
fn 평가_입력에는_문제_턴_제안_결정_실행_최종_코드_통과_수가_든다() {
    let a = build(
        &[
            msg("하나 더해 줘"),
            reply("제안합니다"),
            propose(1, "    return a + 1"),
            LogEvent::EditAccepted { proposal: 1 },
            LogEvent::LocalRun {
                kind: LocalRunKind::Examples,
                passed: Some(1),
                total: Some(1),
                summary: "예제 1/1 통과".into(),
            },
            snap("def f(a):\n    return a + 2\n"),
        ],
        Some("def f(a):\n    return a + 2\n"),
    );
    let input = evaluation_input(
        Some(&problem()),
        &rubric(&[]),
        &a,
        Some(FinalCode {
            language: Language::Python,
            code: "def f(a):\n    return a + 2\n",
        }),
        Some(HiddenPassCount {
            passed: 7,
            total: 10,
        }),
        Some("Qwen2.5-Coder 1.5B"),
    );
    for needle in [
        "AI 도우미 모델: Qwen2.5-Coder 1.5B",
        "하나 더하기",
        "f(a: int) -> int",
        "입력 [1] → 기대 2",
        "instruction — 지시 품질",
        "[턴 1] 응시자: 하나 더해 줘",
        "[턴 1] AI: 제안합니다 (도구 호출: propose_edit)",
        "[제안 #1] 턴 1에 나옴",
        "수락(수락 뒤 응시자가 그 영역을 고치거나 지움)",
        "공개 예제 실행 1/1 통과",
        "return a + 2",
        "서버 히든 테스트 통과: 7/10",
    ] {
        assert!(input.contains(needle), "{needle} 없음:\n{input}");
    }
}
