//! 운영자 화면 이음매 ②(#19): 대표 계정만 받는 `/operator/*` — 시범 조직 승인, 문제은행(운영자 문항·내리기),
//! 큐·비용 지표, 허용 목록. 모든 응답에 숨은 입력 불변식이 걸린다(`common::send`) — 운영자 문항 편집 성공
//! 응답만 `operator_call`로 받는다.

mod common;

use axum::http::StatusCode;
use common::*;
use promptlab_shared::api::{AssignmentView, ExamInvitesCreated, ExamView, MeView, OrgView};
use promptlab_shared::authoring::{PreviewView, Publication};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::exam::Purpose;
use promptlab_shared::inference::LARGEST_MODEL as MODEL;
use promptlab_shared::integrity::ALLOWED_APP_VERSIONS;
use promptlab_shared::operator::{
    AllowlistView, BankProblem, BankProblemSummary, OperatorMetrics, OperatorOrg, ProblemOrigin,
    QueueKind,
};
/// 공개 픽스처의 연습 문항.
const DEFAULT_PROBLEM_ID: &str = "array-sum";
use promptlab_store::PgPool;
use promptlab_worker::{Outcome, run_once};
use serde_json::{Value, json};
use uuid::Uuid;

/// 내장 시험 문항(운영자가 내리기를 시험한다).
const EXAM_PROBLEM: &str = "count-vowels";

async fn ok<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
    expect: StatusCode,
) -> T {
    let (status, body) = call(app, method, uri, Some(token), body).await;
    assert_eq!(status, expect, "{method} {uri}: {body}");
    serde_json::from_value(body).unwrap()
}

async fn status_of(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> StatusCode {
    call(app, method, uri, Some(token), body).await.0
}

/// 운영자 문항 편집 성공 응답(숨은 입력이 일부러 실린다).
async fn operated<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expect: StatusCode,
) -> T {
    let (status, body) = operator_call(app, method, uri, body).await;
    assert_eq!(status, expect, "{method} {uri}: {body}");
    serde_json::from_value(body).unwrap()
}

fn alice() -> String {
    token_for(ALICE)
}

fn owner() -> String {
    owner_token()
}

