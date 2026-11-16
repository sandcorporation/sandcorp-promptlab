//! 응시 앱 내려받기 `/download`(#22): 가짜 GitHub(releases/latest)로 최신 설치 파일 302, Release 없음 안내 200,
//! 조회 실패 + 캐시 있으면 캐시 주소로 302, 캐시 만료 뒤 다시 조회, 인증 없이 열림.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use promptlab_server::download::Download;
use promptlab_store::PgPool;
use serde_json::json;
use tokio::sync::Mutex;
use tower::ServiceExt;

#[derive(Clone)]
enum Mode {
    Latest(&'static str),
    NoRelease,
    Broken,
}

#[derive(Clone)]
struct Fake {
    mode: Arc<Mutex<Mode>>,
    hits: Arc<AtomicUsize>,
}

async fn latest(State(f): State<Fake>) -> axum::response::Response {
    f.hits.fetch_add(1, Ordering::SeqCst);
    match f.mode.lock().await.clone() {
        Mode::Latest(v) => Json(json!({
            "tag_name": format!("app-v{v}"),
            "assets": [
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://gh.test/SHA256SUMS.txt"},
                {"name": format!("promptlab-app-setup-{v}.exe"),
                 "browser_download_url": format!("https://gh.test/o/r/releases/download/app-v{v}/promptlab-app-setup-{v}.exe")},
            ]
        }))
        .into_response(),
        Mode::NoRelease => (StatusCode::NOT_FOUND, "secret-body").into_response(),
        Mode::Broken => (StatusCode::BAD_GATEWAY, "secret-body").into_response(),
    }
}

async fn fake_github(mode: Mode) -> (String, Fake) {
    let fake = Fake {
        mode: Arc::new(Mutex::new(mode)),
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/repos/o/r/releases/latest", get(latest))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, fake)
}

fn app_for(pool: PgPool, api: &str, ttl: Duration) -> Router {
    let mut state = app_state(pool, FixedClock::at(NOW), None);
    state.download = Download::new(api, "o/r", ttl);
    promptlab_server::router(state)
}

/// 인증 없이 부른다. (상태, Location, 본문)
async fn get_download(app: &Router, uri: &str) -> (StatusCode, Option<String>, String) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

const V1: &str = "https://gh.test/o/r/releases/download/app-v0.1.0/promptlab-app-setup-0.1.0.exe";

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 최신_설치_파일로_302_인증_없이_루트와_api_모두(pool: PgPool) {
    let (api, fake) = fake_github(Mode::Latest("0.1.0")).await;
    let app = app_for(pool, &api, Duration::from_secs(600));
    for uri in ["/download", "/api/download"] {
        let (status, location, _) = get_download(&app, uri).await;
        assert_eq!(status, StatusCode::FOUND, "{uri}");
        assert_eq!(location.as_deref(), Some(V1));
    }
    // 두 번째는 캐시 — GitHub를 한 번만 불렀다.
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn release가_없으면_안내_화면_200(pool: PgPool) {
    let (api, _fake) = fake_github(Mode::NoRelease).await;
    let app = app_for(pool, &api, Duration::from_secs(600));
    let (status, location, body) = get_download(&app, "/download").await;
    assert_eq!(status, StatusCode::OK);
    assert!(location.is_none());
    assert!(body.contains("설치 파일을 준비 중입니다. 잠시 뒤 다시 시도해 주세요"));
    assert!(body.contains("https://github.com/o/r/releases"));
    assert!(
        !body.contains("secret-body"),
        "GitHub 응답 본문은 싣지 않는다"
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 조회_실패면_안내_화면_캐시가_있으면_캐시_주소로_302(pool: PgPool) {
    let (api, fake) = fake_github(Mode::Broken).await;
    let app = app_for(pool, &api, Duration::from_millis(300));
    let (status, _, body) = get_download(&app, "/download").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("secret-body"));

    *fake.mode.lock().await = Mode::Latest("0.1.0");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (status, location, _) = get_download(&app, "/download").await;
    assert_eq!((status, location.as_deref()), (StatusCode::FOUND, Some(V1)));

    // 캐시가 만료된 뒤 GitHub가 죽어도 마지막 성공 주소로 넘긴다.
    *fake.mode.lock().await = Mode::Broken;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (status, location, _) = get_download(&app, "/download").await;
    assert_eq!((status, location.as_deref()), (StatusCode::FOUND, Some(V1)));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 캐시가_만료되면_새_판을_다시_조회한다(pool: PgPool) {
    let (api, fake) = fake_github(Mode::Latest("0.1.0")).await;
    let app = app_for(pool, &api, Duration::from_millis(300));
    let (_, location, _) = get_download(&app, "/download").await;
    assert_eq!(location.as_deref(), Some(V1));

    *fake.mode.lock().await = Mode::Latest("0.2.0");
    // 아직 신선하다 — 옛 주소.
    let (_, location, _) = get_download(&app, "/download").await;
    assert_eq!(location.as_deref(), Some(V1));
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (_, location, _) = get_download(&app, "/download").await;
    assert_eq!(
        location.as_deref(),
        Some("https://gh.test/o/r/releases/download/app-v0.2.0/promptlab-app-setup-0.2.0.exe")
    );
    assert_eq!(fake.hits.load(Ordering::SeqCst), 2);
}
