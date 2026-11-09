//! AI 리포트·의심 요약과 토큰·비용 기록(#15) 이음매 ②.
//!
//! - 리포트 흐름(가짜 Claude): 다 채점되면 자동 생성 → 결과 보기 역할이 본다, 실패는 상태로, 다시 만들기·횟수 상한.
//! - 점수 불변, 히든 테스트·모범 답안·채점 상세(`failure_detail`)·AI 대화 원문이 Claude 요청 본문에 없고
//!   언어·통과 수·AI 활용 평가·과정 로그 사건 종류별 개수·편집 기록·최종 코드(#99)·의심 신호가 있다, 응답 불변식(`common::send`).
//! - 리포트는 AI 활용 평가(#99)가 끝난 뒤에 큐에 든다 — `grade_both`가 키 있는 서버면 평가까지 돌린다(평가 시험은
//!   `evaluations_api.rs`).
//! - 키 비노출(가짜 Anthropic HTTP 서버), 키 없음 → 기능 꺼짐.
//! - 권한(결과 보기만, 다른 조직 404, 응시자 미노출), 비용 기록과 집계(스모크 제외), 조직당 동시 상한.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use common::*;
use promptlab_server::claude::{
    API_VERSION, AnthropicClient, BoxFuture, ClaudeError, KEY_ENV, ModelReply, ReportModel,
};
use promptlab_server::reports::{self, JobOutcome, SYSTEM_PROMPT};
use promptlab_server::{AppState, router};
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamResults, ExamView, InviteCreated, LogHead, OrgView, SubmissionView,
};
use promptlab_shared::coding::Language;
use promptlab_shared::exam::Purpose;
use promptlab_shared::log::{Attempt, LogEvent, LogSigner, SignedEvent, event_hash};
use promptlab_shared::report::{
    MAX_GENERATIONS, OrgUsage, REPORT_DISCLAIMER, REPORT_MODEL, REPORT_PRICES, ReportStatus,
    ReportView, TokenUsage, UsageKind,
};
use promptlab_store::PgPool;
use promptlab_worker::Outcome;
use promptlab_worker::fake::Mode;
use serde_json::{Value, json};

const FRANK: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2F";
/// 문항 0의 제출 코드(가짜 판정기가 `correct`를 보고 모두 맞힌다). 최종 코드는 리포트 입력에 든다(#99).
const CODE: &str = "def echo(s):\n    # 코드표식-a91 correct\n    return s\n";
const CODE_MARK: &str = "코드표식-a91";
/// AI 도우미에게 보낸 응시자 메시지(원문은 Claude에 보내지 않는다 — 개수만).
const AI_MARK: &str = "도우미표식-이 코드 어디가 틀렸어?";
const STRENGTH: &str = "리포트표식-공개 예제로 경계 조건을 확인했다";
const T0: i64 = 1_767_225_600_000;

// ---------- 가짜 Claude ----------

/// 받은 요청 본문을 남기고 정해 둔 답을 차례로 낸다(다 쓰면 마지막 것을 되풀이).
struct FakeClaude {
    bodies: Mutex<Vec<Value>>,
    replies: Mutex<Vec<Result<ModelReply, ClaudeError>>>,
}

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 12_000,
        output_tokens: 3_000,
        cache_read_tokens: 700,
        cache_write_tokens: 0,
    }
}

fn reply(text: &str, stop: &str) -> Result<ModelReply, ClaudeError> {
    Ok(ModelReply {
        text: text.to_owned(),
        model: REPORT_MODEL.to_owned(),
        stop_reason: stop.to_owned(),
        usage: usage(),
    })
}

fn good_report() -> String {
    json!({
        "strengths": [STRENGTH],
        "weaknesses": ["빈 입력을 따로 다루지 않았다"],
        "suggestions": ["경계 입력을 직접 실행해 보라"],
        "suspicion_summary": "붙여넣기 신호 1번과 포커스 이탈 신호 1번이 기록되었다. 확인이 필요할 수 있다.",
    })
    .to_string()
}

impl FakeClaude {
    fn new(replies: Vec<Result<ModelReply, ClaudeError>>) -> Arc<Self> {
        Arc::new(Self {
            bodies: Mutex::new(Vec::new()),
            replies: Mutex::new(replies),
        })
    }

