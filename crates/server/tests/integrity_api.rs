//! 잠금·무결성(#12) HTTP API: 문항 시작의 허용 목록(서버 강제) → 의심 신호 목록(포커스·전체화면·붙여넣기 막음·
//! 웹캠, 무결성 실패) → 권한·격리. 모든 응답에 히든 테스트 불변식을 건다(`call()`).

mod common;

use axum::http::StatusCode;
use common::*;
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamInvitesCreated, ExamView, InviteCreated, ItemStarted, LogHead, OrgView,
    SuspicionView,
};
use promptlab_shared::exam::Purpose;
use promptlab_shared::inference::LARGEST_MODEL as MODEL;
use promptlab_shared::integrity::{ALLOWED_APP_VERSIONS, SignalClock, SuspicionKind};
use promptlab_shared::log::{Attempt, LogEvent, LogSigner, SignedEvent, event_hash};
use promptlab_store::PgPool;
use serde_json::{Value, json};

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
) -> (StatusCode, Value) {
    call(app, method, uri, Some(&token_for(sub)), body).await
}

struct Env {
    app: axum::Router,
    pool: PgPool,
    org: OrgView,
    exam: ExamView,
    assignment: AssignmentView,
}

impl Env {
    fn item_uri(&self, tail: &str) -> String {
        format!("/assignments/{}/items/0/{tail}", self.assignment.id)
    }

    fn signals_uri(&self) -> String {
        format!(
            "/orgs/{}/exams/{}/invites/{}/signals",
            self.org.id, self.exam.id, self.assignment.id
        )
    }

    async fn start(&self, body: Option<Value>) -> (StatusCode, Value) {
        status_of(&self.app, "POST", &self.item_uri("start"), ERIN, body).await
    }

    async fn signals(&self, sub: &str) -> SuspicionView {
        ok(
            &self.app,
            "GET",
            &self.signals_uri(),
            sub,
            None,
            StatusCode::OK,
        )
        .await
    }

    /// 설치 키를 등록하고 사건들을 서명해 올린다(앱 시계는 `at_ms`부터 1초씩).
    async fn upload(&self, events: Vec<(i64, LogEvent)>) {
        let signer = LogSigner::from_seed([7; 32]);
        let head: LogHead = ok(
            &self.app,
            "POST",
            &self.item_uri("log/keys"),
            ERIN,
            Some(json!({"public_key": signer.public_key()})),
            StatusCode::OK,
        )
        .await;
        let attempt = Attempt {
            assignment_id: self.assignment.id.clone(),
            position: 0,
        };
        let mut prev = head.head.clone();
        let signed: Vec<SignedEvent> = events
            .into_iter()
            .enumerate()
            .map(|(i, (at_ms, event))| {
                let s = signer.seal(&attempt, head.next_seq + i as u64, &prev, at_ms, event);
                prev = event_hash(&s.body);
                s
            })
            .collect();
        let _: LogHead = ok(
            &self.app,
            "POST",
            &self.item_uri("log"),
            ERIN,
            Some(json!({ "events": signed })),
            StatusCode::OK,
        )
        .await;
    }
}

/// 내장 시험 문항.
const EXAM_PROBLEM: &str = "count-vowels";
const T0: i64 = 1_767_225_600_000;

