//! 데이터 보유·자동 삭제 이음매 ②(#54): 보관 기간 설정(권한·범위·이력), 경계 시각 삭제(다른 조직 무영향,
//! 결과·CSV·회차·사용량·운영자 지표 유지), 개인 연습 1년 무활동 삭제, 탈퇴 웹훅(비밀 여럿), 스모크 조직.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use promptlab_server::auth::token::Clock;
use promptlab_server::retention::{WebhookSecrets, purge_once};
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamResults, ExamView, InviteCreated, MemberRounds, OrgView, SubmissionView,
};
use promptlab_shared::report::OrgUsage;
use promptlab_shared::retention::{DEFAULT_DAYS, RetentionView};
use promptlab_store::PgPool;
use serde_json::{Value, json};
use sha2::Sha256;
use tower::ServiceExt;
use uuid::Uuid;

const DAY: i64 = 86_400;
/// 내장 시험 문항.
const EXAM_PROBLEM: &str = "count-vowels";
const PRACTICE: &str = "array-sum";

struct Env {
    app: axum::Router,
    clock: Arc<FixedClock>,
    pool: PgPool,
}

fn env_with(pool: PgPool, secrets: &str) -> Env {
    let clock = FixedClock::at(NOW);
    let mut state = app_state(pool.clone(), clock.clone(), None);
    state.webhook_secrets = WebhookSecrets::parse(secrets);
    Env {
        app: promptlab_server::router(state),
        clock,
        pool,
    }
}

