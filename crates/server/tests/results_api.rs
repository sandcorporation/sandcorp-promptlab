//! 결과 이음매 ②: 응시 현황·점수·과정 지표, CSV, 같은 구성원 회차 비교, 응시자 점수 공개 설정(#10).
//! 모든 응답에 숨은 입력 불변식이 걸린다(`common::send`).

mod common;

use std::sync::Arc;

use axum::http::{Request, StatusCode, header};
use common::*;
use promptlab_server::results::{csv_cell, results_csv};
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamResults, ExamView, InviteCreated, MemberRounds, OrgView, ResultItem,
    ResultItemStatus, ResultRow, SubmissionState, SubmissionView,
};
use promptlab_shared::coding::Language;
use promptlab_shared::exam::{InviteStatus, Purpose};
use promptlab_store::{PgPool, catalog};
use promptlab_worker::Outcome;
use promptlab_worker::fake::Mode;
use serde_json::{Value, json};

/// 시험 문항 0·1의 문제(내장 시험 문항).
const ITEM0: &str = "count-vowels";
const ITEM1: &str = "second-largest";

/// 문제의 첫 히든 케이스만 맞힌 통과율(코드 `first-only`).
fn first_only_rate(problem: &str) -> f64 {
    1.0 / catalog::find(problem).unwrap().hidden.len() as f64
}

struct Env {
    app: axum::Router,
    clock: Arc<FixedClock>,
    pool: PgPool,
}