/// ALICE(관리자)의 조직에 BOB 출제자·DAVE 열람자. ERIN이 배정받았다(문항은 아직 시작 안 함).
async fn assigned(pool: PgPool) -> Env {
    let app = app(pool.clone());
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
    for (sub, role) in [(BOB, Role::Author), (DAVE, Role::Viewer)] {
        let invite: InviteCreated = ok(
            &app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({ "role": role })),
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
            "title": "코딩 역량 1차",
            "purpose": Purpose::Recruiting,
            "reveal_scores": false,
            "items": [{"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await;
    let created: ExamInvitesCreated = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": ["erin@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created.invites[0].token.clone().unwrap();
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
        pool,
        org,
        exam,
        assignment,
    }
}

async fn started(pool: PgPool) -> Env {
    let e = assigned(pool).await;
    let (status, body) = e.start(Some(start_body())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    e
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 허용_목록에_없는_앱_판이나_모델이면_문항을_시작하지_않고_무결성_실패_신호를_남긴다(
    pool: PgPool,
) {
    let e = assigned(pool).await;
    let other_model = "0".repeat(64);
    let cases = [
        // 낡은 앱(신고 없음).
        (None, false, false),
        (
            Some(json!({"app_version": "0.0.9", "model_sha256": MODEL.sha256})),
            false,
            true,
        ),
        (
            Some(json!({"app_version": ALLOWED_APP_VERSIONS[0], "model_sha256": other_model})),
            true,
            false,
        ),
    ];
    for (body, _, _) in &cases {
        let (status, body) = e.start(body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body["error"].as_str().unwrap().contains("허용되지 않은"));
    }
    // 같은 신고로 다시 시도하면 횟수만 는다.
    let (status, _) = e.start(None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 시작 시각을 남기지 않았다: 로그 키 등록·제출은 「시작하지 않은 문항」.
    let (status, _) = status_of(
        &e.app,
        "POST",
        &e.item_uri("submit"),
        ERIN,
        Some(logged_submit_body(&e.app, &e.item_uri("submit"), &token_for(ERIN), "correct").await),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let view = e.signals(DAVE).await;
    let rejected: Vec<_> = view
        .signals
        .iter()
        .filter_map(|s| match &s.kind {
            SuspicionKind::IntegrityRejected {
                app_ok,
                model_ok,
                count,
                ..
            } => Some((*app_ok, *model_ok, *count, s.clock)),
            _ => None,
        })
        .collect();
    assert_eq!(rejected.len(), 3, "{view:?}");
    for (_, app_ok, model_ok) in &cases {
        assert!(
            rejected
                .iter()
                .any(|r| (r.0, r.1) == (*app_ok, *model_ok) && r.3 == SignalClock::Server),
            "{rejected:?}"
        );
    }
    assert!(rejected.iter().any(|r| r.2 == 2), "신고 없음은 두 번");
    assert!(
        view.signals
            .iter()
            .all(|s| s.kind.label() == "모델 무결성 실패")
    );

    // 허용된 판·모델이면 시작한다.
    let (status, body) = e.start(Some(start_body())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let _: ItemStarted = serde_json::from_value(body).unwrap();
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 포커스_이탈_전체화면_이탈_붙여넣기_막음이_시간순_의심_신호로_뜬다(
    pool: PgPool,
) {
    let e = started(pool).await;
    e.upload(vec![
        (T0, LogEvent::Started),
        (T0 + 5_000, LogEvent::PasteBlocked { chars: 120 }),
        (T0 + 9_000, LogEvent::FocusLost),
        (T0 + 21_000, LogEvent::FocusReturned { away_ms: 12_000 }),
        (T0 + 30_000, LogEvent::FullscreenExited),
        (T0 + 31_000, LogEvent::PasteBlocked { chars: 8 }),
        (T0 + 40_000, LogEvent::FocusLost),
        // 웹캠 감지(#13). 주기 스냅샷은 신호가 아니다.
        (
            T0 + 50_000,
            LogEvent::Snapshot {
                snapshot: "p-1".into(),
            },
        ),
        (
            T0 + 51_000,
            LogEvent::MultipleFaces {
                count: 2,
                snapshot: Some("e-1".into()),
            },
        ),
        (
            T0 + 52_000,
            LogEvent::CameraUnavailable {
                reason: "장치 분리".into(),
            },
        ),
    ])
    .await;

    let view = e.signals(ALICE).await;
    assert_eq!(view.candidate, "erin@example.com");
    let kinds: Vec<(Option<i64>, SuspicionKind)> = view
        .signals
        .iter()
        .map(|s| (s.at_ms, s.kind.clone()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                Some(T0 + 5_000),
                SuspicionKind::PasteBlocked { seq: 1, chars: 120 }
            ),
            (
                Some(T0 + 9_000),
                SuspicionKind::FocusLost {
                    seq: 2,
                    away_ms: Some(12_000)
                }
            ),
            (
                Some(T0 + 30_000),
                SuspicionKind::FullscreenExited { seq: 4 }
            ),
            (
                Some(T0 + 31_000),
                SuspicionKind::PasteBlocked { seq: 5, chars: 8 }
            ),
            (
                Some(T0 + 40_000),
                SuspicionKind::FocusLost {
                    seq: 6,
                    away_ms: None
                }
            ),
            (
                Some(T0 + 51_000),
                SuspicionKind::Webcam {
                    seq: 8,
                    reason: promptlab_shared::webcam::SnapshotReason::MultipleFaces,
                    count: Some(2),
                    snapshot: Some("e-1".into()),
                }
            ),
            (
                Some(T0 + 52_000),
                SuspicionKind::CameraUnavailable {
                    seq: 9,
                    reason: "장치 분리".into(),
                }
            ),
        ]
    );
    assert!(view.signals.iter().all(|s| s.clock == SignalClock::App));
    assert_eq!(
        view.signals[0].kind.describe(),
        "편집기 밖에서 120자를 붙여넣으려 해 막았다"
    );
    assert_eq!(
        view.signals[1].kind.describe(),
        "응시 창을 떠났다가 12초 뒤 돌아왔다"
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 의심_신호는_결과_보기_역할만_보고_다른_조직과_응시자는_못_본다(
    pool: PgPool,
) {
    let e = started(pool).await;
    // 관리자·열람자는 본다.
    for sub in [ALICE, DAVE] {
        let _ = e.signals(sub).await;
    }
    // 출제자는 결과 보기 권한이 없다.
    let (status, _) = status_of(&e.app, "GET", &e.signals_uri(), BOB, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // 응시자 본인·다른 조직 사람은 404.
    let other: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        CAROL,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    for (sub, uri) in [
        (ERIN, e.signals_uri()),
        (CAROL, e.signals_uri()),
        (
            CAROL,
            format!(
                "/orgs/{}/exams/{}/invites/{}/signals",
                other.id, e.exam.id, e.assignment.id
            ),
        ),
        (
            ALICE,
            format!(
                "/orgs/{}/exams/{}/invites/not-a-uuid/signals",
                e.org.id, e.exam.id
            ),
        ),
    ] {
        let (status, body) = status_of(&e.app, "GET", &uri, sub, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{sub} {uri}: {body}");
    }
    let _ = &e.pool;
}
