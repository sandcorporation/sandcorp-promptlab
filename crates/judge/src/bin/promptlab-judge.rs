//! 판정기 HTTP — compose 내부 네트워크에서만 받는다(포트 공개 없음).
//!
//! - `POST /run`: [`RunRequest`] → [`RunOutcome`]. 규칙 밖 422, docker 실패 503.
//! - `GET /health`: 200.
//!
//! 환경변수: `PROMPTLAB_JUDGE_ADDR`(기본 `0.0.0.0:8090`), `PROMPTLAB_JUDGE_CONCURRENCY`(기본 2), docker CLI는
//! `DOCKER_HOST`(운영은 본문 검사 소켓 프록시 `tcp://…:2375`, #95)를 따른다.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use promptlab_judge::{Judge, JudgeError, RunRequest, limits};

/// 요청 본문 상한 — 케이스 합계 1MB + 코드 64KB에 여유.
const BODY_LIMIT: usize = 4 * 1024 * 1024;

async fn run(State(judge): State<Judge>, Json(req): Json<RunRequest>) -> Response {
    match judge.run(&req).await {
        Ok(out) => Json(out).into_response(),
        Err(JudgeError::Invalid(m)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": m })),
        )
            .into_response(),
        Err(e @ JudgeError::Docker(_)) => {
            tracing::error!(error = %e, "러너 실행 실패");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "러너를 돌리지 못했다" })),
            )
                .into_response()
        }
    }
}

fn router(judge: Judge) -> Router {
    Router::new()
        .route("/run", post(run))
        .route("/health", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(judge)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let addr = std::env::var("PROMPTLAB_JUDGE_ADDR").unwrap_or_else(|_| "0.0.0.0:8090".into());
    let concurrency = std::env::var("PROMPTLAB_JUDGE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(limits::MAX_CONCURRENT)
        .min(limits::MAX_CONCURRENT);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("판정기 주소를 열지 못했다");
    tracing::info!(%addr, concurrency, "판정기 시작");
    axum::serve(listener, router(Judge::new("docker", concurrency)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("판정기 서버");
}
