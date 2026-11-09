//! 웹캠 감독(#13): 동의 없는 시작 거부, 기준 사진, 스냅샷 업로드(사진 한 장만 — 영상 경로 없음),
//! 감독관 열람 권한·조직 격리, 보유 기간 삭제, 스모크 기계 주체 면제.

mod common;

use axum::http::{StatusCode, header};
use common::*;
use promptlab_shared::account::Role;
use promptlab_shared::api::{AssignmentView, ExamView, InviteCreated, OrgView};
use promptlab_shared::exam::Purpose;
use promptlab_shared::webcam::{
    DEFAULT_RETENTION_DAYS, NOTICE_VERSION, SnapshotReason, SnapshotStored, WebcamTerms, WebcamView,
};
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

async fn status(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
) -> StatusCode {
    call(app, method, uri, Some(&token_for(sub)), body).await.0
}

struct Env {
    app: axum::Router,
    clock: std::sync::Arc<FixedClock>,
    pool: PgPool,
    org: OrgView,
    exam: ExamView,
    assignment: AssignmentView,
}

impl Env {
    fn start_uri(&self, n: u32) -> String {
        format!("/assignments/{}/items/{n}/start", self.assignment.id)
    }

    fn snapshot_uri(&self, n: u32, query: &str) -> String {
        format!(
            "/assignments/{}/items/{n}/snapshots?{query}",
            self.assignment.id
        )
    }

    fn org_uri(&self, tail: &str) -> String {
        format!(
            "/orgs/{}/exams/{}/invites/{}/webcam{tail}",
            self.org.id, self.exam.id, self.assignment.id
        )
    }

    async fn photo(&self, uri: &str, content_type: &str, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
        let method = if uri.ends_with("/baseline") {
            "PUT"
        } else {
            "POST"
        };
        let (s, _, b) =
            send_raw(&self.app, method, uri, &token_for(ERIN), content_type, body).await;
        (s, b)
    }

    async fn consent(&self, version: &str) -> StatusCode {
        status(
            &self.app,
            "POST",
            &format!("/assignments/{}/consent", self.assignment.id),
            ERIN,
            Some(json!({ "notice_version": version })),
        )
        .await
    }

    fn baseline_uri(&self) -> String {
        format!("/assignments/{}/webcam/baseline", self.assignment.id)
    }
}

