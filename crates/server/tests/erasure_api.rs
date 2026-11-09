//! 고객사 관리자의 응시자 데이터 삭제·감사 로그 이음매 ②(#55): 삭제 뒤 결과·CSV·회차·신호·리포트·과정 로그·웹캠에서
//! 사라짐, 다른 응시자·다른 조직 무영향, 감사 기록(이메일 없음·해시로 찾기·보관 기간), 권한 거부, 멱등,
//! 진행 중 응시·채점 큐·채점 중·리포트 큐와 겹칠 때(워커가 사라진 행을 만나도 오류 없음).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::*;
use promptlab_server::auth::token::Clock;
use promptlab_server::retention::purge_once;
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamResults, ExamView, InviteCreated, MemberRounds, OrgView, SubmissionView,
};
use promptlab_shared::erasure::{AUDIT_RETENTION_DAYS, ErasureLog, ErasureRecord, subject_hash};
use promptlab_shared::grading::GradeReport;
use promptlab_shared::report::{LOCAL_PRICES, OrgUsage, TokenUsage, UsageKind};
use promptlab_store::PgPool;
use promptlab_store::usage::{NewUsage, UsageScope};
use promptlab_worker::Outcome;
use serde_json::{Value, json};
use uuid::Uuid;

const DAY: i64 = 86_400;
/// 내장 시험 문항.
const EXAM_PROBLEM: &str = "count-vowels";
const FRANK: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2F";
const GINA: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2G";

struct Env {
    app: axum::Router,
    clock: Arc<FixedClock>,
    pool: PgPool,
}

