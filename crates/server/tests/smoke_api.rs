//! 시험 이음매 ②: 스모크 기계 토큰(#28). 허용된 기계 클라이언트(`acct=machine`)는 「스모크」 조직의 응시자로만
//! 행동한다 — 배정 목록(이때 스모크 조직·시험·새 배정이 멱등으로 준비된다)·문항 시작·제출·자기 제출 조회.
//! 다른 조직은 404, 관리 API는 403, 스모크 조직은 사람의 대시보드·결과 조회에서 빠진다.
//! 모든 응답에 숨은 입력 불변식이 걸린다(`common::send`).

mod common;

use axum::http::StatusCode;
use common::*;
use promptlab_shared::api::{
    AssignmentView, ItemStarted, ItemState, LogHead, MeView, OrgView, SubmissionState,
    SubmissionView,
};
use promptlab_shared::log::{Attempt, LogEvent, LogSigner};
use promptlab_store::PgPool;
use promptlab_store::smoke::PROBLEM_ID;
use promptlab_worker::Outcome;
use serde_json::{Value, json};
use uuid::Uuid;

fn smoke() -> String {
    machine_token(SMOKE_CLIENT)
}

async fn as_machine(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(app, method, uri, Some(&smoke()), body).await
}

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

async fn assignments(app: &axum::Router) -> Vec<AssignmentView> {
    ok(
        app,
        "GET",
        "/me/assignments",
        &smoke(),
        None,
        StatusCode::OK,
    )
    .await
}

const CODE: &str = "def max_subarray(nums):  # correct\n    pass\n";

/// 로그 없이 내는 제출 본문(거부되는 요청에만 쓴다).
fn submit_body() -> Value {
    json!({"language": "python", "code": CODE, "log_seq": 0})
}

