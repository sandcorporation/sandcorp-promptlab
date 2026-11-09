//! AI 활용 평가·최종 점수·감독자용 기록 API(#99) 이음매 ②. Claude는 가짜다(실제 API를 부르지 않는다).
//!
//! - 시험 제출이 채점되면 평가 큐에 들고, 가짜 Claude가 받은 요청에 공개 문제·턴·제안·결정·수락 뒤 수정·로컬 실행·최종
//!   코드·히든 통과 수가 있고 **히든 테스트·모범 답안·이메일은 없다**.
//! - 근거는 기록에 있는 턴·제안만 남는다. 최종 = 통과율 × w + AI 활용 × (1 − w), 결과·CSV·사용량(`evaluation`).
//! - 형식 오류는 다시 하고 3번째에 실패(최종 = 통과율만). 개인 연습·키 없음은 평가하지 않는다.
//! - 리포트는 평가가 끝난 뒤에 큐에 든다.
//! - `…/items/{n}/activity`: 결과 보기만(출제자 403, 응시자·남 404), 턴·제안·수락 뒤 수정·최종 코드·평가 근거.

mod common;

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use common::*;
use promptlab_server::claude::{BoxFuture, ClaudeError, ModelReply, ReportModel};
use promptlab_server::evaluations::{self, EvalOutcome};
use promptlab_server::{AppState, router};
use promptlab_shared::account::Role;
use promptlab_shared::activity::{AfterAccept, Decision};
use promptlab_shared::api::{
    AssignmentView, ExamResults, ExamView, InviteCreated, ItemActivity, OrgView, SubmissionState,
    SubmissionView,
};
use promptlab_shared::coding::Language;
use promptlab_shared::evaluation::{EvaluationStatus, Evidence, MAX_ATTEMPTS, ScoreBasis};
use promptlab_shared::exam::Purpose;
use promptlab_shared::log::{EditRange, LocalRunKind, LogEvent, ToolCall};
use promptlab_shared::report::{REPORT_MODEL, TokenUsage};
use promptlab_store::PgPool;
use serde_json::{Value, json};

const TURN_MARK: &str = "턴표식-경계 조건을 먼저 정리해 줘";
const BEFORE: &str = "def echo(s):\n    pass\n";
const PROPOSED: &str = "    return s  # correct";
const ACCEPTED: &str = "def echo(s):\n    return s  # correct\n";
/// 제출 코드: 수락한 줄을 응시자가 고쳤다.
const FINAL: &str = "def echo(s):\n    return s  # correct 최종코드표식\n";

// ---------- 가짜 Claude ----------

struct FakeClaude {
    bodies: Mutex<Vec<Value>>,
    reply: String,
}

impl FakeClaude {
    fn new(reply: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            bodies: Mutex::new(Vec::new()),
            reply: reply.into(),
        })
    }

    fn bodies(&self) -> Vec<Value> {
        self.bodies.lock().unwrap().clone()
    }
}

impl ReportModel for FakeClaude {
    fn complete<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<ModelReply, ClaudeError>> {
        self.bodies.lock().unwrap().push(body.clone());
        let reply = Ok(ModelReply {
            text: self.reply.clone(),
            model: REPORT_MODEL.to_owned(),
            stop_reason: "end_turn".into(),
            usage: TokenUsage {
                input_tokens: 9_000,
                output_tokens: 1_000,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
        });
        Box::pin(async move { reply })
    }
}

/// 공통 4항목 모두 4점, 근거 = 턴 1·제안 #1·없는 턴 99.
fn good_evaluation() -> String {
    let criteria: Vec<Value> = ["instruction", "verification", "critique", "efficiency"]
        .iter()
        .map(|key| {
            json!({
                "key": key,
                "score": 4,
                "reason": "턴 1에서 조건을 정리했다",
                "evidence": [
                    {"kind": "turn", "n": 1},
                    {"kind": "proposal", "n": 1},
                    {"kind": "turn", "n": 99},
                ],
            })
        })
        .collect();
    json!({ "criteria": criteria }).to_string()
}

fn good_report() -> String {
    json!({
        "strengths": ["실행으로 확인했다"],
        "weaknesses": ["없음"],
        "suggestions": ["경계 입력을 더 보라"],
        "suspicion_summary": "기록된 의심 신호가 없다.",
    })
    .to_string()
}

// ---------- 환경 ----------

struct Env {
    app: axum::Router,
    state: AppState,
    pool: PgPool,
}

fn env_with(pool: PgPool, model: Option<Arc<dyn ReportModel>>) -> Env {
    let state = app_state(pool.clone(), FixedClock::at(NOW), model);
    Env {
        app: router(state.clone()),
        state,
        pool,
    }
}

async fn ok<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
    expect: StatusCode,
) -> T {
    let (status, body) = call(app, method, uri, Some(&token_for(sub)), body).await;
    assert_eq!(status, expect, "{method} {uri}: {body}");
    serde_json::from_value(body).unwrap()
}