fn env(pool: PgPool) -> Env {
    let clock = FixedClock::at(NOW);
    let state = app_state(pool.clone(), clock.clone(), None);
    Env {
        app: promptlab_server::router(state),
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
            "title": "삭제 시험",
            "purpose": "internal",
            "reveal_scores": false,
            "items": [{"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await
}

/// `sub`가 배정을 받고 문항 0을 시작한다.
async fn start(
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
    a
}

/// 문항 0을 낸다(채점 큐에 들어간다).
async fn submit(e: &Env, a: &AssignmentView, sub: &str) -> SubmissionView {
    ok(
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
    .await
}

/// 배정을 받아 내고 채점까지 끝낸다.
async fn take(
    e: &Env,
    org: &OrgView,
    exam: &ExamView,
    author: &str,
    email: &str,
    sub: &str,
) -> AssignmentView {
    let a = start(e, org, exam, author, email, sub).await;
    submit(e, &a, sub).await;
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

fn erase_uri(org: &OrgView, exam: &ExamView, invite: &str) -> String {
    format!("/orgs/{}/exams/{}/invites/{invite}/erase", org.id, exam.id)
}

/// 각 화면(배정에서 출발하는 것)의 상태 코드: 신호·리포트·과정 로그·웹캠.
async fn invite_views(e: &Env, org: &OrgView, exam: &ExamView, invite: &str) -> Vec<StatusCode> {
    let base = format!("/orgs/{}/exams/{}/invites/{invite}", org.id, exam.id);
    let mut out = Vec::new();
    for path in ["signals", "report", "items/0/log", "webcam"] {
        out.push(status_of(&e.app, "GET", &format!("{base}/{path}"), ALICE, None).await);
    }
    out
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 관리자가_지운_응시는_모든_화면에서_사라지고_다른_응시자와_다른_조직은_그대로다(
    pool: PgPool,
) {
    let e = env(pool);
    let a_org = org_of(&e.app, ALICE, "에이", &[(BOB, Role::Author)]).await;
    let b_org = org_of(&e.app, DAVE, "비", &[]).await;
    let a_exam = exam_of(&e.app, &a_org, BOB).await;
    let b_exam = exam_of(&e.app, &b_org, DAVE).await;

    let erin = take(&e, &a_org, &a_exam, BOB, "Erin@Example.com", ERIN).await;
    seed_children(&e.pool, &erin.id, &a_org.id, &a_exam.id).await;
    let frank = take(&e, &a_org, &a_exam, BOB, "frank@example.com", FRANK).await;
    let carol = take(&e, &b_org, &b_exam, DAVE, "carol@example.com", CAROL).await;
    let usage_before: OrgUsage = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/usage", a_org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(usage_before.attempts, 2);
    assert!(
        invite_views(&e, &a_org, &a_exam, &erin.id)
            .await
            .iter()
            .all(|s| *s == StatusCode::OK)
    );

    e.clock.advance(60);
    let record: ErasureRecord = ok(
        &e.app,
        "POST",
        &erase_uri(&a_org, &a_exam, &erin.id),
        ALICE,
        Some(json!({"reason": "  응시자 삭제 요청(메일 2026-11-06)  "})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(record.by_sub, ALICE);
    assert_eq!(record.at, NOW + 60);
    assert_eq!(record.invite_id, erin.id);
    assert_eq!(record.exam_id.as_deref(), Some(a_exam.id.as_str()));
    assert_eq!(record.exam_title.as_deref(), Some("삭제 시험"));
    assert_eq!(
        record.reason.as_deref(),
        Some("응시자 삭제 요청(메일 2026-11-06)")
    );
    // 웹캠 사진도 즉시, 범위는 #54와 같다.
    let n = record.erased;
    assert_eq!((n.assignments, n.submissions, n.item_starts), (1, 1, 1));
    // 과정 로그: 제출이 올린 사건 3개(언어 고름·코드 스냅숏·제출)·키 1개 + 심은 사건·키 하나씩.
    assert_eq!((n.log_events, n.log_keys, n.log_rejections), (4, 2, 1));
    assert_eq!((n.integrity_rejections, n.reports), (1, 1));
    assert_eq!((n.webcam_consents, n.webcam_photos), (1, 1));
    // 비용 기록: 심은 리포트 한 줄(판정기 채점은 비용 기록이 없다).
    assert_eq!(n.usage_unlinked, 1, "{n:?}");
    for table in [
        "exam_item_starts",
        "submissions",
        "exam_log_events",
        "exam_reports",
        "exam_webcam_photos",
        "exam_webcam_consents",
        "usage_records",
    ] {
        assert_eq!(count(&e.pool, table, &erin.id).await, 0, "{table}");
    }
    // 감사 기록에 이메일 원문은 없다(해시만).
    let raw = serde_json::to_string(&record).unwrap().to_lowercase();
    assert!(!raw.contains("erin@example.com"), "{raw}");
    assert_eq!(
        record.subject_hash,
        subject_hash(&a_org.id, "erin@example.com")
    );

    // 결과·CSV·회차: 지운 응시자 없이, 다른 응시자는 그대로.
    let results_uri = format!("/orgs/{}/exams/{}/results", a_org.id, a_exam.id);
    let r: ExamResults = ok(&e.app, "GET", &results_uri, ALICE, None, StatusCode::OK).await;
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].invite_id, frank.id);
    let (status, _, csv) = send_raw(
        &e.app,
        "GET",
        &format!("{results_uri}.csv"),
        &token_for(ALICE),
        "text/plain",
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let csv = String::from_utf8(csv).unwrap().to_lowercase();
    assert!(!csv.contains("erin@example.com") && csv.contains("frank@example.com"));
    let rounds: Vec<MemberRounds> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/rounds", a_org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    let rounds = serde_json::to_string(&rounds).unwrap();
    assert!(!rounds.contains(ERIN) && rounds.contains(FRANK), "{rounds}");
    // 신호·리포트·과정 로그·웹캠은 그 배정이 없다(404), 다른 응시자는 그대로.
    assert!(
        invite_views(&e, &a_org, &a_exam, &erin.id)
            .await
            .iter()
            .all(|s| *s == StatusCode::NOT_FOUND)
    );
    assert!(
        invite_views(&e, &a_org, &a_exam, &frank.id)
            .await
            .iter()
            .all(|s| *s == StatusCode::OK)
    );
    assert_eq!(count(&e.pool, "submissions", &frank.id).await, 1);
    assert_eq!(count(&e.pool, "exam_webcam_photos", &frank.id).await, 1);
    // 응시자 자신의 배정 목록에서도 사라진다.
    let mine: Vec<AssignmentView> =
        ok(&e.app, "GET", "/me/assignments", ERIN, None, StatusCode::OK).await;
    assert!(mine.is_empty());
    // 사용량 응시 수·비용은 그대로(#54와 같다).
    let usage_after: OrgUsage = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/usage", a_org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(usage_after.attempts, 2);
    assert_eq!(usage_after.cost_usd, usage_before.cost_usd);

    // 다른 조직은 그대로.
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
    assert_eq!(r.rows[0].invite_id, carol.id);
    let log: ErasureLog = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/erasures", b_org.id),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(log.records.is_empty());
    // 스모크 조직의 배정도 그대로(관리자 삭제는 그 조직 배정만 건드린다).
    let smoke: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM erasure_audit a JOIN orgs o ON o.id = a.org_id WHERE o.is_smoke",
    )
    .fetch_one(&e.pool)
    .await
    .unwrap();
    assert_eq!(smoke, 0);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 관리자만_지우고_감사_로그를_본다(pool: PgPool) {
    let e = env(pool);
    let org = org_of(
        &e.app,
        ALICE,
        "에이",
        &[
            (BOB, Role::Author),
            (CAROL, Role::Proctor),
            (GINA, Role::Viewer),
        ],
    )
    .await;
    let other = org_of(&e.app, DAVE, "비", &[]).await;
    let exam = exam_of(&e.app, &org, BOB).await;
    let erin = take(&e, &org, &exam, BOB, "erin@example.com", ERIN).await;
    let uri = erase_uri(&org, &exam, &erin.id);
    let log_uri = format!("/orgs/{}/erasures", org.id);

    for sub in [BOB, CAROL, GINA] {
        assert_eq!(
            status_of(&e.app, "POST", &uri, sub, Some(json!({}))).await,
            StatusCode::FORBIDDEN,
            "{sub}"
        );
        assert_eq!(
            status_of(&e.app, "GET", &log_uri, sub, None).await,
            StatusCode::FORBIDDEN,
            "{sub}"
        );
    }
    // 다른 조직의 관리자·응시자 본인은 404(조직 구성원이 아니다).
    for sub in [DAVE, ERIN] {
        assert_eq!(
            status_of(&e.app, "POST", &uri, sub, Some(json!({}))).await,
            StatusCode::NOT_FOUND,
            "{sub}"
        );
        assert_eq!(
            status_of(&e.app, "GET", &log_uri, sub, None).await,
            StatusCode::NOT_FOUND
        );
    }
    // 다른 조직 경로로 이 배정을 지울 수 없다.
    let other_exam = exam_of(&e.app, &other, DAVE).await;
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &erase_uri(&other, &other_exam, &erin.id),
            DAVE,
            Some(json!({}))
        )
        .await,
        StatusCode::NOT_FOUND
    );
    // 기계 주체는 조직 경로가 404(#28 규칙), 토큰 없으면 401.
    let machine = machine_token(SMOKE_CLIENT);
    assert_eq!(
        call(&e.app, "POST", &uri, Some(&machine), Some(json!({})))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&e.app, "POST", &uri, None, Some(json!({}))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&e.app, "GET", &log_uri, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // 아무도 못 지웠다.
    assert_eq!(count(&e.pool, "submissions", &erin.id).await, 1);
    // 사유가 너무 길면 422, 없는 배정은 404.
    let long = "가".repeat(501);
    assert_eq!(
        status_of(&e.app, "POST", &uri, ALICE, Some(json!({"reason": long}))).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &erase_uri(&org, &exam, &Uuid::new_v4().to_string()),
            ALICE,
            Some(json!({}))
        )
        .await,
        StatusCode::NOT_FOUND
    );

    // 관리자가 지우면 감사 로그에 남고, 이메일로 찾을 수 있다(해시로만).
    let record: ErasureRecord =
        ok(&e.app, "POST", &uri, ALICE, Some(json!({})), StatusCode::OK).await;
    assert_eq!(record.reason, None);
    let log: ErasureLog = ok(&e.app, "GET", &log_uri, ALICE, None, StatusCode::OK).await;
    assert_eq!(log.records, vec![record.clone()]);
    assert_eq!(log.retention_days, AUDIT_RETENTION_DAYS);
    let found: ErasureLog = ok(
        &e.app,
        "GET",
        &format!("{log_uri}?email=%20ERIN%40example.com"),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(found.records.len(), 1);
    let none: ErasureLog = ok(
        &e.app,
        "GET",
        &format!("{log_uri}?email=frank%40example.com"),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(none.records.is_empty());
    let raw: Value = ok(&e.app, "GET", &log_uri, ALICE, None, StatusCode::OK).await;
    assert!(!raw.to_string().contains("erin@example.com"));

    // 감사 기록은 3년 보관 뒤 주기 삭제가 지운다.
    e.clock
        .set(record.at + i64::from(AUDIT_RETENTION_DAYS) * DAY - 1);
    purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    let log: ErasureLog = ok(&e.app, "GET", &log_uri, ALICE, None, StatusCode::OK).await;
    assert_eq!(log.records.len(), 1);
    e.clock
        .set(record.at + i64::from(AUDIT_RETENTION_DAYS) * DAY);
    purge_once(&e.pool, e.clock.as_ref()).await.unwrap();
    let log: ErasureLog = ok(&e.app, "GET", &log_uri, ALICE, None, StatusCode::OK).await;
    assert!(log.records.is_empty());
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 이미_지운_배정을_다시_지우면_같은_기록을_돌려준다(pool: PgPool) {
    let e = env(pool);
    let org = org_of(&e.app, ALICE, "에이", &[(BOB, Role::Admin)]).await;
    let exam = exam_of(&e.app, &org, ALICE).await;
    let erin = take(&e, &org, &exam, ALICE, "erin@example.com", ERIN).await;
    let uri = erase_uri(&org, &exam, &erin.id);

    let first: ErasureRecord = ok(
        &e.app,
        "POST",
        &uri,
        ALICE,
        Some(json!({"reason": "요청"})),
        StatusCode::OK,
    )
    .await;
    e.clock.advance(3600);
    // 다른 관리자가 다른 사유로 다시 눌러도 처음 기록 그대로, 새 기록 없음.
    let again: ErasureRecord = ok(
        &e.app,
        "POST",
        &uri,
        BOB,
        Some(json!({"reason": "다시"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(again, first);
    let log: ErasureLog = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/erasures", org.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(log.records, vec![first]);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 진행_중_응시와_채점_큐_채점_중_리포트_큐와_겹쳐도_오류_없이_지운다(
    pool: PgPool,
) {
    let e = env(pool);
    let org = org_of(&e.app, ALICE, "에이", &[]).await;
    let exam = ok::<ExamView>(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        ALICE,
        Some(json!({
            "title": "두 문항",
            "purpose": "internal",
            "reveal_scores": false,
            "items": [
                {"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 600}},
                {"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 600}},
            ],
        })),
        StatusCode::CREATED,
    )
    .await;

    // ① 진행 중(시작 뒤 미제출): 지우면 응시자 앱의 다음 호출은 404.
    let erin = start(&e, &org, &exam, ALICE, "erin@example.com", ERIN).await;
    let _: ErasureRecord = ok(
        &e.app,
        "POST",
        &erase_uri(&org, &exam, &erin.id),
        ALICE,
        Some(json!({})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("/assignments/{}/items/0/submit", erin.id),
            ERIN,
            Some(
                logged_submit_body(
                    &e.app,
                    &format!("/assignments/{}/items/0/submit", erin.id),
                    &token_for(ERIN),
                    "correct",
                )
                .await
            ),
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("/assignments/{}/items/1/start", erin.id),
            ERIN,
            Some(start_body()),
        )
        .await,
        StatusCode::NOT_FOUND
    );

    // ② 채점 큐에 있는 제출: 지우면 큐에서 빠지고 워커는 할 일이 없다.
    let frank = start(&e, &org, &exam, ALICE, "frank@example.com", FRANK).await;
    submit(&e, &frank, FRANK).await;
    let _: ErasureRecord = ok(
        &e.app,
        "POST",
        &erase_uri(&org, &exam, &frank.id),
        ALICE,
        Some(json!({})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(grade(&e.pool).await, Outcome::Idle);

    // ③ 채점 중(워커가 꺼낸 뒤): 지운 다음 워커가 결과·비용을 써도 오류 없다 — 결과는 0행, 비용은 연결 없이 숫자만.
    let gina = start(&e, &org, &exam, ALICE, "gina@example.com", GINA).await;
    let s = submit(&e, &gina, GINA).await;
    let claimed = promptlab_store::submissions::claim_next(&e.pool)
        .await
        .unwrap()
        .expect("꺼낸 제출");
    assert_eq!(claimed.id.to_string(), s.id);
    let _: ErasureRecord = ok(
        &e.app,
        "POST",
        &erase_uri(&org, &exam, &gina.id),
        ALICE,
        Some(json!({})),
        StatusCode::OK,
    )
    .await;
    let report = GradeReport::from_cases(&claimed.problem_id, claimed.language, &[true]);
    promptlab_store::submissions::complete(&e.pool, claimed.id, &report)
        .await
        .unwrap();
    promptlab_store::submissions::fail(&e.pool, claimed.id, "r", "d")
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_records")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    for scope in [
        UsageScope::Submission(claimed.id),
        UsageScope::Invite(gina.id.parse().unwrap()),
    ] {
        promptlab_store::usage::record(
            &e.pool,
            &NewUsage {
                kind: UsageKind::Report,
                scope,
                model: "m",
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..TokenUsage::default()
                },
                duration_ms: 1,
                prices: LOCAL_PRICES,
            },
        )
        .await
        .unwrap();
    }
    let unlinked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_records WHERE invite_id IS NULL AND submission_id IS NULL",
    )
    .fetch_one(&e.pool)
    .await
    .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_records")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(after, before + 2);
    assert!(unlinked >= 2);
    assert_eq!(count(&e.pool, "submissions", &gina.id).await, 0);

    // ④ 리포트 큐: 리포트를 만드는 도중 지워져도 저장·실패 기록은 0행으로 지나간다.
    let hana = take(&e, &org, &exam, ALICE, "hana@example.com", DAVE).await;
    let hana_id: Uuid = hana.id.parse().unwrap();
    let org_id: Uuid = org.id.parse().unwrap();
    sqlx::query(
        "INSERT INTO exam_reports (invite_id, org_id, status, requested_at) VALUES ($1, $2, 'generating', now())",
    )
    .bind(hana_id)
    .bind(org_id)
    .execute(&e.pool)
    .await
    .unwrap();
    let _: ErasureRecord = ok(
        &e.app,
        "POST",
        &erase_uri(&org, &exam, &hana.id),
        ALICE,
        Some(json!({})),
        StatusCode::OK,
    )
    .await;
    promptlab_store::reports::fail(&e.pool, hana_id, "r", e.clock.now())
        .await
        .unwrap();
    assert_eq!(count(&e.pool, "exam_reports", &hana.id).await, 0);
    // 대조 큐도 비었다 — 워커는 할 일이 없다.
    assert_eq!(grade(&e.pool).await, Outcome::Idle);

    // 넷 다 감사 기록에 남았다.
    let log: ErasureLog = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/erasures", org.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(log.records.len(), 4);
}