fn env(pool: PgPool) -> Env {
    env_with(pool, "")
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

async fn status_of(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
) -> StatusCode {
    call(app, method, uri, Some(&token_for(sub)), body).await.0
}

/// `admin`이 만든 승인된 조직. `staff`는 (사람, 역할)로 들인다.
async fn org_of(app: &axum::Router, admin: &str, name: &str, staff: &[(&str, Role)]) -> OrgView {
    let org: OrgView = ok(
        app,
        "POST",
        "/orgs",
        admin,
        Some(json!({"name": name, "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    approve_org(app, &org.id).await;
    for (sub, role) in staff {
        let invite: InviteCreated = ok(
            app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            admin,
            Some(json!({ "role": role })),
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

async fn exam_of(app: &axum::Router, org: &OrgView, author: &str) -> ExamView {
    ok(
        app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        author,
        Some(json!({
            "title": "보관 시험",
            "purpose": "internal",
            "reveal_scores": false,
            "items": [{"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await
}

/// `sub`가 배정을 받아 문항 하나를 내고 채점까지 끝낸다. 제출 시각은 서버 시계로 맞춘다.
async fn take(
    e: &Env,
    org: &OrgView,
    exam: &ExamView,
    author: &str,
    email: &str,
    sub: &str,
) -> AssignmentView {
    let created: Value = ok(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        author,
        Some(json!({ "emails": [email] })),
        StatusCode::CREATED,
    )
    .await;
    let token = created["invites"][0]["token"].as_str().unwrap().to_string();
    let a: AssignmentView = ok(
        &e.app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        sub,
        None,
        StatusCode::OK,
    )
    .await;
    webcam_ready(&e.app, &token_for(sub), &a.id).await;
    let _: Value = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/0/start", a.id),
        sub,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    let s: SubmissionView = ok(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/0/submit", a.id),
        sub,
        Some(
            logged_submit_body(
                &e.app,
                &format!("/assignments/{}/items/0/submit", a.id),
                &token_for(sub),
                "correct",
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await;
    // 제출 시각은 DB now()라 시험 시계에 맞춘다.
    sqlx::query("UPDATE submissions SET created_at = to_timestamp($2) WHERE id = $1::uuid")
        .bind(&s.id)
        .bind(e.clock.now() as f64)
        .execute(&e.pool)
        .await
        .unwrap();
    grade(&e.pool).await;
    a
}

/// 과정 로그·신호·대조·리포트·비용 기록을 배정에 직접 심는다(서명 검증은 logs_api.rs 몫).
async fn seed_children(pool: &PgPool, invite: &str, org: &str, exam: &str) {
    let invite: Uuid = invite.parse().unwrap();
    let org: Uuid = org.parse().unwrap();
    let exam: Uuid = exam.parse().unwrap();
    for sql in [
        "INSERT INTO exam_log_keys (invite_id, position, public_key, registered_at) VALUES ($1, 0, 'k', now())",
        "INSERT INTO exam_log_events (invite_id, position, seq, body, sig, hash, received_at) VALUES ($1, 0, 1000, '{}', 's', 'h', now())",
        "INSERT INTO exam_log_rejections (invite_id, position, count, last_reason, last_at) VALUES ($1, 0, 1, 'r', now())",
        "INSERT INTO exam_integrity_rejections (invite_id, position, app_version, model_sha256, app_ok, model_ok, count, first_at, last_at) VALUES ($1, 0, 'v', 'm', true, false, 1, now(), now())",
    ] {
        sqlx::query(sql).bind(invite).execute(pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO exam_reports (invite_id, org_id, status, requested_at) VALUES ($1, $2, 'done', now())
         ON CONFLICT (invite_id) DO NOTHING",
    )
    .bind(invite)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO usage_records (kind, org_id, exam_id, invite_id, model, input_tokens, output_tokens,
                                    duration_ms, price_input, price_output, price_cache_read, price_cache_write, cost_usd)
         VALUES ('report', $1, $2, $3, 'claude', 100, 50, 10, 1, 1, 1, 1, 0.25)",
    )
    .bind(org)
    .bind(exam)
    .bind(invite)
    .execute(pool)
    .await
    .unwrap();
}

async fn count(pool: &PgPool, table: &str, invite: &str) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE invite_id = $1::uuid"
    ))
    .bind(invite)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 보관_기간은_기본_1년이고_조직은_관리자_시험은_시험_관리_역할이_바꾼다(
    pool: PgPool,
) {
    let e = env(pool);
    let org = org_of(
        &e.app,
        ALICE,
        "에이",
        &[(BOB, Role::Author), (DAVE, Role::Viewer)],
    )
    .await;
    let exam = exam_of(&e.app, &org, BOB).await;
    let org_uri = format!("/orgs/{}/retention", org.id);
    let exam_uri = format!("/orgs/{}/exams/{}/retention", org.id, exam.id);

    let v: RetentionView = ok(&e.app, "GET", &org_uri, DAVE, None, StatusCode::OK).await;
    assert_eq!(
        (v.org_days, v.effective_days, v.default_days),
        (None, DEFAULT_DAYS, 365)
    );
    assert!(v.history.is_empty());

    // 조직 기본값: 관리자만. 출제자·열람자 403, 바깥 사람 404, 범위 밖 422.
    assert_eq!(
        status_of(&e.app, "PUT", &org_uri, BOB, Some(json!({"days": 90}))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(&e.app, "PUT", &org_uri, DAVE, Some(json!({"days": 90}))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(&e.app, "PUT", &org_uri, ERIN, Some(json!({"days": 90}))).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(&e.app, "GET", &org_uri, ERIN, None).await,
        StatusCode::NOT_FOUND
    );
    for bad in [0, 29, 1096] {
        assert_eq!(
            status_of(&e.app, "PUT", &org_uri, ALICE, Some(json!({ "days": bad }))).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let v: RetentionView = ok(
        &e.app,
        "PUT",
        &org_uri,
        ALICE,
        Some(json!({"days": 90})),
        StatusCode::OK,
    )
    .await;
    assert_eq!((v.org_days, v.effective_days), (Some(90), 90));

    // 시험별: 시험 관리(관리자·출제자). 열람자 403.
    assert_eq!(
        status_of(&e.app, "PUT", &exam_uri, DAVE, Some(json!({"days": 60}))).await,
        StatusCode::FORBIDDEN
    );
    e.clock.advance(10);
    let v: RetentionView = ok(
        &e.app,
        "PUT",
        &exam_uri,
        BOB,
        Some(json!({"days": 60})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (v.org_days, v.exam_days, v.effective_days),
        (Some(90), Some(60), 60)
    );
    // 같은 값은 이력을 늘리지 않는다.
    let v: RetentionView = ok(
        &e.app,
        "PUT",
        &exam_uri,
        BOB,
        Some(json!({"days": 60})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(v.history.len(), 2);
    // 이력: 새것부터, 누가·전·후.
    assert_eq!(v.history[0].by_sub, BOB);
    assert_eq!(v.history[0].exam_id.as_deref(), Some(exam.id.as_str()));
    assert_eq!(
        (v.history[0].old_days, v.history[0].new_days),
        (None, Some(60))
    );
    assert_eq!(v.history[1].by_sub, ALICE);
    assert_eq!(
        (v.history[1].exam_id.as_deref(), v.history[1].new_days),
        (None, Some(90))
    );

    // 시험별을 지우면 조직 기본으로, 조직 기본을 지우면 1년으로.
    let v: RetentionView = ok(
        &e.app,
        "PUT",
        &exam_uri,
        BOB,
        Some(json!({"days": null})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(v.effective_days, 90);
    let v: RetentionView = ok(
        &e.app,
        "PUT",
        &org_uri,
        ALICE,
        Some(json!({"days": null})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(v.effective_days, 365);
    assert_eq!(v.history.len(), 4, "조직 보기는 시험 변경까지 싣는다");
    let r: ExamResults = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/results", org.id, exam.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(r.retention_days, 365);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 경계_시각에_응시_데이터를_지우고_집계는_남고_다른_조직은_그대로다(
    pool: PgPool,
) {
    let e = env(pool);
    let a_org = org_of(&e.app, ALICE, "에이", &[(BOB, Role::Author)]).await;
    let b_org = org_of(&e.app, DAVE, "비", &[]).await;
    let a_exam = exam_of(&e.app, &a_org, BOB).await;
    let b_exam = exam_of(&e.app, &b_org, DAVE).await;
    let _: RetentionView = ok(
        &e.app,
        "PUT",
        &format!("/orgs/{}/exams/{}/retention", a_org.id, a_exam.id),
        BOB,
        Some(json!({"days": 30})),
        StatusCode::OK,
    )
    .await;

    let a = take(&e, &a_org, &a_exam, BOB, "erin@example.com", ERIN).await;
    seed_children(&e.pool, &a.id, &a_org.id, &a_exam.id).await;
    // 다른 조직의 응시는 열흘 뒤(기본 1년).
    e.clock.advance(10 * DAY);
    let b = take(&e, &b_org, &b_exam, DAVE, "carol@example.com", CAROL).await;

    let results_uri = format!("/orgs/{}/exams/{}/results", a_org.id, a_exam.id);
    let r: ExamResults = ok(&e.app, "GET", &results_uri, ALICE, None, StatusCode::OK).await;
    assert_eq!(r.retention_days, 30);
    let delete_at = r.rows[0].delete_at.expect("삭제 시각");
    assert!(r.rows[0].delete_in_days.unwrap() <= 30);
    let usage_before: OrgUsage = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/usage", a_org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(usage_before.attempts, 1);

    // 경계 1초 전에는 아무것도 지우지 않는다.
    e.clock.set(delete_at - 1);
    let n = purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    assert!(n.is_empty(), "{n:?}");
    // 경계 시각에 지운다.
    e.clock.set(delete_at);
    let n = purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    assert_eq!(n.assignments, 1);
    assert_eq!(n.submissions, 1);
    assert_eq!(n.item_starts, 1);
    // 과정 로그: 제출이 올린 사건 3개(언어 고름·코드 스냅숏·제출)·키 1개 + 심은 사건·키 하나씩.
    assert_eq!((n.log_events, n.log_keys, n.log_rejections), (4, 2, 1));
    assert_eq!((n.integrity_rejections, n.reports), (1, 1));
    assert_eq!((n.webcam_consents, n.webcam_photos), (1, 1));
    assert!(n.usage_unlinked >= 1);
    for table in [
        "exam_item_starts",
        "submissions",
        "exam_log_events",
        "exam_reports",
        "exam_webcam_photos",
        "usage_records",
    ] {
        assert_eq!(count(&e.pool, table, &a.id).await, 0, "{table}");
    }
    // 다시 돌려도 0건(멱등).
    assert!(
        purge_once(&e.pool, e.clock.as_ref())
            .await
            .unwrap()
            .is_empty()
    );

    // 결과·CSV·회차는 그 응시 없이 정상, 시험 구성은 남는다.
    let r: ExamResults = ok(&e.app, "GET", &results_uri, ALICE, None, StatusCode::OK).await;
    assert!(r.rows.is_empty());
    let (status, _) = call(
        &e.app,
        "GET",
        &format!("{results_uri}.csv"),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rounds: Vec<MemberRounds> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/rounds", a_org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(rounds.is_empty());
    let _: ExamView = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}", a_org.id, a_exam.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    // 사용량: 응시 수·비용 숫자는 그대로(개인 연결만 끊김).
    let usage_after: OrgUsage = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/usage", a_org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(usage_after.attempts, 1);
    assert_eq!(usage_after.cost_usd, usage_before.cost_usd);
    assert_eq!(usage_after.exams[0].attempts, 1);
    let (status, _) = operator_call(&e.app, "GET", "/operator/metrics", None).await;
    assert_eq!(status, StatusCode::OK);

    // 다른 조직의 응시는 그대로.
    let r: ExamResults = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/results", b_org.id, b_exam.id),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].invite_id, b.id);
    assert_eq!(count(&e.pool, "submissions", &b.id).await, 1);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 응시_가능_시간이_끝나기_전에는_지우지_않는다(pool: PgPool) {
    let e = env(pool);
    let org = org_of(&e.app, ALICE, "에이", &[]).await;
    let exam = exam_of(&e.app, &org, ALICE).await;
    let a = take(&e, &org, &exam, ALICE, "erin@example.com", ERIN).await;
    // 마감을 2년 뒤로(사내 진단이라 API로는 못 여닫는다 — 열은 채용 시험과 같이 쓴다).
    let ends = NOW + 2 * 365 * DAY;
    sqlx::query("UPDATE exams SET window_ends_at = to_timestamp($2) WHERE id = $1::uuid")
        .bind(&exam.id)
        .bind(ends as f64)
        .execute(&e.pool)
        .await
        .unwrap();
    e.clock.set(NOW + 400 * DAY);
    assert!(
        purge_once(&e.pool, e.clock.as_ref())
            .await
            .unwrap()
            .is_empty()
    );
    e.clock.set(ends + 365 * DAY);
    let n = purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    assert_eq!(n.assignments, 1);
    assert_eq!(count(&e.pool, "exam_item_starts", &a.id).await, 0);
}

async fn practice(e: &Env, sub: &str, at: i64) -> String {
    let s: SubmissionView = ok(
        &e.app,
        "POST",
        "/submissions",
        sub,
        Some(serde_json::to_value(practice_request(PRACTICE, "correct", 30, None)).unwrap()),
        StatusCode::ACCEPTED,
    )
    .await;
    sqlx::query("UPDATE submissions SET created_at = to_timestamp($2) WHERE id = $1::uuid")
        .bind(&s.id)
        .bind(at as f64)
        .execute(&e.pool)
        .await
        .unwrap();
    grade(&e.pool).await;
    s.id
}

async fn practice_count(pool: &PgPool, sub: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM submissions WHERE practice AND candidate_sub = $1")
        .bind(sub)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 마지막_연습_뒤_1년_무활동이면_그_사람의_연습_기록을_모두_지운다(
    pool: PgPool,
) {
    let e = env(pool);
    practice(&e, ERIN, NOW - 30 * DAY).await;
    let last = practice(&e, ERIN, NOW).await;
    practice(&e, BOB, NOW + 100 * DAY).await;
    sqlx::query(
        "INSERT INTO usage_records (kind, submission_id, model, input_tokens, output_tokens, duration_ms,
                                    price_input, price_output, price_cache_read, price_cache_write, cost_usd)
         VALUES ('evaluation', $1::uuid, 'local', 10, 5, 1, 0, 0, 0, 0, 0)",
    )
    .bind(&last)
    .execute(&e.pool)
    .await
    .unwrap();

    e.clock.set(NOW + 365 * DAY - 1);
    assert!(
        purge_once(&e.pool, e.clock.as_ref())
            .await
            .unwrap()
            .is_empty()
    );
    e.clock.set(NOW + 365 * DAY);
    let n = purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    assert_eq!(n.submissions, 2, "ERIN의 연습 둘 다(마지막 것 기준)");
    assert!(n.usage_unlinked >= 1);
    assert_eq!(practice_count(&e.pool, ERIN).await, 0);
    assert_eq!(
        practice_count(&e.pool, BOB).await,
        1,
        "최근에 연습한 사람은 그대로"
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_records WHERE kind = 'evaluation' AND submission_id IS NULL",
    )
    .fetch_one(&e.pool)
    .await
    .unwrap();
    assert!(left >= 1, "비용 숫자는 남는다");
    let records: Value = ok(&e.app, "GET", "/me/practice", ERIN, None, StatusCode::OK).await;
    assert_eq!(
        records["attempts"].as_array().map(Vec::len),
        Some(0),
        "{records}"
    );
}

fn signed(secret: &str, t: i64, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{t}.").as_bytes());
    mac.update(body.as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("t={t},v1={hex}")
}

async fn webhook(app: &axum::Router, signature: Option<&str>, body: &str) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/auth/webhook")
        .header("content-type", "application/json")
        .header("x-sandcorp-event", "user.deleted");
    if let Some(sig) = signature {
        request = request.header("x-sandcorp-signature", sig);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let _ = response.into_body().collect().await.unwrap();
    status
}

fn deleted(sub: &str) -> String {
    json!({"id": "01JB0Z8F3W4Q2Y6K7M9N0P1R2S", "type": "user.deleted", "sub": sub, "occurred_at": "2026-01-01T00:00:00Z"})
        .to_string()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 탈퇴_웹훅은_어느_비밀로든_서명이_맞으면_연습_기록을_즉시_지운다(
    pool: PgPool,
) {
    let e = env_with(pool, "first-secret, second-secret");
    practice(&e, ERIN, NOW).await;
    practice(&e, BOB, NOW).await;
    let body = deleted(ERIN);

    // W1 형식·W2 시각·W3 서명.
    assert_eq!(webhook(&e.app, None, &body).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        webhook(&e.app, Some("v1=abc"), &body).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        webhook(
            &e.app,
            Some(&signed("second-secret", NOW - 301, &body)),
            &body
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        webhook(&e.app, Some(&signed("wrong-secret", NOW, &body)), &body).await,
        StatusCode::UNAUTHORIZED
    );
    // 서명한 본문과 다른 본문.
    assert_eq!(
        webhook(
            &e.app,
            Some(&signed("first-secret", NOW, &body)),
            &deleted(BOB)
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        practice_count(&e.pool, ERIN).await,
        1,
        "거부하면 지우지 않는다"
    );

    // 두 번째 비밀(앱 클라이언트)로 서명해도 받는다.
    assert_eq!(
        webhook(
            &e.app,
            Some(&signed("second-secret", NOW + 299, &body)),
            &body
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(practice_count(&e.pool, ERIN).await, 0);
    assert_eq!(practice_count(&e.pool, BOB).await, 1);
    // 같은 이벤트를 다시 받아도(재시도) 2xx.
    assert_eq!(
        webhook(&e.app, Some(&signed("first-secret", NOW, &body)), &body).await,
        StatusCode::NO_CONTENT
    );
    // 모르는 종류는 무시하고 2xx.
    let other = json!({"id": "x", "type": "user.renamed", "sub": BOB}).to_string();
    assert_eq!(
        webhook(&e.app, Some(&signed("first-secret", NOW, &other)), &other).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(practice_count(&e.pool, BOB).await, 1);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 비밀이_없으면_탈퇴_웹훅은_꺼져_있다(pool: PgPool) {
    let e = env(pool);
    let body = deleted(ERIN);
    assert_eq!(
        webhook(&e.app, Some(&signed("any", NOW, &body)), &body).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크_조직의_오래된_응시도_지우고_스모크는_계속_돈다(
    pool: PgPool,
) {
    let e = env(pool);
    let smoke = machine_token(SMOKE_CLIENT);
    let list = |app: axum::Router| {
        let smoke = smoke.clone();
        async move {
            let (status, body) = call(&app, "GET", "/me/assignments", Some(&smoke), None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            serde_json::from_value::<Vec<AssignmentView>>(body).unwrap()
        }
    };
    let first = list(e.app.clone()).await;
    assert_eq!(first.len(), 1);
    let (status, body) = call(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/0/start", first[0].id),
        Some(&smoke),
        Some(start_body()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    e.clock.set(NOW + 2 * 365 * DAY);
    let n = purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    assert!(n.assignments >= 1);
    assert_eq!(count(&e.pool, "exam_item_starts", &first[0].id).await, 0);

    // 다음 스모크: 새 배정이 생겨 시작할 수 있다(시험·조직은 남아 있다).
    let next = list(e.app.clone()).await;
    assert_eq!(next.len(), 1);
    assert_ne!(next[0].id, first[0].id);
    let (status, body) = call(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/0/start", next[0].id),
        Some(&smoke),
        Some(start_body()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
