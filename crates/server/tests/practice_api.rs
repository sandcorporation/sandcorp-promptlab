//! 시험 이음매 ②: 개인 연습(#18). 초대 없는 연습 응시·채점, 내 기록·추이, 남의 기록 404, 기계 주체 거부,
//! 고객사 화면·집계 미노출(같은 사람이 시험 응시자이면서 연습한 경우), 채점 큐에서 시험 우선, 사람마다 대기 한도,
//! 연습 문제와 시험 문항의 분리. 모든 응답에 숨은 입력 불변식이 걸린다(`common::send`).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::*;
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamView, InviteCreated, OrgView, SubmissionState, SubmissionView,
};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::exam::Purpose;
use promptlab_shared::practice::{MAX_PENDING, PracticeRecords};
use promptlab_store::{PgPool, catalog};
use promptlab_worker::Outcome;
use serde_json::{Value, json};

const PRACTICE: &str = "array-sum";
/// 시험 문항(연습 문제가 아니다).
const EXAM_PROBLEM: &str = "count-vowels";
/// 연습 코드에 넣는 표식 — 고객사 응답에 보이면 연습이 샌 것이다.
const PRACTICE_MARK: &str = "연습표식-9b2e";

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

/// 연습 제출 본문. 코드에 `correct`가 있으면 모두, `first-only`면 첫 히든 케이스만 맞힌다(`common::judge`).
fn practice_body(code: &str) -> Value {
    serde_json::to_value(practice_request(PRACTICE, code, 90, None)).unwrap()
}

async fn practice(app: &axum::Router, sub: &str, code: &str) -> SubmissionView {
    ok(
        app,
        "POST",
        "/submissions",
        sub,
        Some(practice_body(code)),
        StatusCode::ACCEPTED,
    )
    .await
}

async fn grade_next(pool: &PgPool) -> uuid::Uuid {
    match common::grade(pool).await {
        Outcome::Graded(id) => id,
        other => panic!("채점해야 한다: {other:?}"),
    }
}

