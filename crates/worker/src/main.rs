//! 채점 워커 바이너리. 채점 큐를 하나씩 비운다 — 코드 실행은 판정기(`promptlab-judge`) HTTP에 맡긴다.
//!
//! 환경변수:
//! - `DATABASE_URL`(필수): Postgres
//! - `PROMPTLAB_JUDGE_URL`: 판정기 주소(기본 `http://judge:8090`)
//! - `SENTRY_DSN`·`SENTRY_RELEASE`(선택): 오류 수집. 설명은 `promptlab-observability`.
//! - `PROMPTLAB_WORKER_POLL_MS`: 큐가 비었을 때 다시 볼 간격(기본 1000)

use std::time::Duration;

use promptlab_worker::{DEFAULT_JUDGE_URL, HttpJudge, Outcome, run_once};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 오류 수집(GlitchTip): SENTRY_DSN 이 없으면 꺼진다. guard 는 main 이 끝날 때까지 쥔다.
    let _sentry = promptlab_observability::init_from_env();
    promptlab_observability::init_tracing();

    // 문제은행(#138): PROMPTLAB_PROBLEMS_DIR(없으면 공개 픽스처)를 전부 싣고 확인한다 — 깨졌으면 뜨지 않는다.
    let catalog =
        promptlab_store::catalog::init().map_err(|e| format!("문제를 싣지 못했다 — {e}"))?;
    {
        use promptlab_store::catalog::Kind;
        tracing::info!(
            source = %catalog.source(),
            practice = catalog.count(Kind::Practice),
            exam = catalog.count(Kind::Exam),
            smoke = catalog.count(Kind::Smoke),
            "문제은행 실음"
        );
    }

    let database_url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL 이 없다")?;
    let poll = Duration::from_millis(
        std::env::var("PROMPTLAB_WORKER_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000),
    );
    let judge_url =
        std::env::var("PROMPTLAB_JUDGE_URL").unwrap_or_else(|_| DEFAULT_JUDGE_URL.to_owned());
    let judge = HttpJudge::new(&judge_url)?;
    tracing::info!(url = %judge_url, "판정기 주소");

    let pool = promptlab_store::connect(&database_url).await?;
    let requeued = promptlab_store::submissions::requeue_grading(&pool).await?;
    if requeued > 0 {
        tracing::warn!(requeued, "채점 중이던 제출을 큐로 되돌렸다");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "채점 워커 시작");

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        let step = tokio::select! {
            _ = &mut shutdown => break,
            step = run_once(&pool, &judge) => step,
        };
        match step {
            Ok(Outcome::Idle | Outcome::Requeued(_)) => {
                tokio::select! {
                    _ = &mut shutdown => break,
                    _ = tokio::time::sleep(poll) => {}
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "DB 오류 — 잠시 뒤 다시");
                tokio::time::sleep(poll * 5).await;
            }
        }
    }
    tracing::info!("채점 워커 종료");
    Ok(())
}

/// Ctrl-C 또는 SIGTERM(docker stop).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}