fn env(pool: PgPool) -> Env {
    let clock = FixedClock::at(NOW);
    Env {
        app: app_parts(
            pool.clone(),
            clock.clone(),
            Arc::new(promptlab_server::mail::Disabled),
            None,
        ),
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

/// ALICE 관리자, BOB 출제자, CAROL 감독관, DAVE 열람자. ERIN은 바깥 사람(응시자).
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

async fn create_exam(
    app: &axum::Router,
    org: &OrgView,
    title: &str,
    purpose: Purpose,
    reveal: bool,
) -> ExamView {
    ok(
        app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(json!({
            "title": title,
            "purpose": purpose,
            "reveal_scores": reveal,
            "items": [
                {"problem_id": ITEM0, "limits": {"time_limit_secs": 600}},
                {"problem_id": ITEM1, "limits": {"time_limit_secs": 1200}},
            ],
        })),
        StatusCode::CREATED,
    )
    .await
}

/// 이메일 하나를 초대하고 `sub`가 링크로 배정을 받는다.
async fn assign(e: &Env, org: &OrgView, exam: &ExamView, email: &str, sub: &str) -> AssignmentView {
    let created: Value = ok(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": [email]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created["invites"][0]["token"].as_str().unwrap().to_string();
    let assignment: AssignmentView = ok(
        &e.app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        sub,
        None,
        StatusCode::OK,
    )
    .await;
    webcam_ready(&e.app, &token_for(sub), &assignment.id).await;
    assignment
}

/// 첫 히든 케이스만 맞히는 제출 코드(`common::judge`).
const CODE: &str = "# first-only";

/// 문항을 시작하고 `secs`초 뒤 낸다.
async fn take_item(
    e: &Env,
    a: &AssignmentView,
    sub: &str,
    position: u32,
    secs: i64,
) -> SubmissionView {
    let _: Value = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/{position}/start", a.id),
        sub,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    e.clock.advance(secs);
    ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/{position}/submit", a.id),
        sub,
        Some(
            logged_submit_body(
                &e.app,
                &format!("/assignments/{}/items/{position}/submit", a.id),
                &token_for(sub),
                CODE,
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await
}

/// 채점 큐에서 하나를 가짜 판정기로 채점한다(코드대로 — `first-only`면 첫 히든만 통과).
async fn grade_one(e: &Env) {
    let outcome = grade(&e.pool).await;
    assert!(matches!(outcome, Outcome::Graded(_)), "{outcome:?}");
}

/// 판정기가 거부해(422) 채점 실패로 남긴다.
async fn grade_rejected(e: &Env) {
    let judge = judge();
    judge.set_mode(Mode::Rejected);
    let outcome = promptlab_worker::run_once(&e.pool, &judge).await.unwrap();
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
}

async fn results(e: &Env, org: &OrgView, exam: &ExamView, sub: &str) -> ExamResults {
    ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/results", org.id, exam.id),
        sub,
        None,
        StatusCode::OK,
    )
    .await
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 응시_현황_점수_지표를_결과_보기_역할이_본다(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, "1차", Purpose::Internal, false).await;
    let a = assign(&e, &org, &exam, "erin@example.com", ERIN).await;

    // 시작 전: 초대됨, 점수 없음.
    let r = results(&e, &org, &exam, DAVE).await;
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].status, InviteStatus::Invited);
    assert_eq!(r.rows[0].score, None);
    assert!(
        r.rows[0]
            .items
            .iter()
            .all(|i| i.status == ResultItemStatus::NotStarted && i.model.is_none())
    );

    // 문항 0: 42초 걸려 냄 → 채점 대기. 문항 1은 시작만 — 더 작은 후보 모델로(#131 「쓴 모델」).
    take_item(&e, &a, ERIN, 0, 42).await;
    let small = promptlab_shared::inference::model_by_id("1.5b").unwrap();
    let _: Value = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/1/start", a.id),
        ERIN,
        Some(json!({
            "app_version": promptlab_shared::integrity::ALLOWED_APP_VERSIONS[0],
            "model_sha256": small.sha256,
        })),
        StatusCode::OK,
    )
    .await;
    let r = results(&e, &org, &exam, CAROL).await;
    assert_eq!(r.rows[0].status, InviteStatus::InProgress);
    assert_eq!(r.rows[0].items[0].status, ResultItemStatus::Submitted);
    assert_eq!(r.rows[0].items[1].status, ResultItemStatus::InProgress);
    assert_eq!(r.rows[0].items[0].elapsed_secs, Some(42));
    assert_eq!(r.rows[0].items[0].language, Some(Language::Python));
    // 쓴 모델: 문항 시작 신고를 후보 표의 이름으로 푼다.
    assert_eq!(
        r.rows[0].items[0].model.as_deref(),
        Some(promptlab_shared::inference::LARGEST_MODEL.label)
    );
    assert_eq!(r.rows[0].items[1].model.as_deref(), Some(small.label));
    assert_eq!(r.rows[0].score, None);

    // 문항 1 제출. 문항 0은 채점 성공(첫 히든만), 문항 1은 채점 실패(0으로 센다).
    e.clock.advance(7);
    let _: SubmissionView = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/1/submit", a.id),
        ERIN,
        Some(
            logged_submit_body(
                &e.app,
                &format!("/assignments/{}/items/1/submit", a.id),
                &token_for(ERIN),
                CODE,
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await;
    grade_one(&e).await;
    let r = results(&e, &org, &exam, ALICE).await;
    assert_eq!(r.rows[0].status, InviteStatus::Submitted);
    grade_rejected(&e).await;

    let r = results(&e, &org, &exam, ALICE).await;
    let row = &r.rows[0];
    assert_eq!(row.status, InviteStatus::Graded);
    let first = &row.items[0];
    assert_eq!(first.status, ResultItemStatus::Done);
    let total = catalog::find(ITEM0).unwrap().hidden.len() as u32;
    assert_eq!((first.passed, first.total), (Some(1), Some(total)));
    assert!((first.pass_rate.unwrap() - first_only_rate(ITEM0)).abs() < 1e-9);
    assert_eq!(first.language, Some(Language::Python));
    // 최종 점수 = 통과율(AI 활용 평가 준비 중).
    let score = first.score.as_ref().expect("채점이 끝나면 점수");
    assert!((score.final_score.unwrap() - first_only_rate(ITEM0) * 100.0).abs() < 1e-6);
    let second = &row.items[1];
    assert_eq!(second.status, ResultItemStatus::Failed);
    assert_eq!(second.elapsed_secs, Some(7));
    assert_eq!(second.pass_rate, None);
    assert!((row.score.unwrap() - first_only_rate(ITEM0) / 2.0).abs() < 1e-9);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 결과는_결과_보기_역할만_출제자_403_다른_조직_404(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, "1차", Purpose::Internal, false).await;
    let other: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        ERIN,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    let exam_path = format!("/orgs/{}/exams/{}", org.id, exam.id);
    for path in [
        format!("{exam_path}/results"),
        format!("{exam_path}/results.csv"),
        format!("/orgs/{}/rounds", org.id),
    ] {
        for sub in [ALICE, CAROL, DAVE] {
            assert_eq!(status_of(&e.app, "GET", &path, sub).await, StatusCode::OK);
        }
        assert_eq!(
            status_of(&e.app, "GET", &path, BOB).await,
            StatusCode::FORBIDDEN,
            "출제자는 결과를 못 본다: {path}"
        );
        // 구성원이 아닌 사람, 그리고 다른 조직 경로로 이 시험을 보려는 사람.
        assert_eq!(
            status_of(&e.app, "GET", &path, ERIN).await,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        status_of(
            &e.app,
            "GET",
            &format!("/orgs/{}/exams/{}/results", other.id, exam.id),
            ERIN
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(
            &e.app,
            "GET",
            &format!("/orgs/{}/exams/{}/results.csv", other.id, exam.id),
            ERIN
        )
        .await,
        StatusCode::NOT_FOUND
    );
    // 토큰이 없으면 401.
    let (status, _) = call(&e.app, "GET", &format!("{exam_path}/results"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn csv는_bom으로_시작하고_수식을_막고_숨은_것을_싣지_않는다(
    pool: PgPool,
) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, "1차", Purpose::Recruiting, false).await;
    // 수식처럼 시작하는 이메일(유효한 주소다).
    let a = assign(&e, &org, &exam, "+kim@example.com", ERIN).await;
    take_item(&e, &a, ERIN, 0, 30).await;
    grade_one(&e).await;

    let request = Request::builder()
        .method("GET")
        .uri(format!("/orgs/{}/exams/{}/results.csv", org.id, exam.id))
        .header(header::AUTHORIZATION, format!("Bearer {}", token_for(DAVE)));
    let (status, headers, _) = send(&e.app, request, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/csv")
    );
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );

    // 본문은 JSON이 아니라 send가 Null로 준다 — 원문을 다시 받는다.
    let response = {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let request = Request::builder()
            .method("GET")
            .uri(format!("/orgs/{}/exams/{}/results.csv", org.id, exam.id))
            .header(header::AUTHORIZATION, format!("Bearer {}", token_for(DAVE)))
            .body(Body::empty())
            .unwrap();
        let response = e.app.clone().oneshot(request).await.unwrap();
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    };
    assert!(response.starts_with('\u{feff}'), "UTF-8 BOM");
    let lines: Vec<&str> = response.trim_start_matches('\u{feff}').lines().collect();
    assert_eq!(lines.len(), 2, "머리 + 응시자 한 명");
    assert!(lines[0].starts_with("이메일,이름,상태,통과율 평균(%),최종 점수,AI 활용 평균"));
    assert!(lines[0].contains("문항2 소요 시간(초)"));
    assert!(lines[0].contains("문항1 언어,문항1 AI 모델,문항1 통과율(%)"));
    for gone in ["턴", "글자 수", "기준선"] {
        assert!(!lines[0].contains(gone), "{gone} 열은 없다: {}", lines[0]);
    }
    assert!(
        lines[1].starts_with("'+kim@example.com,"),
        "수식 주입 방지: {}",
        lines[1]
    );
    let total = catalog::find(ITEM0).unwrap().hidden.len();
    let expected = format!(
        "채점 완료,Python 3.12,{},{:.1},1/{total},30,",
        promptlab_shared::inference::LARGEST_MODEL.label,
        first_only_rate(ITEM0) * 100.0
    );
    assert!(
        lines[1].contains(&expected),
        "{expected} 없음: {}",
        lines[1]
    );
    assert_no_catalog_hidden(&response);
    assert!(!response.contains(CODE), "제출 코드는 싣지 않는다");
}

#[test]
fn csv_칸은_수식_시작과_구분자를_이스케이프한다() {
    for danger in ["=1+1", "+1", "-1", "@SUM(A1)", "\t=1", "\r=1"] {
        let cell = csv_cell(danger);
        let inner = cell.trim_matches('"');
        assert!(inner.starts_with('\''), "{danger:?} → {cell:?}");
    }
    assert_eq!(csv_cell("평범한 글"), "평범한 글");
    assert_eq!(csv_cell("a,b"), "\"a,b\"");
    assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_cell("줄\n바꿈"), "\"줄\n바꿈\"");
    // 이름에 심은 수식도 막는다.
    let row = ResultRow {
        invite_id: "x".into(),
        email: "a@example.com".into(),
        candidate_name: Some("=HYPERLINK(\"http://evil\")".into()),
        status: InviteStatus::Graded,
        items: vec![ResultItem {
            position: 0,
            problem_id: "p".into(),
            status: ResultItemStatus::Done,
            language: Some(Language::Cpp),
            pass_rate: Some(0.5),
            passed: Some(1),
            total: Some(2),
            elapsed_secs: Some(5),
            score: None,
            model: Some("Qwen2.5-Coder 7B".into()),
        }],
        score: Some(0.5),
        final_score: None,
        evaluation_score: None,
        delete_at: None,
        delete_in_days: None,
    };
    let csv = results_csv(&[row]);
    assert!(
        csv.contains("\"'=HYPERLINK(\"\"http://evil\"\")\""),
        "{csv}"
    );
    assert!(csv.contains("50.0"));
    assert!(csv.contains("C++17"));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 같은_사람의_사내_진단_회차를_시간순으로_비교한다(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let first = create_exam(&e.app, &org, "상반기", Purpose::Internal, false).await;
    let hiring = create_exam(&e.app, &org, "채용 라운드", Purpose::Recruiting, false).await;
    let second = create_exam(&e.app, &org, "하반기", Purpose::Internal, true).await;

    // ERIN: 상반기 → 채용 → 하반기 순으로 배정. 상반기는 채점 끝, 하반기는 막 배정.
    let a1 = assign(&e, &org, &first, "erin@example.com", ERIN).await;
    take_item(&e, &a1, ERIN, 0, 10).await;
    take_item(&e, &a1, ERIN, 1, 10).await;
    grade_one(&e).await;
    grade_one(&e).await;
    e.clock.advance(1000);
    assign(&e, &org, &hiring, "erin@example.com", ERIN).await;
    e.clock.advance(1000);
    assign(&e, &org, &second, "erin@example.com", ERIN).await;
    // 다른 사람(FRANK) 하나: 하반기만.
    const FRANK: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2F";
    assign(&e, &org, &second, "frank@example.com", FRANK).await;

    let rounds: Vec<MemberRounds> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/rounds", org.id),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(rounds.len(), 2);
    let erin = rounds.iter().find(|m| m.sub == ERIN).unwrap();
    let titles: Vec<&str> = erin.rounds.iter().map(|r| r.exam_title.as_str()).collect();
    assert_eq!(titles, ["상반기", "하반기"], "채용은 빠지고 시간순");
    assert!(erin.rounds[0].assigned_at < erin.rounds[1].assigned_at);
    assert_eq!(erin.rounds[0].status, InviteStatus::Graded);
    let expected = (first_only_rate(ITEM0) + first_only_rate(ITEM1)) / 2.0;
    assert!((erin.rounds[0].score.unwrap() - expected).abs() < 1e-9);
    assert_eq!(erin.rounds[1].status, InviteStatus::Invited);
    assert_eq!(erin.rounds[1].score, None);
    let frank = rounds.iter().find(|m| m.sub == FRANK).unwrap();
    assert_eq!(frank.rounds.len(), 1);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 응시자_점수_공개_설정이_지켜진다(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, "진단", Purpose::Internal, false).await;
    let a = assign(&e, &org, &exam, "erin@example.com", ERIN).await;
    let sub = take_item(&e, &a, ERIN, 0, 5).await;
    take_item(&e, &a, ERIN, 1, 5).await;
    grade_one(&e).await;
    grade_one(&e).await;
    let path = format!("/submissions/{}", sub.id);
    let seen = |e: &Env| {
        let app = e.app.clone();
        let path = path.clone();
        async move {
            let view: SubmissionView = ok(&app, "GET", &path, ERIN, None, StatusCode::OK).await;
            view.state
        }
    };
    assert_eq!(seen(&e).await, SubmissionState::Completed);

    // 설정을 켜면 응시자가 점수를 본다. 설정은 시험 관리 역할(출제자·관리자)만 바꾼다.
    let exam_path = format!("/orgs/{}/exams/{}", org.id, exam.id);
    for sub in [CAROL, DAVE] {
        let (status, _) = call(
            &e.app,
            "PATCH",
            &exam_path,
            Some(&token_for(sub)),
            Some(json!({"reveal_scores": true})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, _) = call(
        &e.app,
        "PATCH",
        &exam_path,
        Some(&token_for(ERIN)),
        Some(json!({"reveal_scores": true})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let updated: ExamView = ok(
        &e.app,
        "PATCH",
        &exam_path,
        BOB,
        Some(json!({"reveal_scores": true})),
        StatusCode::OK,
    )
    .await;
    assert!(updated.reveal_scores);
    assert!(matches!(seen(&e).await, SubmissionState::Done { .. }));
    assert!(results(&e, &org, &exam, DAVE).await.reveal_scores);

    // 다시 끄면 다시 「응시 완료」만.
    let _: ExamView = ok(
        &e.app,
        "PATCH",
        &exam_path,
        ALICE,
        Some(json!({"reveal_scores": false})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(seen(&e).await, SubmissionState::Completed);

    // 채용은 언제나 「응시 완료」만 — 켜려 하면 422, 만들 때 켜도 무시.
    let hiring = create_exam(&e.app, &org, "채용", Purpose::Recruiting, true).await;
    assert!(!hiring.reveal_scores);
    let (status, _) = call(
        &e.app,
        "PATCH",
        &format!("/orgs/{}/exams/{}", org.id, hiring.id),
        Some(&token_for(BOB)),
        Some(json!({"reveal_scores": true})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let a = assign(&e, &org, &hiring, "gil@example.com", ERIN).await;
    let sub = take_item(&e, &a, ERIN, 0, 5).await;
    grade_one(&e).await;
    let view: SubmissionView = ok(
        &e.app,
        "GET",
        &format!("/submissions/{}", sub.id),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(view.state, SubmissionState::Completed);
}