async fn records(app: &axum::Router, sub: &str) -> PracticeRecords {
    ok(app, "GET", "/me/practice", sub, None, StatusCode::OK).await
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 연습_문제_목록은_시험_문항과_겹치지_않는다(pool: PgPool) {
    let app = app(pool);
    let list: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        "/practice/problems",
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        catalog::practice()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>()
    );
    assert!(!ids.contains(&EXAM_PROBLEM));

    // 시험 쪽 문제은행에는 연습 문제가 없고, 시험에 넣으면 422.
    let org: OrgView = ok(
        &app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(&app, &org.id).await;
    let bank: Vec<PublicCodingProblem> = ok(
        &app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(bank.iter().all(|p| !catalog::is_practice(&p.id)));
    assert!(bank.iter().all(|p| catalog::is_published(&p.id)));
    assert_eq!(
        bank.len(),
        catalog::exam_bank()
            .filter(|p| catalog::is_published(&p.id))
            .count()
    );
    let (status, _) = call(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        Some(&token_for(ALICE)),
        Some(json!({
            "title": "연습 문제로 시험",
            "purpose": Purpose::Recruiting,
            "items": [{"problem_id": PRACTICE, "limits": {"time_limit_secs": 600}}],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 초대_없이_연습하고_채점받고_내_기록과_추이를_본다(pool: PgPool) {
    let app = app(pool.clone());
    // 조직도 초대도 없는 사람.
    let problem: PublicCodingProblem = ok(
        &app,
        "GET",
        &format!("/problems/{PRACTICE}"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(problem, catalog::find(PRACTICE).unwrap().public_view());
    assert_eq!(records(&app, ERIN).await.attempts, vec![]);

    let total = catalog::find(PRACTICE).unwrap().hidden.len();
    let mut ids = Vec::new();
    // 세 번 낸다: 모두 맞힘 → 모두 틀림 → 첫 히든 케이스만 맞힘.
    for code in ["# correct", "# wrong", "# first-only"] {
        let created = practice(&app, ERIN, code).await;
        assert_eq!(created.state, SubmissionState::Queued);
        assert_eq!(created.org_id, None);
        assert_eq!(grade_next(&pool).await.to_string(), created.id);
        ids.push(created.id);
    }
    let view: SubmissionView = ok(
        &app,
        "GET",
        &format!("/submissions/{}", ids[2]),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    let SubmissionState::Done { report } = view.state else {
        panic!("채점이 끝나야 한다");
    };
    assert_eq!((report.passed, report.total as usize), (1, total));

    let r = records(&app, ERIN).await;
    let newest_first: Vec<&str> = r
        .attempts
        .iter()
        .map(|a| a.submission_id.as_str())
        .collect();
    assert_eq!(newest_first, vec![&ids[2], &ids[1], &ids[0]]);
    assert_eq!(r.problems.len(), 1);
    let p = &r.problems[0];
    assert_eq!(p.problem_id, PRACTICE);
    assert_eq!(p.attempts, 3);
    let t = total as f64;
    assert_eq!(p.trend, vec![1.0, 0.0, 1.0 / t]);
    assert_eq!(p.best, Some(1.0));
    assert_eq!(p.latest, Some(1.0 / t));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 남의_연습_기록은_보이지_않는다(pool: PgPool) {
    let app = app(pool.clone());
    let mine = practice(&app, ERIN, "# first-only").await;
    grade_next(&pool).await;
    let (status, _) = call(
        &app,
        "GET",
        &format!("/submissions/{}", mine.id),
        Some(&token_for(BOB)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(records(&app, BOB).await.attempts, vec![]);
    assert_eq!(records(&app, BOB).await.problems, vec![]);
    assert_eq!(records(&app, ERIN).await.attempts.len(), 1);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 기계_주체는_연습할_수_없다(pool: PgPool) {
    let app = app(pool);
    let token = machine_token(SMOKE_CLIENT);
    for (method, uri, body) in [
        ("GET", "/practice/problems".to_owned(), None),
        ("GET", "/me/practice".to_owned(), None),
        ("GET", format!("/problems/{PRACTICE}"), None),
        (
            "POST",
            "/submissions".to_owned(),
            Some(practice_body("기계")),
        ),
        ("GET", "/api/me/practice".to_owned(), None),
    ] {
        let (status, _) = call(&app, method, &uri, Some(&token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 채점을_기다리는_연습은_사람마다_한도가_있다(pool: PgPool) {
    let app = app(pool.clone());
    for n in 0..MAX_PENDING {
        practice(&app, ERIN, &format!("{n}")).await;
    }
    let (status, _) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ERIN)),
        Some(practice_body("넘침")),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // 다른 사람은 따로 센다.
    practice(&app, BOB, "밥의 연습").await;
    // 하나가 채점되면 다시 낼 수 있다.
    grade_next(&pool).await;
    practice(&app, ERIN, "다시").await;
}

/// ALICE 관리자, BOB 출제자, CAROL 감독관, DAVE 열람자 조직과 ERIN이 배정받은 사내 진단 시험.
struct Exam {
    clock: Arc<FixedClock>,
    app: axum::Router,
    org: OrgView,
    exam: ExamView,
    assignment: AssignmentView,
}

async fn exam_with_erin(pool: PgPool) -> Exam {
    let clock = FixedClock::at(NOW);
    let app = app_parts(
        pool,
        clock.clone(),
        Arc::new(promptlab_server::mail::Disabled),
        None,
    );
    let org: OrgView = ok(
        &app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(&app, &org.id).await;
    for (sub, role) in [
        (BOB, Role::Author),
        (CAROL, Role::Proctor),
        (DAVE, Role::Viewer),
    ] {
        let invite: InviteCreated = ok(
            &app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({"role": role})),
            StatusCode::CREATED,
        )
        .await;
        let _: OrgView = ok(
            &app,
            "POST",
            &format!("/invites/{}/accept", invite.token),
            sub,
            Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
            StatusCode::OK,
        )
        .await;
    }
    let exam: ExamView = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(json!({
            "title": "사내 진단",
            "purpose": Purpose::Internal,
            "reveal_scores": true,
            "items": [{"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await;
    let created: Value = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": ["erin@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created["invites"][0]["token"].as_str().unwrap().to_string();
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
    Exam {
        clock,
        app,
        org,
        exam,
        assignment,
    }
}

impl Exam {
    async fn submit_item(&self) -> SubmissionView {
        let _: Value = ok(
            &self.app,
            "POST",
            &format!("/assignments/{}/items/0/start", self.assignment.id),
            ERIN,
            Some(start_body()),
            StatusCode::OK,
        )
        .await;
        self.clock.advance(30);
        ok(
            &self.app,
            "POST",
            &format!("/assignments/{}/items/0/submit", self.assignment.id),
            ERIN,
            Some(
                logged_submit_body(
                    &self.app,
                    &format!("/assignments/{}/items/0/submit", self.assignment.id),
                    &token_for(ERIN),
                    "# correct",
                )
                .await,
            ),
            StatusCode::ACCEPTED,
        )
        .await
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 채점_큐는_시험을_먼저_꺼낸다(pool: PgPool) {
    let e = exam_with_erin(pool.clone()).await;
    // 연습이 먼저 들어와 기다리고 있어도
    let early = practice(&e.app, BOB, "먼저 낸 연습").await;
    let exam = e.submit_item().await;
    // 시험 제출을 먼저 채점하고, 그다음 연습.
    assert_eq!(grade_next(&pool).await.to_string(), exam.id);
    assert_eq!(grade_next(&pool).await.to_string(), early.id);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 시험_응시자가_한_연습은_고객사_화면_어디에도_없다(pool: PgPool) {
    let e = exam_with_erin(pool.clone()).await;
    // ERIN은 이 조직의 시험 응시자이면서 혼자 연습도 한다.
    let mine = practice(&e.app, ERIN, &format!("# {PRACTICE_MARK} correct")).await;
    grade_next(&pool).await;
    e.submit_item().await;
    grade_next(&pool).await;
    assert_eq!(records(&e.app, ERIN).await.attempts.len(), 1);

    let exam_path = format!("/orgs/{}/exams/{}", e.org.id, e.exam.id);
    let invites: Value = ok(
        &e.app,
        "GET",
        &format!("{exam_path}/invites"),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    let invite = invites.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let leaked = |text: &str| {
        text.contains(&mine.id) || text.contains(PRACTICE_MARK) || text.contains(PRACTICE)
    };
    let json_paths = [
        format!("/orgs/{}", e.org.id),
        format!("/orgs/{}/members", e.org.id),
        format!("/orgs/{}/submissions", e.org.id),
        format!("/orgs/{}/exams", e.org.id),
        exam_path.clone(),
        format!("{exam_path}/invites"),
        format!("{exam_path}/results"),
        format!("/orgs/{}/rounds", e.org.id),
        format!("{exam_path}/invites/{invite}/signals"),
        format!("{exam_path}/invites/{invite}/items/0/log"),
        format!("{exam_path}/invites/{invite}/webcam"),
        "/me".to_owned(),
    ];
    for sub in [ALICE, CAROL, DAVE] {
        for path in &json_paths {
            let (status, body) = call(&e.app, "GET", path, Some(&token_for(sub)), None).await;
            if status == StatusCode::OK {
                assert!(!leaked(&body.to_string()), "{sub} {path}: {body}");
            }
        }
        let (status, _, csv) = send_raw(
            &e.app,
            "GET",
            &format!("{exam_path}/results.csv"),
            &token_for(sub),
            "text/plain",
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let csv = String::from_utf8(csv).unwrap();
        assert!(!leaked(&csv), "{csv}");
        // 고객사 구성원은 그 연습 제출을 직접 열어 볼 수도 없다.
        let (status, _) = call(
            &e.app,
            "GET",
            &format!("/submissions/{}", mine.id),
            Some(&token_for(sub)),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // 결과에는 시험 응시 하나만 잡힌다(연습 점수가 섞이지 않는다).
    let results: Value = ok(
        &e.app,
        "GET",
        &format!("{exam_path}/results"),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    let rows = results["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["items"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 연습_제출은_언어와_코드_길이를_검사한다(pool: PgPool) {
    use promptlab_shared::coding::MAX_CODE_BYTES;
    let app = app(pool.clone());
    let post = |body: Value| {
        let app = app.clone();
        async move {
            call(
                &app,
                "POST",
                "/submissions",
                Some(&token_for(ERIN)),
                Some(body),
            )
            .await
        }
    };
    let body = |language: &str, code: String| json!({"problem_id": PRACTICE, "language": language, "code": code, "elapsed_secs": 5});
    // 빈 코드·공백만인 코드·상한을 넘는 코드·모르는 언어는 422(큐에 넣지 않는다).
    for bad in [
        body("python", String::new()),
        body("python", "  \n\t".into()),
        body("python", "x".repeat(MAX_CODE_BYTES + 1)),
        body("rust", "# correct".into()),
    ] {
        let (status, response) = post(bad).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    }
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM submissions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(queued, 0);
    // 상한 바로 아래는 받고, 4개 언어 모두 받는다(채점은 고른 언어로).
    let (status, response) = post(body("python", "#".repeat(MAX_CODE_BYTES))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let (status, response) = post(body("cpp", "// correct".into())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    grade_next(&pool).await;
    let id = grade_next(&pool).await;
    let view: SubmissionView = ok(
        &app,
        "GET",
        &format!("/submissions/{id}"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    let SubmissionState::Done { report } = view.state else {
        panic!("채점이 끝나야 한다: {view:?}");
    };
    assert_eq!(report.language, promptlab_shared::coding::Language::Cpp);
    assert_eq!(report.passed, report.total);
}