    fn bodies(&self) -> Vec<Value> {
        self.bodies.lock().unwrap().clone()
    }
}

impl ReportModel for FakeClaude {
    fn complete<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<ModelReply, ClaudeError>> {
        self.bodies.lock().unwrap().push(body.clone());
        let mut replies = self.replies.lock().unwrap();
        let next = if replies.len() > 1 {
            replies.remove(0)
        } else {
            replies[0].clone()
        };
        Box::pin(async move { next })
    }
}

// ---------- 환경 ----------

struct Env {
    app: axum::Router,
    state: AppState,
    clock: Arc<FixedClock>,
    pool: PgPool,
}

fn env_with(pool: PgPool, reports: Option<Arc<dyn ReportModel>>) -> Env {
    let clock = FixedClock::at(NOW);
    let state = app_state(pool.clone(), clock.clone(), reports);
    Env {
        app: router(state.clone()),
        state,
        clock,
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

async fn status_of(app: &axum::Router, method: &str, uri: &str, sub: &str) -> StatusCode {
    call(app, method, uri, Some(&token_for(sub)), None).await.0
}

/// ALICE 관리자, BOB 출제자, CAROL 감독관, DAVE 열람자.
async fn staffed_org(app: &axum::Router, name: &str) -> OrgView {
    let org: OrgView = ok(
        app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": name, "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(app, &org.id).await;
    for (sub, role) in [
        (BOB, Role::Author),
        (CAROL, Role::Proctor),
        (DAVE, Role::Viewer),
    ] {
        let invite: InviteCreated = ok(
            app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({"role": role})),
            StatusCode::CREATED,
        )
        .await;
        let _: OrgView = ok(
            app,
            "POST",
            &format!("/invites/{}/accept", invite.token),
            sub,
            Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
            StatusCode::OK,
        )
        .await;
    }
    org
}

/// 조직·채용 시험(문항 2개)·ERIN 배정.
struct Taken {
    org: OrgView,
    exam: ExamView,
    a: AssignmentView,
}

impl Taken {
    fn report_uri(&self) -> String {
        format!(
            "/orgs/{}/exams/{}/invites/{}/report",
            self.org.id, self.exam.id, self.a.id
        )
    }
}

/// 히든 테스트에 [`ORG_HIDDEN_MARK`]가 든 조직 문제를 BOB이 만들고 발행한 것으로 둔다.
async fn org_problem(e: &Env, org: &OrgView) -> String {
    let (status, body) = author_call(
        &e.app,
        "POST",
        &format!("/orgs/{}/problems", org.id),
        BOB,
        Some(org_source("메아리")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    force_publish(&e.pool, &id).await;
    id
}

async fn assigned(e: &Env) -> Taken {
    let org = staffed_org(&e.app, "에이 회사").await;
    let problem = org_problem(e, &org).await;
    let exam: ExamView = ok(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(json!({
            "title": "채용 1차",
            "purpose": Purpose::Recruiting,
            "reveal_scores": false,
            "items": [
                {"problem_id": problem, "limits": {"time_limit_secs": 600}},
                {"problem_id": "count-vowels", "limits": {"time_limit_secs": 600}},
            ],
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
    webcam_ready(&e.app, &token_for(ERIN), &a.id).await;
    Taken { org, exam, a }
}

async fn start(e: &Env, t: &Taken, position: u32) {
    let _: Value = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/{position}/start", t.a.id),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
}

async fn submit(e: &Env, t: &Taken, position: u32, code: &str) {
    e.clock.advance(60);
    let _: SubmissionView = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/{position}/submit", t.a.id),
        ERIN,
        Some(
            logged_submit_body(
                &e.app,
                &format!("/assignments/{}/items/{position}/submit", t.a.id),
                &token_for(ERIN),
                code,
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await;
}

/// 문항 0의 과정 로그를 서명해 올린다.
async fn upload(e: &Env, t: &Taken, events: Vec<LogEvent>) {
    let signer = LogSigner::from_seed([7; 32]);
    let uri = |tail: &str| format!("/assignments/{}/items/0/{tail}", t.a.id);
    let head: LogHead = ok(
        &e.app,
        "POST",
        &uri("log/keys"),
        ERIN,
        Some(json!({"public_key": signer.public_key()})),
        StatusCode::OK,
    )
    .await;
    let attempt = Attempt {
        assignment_id: t.a.id.clone(),
        position: 0,
    };
    let mut prev = head.head.clone();
    let signed: Vec<SignedEvent> = events
        .into_iter()
        .enumerate()
        .map(|(i, event)| {
            let s = signer.seal(
                &attempt,
                head.next_seq + i as u64,
                &prev,
                T0 + i as i64 * 1000,
                event,
            );
            prev = event_hash(&s.body);
            s
        })
        .collect();
    let _: LogHead = ok(
        &e.app,
        "POST",
        &uri("log"),
        ERIN,
        Some(json!({ "events": signed })),
        StatusCode::OK,
    )
    .await;
}

/// 두 문항을 풀고 낸다(문항 0은 과정 로그와 함께). 채점은 하지 않는다.
async fn taken(e: &Env) -> Taken {
    let t = assigned(e).await;
    start(e, &t, 0).await;
    upload(
        e,
        &t,
        vec![
            LogEvent::Started,
            LogEvent::LanguageChosen {
                language: Language::Javascript,
            },
            LogEvent::AiMessage {
                text: AI_MARK.into(),
            },
            LogEvent::PasteBlocked { chars: 40 },
            LogEvent::FocusLost,
            LogEvent::FocusReturned { away_ms: 5_000 },
            LogEvent::NoFace {
                snapshot: Some("스냅샷참조-9d1".into()),
            },
        ],
    )
    .await;
    // 제출이 언어 고름(Python)·코드 스냅숏·제출 사건을 더 올린다.
    submit(e, &t, 0, CODE).await;
    start(e, &t, 1).await;
    submit(e, &t, 1, "def max_subarray(nums):\n    return 0\n").await;
    t
}

/// 공통 4항목 모두 3점인 AI 활용 평가.
fn good_evaluation() -> String {
    let criteria: Vec<Value> = ["instruction", "verification", "critique", "efficiency"]
        .iter()
        .map(|key| json!({"key": key, "score": 3, "reason": "보통", "evidence": []}))
        .collect();
    json!({ "criteria": criteria }).to_string()
}

/// 두 제출을 채점한다: 문항 0은 히든 6/6, 문항 1은 판정기가 받지 않아 채점 실패. 키 있는 서버면 AI 활용 평가(문항 0)까지
/// 끝낸다(따로 둔 가짜 Claude — 리포트 가짜의 요청 기록에 섞이지 않는다).
async fn grade_both(e: &Env) {
    let judge = judge();
    assert!(matches!(
        promptlab_worker::run_once(&e.pool, &judge).await.unwrap(),
        Outcome::Graded(_)
    ));
    judge.set_mode(Mode::Rejected);
    assert!(matches!(
        promptlab_worker::run_once(&e.pool, &judge).await.unwrap(),
        Outcome::Failed(_)
    ));
    if e.state.reports.is_some() {
        let eval = FakeClaude::new(vec![reply(&good_evaluation(), "end_turn")]);
        while promptlab_server::evaluations::run_once(&e.state, eval.as_ref())
            .await
            .unwrap()
            .is_some()
        {}
    }
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

async fn report(e: &Env, t: &Taken, sub: &str) -> ReportView {
    ok(&e.app, "GET", &t.report_uri(), sub, None, StatusCode::OK).await
}

async fn org_usage(e: &Env, org: &OrgView) -> OrgUsage {
    ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/usage", org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await
}

// ---------- 시험 ----------

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 다_채점되면_리포트를_자동으로_만들고_점수는_그대로다(pool: PgPool) {
    let fake = FakeClaude::new(vec![reply(&good_report(), "end_turn")]);
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;

    // 채점 전: 자동 생성 대상이 아니다.
    assert_eq!(
        reports::run_once(&e.state, fake.as_ref()).await.unwrap(),
        None
    );
    assert_eq!(report(&e, &t, DAVE).await.status, ReportStatus::None);

    grade_both(&e).await;
    let before = results(&e, &t).await;
    let done = reports::run_once(&e.state, fake.as_ref()).await.unwrap();
    assert!(matches!(done, Some(JobOutcome::Done(_))), "{done:?}");
    // 다음 걸음에는 할 일이 없다(한 번만 만든다).
    assert_eq!(
        reports::run_once(&e.state, fake.as_ref()).await.unwrap(),
        None
    );

    let view = report(&e, &t, DAVE).await;
    assert_eq!(view.status, ReportStatus::Done);
    let body = view.report.unwrap();
    assert_eq!(body.strengths, vec![STRENGTH.to_string()]);
    assert!(body.suspicion_summary.contains("확인이 필요할 수 있다"));
    assert_eq!(view.model.as_deref(), Some(REPORT_MODEL));
    assert_eq!(view.generations, 1);
    assert_eq!(view.disclaimer, REPORT_DISCLAIMER);
    assert!(view.can_request, "다시 만들 수 있다");

    // 점수는 그대로다.
    assert_eq!(results(&e, &t).await, before);

    // 보낸 요청: 고정 모델·시스템 프롬프트(캐시 지점)·구조화 출력, 과정 로그·채점 요약·의심 신호가 들었다.
    let bodies = fake.bodies();
    assert_eq!(bodies.len(), 1);
    let sent = &bodies[0];
    assert_eq!(sent["model"], REPORT_MODEL);
    assert_eq!(sent["system"][0]["text"], SYSTEM_PROMPT);
    assert_eq!(sent["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(sent["output_config"]["format"]["type"], "json_schema");
    let user = sent["messages"][0]["content"].as_str().unwrap();
    for needle in [
        "언어 Python 3.12",
        "히든 테스트 통과 6/6",
        "언어 고름: JavaScript (Node 22) → Python 3.12",
        "코드 스냅숏 1개",
        "응시자 메시지 1개",
        "붙여넣기 막음 1번(40자)",
        "포커스 이탈 1번",
        "얼굴 없음 1번",
        "제출 사건 1개",
        "AI 활용 평가 60.0점",
        "최종 코드(Python 3.12)",
        CODE_MARK,
        "채점 실패(점수 0)",
        "채용 1차",
        "메아리",
    ] {
        assert!(user.contains(needle), "요청에 {needle} 이 없다:\n{user}");
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 히든_테스트_모범_답안_채점_상세_ai_대화_웹캠_참조_이메일은_claude에_보내지_않는다(
    pool: PgPool,
) {
    let fake = FakeClaude::new(vec![reply(&good_report(), "end_turn")]);
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;
    grade_both(&e).await;
    // 채점 상세(failure_detail)는 서버 안에만 — 그것이 새지 않는지 본다.
    let detail: String = sqlx::query_scalar(
        "SELECT failure_detail FROM submissions WHERE failure_detail IS NOT NULL",
    )
    .fetch_one(&e.pool)
    .await
    .unwrap();
    reports::run_once(&e.state, fake.as_ref()).await.unwrap();

    let sent = fake.bodies().remove(0);
    // 응답 불변식과 같은 검사(내장 히든 테스트·모범 답안·조직 문제 표식·금지 키).
    assert_no_hidden(&sent);
    let text = sent.to_string();
    assert!(
        !text.contains(ORG_HIDDEN_MARK),
        "조직 문제 히든 테스트가 요청에 있다"
    );
    assert!(!text.contains(AI_MARK), "AI 도우미 대화 원문이 요청에 있다");
    assert!(!text.contains(&detail), "failure_detail 이 요청에 있다");
    assert!(!text.contains("erin@example.com"), "이메일이 요청에 있다");
    assert!(
        !text.contains("스냅샷참조-9d1"),
        "웹캠 스냅샷 참조가 요청에 있다"
    );

    // 새 응답(리포트·사용량)도 응답 불변식을 지난다(`call`이 건다).
    let _ = report(&e, &t, ALICE).await;
    let _ = org_usage(&e, &t.org).await;
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 결과_보기_역할만_보고_응시자와_다른_조직은_못_본다(pool: PgPool) {
    let fake = FakeClaude::new(vec![reply(&good_report(), "end_turn")]);
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;
    grade_both(&e).await;
    reports::run_once(&e.state, fake.as_ref()).await.unwrap();

    for sub in [ALICE, CAROL, DAVE] {
        assert_eq!(report(&e, &t, sub).await.status, ReportStatus::Done);
    }
    assert_eq!(
        status_of(&e.app, "GET", &t.report_uri(), BOB).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(&e.app, "POST", &t.report_uri(), BOB).await,
        StatusCode::FORBIDDEN
    );
    // 응시자 본인(구성원 아님)과 다른 조직 사람은 404.
    let _ = ok::<OrgView>(
        &e.app,
        "POST",
        "/orgs",
        FRANK,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    for sub in [ERIN, FRANK] {
        assert_eq!(
            status_of(&e.app, "GET", &t.report_uri(), sub).await,
            StatusCode::NOT_FOUND
        );
    }
    // 응시자에게 보이는 응답에는 리포트가 없다(채용 — 「응시 완료」만).
    let (_, mine) = call(
        &e.app,
        "GET",
        "/me/assignments",
        Some(&token_for(ERIN)),
        None,
    )
    .await;
    let mine = mine.to_string();
    assert!(
        !mine.contains(STRENGTH) && !mine.contains("suspicion"),
        "{mine}"
    );

    // 사용량은 구성원 관리(관리자)만.
    let usage_uri = format!("/orgs/{}/usage", t.org.id);
    for (sub, expect) in [
        (BOB, StatusCode::FORBIDDEN),
        (CAROL, StatusCode::FORBIDDEN),
        (DAVE, StatusCode::FORBIDDEN),
        (FRANK, StatusCode::NOT_FOUND),
        (ALICE, StatusCode::OK),
    ] {
        assert_eq!(
            status_of(&e.app, "GET", &usage_uri, sub).await,
            expect,
            "{sub}"
        );
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 실패는_상태로_보이고_다시_만들기는_횟수_상한까지(pool: PgPool) {
    let fake = FakeClaude::new(vec![
        reply("", "refusal"),
        Err(ClaudeError::Status(529)),
        reply("형식이 아닌 글", "end_turn"),
    ]);
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;

    // 채점이 끝나기 전에는 요청할 수 없다.
    assert_eq!(
        status_of(&e.app, "POST", &t.report_uri(), DAVE).await,
        StatusCode::CONFLICT
    );
    grade_both(&e).await;

    let mut reasons = Vec::new();
    for round in 0..MAX_GENERATIONS {
        if round > 0 {
            let queued: ReportView = ok(
                &e.app,
                "POST",
                &t.report_uri(),
                DAVE,
                None,
                StatusCode::ACCEPTED,
            )
            .await;
            assert_eq!(queued.status, ReportStatus::Queued);
            assert!(!queued.can_request);
            // 대기 중에 또 요청하면 409.
            assert_eq!(
                status_of(&e.app, "POST", &t.report_uri(), DAVE).await,
                StatusCode::CONFLICT
            );
        }
        let outcome = reports::run_once(&e.state, fake.as_ref()).await.unwrap();
        assert!(
            matches!(outcome, Some(JobOutcome::Failed(..))),
            "{outcome:?}"
        );
        let view = report(&e, &t, DAVE).await;
        assert_eq!(view.status, ReportStatus::Failed);
        assert_eq!(view.generations, round + 1);
        reasons.push(view.failure_reason.unwrap());
    }
    assert_eq!(
        reasons,
        vec![
            "AI가 리포트 작성을 거절했다",
            "AI 서비스 오류(529)",
            "AI 응답 형식이 맞지 않다"
        ]
    );
    let view = report(&e, &t, DAVE).await;
    assert!(!view.can_request);
    assert_eq!(
        status_of(&e.app, "POST", &t.report_uri(), DAVE).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 키가_없으면_리포트만_꺼지고_나머지는_돈다(pool: PgPool) {
    if std::env::var(KEY_ENV).is_err() {
        assert!(AnthropicClient::from_env().is_none());
    }
    let e = env_with(pool, None);
    let t = taken(&e).await;
    grade_both(&e).await;
    // 결과·채점은 그대로 돈다.
    assert_eq!(results(&e, &t).await.rows.len(), 1);
    let view = report(&e, &t, DAVE).await;
    assert_eq!(view.status, ReportStatus::Disabled);
    assert!(!view.can_request && view.report.is_none());
    assert_eq!(
        status_of(&e.app, "POST", &t.report_uri(), DAVE).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 호출마다_토큰과_추정_비용을_남기고_조직_시험으로_묶는다(
    pool: PgPool,
) {
    let fake = FakeClaude::new(vec![reply(&good_report(), "end_turn")]);
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;
    grade_both(&e).await;
    reports::run_once(&e.state, fake.as_ref()).await.unwrap();

    let u = org_usage(&e, &t.org).await;
    assert_eq!(u.attempts, 1);
    let kinds: Vec<UsageKind> = u.by_kind.iter().map(|k| k.kind).collect();
    // 판정기 채점은 토큰이 없어 기록하지 않는다 — AI 리포트와 AI 활용 평가(문항 0, #99).
    assert_eq!(kinds, vec![UsageKind::Report, UsageKind::Evaluation]);
    let report_totals = &u.by_kind[0].totals;
    assert_eq!(report_totals.calls, 1);
    assert_eq!(report_totals.input_tokens, 12_000);
    assert_eq!(report_totals.cache_read_tokens, 700);
    let expected = REPORT_PRICES.cost_usd(&usage());
    assert!((report_totals.cost_usd - expected).abs() < 1e-9);
    assert_eq!(u.by_kind[1].totals.calls, 1);
    assert!((u.cost_usd - 2.0 * expected).abs() < 1e-9);
    assert_eq!(u.exams.len(), 1);
    assert_eq!(u.exams[0].attempts, 1);
    assert_eq!(u.exams[0].by_kind, u.by_kind);

    // 단가는 기록 시점 값이 함께 남는다.
    let (price, model): (f64, String) =
        sqlx::query_as("SELECT price_output, model FROM usage_records WHERE kind = 'report'")
            .fetch_one(&e.pool)
            .await
            .unwrap();
    assert_eq!(
        (price, model.as_str()),
        (REPORT_PRICES.output, REPORT_MODEL)
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크_조직은_리포트도_집계도_없다(pool: PgPool) {
    let fake = FakeClaude::new(vec![reply(&good_report(), "end_turn")]);
    let e = env_with(pool, Some(fake.clone()));
    let t = taken(&e).await;
    sqlx::query("UPDATE orgs SET is_smoke = true WHERE id = $1::uuid")
        .bind(&t.org.id)
        .execute(&e.pool)
        .await
        .unwrap();
    // 스모크 조직의 제출은 AI 활용 평가 큐에도 들지 않는다(grade_both가 평가를 돌려도 할 일이 없다).
    grade_both(&e).await;
    // 스모크 조직의 응시는 자동 생성 대상이 아니다(Claude를 부르지 않는다).
    assert_eq!(
        reports::run_once(&e.state, fake.as_ref()).await.unwrap(),
        None
    );
    assert!(fake.bodies().is_empty());
    // 비용 기록도 없다(판정기 채점은 기록하지 않는다).
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_records")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let u = promptlab_store::usage::org_usage(&e.pool, t.org.id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(u.attempts, 0);
    assert!(u.by_kind.is_empty() && u.exams.is_empty());
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 조직마다_동시에_만드는_리포트는_상한까지(pool: PgPool) {
    use promptlab_store::reports::{claim_next, request};
    let e = env_with(pool, None);
    let t = assigned(&e).await;
    let org_id: uuid::Uuid = t.org.id.parse().unwrap();
    // 같은 조직 응시 셋을 큐에 넣는다(초대 행만 있으면 된다).
    let mut ids = vec![t.a.id.parse::<uuid::Uuid>().unwrap()];
    for email in ["x@example.com", "y@example.com"] {
        let created: Value = ok(
            &e.app,
            "POST",
            &format!("/orgs/{}/exams/{}/invites", t.org.id, t.exam.id),
            BOB,
            Some(json!({"emails": [email]})),
            StatusCode::CREATED,
        )
        .await;
        ids.push(
            created["invites"][0]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        );
    }
    for id in &ids {
        request(&e.pool, *id, org_id, NOW).await.unwrap();
    }
    let limit = reports::ORG_CONCURRENCY;
    for _ in 0..limit {
        assert!(claim_next(&e.pool, limit, NOW).await.unwrap().is_some());
    }
    assert_eq!(claim_next(&e.pool, limit, NOW).await.unwrap(), None);
}

// ---------- 실제 HTTP 클라이언트(가짜 Anthropic 서버) ----------

const KEY: &str = "sk-ant-시험키-5b7e";

/// 가짜 Anthropic 서버: 정해 둔 (상태, 본문)을 차례로 내고 받은 헤더를 남긴다.
struct FakeApi {
    calls: AtomicUsize,
    plan: Vec<(u16, Value)>,
    headers: Mutex<Vec<HeaderMap>>,
}

async fn fake_api(plan: Vec<(u16, Value)>) -> (String, Arc<FakeApi>) {
    let api = Arc::new(FakeApi {
        calls: AtomicUsize::new(0),
        plan,
        headers: Mutex::new(Vec::new()),
    });
    let shared = api.clone();
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(move |headers: HeaderMap, _body: String| {
            let api = shared.clone();
            async move {
                api.headers.lock().unwrap().push(headers);
                let n = api.calls.fetch_add(1, Ordering::SeqCst);
                let (status, body) = api.plan[n.min(api.plan.len() - 1)].clone();
                (StatusCode::from_u16(status).unwrap(), axum::Json(body))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), api)
}

fn api_reply() -> Value {
    json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": REPORT_MODEL,
        "stop_reason": "end_turn",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "x"},
            {"type": "text", "text": good_report()},
        ],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 20,
        },
    })
}

#[tokio::test]
async fn http_클라이언트는_키를_헤더에만_싣고_일시_오류는_다시_한다() {
    let (url, api) = fake_api(vec![
        (
            529,
            json!({"type": "error", "error": {"type": "overloaded_error"}}),
        ),
        (200, api_reply()),
    ])
    .await;
    let client = AnthropicClient::new(KEY, &url, Duration::from_millis(1));
    assert!(!format!("{client:?}").contains(KEY), "Debug에 키가 있다");
    let got = client
        .complete(&reports::request_body("응시 기록"))
        .await
        .unwrap();
    assert_eq!(api.calls.load(Ordering::SeqCst), 2, "529 뒤 한 번 더");
    assert_eq!(got.text, good_report(), "사고 블록은 빼고 글만");
    assert_eq!(
        got.usage,
        TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 30,
            cache_write_tokens: 20,
        }
    );
    let headers = api.headers.lock().unwrap();
    assert_eq!(headers[1]["x-api-key"], KEY);
    assert_eq!(headers[1]["anthropic-version"], API_VERSION);
}

#[tokio::test]
async fn http_오류에는_키도_응답_본문도_싣지_않는다() {
    // 서버가 오류 본문에 키를 되돌려 줘도 오류 메시지에는 상태 코드만 남는다.
    let (url, api) = fake_api(vec![(
        401,
        json!({"type": "error", "error": {"type": "authentication_error", "message": format!("bad key {KEY}")}}),
    )])
    .await;
    let client = AnthropicClient::new(KEY, &url, Duration::from_millis(1));
    let err = client
        .complete(&reports::request_body("응시 기록"))
        .await
        .unwrap_err();
    assert_eq!(err, ClaudeError::Status(401));
    assert_eq!(
        api.calls.load(Ordering::SeqCst),
        1,
        "401은 다시 하지 않는다"
    );
    assert!(!err.to_string().contains(KEY) && !format!("{err:?}").contains(KEY));

    // 계속 429면 상한까지만 하고 실패.
    let (url, api) = fake_api(vec![(429, json!({}))]).await;
    let client = AnthropicClient::new(KEY, &url, Duration::from_millis(1));
    let err = client.complete(&json!({})).await.unwrap_err();
    assert_eq!(err, ClaudeError::Status(429));
    assert_eq!(
        api.calls.load(Ordering::SeqCst),
        promptlab_server::claude::MAX_ATTEMPTS as usize
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 실제_클라이언트로_실패해도_리포트_응답에_키가_없다(pool: PgPool) {
    let (url, _api) = fake_api(vec![(
        400,
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": KEY}}),
    )])
    .await;
    let client: Arc<dyn ReportModel> =
        Arc::new(AnthropicClient::new(KEY, &url, Duration::from_millis(1)));
    let e = env_with(pool, Some(client.clone()));
    let t = taken(&e).await;
    grade_both(&e).await;
    let outcome = reports::run_once(&e.state, client.as_ref()).await.unwrap();
    assert!(matches!(outcome, Some(JobOutcome::Failed(..))));
    let (_, body) = call(&e.app, "GET", &t.report_uri(), Some(&token_for(DAVE)), None).await;
    assert_eq!(body["failure_reason"], "AI 서비스 오류(400)");
    assert!(!body.to_string().contains(KEY));
    let stored: String = sqlx::query_scalar("SELECT failure_reason FROM exam_reports")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert!(!stored.contains(KEY));
}
