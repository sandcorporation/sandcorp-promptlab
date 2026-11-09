//! `promptlab-docker-proxy` — compose 내부 네트워크에서만 받는다(포트 공개 없음).
//!
//! 환경변수: `PROMPTLAB_PROXY_ADDR`(기본 `0.0.0.0:2375`), `PROMPTLAB_DOCKER_SOCKET`(기본 `/var/run/docker.sock`),
//! `PROMPTLAB_PROXY_PULL`(기본 켜짐, `0`이면 러너 이미지를 미리 받지 않는다 — 시험용).

use std::path::PathBuf;
use std::sync::Arc;

use promptlab_docker_proxy::{Proxy, privileges, pull, rules, serve};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let addr = std::env::var("PROMPTLAB_PROXY_ADDR").unwrap_or_else(|_| "0.0.0.0:2375".into());
    let socket = PathBuf::from(
        std::env::var("PROMPTLAB_DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".into()),
    );
    if let Err(e) = privileges::drop_to_socket_group(&socket) {
        tracing::error!(error = %e, "권한을 내리지 못했다");
        std::process::exit(1);
    }
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("프록시 주소를 열지 못했다");
    tracing::info!(%addr, socket = %socket.display(), images = ?rules::allowed_images(), "소켓 프록시 시작");
    if std::env::var("PROMPTLAB_PROXY_PULL").as_deref() != Ok("0") {
        tokio::spawn(pull::prefetch(socket.clone(), rules::allowed_images()));
    }
    let proxy = Arc::new(Proxy::new(socket));
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
    tokio::select! {
        r = serve(listener, proxy) => {
            tracing::error!(?r, "받기 고리가 끝났다");
            std::process::exit(1);
        }
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
