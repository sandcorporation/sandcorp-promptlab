//! 시험 이음매 ②: 서버 HTTP API. 라우터에 프로세스 안에서 요청을 보낸다(포트를 열지 않는다).
//! 저장소는 시험 DB(`DATABASE_URL`) — `#[sqlx::test]`가 시험마다 새 DB를 만들고 마이그레이션을 돌린다.
//! 채점은 채점 워커의 한 걸음(`promptlab_worker::run_once`)을 가짜 판정기(`common::judge`)로 돌린다.
//!
//! 불변식 시험: 문제 조회·제출·결과 조회 응답 어디에도 히든 테스트·모범 답안·케이스별 실행 결과가 없다
//! (`common::call`이 모든 응답 본문을 훑는다).

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, app, call, token_for};
use promptlab_shared::api::{HealthResponse, SubmissionState, SubmissionView};
use promptlab_shared::coding::{Language, MAX_CODE_BYTES, PublicCodingProblem};
/// 공개 픽스처의 시험 문항(`crates/store/fixtures/problems/`).
const EXAM_PROBLEM: &str = "count-vowels";
use promptlab_store::{PgPool, catalog};
use promptlab_worker::fake::Mode;
use promptlab_worker::{Outcome, run_once};
use serde_json::Value;

/// 개인 제출은 연습 문제만 받는다(#18).
const PRACTICE: &str = "array-sum";

