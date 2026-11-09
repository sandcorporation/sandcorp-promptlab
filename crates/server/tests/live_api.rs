//! 실시간 감독(#14): 채용 시험의 응시 가능 시간(닫힌 시간의 start 거부), WebRTC 시그널링(감독 권한·조직 격리,
//! 열기 → 송출 요청 → offer/answer/ICE → 닫기 → 중단, 감독관 연결 끊김), TURN 시간 제한 자격(만료·HMAC·비밀 비노출),
//! TURN 설정이 없으면 실시간 보기만 꺼짐, 스모크 무영향. 응답은 모두 `call()`(숨은 입력 불변식)로 받는다.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::*;
use promptlab_server::turn::{TurnConfig, credential};
use promptlab_shared::account::Role;
use promptlab_shared::api::{AssignmentView, ExamView, InviteCreated, OrgView};
use promptlab_shared::exam::Purpose;
use promptlab_shared::live::{
    ExamWindowView, LiveRequest, LiveSession, MAX_SESSION_SECS, SignalBatch, SignalKind,
    VIEWER_IDLE_SECS,
};
use promptlab_store::PgPool;
use serde_json::{Value, json};

/// 다른 조직의 관리자.
const FRANK: &str = "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2F";
const SECRET: &str = "test-turn-secret-0123456789";
const TURN_URL: &str = "turn:turn.example.test:3478?transport=udp";

fn turn() -> TurnConfig {
    TurnConfig::new(SECRET.into(), vec![TURN_URL.into()], MAX_SESSION_SECS).unwrap()
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
    assert!(
        !body.to_string().contains(SECRET),
        "TURN 공유 비밀이 응답에 샜다: {method} {uri}"
    );
    serde_json::from_value(body).unwrap()
}

async fn status(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
) -> StatusCode {
    let (s, body) = call(app, method, uri, Some(&token_for(sub)), body).await;
    assert!(!body.to_string().contains(SECRET));
    s
}

struct Env {
    app: axum::Router,
    clock: Arc<FixedClock>,
    pool: PgPool,
    org: OrgView,
    exam: ExamView,
    assignment: AssignmentView,
}

impl Env {
    fn window_uri(&self) -> String {
        format!("/orgs/{}/exams/{}/window", self.org.id, self.exam.id)
    }
    fn live_uri(&self, tail: &str) -> String {
        format!(
            "/orgs/{}/exams/{}/invites/{}/live{tail}",
            self.org.id, self.exam.id, self.assignment.id
        )
    }
    fn mine_uri(&self, tail: &str) -> String {
        format!("/assignments/{}/live{tail}", self.assignment.id)
    }
    fn start_uri(&self, n: u32) -> String {
        format!("/assignments/{}/items/{n}/start", self.assignment.id)
    }
    async fn start(&self, n: u32) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            &self.start_uri(n),
            Some(&token_for(ERIN)),
            Some(start_body()),
        )
        .await
    }
    async fn set_window(&self, by: &str, body: Value) -> (StatusCode, Value) {
        call(
            &self.app,
            "PUT",
            &self.window_uri(),
            Some(&token_for(by)),
            Some(body),
        )
        .await
    }
    async fn signal(&self, uri: &str, by: &str, kind: SignalKind, data: &str) -> StatusCode {
        status(
            &self.app,
            "POST",
            uri,
            by,
            Some(json!({"kind": kind, "data": data})),
        )
        .await
    }
}

/// 관리자 ALICE·출제자 BOB·감독관 CAROL·열람자 DAVE인 조직, `purpose` 시험(문항 둘), ERIN 배정(웹캠 준비 끝).
async fn setup_with(pool: PgPool, purpose: Purpose, turn: Option<TurnConfig>) -> Env {
    let clock = FixedClock::at(NOW);
    let app = app_live(pool.clone(), clock.clone(), turn);
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
            "title": "실시간 감독 시험",
            "purpose": purpose,
            "reveal_scores": false,
            "items": [
                {"problem_id": "count-vowels", "limits": {"time_limit_secs": 600}},
                {"problem_id": "count-vowels", "limits": {"time_limit_secs": 600}},
            ],
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
    Env {
        app,
        clock,
        pool,
        org,
        exam,
        assignment,
    }
}