async fn new_org(app: &axum::Router, sub: &str, name: &str) -> OrgView {
    ok(
        app,
        "POST",
        "/orgs",
        &token_for(sub),
        Some(json!({"name": name, "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await
}

fn exam_body(problem_id: &str) -> Value {
    json!({
        "title": "프롬프트 역량",
        "purpose": Purpose::Recruiting,
        "reveal_scores": false,
        "items": [
            {"problem_id": problem_id, "limits": {"time_limit_secs": 600}},
            {"problem_id": problem_id, "limits": {"time_limit_secs": 600}},
        ],
    })
}

fn exam_uri(org: &OrgView) -> String {
    format!("/orgs/{}/exams", org.id)
}

/// 승인된 조직에 시험(문항 둘)을 만들고 ERIN을 배정까지(웹캠 준비 포함).
async fn assigned(app: &axum::Router, problem_id: &str) -> (OrgView, ExamView, AssignmentView) {
    let org = new_org(app, ALICE, "에이 회사").await;
    approve_org(app, &org.id).await;
    let exam: ExamView = ok(
        app,
        "POST",
        &exam_uri(&org),
        &alice(),
        Some(exam_body(problem_id)),
        StatusCode::CREATED,
    )
    .await;
    let assignment = assign(app, &org, &exam).await;
    (org, exam, assignment)
}

async fn assign(app: &axum::Router, org: &OrgView, exam: &ExamView) -> AssignmentView {
    let created: ExamInvitesCreated = ok(
        app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        &alice(),
        Some(json!({"emails": ["erin@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created.invites[0].token.clone().unwrap();
    let assignment: AssignmentView = ok(
        app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        &token_for(ERIN),
        None,
        StatusCode::OK,
    )
    .await;
    webcam_ready(app, &token_for(ERIN), &assignment.id).await;
    assignment
}

fn start_uri(a: &AssignmentView, n: u32) -> String {
    format!("/assignments/{}/items/{n}/start", a.id)
}

/// 운영자 문항 원본(`common::org_source` — 히든 테스트·모범 답안에 표식을 넣어 새면 `call`이 잡는다).
fn source(description: &str) -> Value {
    let mut source = org_source("운영자 문항");
    source["description"] = json!(description);
    source
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 운영자_경로는_대표_계정만_받는다(pool: PgPool) {
    let app = app(pool);
    let org = new_org(&app, ALICE, "에이 회사").await;
    let routes: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", "/operator/orgs".into(), None),
        (
            "PUT",
            format!("/operator/orgs/{}/approval", org.id),
            Some(json!({"approved": true})),
        ),
        ("GET", "/operator/problems".into(), None),
        (
            "POST",
            "/operator/problems".into(),
            Some(json!({"practice": false, "source": source("새 문항")})),
        ),
        (
            "PUT",
            format!("/operator/problems/{EXAM_PROBLEM}/retirement"),
            Some(json!({"retired": true})),
        ),
        ("GET", "/operator/metrics".into(), None),
        ("GET", "/operator/allowlist".into(), None),
        (
            "POST",
            "/operator/allowlist".into(),
            Some(json!({"kind": "app_version", "value": "9.9.9", "action": "allow"})),
        ),
    ];
    for (method, uri, body) in &routes {
        for (who, token) in [
            ("member", token_for(ALICE)),
            ("machine", machine_token(SMOKE_CLIENT)),
        ] {
            assert_eq!(
                status_of(&app, method, uri, &token, body.clone()).await,
                StatusCode::FORBIDDEN,
                "{who} {method} {uri}"
            );
        }
        assert_eq!(
            call(&app, method, uri, None, body.clone()).await.0,
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
    // 아무것도 바뀌지 않았다.
    let orgs: Vec<OperatorOrg> = ok(
        &app,
        "GET",
        "/operator/orgs",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0].approved_at, None);

    // 대시보드는 `/me`의 is_owner로 운영자 메뉴를 보인다.
    let me: MeView = ok(&app, "GET", "/me", &owner(), None, StatusCode::OK).await;
    assert!(me.is_owner);
    let me: MeView = ok(&app, "GET", "/me", &alice(), None, StatusCode::OK).await;
    assert!(!me.is_owner);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 승인_전_조직은_시험을_열지_못하고_승인을_거두면_새_문항을_시작하지_못한다(
    pool: PgPool,
) {
    let app = app(pool);
    let org = new_org(&app, ALICE, "에이 회사").await;
    assert!(!org.approved, "새 조직은 승인 대기");
    assert_eq!(
        status_of(
            &app,
            "POST",
            &exam_uri(&org),
            &alice(),
            Some(exam_body(EXAM_PROBLEM))
        )
        .await,
        StatusCode::FORBIDDEN
    );

    // 운영자가 승인하면 시험을 만든다. 조직 화면에도 승인이 보인다.
    let approved: OperatorOrg = ok(
        &app,
        "PUT",
        &format!("/operator/orgs/{}/approval", org.id),
        &owner(),
        Some(json!({"approved": true})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(approved.approved_at, Some(NOW));
    assert_eq!(approved.members, 1);
    let me: MeView = ok(&app, "GET", "/me", &alice(), None, StatusCode::OK).await;
    assert!(me.orgs[0].approved);
    let exam: ExamView = ok(
        &app,
        "POST",
        &exam_uri(&org),
        &alice(),
        Some(exam_body(EXAM_PROBLEM)),
        StatusCode::CREATED,
    )
    .await;
    let assignment = assign(&app, &org, &exam).await;
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 0),
            &token_for(ERIN),
            Some(start_body())
        )
        .await,
        StatusCode::OK
    );

    // 승인을 거두면 이미 만든 시험도 새 문항을 시작하지 못한다.
    let revoked: OperatorOrg = ok(
        &app,
        "PUT",
        &format!("/operator/orgs/{}/approval", org.id),
        &owner(),
        Some(json!({"approved": false})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(revoked.approved_at, None);
    assert_eq!(revoked.exams, 1);
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 1),
            &token_for(ERIN),
            Some(start_body())
        )
        .await,
        StatusCode::FORBIDDEN
    );

    // 없는 조직은 404.
    assert_eq!(
        status_of(
            &app,
            "PUT",
            &format!("/operator/orgs/{}/approval", Uuid::new_v4()),
            &owner(),
            Some(json!({"approved": true}))
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

/// 0012 마이그레이션 전에 있던 조직(운영 중인 조직)은 승인된 것으로 둔다.
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 내린_문항은_새_시험에_못_넣고_이미_만든_시험은_그대로_돈다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let (org, _, assignment) = assigned(&app, EXAM_PROBLEM).await;

    let retired: BankProblemSummary = ok(
        &app,
        "PUT",
        &format!("/operator/problems/{EXAM_PROBLEM}/retirement"),
        &owner(),
        Some(json!({"retired": true, "reason": "유출"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(retired.origin, ProblemOrigin::Builtin);
    assert_eq!(retired.retired_at, Some(NOW));
    assert_eq!(retired.retired_reason.as_deref(), Some("유출"));

    // 새 시험: 문제은행에서 빠지고 만들기는 422.
    let bank: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!bank.iter().any(|p| p.id == EXAM_PROBLEM));
    assert_eq!(
        status_of(
            &app,
            "POST",
            &exam_uri(&org),
            &alice(),
            Some(exam_body(EXAM_PROBLEM))
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // 이미 만든 시험: 문항 시작·제출·채점이 그대로 된다.
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 0),
            &token_for(ERIN),
            Some(start_body())
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(
            &app,
            "POST",
            &format!("/assignments/{}/items/0/submit", assignment.id),
            &token_for(ERIN),
            Some(
                logged_submit_body(
                    &app,
                    &format!("/assignments/{}/items/0/submit", assignment.id),
                    &token_for(ERIN),
                    "correct"
                )
                .await
            )
        )
        .await,
        StatusCode::ACCEPTED
    );
    assert!(matches!(
        run_once(&pool, &judge()).await.unwrap(),
        Outcome::Graded(_)
    ));

    // 되살리면 다시 문제은행에.
    let back: BankProblemSummary = ok(
        &app,
        "PUT",
        &format!("/operator/problems/{EXAM_PROBLEM}/retirement"),
        &owner(),
        Some(json!({"retired": false})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(back.retired_at, None);
    assert_eq!(
        status_of(
            &app,
            "POST",
            &exam_uri(&org),
            &alice(),
            Some(exam_body(EXAM_PROBLEM))
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        status_of(
            &app,
            "PUT",
            "/operator/problems/no-such-problem/retirement",
            &owner(),
            Some(json!({"retired": true}))
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 운영자_문항을_만들고_고치고_숨은_입력은_운영자_편집_응답에만_싣는다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let org = new_org(&app, ALICE, "에이 회사").await;
    approve_org(&app, &org.id).await;

    // 시험 문항.
    let made: BankProblem = operated(
        &app,
        "POST",
        "/operator/problems",
        Some(json!({"practice": false, "source": source("운영자 시험 문항")})),
        StatusCode::CREATED,
    )
    .await;
    assert!(made.id.starts_with("bank-"));
    assert!(!made.practice);
    assert!(
        made.source.hidden[0].args[0]
            .to_string()
            .contains(ORG_HIDDEN_MARK)
    );
    let got: BankProblem = operated(
        &app,
        "GET",
        &format!("/operator/problems/{}", made.id),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(got, made);
    let fixed: BankProblem = operated(
        &app,
        "PUT",
        &format!("/operator/problems/{}", made.id),
        Some(source("운영자 시험 문항(고침)")),
        StatusCode::OK,
    )
    .await;
    assert_eq!(fixed.source.description, "운영자 시험 문항(고침)");

    // 목록(히든 테스트 없음 — call이 건다)에는 내장 12 + 운영자 문항.
    let list: Vec<BankProblemSummary> = ok(
        &app,
        "GET",
        "/operator/problems",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        list.iter()
            .filter(|p| p.origin == ProblemOrigin::Builtin)
            .count(),
        promptlab_store::catalog::bank().count()
    );
    let row = list.iter().find(|p| p.id == made.id).unwrap();
    assert_eq!((row.origin, row.hidden_count), (ProblemOrigin::Operator, 6));
    assert!(!row.published && !fixed.publication.published);

    // 발행 전(#73): 조직 문제은행에 없고 시험에 넣지 못한다. 점검·발행 흐름은 아래 시험이 따로 본다.
    let bank: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!bank.iter().any(|p| p.id == made.id));
    assert_eq!(
        status_of(
            &app,
            "POST",
            &exam_uri(&org),
            &alice(),
            Some(exam_body(&made.id))
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    force_publish(&pool, &made.id).await;

    // 조직 문제은행에 공개 보기로 들어가고 시험에 넣는다.
    let bank: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(bank.iter().any(|p| p.id == made.id));
    assert_eq!(
        status_of(
            &app,
            "POST",
            &exam_uri(&org),
            &alice(),
            Some(exam_body(&made.id))
        )
        .await,
        StatusCode::CREATED
    );

    // 연습 문항: 연습 목록에만, 시험에는 422. 내리면 연습에서도 빠진다(404).
    let practice: BankProblem = operated(
        &app,
        "POST",
        "/operator/problems",
        Some(json!({"practice": true, "source": source("운영자 연습 문항")})),
        StatusCode::CREATED,
    )
    .await;
    // 발행 전 연습 문항은 연습 목록·문제 보기·제출에서 빠진다.
    let listed: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        "/practice/problems",
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!listed.iter().any(|p| p.id == practice.id));
    assert_eq!(
        status_of(
            &app,
            "GET",
            &format!("/problems/{}", practice.id),
            &alice(),
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    force_publish(&pool, &practice.id).await;
    let listed: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        "/practice/problems",
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(listed.iter().any(|p| p.id == practice.id));
    assert!(!listed.iter().any(|p| p.id == made.id));
    assert_eq!(
        status_of(
            &app,
            "POST",
            &exam_uri(&org),
            &alice(),
            Some(exam_body(&practice.id))
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let submit = serde_json::to_value(practice_request(&practice.id, "correct", 5, None)).unwrap();
    assert_eq!(
        status_of(
            &app,
            "POST",
            "/submissions",
            &token_for(DAVE),
            Some(submit.clone())
        )
        .await,
        StatusCode::ACCEPTED
    );
    let _: BankProblemSummary = ok(
        &app,
        "PUT",
        &format!("/operator/problems/{}/retirement", practice.id),
        &owner(),
        Some(json!({"retired": true, "reason": "교체"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        status_of(
            &app,
            "GET",
            &format!("/problems/{}", practice.id),
            &alice(),
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(&app, "POST", "/submissions", &token_for(ERIN), Some(submit)).await,
        StatusCode::NOT_FOUND
    );
    let listed: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        "/practice/problems",
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!listed.iter().any(|p| p.id == practice.id));
    assert!(listed.iter().any(|p| p.id == DEFAULT_PROBLEM_ID));

    // 내장 문항은 저장소 파일이 정본 — 화면에서 읽거나 고치지 않는다. 잘못된 원본은 422.
    for (method, body) in [("GET", None), ("PUT", Some(source("내장 고치기")))] {
        assert_eq!(
            status_of(
                &app,
                method,
                &format!("/operator/problems/{EXAM_PROBLEM}"),
                &owner(),
                body
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }
    let mut bad = source("x");
    bad["description"] = json!("");
    assert_eq!(
        status_of(
            &app,
            "POST",
            "/operator/problems",
            &owner(),
            Some(json!({"practice": false, "source": bad}))
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 지표는_큐_실패_조직별_비용을_보이고_스모크는_빼고_개인_연습은_따로_싣는다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let (org, exam, assignment) = assigned(&app, EXAM_PROBLEM).await;
    // 쓴 것이 없는 조직: 합이 비어도 -0.0(화면에 「$-0.00」)이 아니라 0.0이어야 한다.
    let idle = new_org(&app, ALICE, "빈 회사").await;
    // 시험 제출 하나(대기), 개인 연습 둘 — 하나는 실패로 만든다.
    let _ = status_of(
        &app,
        "POST",
        &start_uri(&assignment, 0),
        &token_for(ERIN),
        Some(start_body()),
    )
    .await;
    assert_eq!(
        status_of(
            &app,
            "POST",
            &format!("/assignments/{}/items/0/submit", assignment.id),
            &token_for(ERIN),
            Some(
                logged_submit_body(
                    &app,
                    &format!("/assignments/{}/items/0/submit", assignment.id),
                    &token_for(ERIN),
                    "correct"
                )
                .await
            )
        )
        .await,
        StatusCode::ACCEPTED
    );
    let practice =
        serde_json::to_value(practice_request(DEFAULT_PROBLEM_ID, "correct", 5, None)).unwrap();
    for sub in [DAVE, CAROL] {
        assert_eq!(
            status_of(
                &app,
                "POST",
                "/submissions",
                &token_for(sub),
                Some(practice.clone())
            )
            .await,
            StatusCode::ACCEPTED
        );
    }
    let failed: Uuid = sqlx::query_scalar(
        "UPDATE submissions SET status = 'failed', failure_reason = '채점하지 못했다',
                failure_detail = '상세-비밀-7f3a', started_at = now(), finished_at = now()
          WHERE id = (SELECT id FROM submissions WHERE candidate_sub = $1 AND practice)
          RETURNING id",
    )
    .bind(CAROL)
    .fetch_one(&pool)
    .await
    .unwrap();

    let metrics: OperatorMetrics = ok(
        &app,
        "GET",
        "/operator/metrics",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    let queue = |m: &OperatorMetrics, kind: QueueKind| {
        m.queues.iter().find(|q| q.kind == kind).unwrap().clone()
    };
    assert_eq!(queue(&metrics, QueueKind::Grading).waiting, 1);
    assert_eq!(queue(&metrics, QueueKind::PracticeGrading).waiting, 1);
    assert_eq!(queue(&metrics, QueueKind::PracticeGrading).failed, 1);
    assert_eq!(metrics.failures.len(), 1);
    assert_eq!(metrics.failures[0].kind, QueueKind::PracticeGrading);
    assert_eq!(metrics.failures[0].reason, "채점하지 못했다");
    assert!(
        !serde_json::to_string(&metrics)
            .unwrap()
            .contains("상세-비밀-7f3a"),
        "채점 상세는 싣지 않는다"
    );

    // 채점(시험 먼저) 뒤에는 끝난 일로.
    for _ in 0..2 {
        assert!(matches!(
            run_once(&pool, &judge()).await.unwrap(),
            Outcome::Graded(_)
        ));
    }

    // 비용: 조직·스모크 조직·개인 연습 기록을 넣는다(#15 집계 그대로).
    let smoke: Uuid = sqlx::query_scalar(
        "INSERT INTO orgs (name, created_by, is_smoke) VALUES ('스모크', $1, true) RETURNING id",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await
    .unwrap();
    let org_id: Uuid = org.id.parse().unwrap();
    let exam_id: Uuid = exam.id.parse().unwrap();
    let practice_submission: Uuid =
        sqlx::query_scalar("SELECT id FROM submissions WHERE candidate_sub = $1 AND practice")
            .bind(DAVE)
            .fetch_one(&pool)
            .await
            .unwrap();
    for (kind, org, exam, submission, cost) in [
        ("report", Some(org_id), Some(exam_id), None, 1.5),
        ("report", Some(smoke), None, None, 9.0),
        ("evaluation", None, None, Some(practice_submission), 0.25),
        ("evaluation", None, None, Some(failed), 0.0),
    ] {
        sqlx::query(
            "INSERT INTO usage_records (kind, org_id, exam_id, submission_id, model, input_tokens, output_tokens,
                    duration_ms, price_input, price_output, price_cache_read, price_cache_write, cost_usd)
             VALUES ($1, $2, $3, $4, 'm', 1000, 100, 10, 1, 1, 1, 1, $5)",
        )
        .bind(kind)
        .bind(org)
        .bind(exam)
        .bind(submission)
        .bind(cost)
        .execute(&pool)
        .await
        .unwrap();
    }

    let metrics: OperatorMetrics = ok(
        &app,
        "GET",
        "/operator/metrics",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    let grading = queue(&metrics, QueueKind::Grading);
    assert_eq!((grading.waiting, grading.done), (0, 1));
    assert!(grading.avg_run_secs.is_some());
    assert_eq!(queue(&metrics, QueueKind::PracticeGrading).done, 1);
    assert!(
        metrics.costs.iter().all(|c| c.name != "스모크"),
        "스모크 조직은 빠진다"
    );
    let first = &metrics.costs[0];
    assert_eq!(first.org_id.as_deref(), Some(org.id.as_str()));
    assert_eq!(first.attempts, 1);
    assert!((first.cost_usd - 1.5).abs() < 1e-9);
    let last = metrics.costs.last().unwrap();
    assert_eq!(
        (last.org_id.as_deref(), last.name.as_str()),
        (None, "개인 연습")
    );
    assert_eq!(last.attempts, 2);
    assert!((last.cost_usd - 0.25).abs() < 1e-9);
    assert!((metrics.total_cost_usd - 1.75).abs() < 1e-9);
    let idle_cost = metrics
        .costs
        .iter()
        .find(|c| c.org_id.as_deref() == Some(idle.id.as_str()))
        .expect("쓴 것이 없는 조직도 한 줄");
    assert!(
        idle_cost.cost_usd == 0.0 && idle_cost.cost_usd.is_sign_positive(),
        "빈 합계는 +0.0: {}",
        idle_cost.cost_usd
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 허용_목록_변경이_문항_시작에_반영되고_이력이_남는다(pool: PgPool) {
    let app = app(pool);
    let (_, _, assignment) = assigned(&app, EXAM_PROBLEM).await;
    let current = ALLOWED_APP_VERSIONS[0];
    let change = |kind: &str, value: &str, action: &str| json!({"kind": kind, "value": value, "action": action, "note": format!("{action} {value}")});
    let erin = token_for(ERIN);
    let start_as = |version: &str| json!({"app_version": version, "model_sha256": MODEL.sha256});

    // 지금 판을 막으면(상수에 있어도) 시작하지 못한다.
    let view: AllowlistView = ok(
        &app,
        "POST",
        "/operator/allowlist",
        &owner(),
        Some(change("app_version", current, "block")),
        StatusCode::CREATED,
    )
    .await;
    assert!(!view.effective.app_versions.iter().any(|v| v == current));
    assert!(view.constants.app_versions.iter().any(|v| v == current));
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 0),
            &erin,
            Some(start_body())
        )
        .await,
        StatusCode::FORBIDDEN
    );

    // 상수에 없는 판을 더하면 그 판으로 시작한다.
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 0),
            &erin,
            Some(start_as("9.9.9"))
        )
        .await,
        StatusCode::FORBIDDEN
    );
    let _: AllowlistView = ok(
        &app,
        "POST",
        "/operator/allowlist",
        &owner(),
        Some(change("app_version", "9.9.9", "allow")),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 0),
            &erin,
            Some(start_as("9.9.9"))
        )
        .await,
        StatusCode::OK
    );

    // 되돌리면 상수 기본값.
    let view: AllowlistView = ok(
        &app,
        "POST",
        "/operator/allowlist",
        &owner(),
        Some(change("app_version", current, "reset")),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(
        status_of(
            &app,
            "POST",
            &start_uri(&assignment, 1),
            &erin,
            Some(start_body())
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        view.effective.app_versions,
        vec![current.to_owned(), "9.9.9".to_owned()]
    );

    // 이력: 새것부터, 누가·무엇을·왜.
    let view: AllowlistView = ok(
        &app,
        "GET",
        "/operator/allowlist",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    let actions: Vec<(&str, &str)> = view
        .history
        .iter()
        .map(|c| (c.value.as_str(), c.action.as_str()))
        .collect();
    assert_eq!(
        actions,
        [(current, "reset"), ("9.9.9", "allow"), (current, "block")]
    );
    assert!(
        view.history
            .iter()
            .all(|c| c.changed_by == OWNER_SUB && c.changed_at == NOW)
    );
    assert_eq!(view.history[1].note, "allow 9.9.9");

    // 모델은 더하지 못한다(막기만). 형식이 틀리면 422.
    for body in [
        change("model_sha256", &"a".repeat(64), "allow"),
        change("model_sha256", "zz", "block"),
        change("app_version", "0.1 0", "allow"),
    ] {
        assert_eq!(
            status_of(
                &app,
                "POST",
                "/operator/allowlist",
                &owner(),
                Some(body.clone())
            )
            .await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
    }
    let view: AllowlistView = ok(
        &app,
        "GET",
        "/operator/allowlist",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(view.history.len(), 3);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 운영자_문항도_점검을_통과해_발행해야_연습_목록에_나오고_고치면_빠진다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let mut src = source("운영자 연습 문항: 입력을 그대로 돌려줘라");
    let made: BankProblem = operated(
        &app,
        "POST",
        "/operator/problems",
        Some(json!({"practice": true, "source": src.clone()})),
        StatusCode::CREATED,
    )
    .await;
    let body = json!({"problem_id": made.id});
    // 대표만, 내장 문항은 저장소 시험이 대신해 점검 대상이 아니다.
    for uri in [
        "/operator/problem-checks".to_owned(),
        format!("/operator/problems/{}/publish", made.id),
    ] {
        assert_eq!(
            status_of(&app, "POST", &uri, &alice(), Some(body.clone())).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        status_of(
            &app,
            "POST",
            "/operator/problem-checks",
            &owner(),
            Some(json!({"problem_id": DEFAULT_PROBLEM_ID}))
        )
        .await,
        StatusCode::NOT_FOUND
    );

    let started: PreviewView = ok(
        &app,
        "POST",
        "/operator/problem-checks",
        &owner(),
        Some(body.clone()),
        StatusCode::ACCEPTED,
    )
    .await;
    // 모범 답안(`return s`)은 인자를 그대로 돌려준다.
    let judge = promptlab_worker::fake::FakeJudge::new(|code, args| match args {
        [Value::String(s)] if code.contains("return s") => Some(Value::String(s.clone())),
        _ => None,
    });
    let mut checked = false;
    loop {
        match run_once(&pool, &judge).await.unwrap() {
            Outcome::Idle => break,
            Outcome::Checked(_) => checked = true,
            _ => {}
        }
    }
    assert!(checked);
    let view: PreviewView = ok(
        &app,
        "GET",
        &format!("/operator/problem-checks/{}", started.id),
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(view.publishable, "{view:?}");
    assert_eq!(
        status_of(
            &app,
            "GET",
            &format!("/operator/problem-checks/{}", started.id),
            &alice(),
            None
        )
        .await,
        StatusCode::FORBIDDEN
    );
    let published: Publication = ok(
        &app,
        "POST",
        &format!("/operator/problems/{}/publish", made.id),
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(published.published);
    let listed: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        "/practice/problems",
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(listed.iter().any(|p| p.id == made.id));
    // 운영자 화면의 큐 지표에도 조직 없는 점검이 끝남으로 잡힌다.
    let metrics: OperatorMetrics = ok(
        &app,
        "GET",
        "/operator/metrics",
        &owner(),
        None,
        StatusCode::OK,
    )
    .await;
    let preview = metrics
        .queues
        .iter()
        .find(|q| q.kind == QueueKind::Preview)
        .unwrap();
    assert_eq!(preview.done, 1);

    // 고치면 발행이 풀려 연습 목록에서 빠진다.
    src["description"] = json!("운영자 연습 문항(고침)");
    let fixed: BankProblem = operated(
        &app,
        "PUT",
        &format!("/operator/problems/{}", made.id),
        Some(src),
        StatusCode::OK,
    )
    .await;
    assert!(!fixed.publication.published);
    let listed: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        "/practice/problems",
        &alice(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!listed.iter().any(|p| p.id == made.id));
}