struct Taken {
    org: OrgView,
    exam: ExamView,
    a: AssignmentView,
}

impl Taken {
    fn item_uri(&self, tail: &str) -> String {
        format!(
            "/orgs/{}/exams/{}/invites/{}/items/0/{tail}",
            self.org.id, self.exam.id, self.a.id
        )
    }
}

/// ALICE 관리자·BOB 출제자·DAVE 열람자 조직, 조직 문제(히든 표식) 한 문항 채용 시험, ERIN이 과정 로그와 함께 낸다.
async fn taken(e: &Env) -> Taken {
    let org: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    approve_org(&e.app, &org.id).await;
    for (sub, role) in [(BOB, Role::Author), (DAVE, Role::Viewer)] {
        let invite: InviteCreated = ok(
            &e.app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({"role": role})),
            StatusCode::CREATED,
        )
        .await;
        let _: OrgView = ok(
            &e.app,
            "POST",
            &format!("/invites/{}/accept", invite.token),
            sub,
            Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
            StatusCode::OK,
        )
        .await;
    }
    let (status, body) = author_call(
        &e.app,
        "POST",
        &format!("/orgs/{}/problems", org.id),
        BOB,
        Some(org_source("메아리")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let problem = body["id"].as_str().unwrap().to_string();
    force_publish(&e.pool, &problem).await;
    let exam: ExamView = ok(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(json!({
            "title": "채용 1차",
            "purpose": Purpose::Recruiting,
            "reveal_scores": false,
            "items": [{"problem_id": problem, "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await;
    let created: Value = ok(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": ["erin@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created["invites"][0]["token"].as_str().unwrap().to_string();
    let a: AssignmentView = ok(
        &e.app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    let erin = token_for(ERIN);
    webcam_ready(&e.app, &erin, &a.id).await;
    let item = format!("/assignments/{}/items/0", a.id);
    let _: Value = ok(
        &e.app,
        "POST",
        &format!("{item}/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    let propose = |n: u32, text: &str| LogEvent::EditProposed {
        proposal: n,
        range: EditRange {
            start_line: 2,
            start_column: 1,
            end_line: 2,
            end_column: 9,
        },
        text: text.into(),
    };
    let call_edit = || ToolCall {
        name: "propose_edit".into(),
        arguments: json!({"start_line": 2, "end_line": 2, "text": PROPOSED}),
    };
    upload_events(
        &e.app,
        &item,
        &erin,
        vec![
            LogEvent::Started,
            LogEvent::LanguageChosen {
                language: Language::Python,
            },
            LogEvent::CodeSnapshot {
                code: BEFORE.into(),
            },
            LogEvent::AiMessage {
                text: TURN_MARK.into(),
            },
            LogEvent::AiReply {
                text: "그대로 돌려주면 됩니다".into(),
                tool_calls: vec![call_edit()],
                stopped: false,
            },
            propose(1, PROPOSED),
            LogEvent::EditAccepted { proposal: 1 },
            LogEvent::CodeSnapshot {
                code: ACCEPTED.into(),
            },
            LogEvent::LocalRun {
                kind: LocalRunKind::Examples,
                passed: Some(1),
                total: Some(1),
                summary: "예제 1/1 통과".into(),
            },
            LogEvent::AiMessage {
                text: "더 짧게?".into(),
            },
            LogEvent::AiReply {
                text: "이렇게요".into(),
                tool_calls: vec![call_edit()],
                stopped: false,
            },
            propose(2, "    return s[:]"),
            LogEvent::EditRejected { proposal: 2 },
        ],
    )
    .await;
    let submit = format!("{item}/submit");
    let _: SubmissionView = ok(
        &e.app,
        "POST",
        &submit,
        ERIN,
        Some(logged_submit_body(&e.app, &submit, &erin, FINAL).await),
        StatusCode::ACCEPTED,
    )
    .await;
    assert!(matches!(
        grade(&e.pool).await,
        promptlab_worker::Outcome::Graded(_)
    ));
    Taken { org, exam, a }
}

async fn results(e: &Env, t: &Taken) -> ExamResults {
    ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/results", t.org.id, t.exam.id),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await
}

// ---------- 시험 ----------

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 채점된_시험_제출을_평가하고_최종은_통과율과_ai_활용의_가중_평균이다(
    pool: PgPool,
) {
    let fake = FakeClaude::new(good_evaluation());
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;

    // 평가 전: 곧 큐에 든다(최종 점수 없음).
    let before = results(&e, &t).await;
    let score = before.rows[0].items[0].score.clone().unwrap();
    assert_eq!(score.evaluation_status, EvaluationStatus::Queued);
    assert_eq!(score.basis, ScoreBasis::Pending);

    let done = evaluations::run_once(&e.state, fake.as_ref())
        .await
        .unwrap();
    assert!(matches!(done, Some(EvalOutcome::Done(_))), "{done:?}");
    assert_eq!(
        evaluations::run_once(&e.state, fake.as_ref())
            .await
            .unwrap(),
        None,
        "한 번만 평가한다"
    );

    let after = results(&e, &t).await;
    let row = &after.rows[0];
    let score = row.items[0].score.clone().unwrap();
    assert_eq!(score.evaluation_status, EvaluationStatus::Done);
    assert_eq!(score.basis, ScoreBasis::Weighted);
    let hidden = score.hidden_score.unwrap();
    assert_eq!(score.evaluation_score, Some(80.0), "4점 × 4항목 / 20");
    let expected = hidden * 0.5 + 80.0 * 0.5;
    assert!((score.final_score.unwrap() - expected).abs() < 1e-9);
    assert!(
        score.formula.contains("AI 활용 80.0 × 50%"),
        "{}",
        score.formula
    );
    assert_eq!(row.evaluation_score, Some(80.0));
    assert!((row.final_score.unwrap() - expected).abs() < 1e-9);
    let evaluation = score.evaluation.unwrap();
    assert_eq!(
        evaluation.criteria[0].evidence,
        [Evidence::Turn(1), Evidence::Proposal(1)],
        "없는 턴 99는 버린다"
    );
    assert_eq!(evaluation.dropped_evidence, 4);

    // CSV에 AI 활용 평균·계산식이 있다.
    let csv = promptlab_server::results::results_csv(&after.rows);
    assert!(
        csv.contains("AI 활용 평균") && csv.contains("문항1 계산식"),
        "{csv}"
    );
    assert!(csv.contains(",80.0,"), "{csv}");

    // 비용 기록: 종류 evaluation 한 줄.
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM usage_records")
        .fetch_all(&e.pool)
        .await
        .unwrap();
    assert_eq!(kinds, ["evaluation"]);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 평가_요청에는_기록이_들고_히든_테스트_모범_답안_이메일은_없다(
    pool: PgPool,
) {
    let fake = FakeClaude::new(good_evaluation());
    let e = env_with(pool, Some(fake.clone()));
    let _t = taken(&e).await;
    evaluations::run_once(&e.state, fake.as_ref())
        .await
        .unwrap();

    let sent = fake.bodies().remove(0);
    assert_eq!(sent["model"], REPORT_MODEL);
    assert_eq!(sent["system"][0]["text"], evaluations::SYSTEM_PROMPT);
    assert_eq!(sent["system"][0]["cache_control"]["type"], "ephemeral");
    let keys = &sent["output_config"]["format"]["schema"]["properties"]["criteria"]["items"]["properties"]
        ["key"]["enum"];
    assert_eq!(
        keys,
        &json!(["instruction", "verification", "critique", "efficiency"])
    );
    let user = sent["messages"][0]["content"].as_str().unwrap();
    for needle in [
        "메아리",
        &format!("[턴 1] 응시자: {TURN_MARK}"),
        "[턴 1] AI: 그대로 돌려주면 됩니다 (도구 호출: propose_edit)",
        "[제안 #1] 턴 1에 나옴, 줄 2~2 바꿈 → 수락(수락 뒤 응시자가 그 영역을 고치거나 지움)",
        "[제안 #2] 턴 2에 나옴, 줄 2~2 바꿈 → 거절",
        "턴 1 뒤: 공개 예제 실행 1/1 통과",
        "최종코드표식",
        "서버 히든 테스트 통과:",
        "instruction — 지시 품질",
    ] {
        assert!(user.contains(needle), "요청에 {needle} 이 없다:\n{user}");
    }
    // 숨은 입력 불변식(응답과 같은 검사)·조직 문제 히든 표식·이메일.
    assert_no_hidden(&sent);
    let text = sent.to_string();
    assert!(
        !text.contains(ORG_HIDDEN_MARK),
        "조직 문제 히든 테스트·모범 답안이 요청에 있다"
    );
    assert!(!text.contains("erin@example.com"), "이메일이 요청에 있다");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 형식이_틀리면_다시_하고_세_번째에_실패해_통과율만_남는다(
    pool: PgPool,
) {
    let fake = FakeClaude::new("평가 아님");
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;
    for n in 1..=MAX_ATTEMPTS {
        let outcome = evaluations::run_once(&e.state, fake.as_ref())
            .await
            .unwrap();
        if n < MAX_ATTEMPTS {
            assert!(
                matches!(outcome, Some(EvalOutcome::Retry(..))),
                "{n}: {outcome:?}"
            );
        } else {
            assert!(
                matches!(outcome, Some(EvalOutcome::Failed(..))),
                "{outcome:?}"
            );
        }
    }
    assert_eq!(
        evaluations::run_once(&e.state, fake.as_ref())
            .await
            .unwrap(),
        None
    );
    let score = results(&e, &t).await.rows[0].items[0]
        .score
        .clone()
        .unwrap();
    assert_eq!(score.evaluation_status, EvaluationStatus::Failed);
    assert_eq!(score.basis, ScoreBasis::HiddenOnly);
    assert_eq!(score.final_score, score.hidden_score);
    assert!(score.formula.contains("평가 실패"), "{}", score.formula);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 리포트는_평가가_끝난_뒤에_만들고_평가_결과를_싣는다(pool: PgPool) {
    let eval = FakeClaude::new(good_evaluation());
    let report = FakeClaude::new(good_report());
    let e = env_with(pool, Some(eval.clone()));
    let _t = taken(&e).await;
    assert_eq!(
        promptlab_server::reports::run_once(&e.state, report.as_ref())
            .await
            .unwrap(),
        None,
        "평가 전에는 리포트를 큐에 넣지 않는다"
    );
    evaluations::run_once(&e.state, eval.as_ref())
        .await
        .unwrap();
    let done = promptlab_server::reports::run_once(&e.state, report.as_ref())
        .await
        .unwrap();
    assert!(
        matches!(done, Some(promptlab_server::reports::JobOutcome::Done(_))),
        "{done:?}"
    );
    let sent = report.bodies().remove(0);
    let user = sent["messages"][0]["content"].as_str().unwrap();
    for needle in [
        "AI 활용 평가 80.0점",
        "지시 품질 4/5",
        "(근거: 턴 1, 제안 #1)",
        "제안 #1(줄 2~2, 1줄): 수락 — 수락 뒤 응시자가 고침",
        "제안 #2(줄 2~2, 1줄): 거절",
        "최종 코드(Python 3.12)",
        "최종코드표식",
    ] {
        assert!(
            user.contains(needle),
            "리포트 요청에 {needle} 이 없다:\n{user}"
        );
    }
    assert!(
        !user.contains(TURN_MARK),
        "리포트에는 AI 대화 원문을 싣지 않는다"
    );
    assert_no_hidden(&sent);
    assert!(!sent.to_string().contains(ORG_HIDDEN_MARK));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 감독자는_턴_제안_수락_뒤_수정_최종_코드_평가_근거를_본다(
    pool: PgPool,
) {
    let fake = FakeClaude::new(good_evaluation());
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;
    evaluations::run_once(&e.state, fake.as_ref())
        .await
        .unwrap();

    let view: ItemActivity = ok(
        &e.app,
        "GET",
        &t.item_uri("activity"),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await;
    let a = &view.activity;
    assert_eq!(a.turns.len(), 2);
    assert_eq!(a.turns[0].message, TURN_MARK);
    assert_eq!(a.turns[0].replies[0].tool_calls[0].name, "propose_edit");
    assert_eq!(a.proposals[0].decision, Decision::Accepted);
    assert!(matches!(
        a.proposals[0].after_accept,
        Some(AfterAccept::Modified { seq: Some(_) })
    ));
    assert_eq!(a.proposals[1].decision, Decision::Rejected);
    assert_eq!(a.runs.len(), 1);
    assert_eq!(view.language, Some(Language::Python));
    assert_eq!(view.final_code.as_deref(), Some(FINAL));
    let score = view.score.unwrap();
    assert_eq!(
        score.evaluation.unwrap().criteria[2].evidence,
        [Evidence::Turn(1), Evidence::Proposal(1)]
    );

    // 결과 보기 역할만: 출제자 403, 응시자(구성원 아님) 404, 없는 문항 404.
    for (sub, expect) in [(BOB, StatusCode::FORBIDDEN), (ERIN, StatusCode::NOT_FOUND)] {
        let (status, _) = call(
            &e.app,
            "GET",
            &t.item_uri("activity"),
            Some(&token_for(sub)),
            None,
        )
        .await;
        assert_eq!(status, expect, "{sub}");
    }
    let (status, _) = call(
        &e.app,
        "GET",
        &t.item_uri("activity").replace("/items/0/", "/items/5/"),
        Some(&token_for(DAVE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 키가_없으면_평가_꺼짐이고_최종은_통과율이다(pool: PgPool) {
    let e = env_with(pool, None);
    let t = taken(&e).await;
    let score = results(&e, &t).await.rows[0].items[0]
        .score
        .clone()
        .unwrap();
    assert_eq!(score.evaluation_status, EvaluationStatus::Disabled);
    assert_eq!(score.evaluation_status.label(), "AI 활용 평가 꺼짐");
    assert_eq!(score.basis, ScoreBasis::HiddenOnly);
    assert_eq!(score.final_score, score.hidden_score);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 개인_연습은_평가하지_않는다(pool: PgPool) {
    let fake = FakeClaude::new(good_evaluation());
    let e = env_with(pool.clone(), Some(fake.clone()));
    let created: SubmissionView = ok(
        &e.app,
        "POST",
        "/submissions",
        ERIN,
        Some(serde_json::to_value(practice_request("array-sum", "first-only", 30, None)).unwrap()),
        StatusCode::ACCEPTED,
    )
    .await;
    assert!(matches!(
        grade(&pool).await,
        promptlab_worker::Outcome::Graded(_)
    ));
    assert_eq!(
        evaluations::run_once(&e.state, fake.as_ref())
            .await
            .unwrap(),
        None
    );
    assert!(fake.bodies().is_empty());
    let view: SubmissionView = ok(
        &e.app,
        "GET",
        &format!("/submissions/{}", created.id),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    let SubmissionState::Done { report } = &view.state else {
        panic!("채점 끝: {:?}", view.state);
    };
    let score = view.score.expect("채점이 끝났으면 점수가 있다");
    assert_eq!(score.evaluation_status, EvaluationStatus::Practice);
    assert_eq!(score.basis, ScoreBasis::HiddenOnly);
    assert!((score.final_score.unwrap() - report.pass_rate() * 100.0).abs() < 1e-6);
}