fn submission_body(code: &str) -> Value {
    serde_json::to_value(common::practice_request(PRACTICE, code, 300, None)).unwrap()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn health는_토큰_없이_200과_ok를_준다(pool: PgPool) {
    let (status, body) = call(&app(pool), "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let body: HealthResponse = serde_json::from_value(body).unwrap();
    assert_eq!(body.status, "ok");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 토큰이_없거나_틀리면_401(pool: PgPool) {
    let app = app(pool);
    let problem = format!("/problems/{PRACTICE}");
    for token in [None, Some("틀린-토큰"), Some("")] {
        let (status, _) = call(&app, "GET", &problem, token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token:?}");
        let (status, _) = call(
            &app,
            "POST",
            "/submissions",
            token,
            Some(submission_body("correct")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 남의_개인_제출은_404(pool: PgPool) {
    let app = app(pool);
    let (_, body) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(submission_body("correct")),
    )
    .await;
    let created: SubmissionView = serde_json::from_value(body).unwrap();
    let (status, _) = call(
        &app,
        "GET",
        &format!("/submissions/{}", created.id),
        Some(&token_for(BOB)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 문제_조회는_공개_보기만_준다(pool: PgPool) {
    let (status, body) = call(
        &app(pool),
        "GET",
        &format!("/problems/{PRACTICE}"),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let problem: PublicCodingProblem = serde_json::from_value(body).unwrap();
    let full = catalog::find(PRACTICE).unwrap();
    assert_eq!(problem, full.public_view());
    assert_eq!(problem.templates.len(), 4, "4개 언어 틀");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 시험_문항은_연습에_열지_않는다(pool: PgPool) {
    let app = app(pool);
    assert!(!catalog::is_practice(EXAM_PROBLEM));
    let (status, _) = call(
        &app,
        "GET",
        &format!("/problems/{EXAM_PROBLEM}"),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut body = submission_body("correct");
    body["problem_id"] = EXAM_PROBLEM.into();
    let (status, _) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 모르는_문제는_404(pool: PgPool) {
    let app = app(pool);
    let (status, _) = call(
        &app,
        "GET",
        "/problems/없는-문제",
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let mut body = submission_body("correct");
    body["problem_id"] = "없는-문제".into();
    let (status, _) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 빈_코드나_상한을_넘는_코드는_422(pool: PgPool) {
    let app = app(pool);
    let too_long = "x".repeat(MAX_CODE_BYTES + 1);
    for code in ["", "   \n", too_long.as_str()] {
        let (status, _) = call(
            &app,
            "POST",
            "/submissions",
            Some(&token_for(ALICE)),
            Some(submission_body(code)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}바이트",
            code.len()
        );
    }
    // 딱 상한이면 받는다.
    let (status, _) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(submission_body(&"x".repeat(MAX_CODE_BYTES))),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 언어를_고르지_않거나_모르는_언어면_받지_않는다(pool: PgPool) {
    let app = app(pool);
    let mut unknown = submission_body("correct");
    unknown["language"] = "rust".into();
    let mut missing = submission_body("correct");
    missing.as_object_mut().unwrap().remove("language");
    for body in [unknown, missing] {
        let (status, _) = call(
            &app,
            "POST",
            "/submissions",
            Some(&token_for(ALICE)),
            Some(body),
        )
        .await;
        assert!(status.is_client_error(), "{status}");
        assert_ne!(status, StatusCode::ACCEPTED);
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 모르는_제출은_404(pool: PgPool) {
    let app = app(pool);
    for id in ["00000000-0000-0000-0000-000000000000", "uuid-아님"] {
        let (status, _) = call(
            &app,
            "GET",
            &format!("/submissions/{id}"),
            Some(&token_for(ALICE)),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 제출하면_큐에_들어가고_워커가_채점한_결과를_조회한다(pool: PgPool) {
    let app = app(pool.clone());

    let (status, body) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(submission_body("correct")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let created: SubmissionView = serde_json::from_value(body).unwrap();
    assert_eq!(created.state, SubmissionState::Queued);
    let uri = format!("/submissions/{}", created.id);

    let (status, body) = call(&app, "GET", &uri, Some(&token_for(ALICE)), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<SubmissionView>(body).unwrap(),
        created
    );

    let outcome = common::grade(&pool).await;
    assert_eq!(outcome, Outcome::Graded(created.id.parse().unwrap()));

    let (status, body) = call(&app, "GET", &uri, Some(&token_for(ALICE)), None).await;
    assert_eq!(status, StatusCode::OK);
    // 응시자에게는 언어·통과 수만 — 케이스별 결과·실행 출력은 없다(call()이 키·히든 테스트를 훑는다).
    let report = &body["state"]["report"];
    let mut keys: Vec<&str> = report
        .as_object()
        .expect("채점 보고")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["language", "passed", "problem_id", "total"],
        "{body}"
    );
    let view: SubmissionView = serde_json::from_value(body).unwrap();
    let SubmissionState::Done { report } = view.state else {
        panic!("채점이 끝나야 한다: {view:?}");
    };
    let total = catalog::find(PRACTICE).unwrap().hidden.len() as u32;
    assert_eq!(report.problem_id, PRACTICE);
    assert_eq!(report.language, Language::Python);
    assert_eq!((report.passed, report.total), (total, total));
    // 최종 점수 = 통과율(AI 활용 평가 준비 중).
    let score = view.score.expect("채점이 끝나면 점수");
    assert_eq!(score.final_score, Some(100.0));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 첫_히든만_맞히면_통과_수가_1이다(pool: PgPool) {
    let app = app(pool.clone());
    let (_, body) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(submission_body("first-only")),
    )
    .await;
    let created: SubmissionView = serde_json::from_value(body).unwrap();
    common::grade(&pool).await;
    let (_, body) = call(
        &app,
        "GET",
        &format!("/submissions/{}", created.id),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    let view: SubmissionView = serde_json::from_value(body).unwrap();
    let SubmissionState::Done { report } = view.state else {
        panic!("{view:?}");
    };
    assert_eq!(report.passed, 1);
    assert_eq!(
        report.total as usize,
        catalog::find(PRACTICE).unwrap().hidden.len()
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 채점에_실패하면_실패_이유만_보인다(pool: PgPool) {
    let app = app(pool.clone());
    let (_, body) = call(
        &app,
        "POST",
        "/submissions",
        Some(&token_for(ALICE)),
        Some(submission_body("correct")),
    )
    .await;
    let created: SubmissionView = serde_json::from_value(body).unwrap();

    // 판정기가 요청을 거부(422)하면 채점 실패 — 응시자에게는 짧은 이유만(call()이 히든 테스트·상세 키를 훑는다).
    let judge = common::judge();
    judge.set_mode(Mode::Rejected);
    run_once(&pool, &judge).await.unwrap();

    let (status, body) = call(
        &app,
        "GET",
        &format!("/submissions/{}", created.id),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view: SubmissionView = serde_json::from_value(body).unwrap();
    assert!(
        matches!(&view.state, SubmissionState::Failed { reason } if reason == promptlab_worker::REASON_JUDGE_REJECTED),
        "{:?}",
        view.state
    );
    // 판정기에는 히든 케이스 인자만 갔다(기대값 없음) — 요청 본문에 expected가 없다.
    for request in judge.requests() {
        let text = serde_json::to_string(&request).unwrap();
        assert!(!text.contains("expected"), "{text}");
    }
}