async fn setup(pool: PgPool) -> Env {
    setup_with(pool, Purpose::Recruiting, Some(turn())).await
}

// ───────────────────────── 응시 가능 시간 ─────────────────────────

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 닫힌_시간에는_문항을_시작하지_않고_열면_시작한다(pool: PgPool) {
    let e = setup(pool).await;
    let view: ExamWindowView =
        ok(&e.app, "GET", &e.window_uri(), CAROL, None, StatusCode::OK).await;
    assert!(
        view.applies && view.open_now && view.window.open,
        "기본은 열림"
    );

    let (s, body) = e.set_window(CAROL, json!({"open": false})).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, body) = e.start(0).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(body["error"].as_str().unwrap().contains("응시 가능 시간"));
    let started: i64 = sqlx::query_scalar("SELECT count(*) FROM exam_item_starts")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(started, 0, "거부한 시작은 시각을 남기지 않는다");

    let (s, _) = e.set_window(ALICE, json!({"open": true})).await;
    assert_eq!(s, StatusCode::OK);
    let (s, body) = e.start(0).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 시각_범위_밖이면_거부하고_범위_안이면_시작한다(pool: PgPool) {
    let e = setup(pool).await;
    let (s, body) = e
        .set_window(
            CAROL,
            json!({"open": true, "starts_at": NOW + 600, "ends_at": NOW + 1200}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let view: ExamWindowView = serde_json::from_value(body).unwrap();
    assert!(!view.open_now);
    assert_eq!(e.start(0).await.0, StatusCode::FORBIDDEN, "시작 전");
    e.clock.advance(600);
    assert_eq!(e.start(0).await.0, StatusCode::OK, "범위 안");
    e.clock.advance(600);
    assert_eq!(e.start(1).await.0, StatusCode::FORBIDDEN, "끝난 뒤");
    // 끝이 시작보다 이르면 422.
    let (s, _) = e
        .set_window(
            CAROL,
            json!({"open": true, "starts_at": NOW, "ends_at": NOW}),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 이미_시작한_문항은_닫혀도_그대로_진행한다(pool: PgPool) {
    let e = setup(pool).await;
    assert_eq!(e.start(0).await.0, StatusCode::OK);
    let (s, _) = e.set_window(CAROL, json!({"open": false})).await;
    assert_eq!(s, StatusCode::OK);
    // 앱을 다시 켜 같은 문항을 다시 시작해도(시작 시각은 처음 그대로) 막지 않고, 제출도 받는다.
    assert_eq!(e.start(0).await.0, StatusCode::OK);
    let (s, body) = call(
        &e.app,
        "POST",
        &format!("/assignments/{}/items/0/submit", e.assignment.id),
        Some(&token_for(ERIN)),
        Some(
            logged_submit_body(
                &e.app,
                &format!("/assignments/{}/items/0/submit", e.assignment.id),
                &token_for(ERIN),
                "correct",
            )
            .await,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{body}");
    // 다음 문항은 닫힌 동안 시작하지 못한다.
    assert_eq!(e.start(1).await.0, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 응시_시간은_감독_권한만_바꾸고_사내_진단에는_걸지_않는다(
    pool: PgPool,
) {
    let e = setup(pool.clone()).await;
    let closed = json!({"open": false});
    assert_eq!(
        e.set_window(BOB, closed.clone()).await.0,
        StatusCode::FORBIDDEN,
        "출제자"
    );
    assert_eq!(
        e.set_window(DAVE, closed.clone()).await.0,
        StatusCode::FORBIDDEN,
        "열람자"
    );
    assert_eq!(
        e.set_window(ERIN, closed.clone()).await.0,
        StatusCode::NOT_FOUND,
        "구성원 아님"
    );
    assert_eq!(
        status(&e.app, "GET", &e.window_uri(), DAVE, None).await,
        StatusCode::FORBIDDEN
    );

    let internal = setup_with(pool, Purpose::Internal, None).await;
    let view: ExamWindowView = ok(
        &internal.app,
        "GET",
        &internal.window_uri(),
        CAROL,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!view.applies && view.open_now);
    assert_eq!(
        internal.set_window(CAROL, closed).await.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "사내 진단은 열고 닫지 않는다"
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크_시험은_응시_시간과_실시간_감독의_영향을_받지_않는다(
    pool: PgPool,
) {
    let app = app_live(pool.clone(), FixedClock::at(NOW), Some(turn()));
    let smoke = machine_token(SMOKE_CLIENT);
    let (s, list) = call(&app, "GET", "/me/assignments", Some(&smoke), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    let a: Vec<AssignmentView> = serde_json::from_value(list).unwrap();
    // 누가 DB에서 닫아 두어도 스모크 조직 시험은 시작한다.
    sqlx::query(
        "UPDATE exams SET window_open = false
          WHERE org_id IN (SELECT id FROM orgs WHERE is_smoke) AND purpose = 'recruiting'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (s, body) = call(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/start", a[0].id),
        Some(&smoke),
        Some(start_body()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    // 기계 주체는 실시간 감독 경로를 쓰지 못한다(사람이 아니고 카메라가 없다).
    let (s, _) = call(
        &app,
        "GET",
        &format!("/assignments/{}/live", a[0].id),
        Some(&smoke),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// ───────────────────────── 실시간 보기 ─────────────────────────

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 감독관이_열면_앱이_송출하고_닫으면_멈춘다(pool: PgPool) {
    let e = setup(pool).await;
    // 열기 전에는 송출 요청이 없다.
    let req: LiveRequest = ok(&e.app, "GET", &e.mine_uri(""), ERIN, None, StatusCode::OK).await;
    assert!(req.session.is_none());

    let viewer: LiveSession = ok(
        &e.app,
        "POST",
        &e.live_uri(""),
        CAROL,
        None,
        StatusCode::CREATED,
    )
    .await;
    let req: LiveRequest = ok(&e.app, "GET", &e.mine_uri(""), ERIN, None, StatusCode::OK).await;
    let cand = req.session.expect("감독관이 열면 앱이 송출 요청을 받는다");
    assert_eq!(cand.session_id, viewer.session_id);
    assert_ne!(
        cand.ice_servers[0].username, viewer.ice_servers[0].username,
        "양쪽 자격은 따로"
    );

    // 가짜 피어: 앱 offer → 감독관이 받고 answer → 앱이 받음, ICE 양쪽.
    let sid = &viewer.session_id;
    let mine = e.mine_uri(&format!("/{sid}/signals"));
    let theirs = e.live_uri(&format!("/{sid}/signals"));
    assert_eq!(
        e.signal(&mine, ERIN, SignalKind::Offer, "v=0 offer").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        e.signal(&mine, ERIN, SignalKind::Ice, "{\"c\":1}").await,
        StatusCode::NO_CONTENT
    );
    let got: SignalBatch = ok(&e.app, "GET", &theirs, CAROL, None, StatusCode::OK).await;
    assert!(got.open);
    assert_eq!(
        got.signals
            .iter()
            .map(|s| (s.seq, s.kind))
            .collect::<Vec<_>>(),
        vec![(1, SignalKind::Offer), (2, SignalKind::Ice)]
    );
    assert_eq!(got.signals[0].data, "v=0 offer");
    let got: SignalBatch = ok(
        &e.app,
        "GET",
        &format!("{theirs}?after=1"),
        CAROL,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(got.signals.len(), 1, "after 뒤만");
    assert_eq!(
        e.signal(&theirs, CAROL, SignalKind::Answer, "v=0 answer")
            .await,
        StatusCode::NO_CONTENT
    );
    let got: SignalBatch = ok(&e.app, "GET", &mine, ERIN, None, StatusCode::OK).await;
    assert_eq!(got.signals[0].kind, SignalKind::Answer);
    // 방향이 틀린 종류는 422.
    assert_eq!(
        e.signal(&mine, ERIN, SignalKind::Answer, "x").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        e.signal(&theirs, CAROL, SignalKind::Offer, "x").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // 너무 큰 메시지는 413.
    assert_eq!(
        e.signal(&mine, ERIN, SignalKind::Ice, &"x".repeat(20_000))
            .await,
        StatusCode::PAYLOAD_TOO_LARGE
    );

    // 닫기 → 앱의 다음 폴링에서 송출 요청이 사라지고 시그널 폴링은 닫힘.
    assert_eq!(
        status(
            &e.app,
            "DELETE",
            &e.live_uri(&format!("/{sid}")),
            CAROL,
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let req: LiveRequest = ok(&e.app, "GET", &e.mine_uri(""), ERIN, None, StatusCode::OK).await;
    assert!(req.session.is_none(), "닫으면 송출 요청이 없다");
    let got: SignalBatch = ok(&e.app, "GET", &mine, ERIN, None, StatusCode::OK).await;
    assert!(!got.open, "앱은 닫힘을 보고 송출을 멈춘다");
    assert_eq!(
        e.signal(&mine, ERIN, SignalKind::Ice, "x").await,
        StatusCode::GONE
    );
    // 두 번 닫아도 204.
    assert_eq!(
        status(
            &e.app,
            "DELETE",
            &e.live_uri(&format!("/{sid}")),
            CAROL,
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 감독관_연결이_끊기면_세션이_닫힌다(pool: PgPool) {
    let e = setup(pool).await;
    let viewer: LiveSession = ok(
        &e.app,
        "POST",
        &e.live_uri(""),
        ALICE,
        None,
        StatusCode::CREATED,
    )
    .await;
    let theirs = e.live_uri(&format!("/{}/signals", viewer.session_id));
    e.clock.advance(VIEWER_IDLE_SECS - 1);
    let got: SignalBatch = ok(&e.app, "GET", &theirs, ALICE, None, StatusCode::OK).await;
    assert!(got.open, "폴링하는 동안은 열려 있다");
    e.clock.advance(VIEWER_IDLE_SECS + 1);
    let req: LiveRequest = ok(&e.app, "GET", &e.mine_uri(""), ERIN, None, StatusCode::OK).await;
    assert!(req.session.is_none(), "감독관 폴링이 끊기면 송출을 멈춘다");
    // 앱이 bye를 보내도(카메라를 못 쓰게 됐다) 닫힌다.
    let viewer: LiveSession = ok(
        &e.app,
        "POST",
        &e.live_uri(""),
        ALICE,
        None,
        StatusCode::CREATED,
    )
    .await;
    let mine = e.mine_uri(&format!("/{}/signals", viewer.session_id));
    assert_eq!(
        e.signal(&mine, ERIN, SignalKind::Bye, "").await,
        StatusCode::NO_CONTENT
    );
    let got: SignalBatch = ok(
        &e.app,
        "GET",
        &e.live_uri(&format!("/{}/signals", viewer.session_id)),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(!got.open);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 실시간_보기는_감독_권한만_열고_다른_조직과_다른_응시자는_막는다(
    pool: PgPool,
) {
    let e = setup(pool.clone()).await;
    assert_eq!(
        status(&e.app, "POST", &e.live_uri(""), BOB, None).await,
        StatusCode::FORBIDDEN,
        "출제자"
    );
    assert_eq!(
        status(&e.app, "POST", &e.live_uri(""), DAVE, None).await,
        StatusCode::FORBIDDEN,
        "열람자"
    );
    assert_eq!(
        status(&e.app, "POST", &e.live_uri(""), ERIN, None).await,
        StatusCode::NOT_FOUND,
        "응시자"
    );

    let viewer: LiveSession = ok(
        &e.app,
        "POST",
        &e.live_uri(""),
        CAROL,
        None,
        StatusCode::CREATED,
    )
    .await;
    let sid = &viewer.session_id;
    // 열람자는 시그널도 못 본다.
    assert_eq!(
        status(
            &e.app,
            "GET",
            &e.live_uri(&format!("/{sid}/signals")),
            DAVE,
            None
        )
        .await,
        StatusCode::FORBIDDEN
    );

    // 다른 조직(FRANK가 관리자인 조직)은 이 조직 경로에 404.
    let other: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        FRANK,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    let foreign = format!(
        "/orgs/{}/exams/{}/invites/{}/live",
        other.id, e.exam.id, e.assignment.id
    );
    assert_eq!(
        status(&e.app, "POST", &foreign, FRANK, None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&e.app, "POST", &e.live_uri(""), FRANK, None).await,
        StatusCode::NOT_FOUND
    );
    // 남의 배정으로는 송출 요청을 못 받는다.
    assert_eq!(
        status(&e.app, "GET", &e.mine_uri(""), CAROL, None).await,
        StatusCode::NOT_FOUND
    );
    // 다른 조직 관리자는 세션 id를 알아도 시그널 경로에 404.
    assert_eq!(
        status(
            &e.app,
            "GET",
            &e.live_uri(&format!("/{sid}/signals")),
            FRANK,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn turn_자격은_만료가_붙은_hmac이고_비밀은_내보내지_않는다(pool: PgPool) {
    let e = setup(pool).await;
    let viewer: LiveSession = ok(
        &e.app,
        "POST",
        &e.live_uri(""),
        CAROL,
        None,
        StatusCode::CREATED,
    )
    .await;
    let ice = &viewer.ice_servers[0];
    assert_eq!(ice.urls, vec![TURN_URL.to_owned()]);
    let (expiry, label) = ice.username.split_once(':').unwrap();
    assert_eq!(
        expiry.parse::<i64>().unwrap(),
        NOW + MAX_SESSION_SECS,
        "만료 = 발급 시각 + 수명"
    );
    assert!(
        label.starts_with("v-"),
        "꼬리표에 응시자 정보가 없다: {label}"
    );
    assert_eq!(ice.credential, credential(SECRET, &ice.username));
    assert_eq!(viewer.expires_at, NOW + MAX_SESSION_SECS);
    // coturn과 같은 계산(base64(HMAC-SHA1)) — 고정 벡터(python hmac로 따로 구함).
    assert_eq!(
        credential(SECRET, "1700003600:c-ab12"),
        "0oBfgBAYba8CkFEJ5EHPGmFwZE8="
    );
    // 설정의 Debug에도 비밀이 없다.
    assert!(!format!("{:?}", turn()).contains(SECRET));
    // 세션 수명이 지나면 닫힌다(자격 수명과 같다).
    e.clock.advance(MAX_SESSION_SECS);
    let req: LiveRequest = ok(&e.app, "GET", &e.mine_uri(""), ERIN, None, StatusCode::OK).await;
    assert!(req.session.is_none());
}

#[test]
fn turn_설정은_비밀이_없으면_꺼지고_값이_틀리면_거부한다() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    let public = "https://promptlab.sandcorp.co.kr";
    assert!(TurnConfig::from_lookup(env(&[]), public).unwrap().is_none());
    let t = TurnConfig::from_lookup(env(&[("PROMPTLAB_TURN_SECRET", SECRET)]), public)
        .unwrap()
        .unwrap();
    let ice = t.ice_server(NOW, "v-x");
    assert_eq!(
        ice.urls,
        vec![
            "turn:promptlab.sandcorp.co.kr:3478?transport=udp".to_owned(),
            "turn:promptlab.sandcorp.co.kr:3478?transport=tcp".to_owned(),
        ]
    );
    assert_eq!(t.ttl_secs(), MAX_SESSION_SECS);
    assert!(TurnConfig::from_lookup(env(&[("PROMPTLAB_TURN_SECRET", "짧음")]), public).is_err());
    assert!(
        TurnConfig::from_lookup(
            env(&[
                ("PROMPTLAB_TURN_SECRET", SECRET),
                ("PROMPTLAB_TURN_URLS", "http://x")
            ]),
            public
        )
        .is_err()
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn turn_설정이_없으면_실시간_보기만_꺼지고_나머지는_돈다(pool: PgPool) {
    let e = setup_with(pool, Purpose::Recruiting, None).await;
    let (s, body) = call(
        &e.app,
        "POST",
        &e.live_uri(""),
        Some(&token_for(CAROL)),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let req: LiveRequest = ok(&e.app, "GET", &e.mine_uri(""), ERIN, None, StatusCode::OK).await;
    assert!(req.session.is_none());
    assert_eq!(e.start(0).await.0, StatusCode::OK, "응시는 그대로");
    assert_eq!(
        e.set_window(CAROL, json!({"open": false})).await.0,
        StatusCode::OK
    );
}