/// 관리자 ALICE·출제자 BOB·감독관 CAROL·열람자 DAVE인 조직, 시험 하나, ERIN 배정(아직 동의 전).
async fn setup(pool: PgPool) -> Env {
    let clock = FixedClock::at(NOW);
    let app = app_parts(
        pool.clone(),
        clock.clone(),
        std::sync::Arc::new(promptlab_server::mail::Disabled),
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
            "title": "웹캠 시험",
            "purpose": Purpose::Recruiting,
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
    Env {
        app,
        clock,
        pool,
        org,
        exam,
        assignment,
    }
}

/// JPEG 머리(SOF)의 가로를 바꾼 사본 — 고해상도 사진 흉내.
fn with_width(width: u16) -> Vec<u8> {
    let mut bytes = PHOTO.to_vec();
    let sof = (2..bytes.len() - 1)
        .find(|&i| bytes[i] == 0xFF && bytes[i + 1] == 0xC0)
        .expect("기준 JPEG에 SOF0이 있다");
    bytes[sof + 7..sof + 9].copy_from_slice(&width.to_be_bytes());
    bytes
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 동의와_기준_사진_없이는_문항을_시작하지_않는다(pool: PgPool) {
    let e = setup(pool).await;
    let terms = e.assignment.webcam.clone().unwrap();
    assert_eq!(
        terms,
        WebcamTerms {
            notice_version: NOTICE_VERSION.into(),
            retention_days: DEFAULT_RETENTION_DAYS,
            consented_at: None,
            baseline_uploaded: false,
            required: true,
        }
    );
    let (s, body) = call(
        &e.app,
        "POST",
        &e.start_uri(0),
        Some(&token_for(ERIN)),
        Some(start_body()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("동의"), "{body}");

    // 동의 전에는 사진을 받지 않는다. 옛 판 안내문 동의는 409.
    assert_eq!(
        e.photo(&e.baseline_uri(), "image/jpeg", PHOTO.to_vec())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(e.consent("2000-01-01").await, StatusCode::CONFLICT);
    assert_eq!(e.consent(NOTICE_VERSION).await, StatusCode::OK);

    // 동의만으로는 아직 — 기준 사진이 있어야 한다.
    let (s, body) = call(
        &e.app,
        "POST",
        &e.start_uri(0),
        Some(&token_for(ERIN)),
        Some(start_body()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(
        body["error"].as_str().unwrap().contains("기준 얼굴 사진"),
        "{body}"
    );

    assert_eq!(
        e.photo(&e.baseline_uri(), "image/jpeg", PHOTO.to_vec())
            .await
            .0,
        StatusCode::OK
    );
    let mine: Vec<AssignmentView> =
        ok(&e.app, "GET", "/me/assignments", ERIN, None, StatusCode::OK).await;
    let terms = mine[0].webcam.clone().unwrap();
    assert_eq!(terms.consented_at, Some(NOW));
    assert!(terms.baseline_uploaded);
    assert_eq!(
        status(&e.app, "POST", &e.start_uri(0), ERIN, Some(start_body())).await,
        StatusCode::OK
    );

    // 문항을 시작한 뒤에는 기준 사진을 바꾸지 못한다(대리 응시자 얼굴로 바꿔 치기 막기).
    assert_eq!(
        e.photo(&e.baseline_uri(), "image/jpeg", PHOTO.to_vec())
            .await
            .0,
        StatusCode::CONFLICT
    );
    // 남의 배정에는 동의할 수 없다.
    assert_eq!(
        status(
            &e.app,
            "POST",
            &format!("/assignments/{}/consent", e.assignment.id),
            DAVE,
            Some(json!({"notice_version": NOTICE_VERSION}))
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스냅샷은_저해상도_jpeg_한_장씩만_받고_영상은_받지_않는다(
    pool: PgPool,
) {
    let e = setup(pool).await;
    webcam_ready(&e.app, &token_for(ERIN), &e.assignment.id).await;

    // 시작하지 않은 문항에는 받지 않는다.
    let periodic = "reason=periodic&ref=p-1&at_ms=1767225601000";
    assert_eq!(
        e.photo(&e.snapshot_uri(0, periodic), "image/jpeg", PHOTO.to_vec())
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        status(&e.app, "POST", &e.start_uri(0), ERIN, Some(start_body())).await,
        StatusCode::OK
    );

    let (s, body) = e
        .photo(&e.snapshot_uri(0, periodic), "image/jpeg", PHOTO.to_vec())
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let first: SnapshotStored = serde_json::from_slice(&body).unwrap();
    // 같은 참조를 다시 올리면(응답이 끊긴 재전송) 같은 사진.
    let (s, body) = e
        .photo(&e.snapshot_uri(0, periodic), "image/jpeg", PHOTO.to_vec())
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<SnapshotStored>(&body).unwrap(),
        first
    );

    // 전체 녹화(영상)·큰 사진·고해상도·JPEG 아닌 본문은 받지 않는다.
    let event = "reason=no_face&ref=e-1";
    let webm = [&[0x1A, 0x45, 0xDF, 0xA3][..], &[0u8; 2048][..]].concat();
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "video/webm", webm.clone())
            .await
            .0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "video/mp4", PHOTO.to_vec())
            .await
            .0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "image/jpeg", webm)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "image/jpeg", with_width(1280))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let big = [PHOTO, &vec![0u8; 70 * 1024]].concat();
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "image/jpeg", big)
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let huge = vec![0u8; 5 * 1024 * 1024];
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "image/jpeg", huge)
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        e.photo(
            &e.snapshot_uri(0, "reason=movie"),
            "image/jpeg",
            PHOTO.to_vec()
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    e.clock.advance(5);
    assert_eq!(
        e.photo(&e.snapshot_uri(0, event), "image/jpeg", PHOTO.to_vec())
            .await
            .0,
        StatusCode::CREATED
    );

    let view: WebcamView = ok(&e.app, "GET", &e.org_uri(""), CAROL, None, StatusCode::OK).await;
    assert!(view.candidate.contains("erin@example.com"));
    assert_eq!(view.consent, Some((NOTICE_VERSION.to_owned(), NOW)));
    assert!(view.baseline.is_some());
    let reasons: Vec<_> = view
        .snapshots
        .iter()
        .map(|s| (s.reason, s.client_ref.clone()))
        .collect();
    assert_eq!(
        reasons,
        [
            (SnapshotReason::Periodic, Some("p-1".to_owned())),
            (SnapshotReason::NoFace, Some("e-1".to_owned())),
        ]
    );
    assert_eq!(view.snapshots[0].app_at_ms, Some(1_767_225_601_000));
    assert!(view.snapshots[0].at_ms < view.snapshots[1].at_ms);
    assert_eq!(view.retention_days, DEFAULT_RETENTION_DAYS);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 사진은_감독_권한만_인증된_요청으로_보고_다른_조직은_못_본다(
    pool: PgPool,
) {
    let e = setup(pool).await;
    webcam_ready(&e.app, &token_for(ERIN), &e.assignment.id).await;
    assert_eq!(
        status(&e.app, "POST", &e.start_uri(0), ERIN, Some(start_body())).await,
        StatusCode::OK
    );
    assert_eq!(
        e.photo(
            &e.snapshot_uri(0, "reason=gaze_away&ref=g"),
            "image/jpeg",
            PHOTO.to_vec()
        )
        .await
        .0,
        StatusCode::CREATED
    );

    for sub in [ALICE, CAROL] {
        let view: WebcamView = ok(&e.app, "GET", &e.org_uri(""), sub, None, StatusCode::OK).await;
        for id in [
            view.baseline.clone().unwrap().id,
            view.snapshots[0].id.clone(),
        ] {
            let request = axum::http::Request::builder()
                .method("GET")
                .uri(e.org_uri(&format!("/photos/{id}")))
                .header(header::AUTHORIZATION, format!("Bearer {}", token_for(sub)));
            let (s, headers, _) = send(&e.app, request, None).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(headers[header::CONTENT_TYPE], "image/jpeg");
            assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
            let (s, _, bytes) = send_raw(
                &e.app,
                "GET",
                &e.org_uri(&format!("/photos/{id}")),
                &token_for(sub),
                "image/jpeg",
                vec![],
            )
            .await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(bytes, PHOTO);
        }
    }
    let view: WebcamView = ok(&e.app, "GET", &e.org_uri(""), CAROL, None, StatusCode::OK).await;
    let photo = e.org_uri(&format!("/photos/{}", view.snapshots[0].id));
    // 출제자·열람자는 감독 권한이 없다(403). 응시자 본인은 조직 경로가 없다(404). 인증 없으면 401.
    for (sub, expect) in [
        (BOB, StatusCode::FORBIDDEN),
        (DAVE, StatusCode::FORBIDDEN),
        (ERIN, StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            status(&e.app, "GET", &e.org_uri(""), sub, None).await,
            expect,
            "{sub}"
        );
        assert_eq!(
            status(&e.app, "GET", &photo, sub, None).await,
            expect,
            "{sub}"
        );
    }
    assert_eq!(
        call(&e.app, "GET", &photo, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );

    // 다른 조직의 관리자(ERIN이 만든 조직)는 이 조직을 못 본다 — 404. 사진 id만 알아도 다른 초대 경로로는 없다.
    let other: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        ERIN,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(
        status(&e.app, "GET", &photo, ERIN, None).await,
        StatusCode::NOT_FOUND
    );
    let foreign = format!(
        "/orgs/{}/exams/{}/invites/{}/webcam/photos/{}",
        other.id, e.exam.id, e.assignment.id, view.snapshots[0].id
    );
    assert_eq!(
        status(&e.app, "GET", &foreign, ERIN, None).await,
        StatusCode::NOT_FOUND
    );
    let wrong_photo = e.org_uri(&format!("/photos/{}", uuid::Uuid::new_v4()));
    assert_eq!(
        status(&e.app, "GET", &wrong_photo, ALICE, None).await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 보유_기간이_지난_사진은_지운다(pool: PgPool) {
    let e = setup(pool).await;
    webcam_ready(&e.app, &token_for(ERIN), &e.assignment.id).await;
    let days = 30;
    let clock = FixedClock::at(NOW + i64::from(days) * 86_400 - 1);
    assert_eq!(
        promptlab_server::webcam::purge_once(&e.pool, clock.as_ref(), days)
            .await
            .unwrap(),
        0
    );
    clock.advance(2);
    assert_eq!(
        promptlab_server::webcam::purge_once(&e.pool, clock.as_ref(), days)
            .await
            .unwrap(),
        1
    );
    let view: WebcamView = ok(&e.app, "GET", &e.org_uri(""), ALICE, None, StatusCode::OK).await;
    assert!(view.baseline.is_none());
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 스모크_기계_주체는_웹캠_없이_시작하고_웹캠_경로는_못_쓴다(
    pool: PgPool,
) {
    let app = app(pool);
    let smoke = machine_token(SMOKE_CLIENT);
    let (s, list) = call(&app, "GET", "/me/assignments", Some(&smoke), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    let a: Vec<AssignmentView> = serde_json::from_value(list).unwrap();
    let a = &a[0];
    assert!(!a.webcam.as_ref().unwrap().required);
    let (s, body) = call(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/start", a.id),
        Some(&smoke),
        Some(start_body()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, _) = call(
        &app,
        "POST",
        &format!("/assignments/{}/consent", a.id),
        Some(&smoke),
        Some(json!({"notice_version": NOTICE_VERSION})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = send_raw(
        &app,
        "POST",
        &format!("/assignments/{}/items/0/snapshots?reason=periodic", a.id),
        &smoke,
        "image/jpeg",
        PHOTO.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
