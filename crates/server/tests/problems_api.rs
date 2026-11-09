//! 시험 이음매 ②: 조직 코딩 문제 편집·발행 전 점검·발행(#16·#73).
//!
//! 조직 문제의 히든 테스트·모범 답안에는 `ORG_HIDDEN_MARK`를 넣는다(`common::org_source`) — 출제자 편집 응답
//! ([`author_call`]) 말고 어떤 응답(응시자·앱·다른 역할·다른 조직·문제은행·결과·점검)에 보여도 `common::call`이
//! 실패시킨다. 채점·점검은 `worker::run_once` + 가짜 판정기로 돈다.

mod common;

use axum::http::StatusCode;
use common::*;
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamInvitesCreated, ExamView, InviteCreated, ItemStarted, OrgView,
    SubmissionState, SubmissionView,
};
use promptlab_shared::authoring::{
    OrgProblem, OrgProblemSummary, PreviewState, PreviewView, Publication,
};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_store::PgPool;
use promptlab_worker::fake::{FakeJudge, Mode};
use promptlab_worker::{Outcome, run_once};
use serde_json::{Value, json};

/// 가짜 판정기: 코드에 `return s`(픽스처 모범 답안)나 `correct`가 있으면 문자열 인자를 그대로 돌려주고, 아니면
/// 런타임 오류.
fn echo_judge() -> FakeJudge {
    FakeJudge::new(|code, args| match args {
        [Value::String(s)] if code.contains("return s") || code.contains("correct") => {
            Some(Value::String(s.clone()))
        }
        _ => None,
    })
}

fn source() -> Value {
    org_source("메아리")
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

/// 출제자 편집 응답(숨은 입력 포함)을 받는다.
async fn authored<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
    expect: StatusCode,
) -> T {
    let (status, body) = author_call(app, method, uri, sub, body).await;
    assert_eq!(status, expect, "{method} {uri}: {body}");
    serde_json::from_value(body).unwrap()
}

async fn status_of(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(app, method, uri, Some(&token_for(sub)), body).await
}