/// 스모크 한 번: 배정 → 시작 → 제출 → 채점. 제출 id.
async fn smoke_run(app: &axum::Router, pool: &PgPool) -> (AssignmentView, String) {
    let list = assignments(app).await;
    assert_eq!(list.len(), 1, "새 배정 하나만 보인다");
    let a = list.into_iter().next().unwrap();
    let started: ItemStarted = ok(
        app,
        "POST",
        &format!("/assignments/{}/items/0/start", a.id),
        &smoke(),
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(started.problem.id, PROBLEM_ID);
    // 과정 로그(#9): 스모크 앱도 설치 키를 등록하고 서명한 사건을 올린다.
    let signer = LogSigner::from_seed([5; 32]);
    let head: LogHead = ok(
        app,
        "POST",
        &format!("/assignments/{}/items/0/log/keys", a.id),
        &smoke(),
        Some(json!({"public_key": signer.public_key()})),
        StatusCode::OK,
    )
    .await;
    let attempt = Attempt {
        assignment_id: a.id.clone(),
        position: 0,
    };
    let first = signer.seal(&attempt, head.next_seq, &head.head, 0, LogEvent::Started);
    // 로컬 실행 사건도 올린다.
    let second = signer.seal(
        &attempt,
        head.next_seq + 1,
        &promptlab_shared::log::event_hash(&first.body),
        1,
        LogEvent::LocalRun {
            kind: promptlab_shared::log::LocalRunKind::Examples,
            passed: Some(2),
            total: Some(2),
            summary: "예제 2/2".into(),
        },
    );
    let head: LogHead = ok(
        app,
        "POST",
        &format!("/assignments/{}/items/0/log", a.id),
        &smoke(),
        Some(json!({"events": [first, second]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(head.next_seq, 2);
    let submitted: SubmissionView = ok(
        app,
        "POST",
        &format!("/assignments/{}/items/0/submit", a.id),
        &smoke(),
        Some(
            logged_submit_body(
                app,
                &format!("/assignments/{}/items/0/submit", a.id),
                &smoke(),
                CODE,
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await;
    assert!(matches!(grade(pool).await, Outcome::Graded(_)));
    assert_eq!(grade(pool).await, Outcome::Idle, "큐가 빈다");
    (a, submitted.id)
}

async fn smoke_org_id(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM orgs WHERE is_smoke")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 기계_토큰이_스모크_배정을_받아_응시_제출하고_채점_결과를_본다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    let first = assignments(&app).await;
    assert_eq!(first.len(), 1);
    let a = &first[0];
    assert_eq!(a.org_name, "스모크");
    assert!(
        a.scores_visible,
        "스모크 시험은 점수를 보인다(채점까지 확인)"
    );
    assert_eq!(a.items.len(), 1);
    assert_eq!(a.items[0].state, ItemState::NotStarted);
    // 다시 불러도 같은 배정(시작 전이면 새로 만들지 않는다).
    assert_eq!(assignments(&app).await[0].id, a.id);

    let (run, submission) = smoke_run(&app, &pool).await;
    assert_eq!(run.id, a.id);
    let view: SubmissionView = ok(
        &app,
        "GET",
        &format!("/submissions/{submission}"),
        &smoke(),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(
        matches!(view.state, SubmissionState::Done { .. }),
        "{view:?}"
    );
    // 같은 문항은 두 번 못 낸다 — 다음 스모크는 새 배정을 받는다.
    let (status, _) = as_machine(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/submit", a.id),
        Some(submit_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (next, _) = smoke_run(&app, &pool).await;
    assert_ne!(next.id, a.id);
    // 조직·시험은 하나뿐.
    let (orgs, exams): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM orgs WHERE is_smoke), (SELECT count(*) FROM exams)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((orgs, exams), (1, 1));
}

/// #39: 같은 서버에 스모크를 거듭 돌려도 매번 시작 전 문항이 있는 배정 하나가 나온다
/// (app-cli가 부르는 순서 그대로: 조회 → start → 로그 키·업로드 → submit → 워커 채점 → 다시 조회).
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크를_세_번_연달아_돌려도_매번_새_배정이_나온다(pool: PgPool) {
    let app = app(pool.clone());
    let mut seen = Vec::new();
    for round in 0..3 {
        let (a, _) = smoke_run(&app, &pool).await;
        assert!(!seen.contains(&a.id), "{round}번째가 지난 배정을 다시 줬다");
        seen.push(a.id);
        // 끝난 직후 다시 조회해도(다음 실행 전) 시작 전 문항이 있는 배정이 있어야 한다.
        let after = assignments(&app).await;
        assert_eq!(after.len(), 1, "{round}번째 뒤: {after:?}");
        assert_eq!(after[0].items[0].state, ItemState::NotStarted);
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크_문항이_바뀌면_새_시험을_만들고_옛_시험의_쓰지_않은_배정을_지운다(
    pool: PgPool,
) {
    let app = app(pool.clone());
    // 한 번 끝까지 돈 뒤(결과가 남는 배정) 옛 문항을 쓰던 상태를 흉내 낸다 — 쓰지 않은 배정도 하나 있다.
    let (done, _) = smoke_run(&app, &pool).await;
    assert_eq!(assignments(&app).await.len(), 1);
    sqlx::query("UPDATE exam_items SET problem_id = 'count-vowels'")
        .execute(&pool)
        .await
        .unwrap();

    let after = assignments(&app).await;
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after[0].exam_id, done.exam_id, "새 시험의 배정");
    assert_eq!(after[0].items[0].state, ItemState::NotStarted);
    let (exams, invites_old): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM exams),
                (SELECT count(*) FROM exam_invites WHERE exam_id = $1::uuid)",
    )
    .bind(&done.exam_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(exams, 2);
    assert_eq!(invites_old, 1, "끝난 배정만 남는다");
    let problem: String =
        sqlx::query_scalar("SELECT problem_id FROM exam_items WHERE exam_id = $1::uuid")
            .bind(&after[0].exam_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(problem, PROBLEM_ID);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 겹친_요청도_스모크_조직_시험_배정을_한_번만_만든다(pool: PgPool) {
    let app = app(pool.clone());
    let (a, b, c) = tokio::join!(assignments(&app), assignments(&app), assignments(&app));
    assert_eq!(a, b);
    assert_eq!(b, c);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM orgs), (SELECT count(*) FROM exams), (SELECT count(*) FROM exam_invites)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1, 1));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 허용_목록_밖_기계_토큰은_401(pool: PgPool) {
    let app = app(pool);
    for uri in ["/me/assignments", "/api/me/assignments", "/me"] {
        let (status, _) = call(&app, "GET", uri, Some(&machine_token(OTHER_MACHINE)), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 기계_토큰은_다른_조직에_404_관리_api에_403(pool: PgPool) {
    let app = app(pool.clone());
    let alice = token_for(ALICE);
    let org: OrgView = ok(
        &app,
        "POST",
        "/orgs",
        &alice,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(&app, &org.id).await;
    let exam: Value = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        &alice,
        Some(json!({
            "title": "1차", "purpose": "internal", "reveal_scores": true,
            "items": [{"problem_id": "count-vowels", "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await;
    let exam_id = exam["id"].as_str().unwrap();
    // 사람 응시자의 배정과 제출.
    let invites: Value = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams/{exam_id}/invites", org.id),
        &alice,
        Some(json!({"emails": ["erin@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = invites["invites"][0]["token"].as_str().unwrap();
    let erin = token_for(ERIN);
    let human: AssignmentView = ok(
        &app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        &erin,
        None,
        StatusCode::OK,
    )
    .await;
    webcam_ready(&app, &erin, &human.id).await;
    let _: ItemStarted = ok(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/start", human.id),
        &erin,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    let human_sub: SubmissionView = ok(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/submit", human.id),
        &erin,
        Some(
            logged_submit_body(
                &app,
                &format!("/assignments/{}/items/0/submit", human.id),
                &erin,
                CODE,
            )
            .await,
        ),
        StatusCode::ACCEPTED,
    )
    .await;

    for prefix in ["", "/api"] {
        let o = &org.id;
        for (method, uri, body) in [
            ("GET", format!("{prefix}/orgs/{o}"), None),
            ("GET", format!("{prefix}/orgs/{o}/members"), None),
            ("GET", format!("{prefix}/orgs/{o}/submissions"), None),
            ("GET", format!("{prefix}/orgs/{o}/exams"), None),
            ("GET", format!("{prefix}/orgs/{o}/exams/{exam_id}"), None),
            ("POST", format!("{prefix}/orgs/{o}/exams"), Some(json!({}))),
            (
                "POST",
                format!("{prefix}/orgs/{o}/invites"),
                Some(json!({"role": "viewer"})),
            ),
            ("GET", format!("{prefix}/orgs/{o}/problem-bank"), None),
        ] {
            let (status, _) = as_machine(&app, method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        }
        for (method, uri, body) in [
            ("GET", format!("{prefix}/me"), None),
            (
                "POST",
                format!("{prefix}/orgs"),
                Some(json!({"name": "기계 회사"})),
            ),
            ("POST", format!("{prefix}/invites/x/accept"), None),
            (
                "POST",
                format!("{prefix}/exam-invites/{token}/accept"),
                None,
            ),
            (
                "POST",
                format!("{prefix}/submissions"),
                Some(
                    json!({"problem_id": PROBLEM_ID, "language": "python", "code": CODE, "elapsed_secs": 1}),
                ),
            ),
            ("GET", format!("{prefix}/problems/{PROBLEM_ID}"), None),
            ("POST", format!("{prefix}/auth/logout"), None),
        ] {
            let (status, _) = as_machine(&app, method, &uri, body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
        // 허용 경로라도 남의 배정·제출은 404.
        for (method, uri, body) in [
            (
                "POST",
                format!("{prefix}/assignments/{}/items/0/start", human.id),
                Some(start_body()),
            ),
            (
                "POST",
                format!("{prefix}/assignments/{}/items/0/submit", human.id),
                Some(submit_body()),
            ),
            (
                "GET",
                format!("{prefix}/submissions/{}", human_sub.id),
                None,
            ),
        ] {
            let (status, _) = as_machine(&app, method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        }
    }
    // 기계 주체의 배정 목록에 사람 시험은 없다.
    let list = assignments(&app).await;
    assert!(list.iter().all(|a| a.org_name == "스모크"));
    // 초대 링크를 기계가 받지 못했다.
    let candidate: Option<String> =
        sqlx::query_scalar("SELECT candidate_sub FROM exam_invites WHERE exam_id = $1::uuid")
            .bind(exam_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(candidate.as_deref(), Some(ERIN));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크_조직은_대시보드_목록과_결과_조회에서_빠진다(pool: PgPool) {
    let app = app(pool.clone());
    let (_, submission) = smoke_run(&app, &pool).await;
    let smoke_org = smoke_org_id(&pool).await;
    // 누가 DB로 스모크 조직에 관리자를 넣어도(대장 손 실수 등) 대시보드에는 나오지 않는다.
    for sub in [ALICE, OWNER_SUB] {
        sqlx::query("INSERT INTO users (sub) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(sub)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships (org_id, sub, role) VALUES ($1, $2, 'admin')")
            .bind(smoke_org)
            .bind(sub)
            .execute(&pool)
            .await
            .unwrap();
    }
    let alice = token_for(ALICE);
    let _: OrgView = ok(
        &app,
        "POST",
        "/orgs",
        &alice,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    let me: MeView = ok(&app, "GET", "/me", &alice, None, StatusCode::OK).await;
    assert_eq!(me.orgs.len(), 1);
    assert_eq!(me.orgs[0].name, "에이 회사");
    for uri in [
        format!("/orgs/{smoke_org}"),
        format!("/orgs/{smoke_org}/submissions"),
        format!("/orgs/{smoke_org}/exams"),
        format!("/orgs/{smoke_org}/members"),
        format!("/orgs/{smoke_org}/rounds"),
        format!("/orgs/{smoke_org}/exams/{}/results", uuid::Uuid::nil()),
        format!("/orgs/{smoke_org}/exams/{}/results.csv", uuid::Uuid::nil()),
        format!("/submissions/{submission}"),
    ] {
        let (status, _) = call(&app, "GET", &uri, Some(&alice), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}