/// ALICE(관리자)의 조직에 BOB 출제자, CAROL 감독관, DAVE 열람자.
async fn staffed_org(app: &axum::Router) -> OrgView {
    let org: OrgView = ok(
        app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
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

/// ERIN이 따로 만든 조직(다른 테넌트).
async fn other_org(app: &axum::Router) -> OrgView {
    ok(
        app,
        "POST",
        "/orgs",
        ERIN,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await
}

async fn create_problem(app: &axum::Router, org: &OrgView, by: &str) -> OrgProblem {
    authored(
        app,
        "POST",
        &format!("/orgs/{}/problems", org.id),
        by,
        Some(source()),
        StatusCode::CREATED,
    )
    .await
}

/// 워커를 큐가 빌 때까지 돌리고 한 일들을 돌려준다.
async fn drain_with(pool: &PgPool, judge: &FakeJudge) -> Vec<Outcome> {
    let mut steps = Vec::new();
    loop {
        match run_once(pool, judge).await.unwrap() {
            Outcome::Idle => return steps,
            step => steps.push(step),
        }
    }
}

async fn drain(pool: &PgPool) -> Vec<Outcome> {
    drain_with(pool, &echo_judge()).await
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 출제자가_조직_문제를_만들고_고치고_관리자도_본다(pool: PgPool) {
    let app = app(pool);
    let org = staffed_org(&app).await;
    let created = create_problem(&app, &org, BOB).await;
    assert!(created.id.starts_with("org-"));
    assert_eq!(created.source.hidden.len(), 6);
    assert!(
        created.source.hidden[0].args[0]
            .to_string()
            .contains(ORG_HIDDEN_MARK)
    );
    assert!(created.source.reference_solution.contains(ORG_HIDDEN_MARK));

    // 목록에는 숨은 입력이 없다(엄격한 불변식 검사 `call`로 받는다).
    let list: Vec<OrgProblemSummary> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problems", org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].hidden_count, 6);
    assert_eq!(list[0].example_count, 1);
    assert_eq!(list[0].title, "메아리");

    let mut edited = source();
    edited["description"] = json!("고친 설명");
    edited["hidden"].as_array_mut().unwrap().pop();
    let updated: OrgProblem = authored(
        &app,
        "PUT",
        &format!("/orgs/{}/problems/{}", org.id, created.id),
        ALICE,
        Some(edited),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated.source.hidden.len(), 5);
    let fetched: OrgProblem = authored(
        &app,
        "GET",
        &format!("/orgs/{}/problems/{}", org.id, created.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched.source.description, "고친 설명");
    assert!(fetched.source.reference_solution.contains("return s"));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 출제자_관리자만_편집하고_다른_조직은_404(pool: PgPool) {
    let app = app(pool);
    let org = staffed_org(&app).await;
    let other = other_org(&app).await;
    let problem = create_problem(&app, &org, BOB).await;
    let list = format!("/orgs/{}/problems", org.id);
    let one = format!("/orgs/{}/problems/{}", org.id, problem.id);
    let previews = format!("/orgs/{}/problem-previews", org.id);
    let requests: [(&str, &str, Option<Value>); 5] = [
        ("GET", &list, None),
        ("POST", &list, Some(source())),
        ("GET", &one, None),
        ("PUT", &one, Some(source())),
        ("POST", &previews, Some(json!({"problem_id": problem.id}))),
    ];
    for (method, uri, body) in &requests {
        // 감독관·열람자: 구성원이지만 시험 관리 권한이 없다.
        for sub in [CAROL, DAVE] {
            let (status, _) = status_of(&app, method, uri, sub, body.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{sub} {method} {uri}");
        }
        // 다른 조직의 관리자: 있는지도 알리지 않는다.
        let (status, _) = status_of(&app, method, uri, ERIN, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "ERIN {method} {uri}");
    }
    // 다른 조직 경로로 이 조직의 문제를 찾아도 404.
    for uri in [format!("/orgs/{}/problems/{}", other.id, problem.id)] {
        let (status, _) = status_of(&app, "GET", &uri, ERIN, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let (status, _) = status_of(
        &app,
        "PUT",
        &format!("/orgs/{}/problems/{}", other.id, problem.id),
        ERIN,
        Some(source()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 다른 조직의 문제은행에는 이 조직의 문제가 없다.
    let bank: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-bank", other.id),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(bank.iter().all(|p| p.id != problem.id));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 잘못된_문제는_422(pool: PgPool) {
    let app = app(pool);
    let org = staffed_org(&app).await;
    let uri = format!("/orgs/{}/problems", org.id);
    let with = |key: &str, value: Value| {
        let mut body = source();
        body[key] = value;
        body
    };
    for (body, needle) in [
        (with("hidden", json!([])), "히든"),
        (with("description", json!("  ")), "설명"),
        (with("title", json!("")), "제목"),
        (
            with(
                "signature",
                json!({"name": "class", "params": [{"name": "s", "type": "string"}], "returns": "string"}),
            ),
            "class",
        ),
        (
            with(
                "reference_solution",
                json!("#".repeat(promptlab_shared::coding::MAX_CODE_BYTES + 1)),
            ),
            "모범 답안",
        ),
    ] {
        let (status, error) = status_of(&app, "POST", &uri, BOB, Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
        assert!(error.to_string().contains(needle), "{needle}: {error}");
    }
    // 점검은 저장된 문제만 받는다(없는 문제 404 — 큐에 넣지 않는다).
    let (status, _) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/problem-previews", org.id),
        BOB,
        Some(json!({"problem_id": "org-없음"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 조직_문제로_시험을_치르고_통과_수로_채점한다(pool: PgPool) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let other = other_org(&app).await;
    // 승인된 조직이어야 문제 확인(422)까지 간다(#19).
    common::approve_org(&app, &other.id).await;
    let problem = create_problem(&app, &org, BOB).await;
    // 발행 전 점검(#73)은 따로 시험한다 — 여기서는 발행한 문제로 친다.
    common::force_publish(&pool, &problem.id).await;

    // 문제은행(시험 만들기의 문항 고르기)에 이 조직의 문제가 공개 보기로 나온다.
    let bank: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    let mine = bank.iter().find(|p| p.id == problem.id).expect("조직 문제");
    assert_eq!(mine.examples.len(), 1);
    assert_eq!(mine.templates.len(), 4);

    let exam_body = json!({
        "title": "사내 진단",
        "purpose": "internal",
        "reveal_scores": true,
        "items": [{"problem_id": problem.id, "limits": {"time_limit_secs": 600}}],
    });
    // 다른 조직은 이 문제로 시험을 만들지 못한다.
    let (status, _) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/exams", other.id),
        ERIN,
        Some(exam_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let exam: ExamView = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(exam_body),
        StatusCode::CREATED,
    )
    .await;
    let created: ExamInvitesCreated = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": ["frank@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created.invites[0].token.clone().unwrap();
    // 응시자(ERIN — 다른 조직 사람이어도 초대받으면 응시한다).
    let assignment: AssignmentView = ok(
        &app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    webcam_ready(&app, &token_for(ERIN), &assignment.id).await;
    let started: ItemStarted = ok(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/start", assignment.id),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(started.problem.id, problem.id);
    assert_eq!(started.problem.examples[0].args, vec![json!("abc")]);
    let submitted: SubmissionView = ok(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/submit", assignment.id),
        ERIN,
        Some(
            logged_submit_body(
                &app,
                &format!("/assignments/{}/items/0/submit", assignment.id),
                &token_for(ERIN),
                "def echo(s):\n    return s  # correct\n",
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await;

    let steps = drain(&pool).await;
    assert!(steps.contains(&Outcome::Graded(submitted.id.parse().unwrap())));
    let view: SubmissionView = ok(
        &app,
        "GET",
        &format!("/submissions/{}", submitted.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    let SubmissionState::Done { report } = view.state else {
        panic!("채점이 끝나야 한다: {view:?}")
    };
    assert_eq!((report.passed, report.total), (6, 6));
}

/// 발행할 수 있는 원본(모범 답안이 모든 케이스를 통과한다). `description`만 바꿔 원본을 달리한다.
fn publishable_source(description: &str) -> Value {
    let mut source = source();
    source["description"] = json!(description);
    source
}

async fn create_from(app: &axum::Router, org: &OrgView, source: Value) -> OrgProblem {
    authored(
        app,
        "POST",
        &format!("/orgs/{}/problems", org.id),
        BOB,
        Some(source),
        StatusCode::CREATED,
    )
    .await
}

/// 점검을 넣고 워커로 끝까지 돌린 뒤 결과를 본다.
async fn check(app: &axum::Router, pool: &PgPool, org: &OrgView, problem_id: &str) -> PreviewView {
    let started: PreviewView = ok(
        app,
        "POST",
        &format!("/orgs/{}/problem-previews", org.id),
        BOB,
        Some(json!({"problem_id": problem_id})),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(started.state, PreviewState::Queued);
    assert!(started.current);
    let steps = drain(pool).await;
    assert!(
        steps.iter().any(|s| matches!(s, Outcome::Checked(_))),
        "{steps:?}"
    );
    ok(
        app,
        "GET",
        &format!("/orgs/{}/problem-previews/{}", org.id, started.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await
}

fn exam_body(problem_id: &str) -> Value {
    json!({"title": "발행 시험", "purpose": "internal",
           "items": [{"problem_id": problem_id, "limits": {"time_limit_secs": 600}}]})
}

async fn bank_ids(app: &axum::Router, org: &OrgView) -> Vec<String> {
    let bank: Vec<PublicCodingProblem> = ok(
        app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    bank.into_iter().map(|p| p.id).collect()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 점검을_통과해야_발행되고_그때부터_시험에_넣는다(pool: PgPool) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let problem = create_from(&app, &org, publishable_source("입력에 답: 을 붙여라.")).await;
    assert!(!problem.publication.published);
    assert!(problem.publication.last_check.is_none());

    // 발행 전: 문제은행에 없고 시험 만들기 422, 발행도 422(점검 안 함).
    assert!(!bank_ids(&app, &org).await.contains(&problem.id));
    let (status, _) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(exam_body(&problem.id)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let publish_uri = format!("/orgs/{}/problems/{}/publish", org.id, problem.id);
    let (status, body) = status_of(&app, "POST", &publish_uri, BOB, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("점검"), "{body}");

    let view = check(&app, &pool, &org, &problem.id).await;
    assert_eq!(view.state, PreviewState::Done);
    assert!(view.current);
    assert_eq!((view.example_count, view.hidden_count), (1, 6));
    let result = view.result.as_ref().unwrap();
    assert_eq!((result.passed, result.total, result.compiled), (7, 7, true));
    // 4개 언어 빈 틀이 모두 컴파일된다(#96).
    assert_eq!(
        result
            .templates
            .iter()
            .map(|t| t.language)
            .collect::<Vec<_>>(),
        promptlab_shared::coding::Language::ALL.to_vec()
    );
    assert!(result.templates.iter().all(|t| t.compiled));
    assert!(view.publishable);
    assert_eq!(view.reason, None);

    let published: Publication = ok(&app, "POST", &publish_uri, BOB, None, StatusCode::OK).await;
    assert!(published.published);
    assert_eq!(
        published.last_check.as_ref().map(|c| c.id.as_str()),
        Some(view.id.as_str())
    );
    // 두 번 눌러도 그대로(멱등).
    let again: Publication = ok(&app, "POST", &publish_uri, ALICE, None, StatusCode::OK).await;
    assert_eq!(again.published_at, published.published_at);

    assert!(bank_ids(&app, &org).await.contains(&problem.id));
    let list: Vec<OrgProblemSummary> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problems", org.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(list[0].published);
    let _: ExamView = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(exam_body(&problem.id)),
        StatusCode::CREATED,
    )
    .await;
}

async fn check_with(
    app: &axum::Router,
    pool: &PgPool,
    org: &OrgView,
    problem_id: &str,
    judge: &FakeJudge,
) -> PreviewView {
    let started: PreviewView = ok(
        app,
        "POST",
        &format!("/orgs/{}/problem-previews", org.id),
        BOB,
        Some(json!({"problem_id": problem_id})),
        StatusCode::ACCEPTED,
    )
    .await;
    drain_with(pool, judge).await;
    ok(
        app,
        "GET",
        &format!("/orgs/{}/problem-previews/{}", org.id, started.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 모범_답안이_틀리면_점검_불통과_발행_422(pool: PgPool) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let mut wrong = source();
    wrong["reference_solution"] = json!(format!(
        "def echo(s):\n    # {ORG_HIDDEN_MARK}\n    return s[::-1]\n"
    ));
    // 첫 히든 케이스만 맞히는 판정기: 예제 1 + 히든 1 통과.
    let judge = FakeJudge::new(|code, args| match args {
        [Value::String(s)] if code.contains("s[::-1]") && (s == "abc" || s.ends_with("-1")) => {
            Some(Value::String(s.clone()))
        }
        _ => None,
    });
    let problem = create_from(&app, &org, wrong).await;
    let view = check_with(&app, &pool, &org, &problem.id, &judge).await;
    assert_eq!(view.state, PreviewState::Done);
    let result = view.result.as_ref().unwrap();
    assert_eq!((result.passed, result.total, result.compiled), (2, 7, true));
    assert!(!view.publishable);
    assert!(
        view.reason.as_deref().unwrap().contains("2 / 7"),
        "{view:?}"
    );
    let (status, body) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/problems/{}/publish", org.id, problem.id),
        BOB,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("2 / 7"), "{body}");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 모범_답안이_컴파일되지_않거나_판정기가_안되면_발행하지_못한다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let problem = create_from(&app, &org, source()).await;
    let publish_uri = format!("/orgs/{}/problems/{}/publish", org.id, problem.id);

    let judge = echo_judge();
    judge.set_mode(Mode::CompileError);
    let view = check_with(&app, &pool, &org, &problem.id, &judge).await;
    assert_eq!(view.state, PreviewState::Done);
    let result = view.result.as_ref().unwrap();
    assert!(!result.compiled);
    assert_eq!(result.passed, 0);
    assert!(!view.publishable);
    assert!(
        view.reason.as_deref().unwrap().contains("컴파일"),
        "{view:?}"
    );
    let (status, _) = status_of(&app, "POST", &publish_uri, BOB, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // 판정기가 받지 않으면(422) 점검 실패 — 짧은 까닭만.
    judge.set_mode(Mode::Rejected);
    let view = check_with(&app, &pool, &org, &problem.id, &judge).await;
    assert_eq!(view.state, PreviewState::Failed);
    assert!(view.failure_reason.is_some());
    assert!(!view.publishable);
    let (status, body) = status_of(&app, "POST", &publish_uri, BOB, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("실패"), "{body}");

    // 판정기가 고쳐지면 다시 점검해 발행한다.
    judge.set_mode(Mode::Run);
    let view = check_with(&app, &pool, &org, &problem.id, &judge).await;
    assert!(view.publishable, "{view:?}");
    let _: Publication = ok(&app, "POST", &publish_uri, BOB, None, StatusCode::OK).await;
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 문제를_고치면_발행이_풀리고_다시_점검해야_하며_이미_만든_시험은_그대로(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let problem = create_from(&app, &org, publishable_source("입력에 답: 을 붙여라.")).await;
    let first = check(&app, &pool, &org, &problem.id).await;
    let publish_uri = format!("/orgs/{}/problems/{}/publish", org.id, problem.id);
    let _: Publication = ok(&app, "POST", &publish_uri, BOB, None, StatusCode::OK).await;
    let exam: ExamView = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(exam_body(&problem.id)),
        StatusCode::CREATED,
    )
    .await;

    // 같은 원본을 다시 저장하면 발행은 그대로다.
    let same: OrgProblem = authored(
        &app,
        "PUT",
        &format!("/orgs/{}/problems/{}", org.id, problem.id),
        BOB,
        Some(publishable_source("입력에 답: 을 붙여라.")),
        StatusCode::OK,
    )
    .await;
    assert!(same.publication.published);

    let edited: OrgProblem = authored(
        &app,
        "PUT",
        &format!("/orgs/{}/problems/{}", org.id, problem.id),
        BOB,
        Some(publishable_source("고친 설명: 입력에 답: 을 붙여라.")),
        StatusCode::OK,
    )
    .await;
    assert!(!edited.publication.published);
    assert!(
        edited.publication.last_check.is_none(),
        "옛 점검은 지금 원본의 것이 아니다"
    );
    let old: PreviewView = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-previews/{}", org.id, first.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!old.current);
    let (status, body) = status_of(&app, "POST", &publish_uri, BOB, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"].as_str().unwrap().contains("다시 점검"),
        "{body}"
    );
    assert!(!bank_ids(&app, &org).await.contains(&problem.id));
    let (status, _) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(exam_body(&problem.id)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // 이미 만든 시험은 그대로 그 문항을 쓴다(문제 찾기는 발행과 상관없다).
    let detail: ExamView = ok(
        &app,
        "GET",
        &format!("/orgs/{}/exams/{}", org.id, exam.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(detail.items[0].problem_id, problem.id);
    assert!(
        promptlab_store::problems::find(&pool, &problem.id)
            .await
            .unwrap()
            .is_some()
    );

    // 다시 점검하면 발행할 수 있다.
    let again = check(&app, &pool, &org, &problem.id).await;
    assert!(again.current && again.publishable);
    let _: Publication = ok(&app, "POST", &publish_uri, BOB, None, StatusCode::OK).await;
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 점검_발행은_시험_관리_역할만_다른_조직은_404_진행_중은_3개까지(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let problem = create_from(&app, &org, publishable_source("입력에 답: 을 붙여라.")).await;
    let previews = format!("/orgs/{}/problem-previews", org.id);
    let publish_uri = format!("/orgs/{}/problems/{}/publish", org.id, problem.id);
    let body = json!({"problem_id": problem.id});
    for who in [CAROL, DAVE] {
        let (status, _) = status_of(&app, "POST", &previews, who, Some(body.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
        let (status, _) = status_of(&app, "POST", &publish_uri, who, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
    }
    // 다른 조직의 관리자가 자기 조직 경로로 이 문제를 점검·발행하려 해도 없는 문제다.
    let other = other_org(&app).await;
    let (status, _) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/problem-previews", other.id),
        ERIN,
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/problems/{}/publish", other.id, problem.id),
        ERIN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = status_of(&app, "POST", &previews, ERIN, Some(body.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "구성원이 아닌 조직");

    let mut ids = Vec::new();
    for _ in 0..3 {
        let v: PreviewView = ok(
            &app,
            "POST",
            &previews,
            BOB,
            Some(body.clone()),
            StatusCode::ACCEPTED,
        )
        .await;
        ids.push(v.id);
    }
    let (status, _) = status_of(&app, "POST", &previews, BOB, Some(body.clone())).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, _) = status_of(
        &app,
        "GET",
        &format!("/orgs/{}/problem-previews/{}", other.id, ids[0]),
        ERIN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 한_언어_틀이_컴파일되지_않으면_발행하지_못한다(pool: PgPool) {
    let app = app(pool.clone());
    let org = staffed_org(&app).await;
    let problem = create_from(&app, &org, source()).await;

    let judge = echo_judge();
    judge.set_mode(Mode::CompileErrorIn(
        promptlab_shared::coding::Language::Java,
    ));
    let view = check_with(&app, &pool, &org, &problem.id, &judge).await;
    assert_eq!(view.state, PreviewState::Done);
    let result = view.result.as_ref().unwrap();
    // 모범 답안(Python)은 모두 통과하지만 Java 틀만 컴파일되지 않는다.
    assert_eq!((result.passed, result.total, result.compiled), (7, 7, true));
    let failed: Vec<_> = result
        .templates
        .iter()
        .filter(|t| !t.compiled)
        .map(|t| t.language)
        .collect();
    assert_eq!(failed, vec![promptlab_shared::coding::Language::Java]);
    assert!(!view.publishable);
    assert!(view.reason.as_deref().unwrap().contains("Java"), "{view:?}");
    let (status, body) = status_of(
        &app,
        "POST",
        &format!("/orgs/{}/problems/{}/publish", org.id, problem.id),
        BOB,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("Java"), "{body}");
}
