//! 러너 이미지 미리 받기. 판정기는 `--pull never`이고 프록시는 클라이언트의 pull을 막으니, 소켓을 가진 프록시가
//! 시작할 때 [`crate::rules::allowed_images`](= 판정기가 쓰는 `RunnerImage::reference()`와 같은 문자열)를 직접 받는다.
//!
//! - 이미 있으면 받지 않는다(digest 참조는 바뀌지 않고, digest를 채우기 전 태그 참조는 로컬에서 구운
//!   `build-runners.sh --local` 이미지를 레지스트리 것으로 덮지 않는다).
//! - 실패하면 지수 대기(10초 → 최대 10분)로 다시 한다. 받는 동안에도 판정 요청은 넘긴다(이미지가 없으면 docker가
//!   만들기에서 오류를 주고 판정기는 503 → 워커가 큐로 되돌린다).

use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Request, StatusCode};

use crate::proxy::send;

/// 참조를 `fromImage`·`tag`로 나눈다(`name@sha256:…` → digest가 tag 자리, `name:tag` → 태그).
pub fn split_reference(r: &str) -> (&str, &str) {
    if let Some((name, digest)) = r.split_once('@') {
        return (name, digest);
    }
    match r.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => (name, tag),
        _ => (r, "latest"),
    }
}

async fn present(socket: &Path, reference: &str) -> Result<bool, String> {
    let req = Request::get(format!("/images/{reference}/json"))
        .header("host", "docker")
        .body(Empty::<Bytes>::new())
        .map_err(|e| e.to_string())?;
    let resp = send(socket, req).await.map_err(|e| e.to_string())?;
    Ok(resp.status() == StatusCode::OK)
}

/// 하나 받기. 진행 줄 중 `error`가 있으면 실패(상태 코드가 200이어도).
async fn pull_once(socket: &Path, reference: &str) -> Result<(), String> {
    let (from, tag) = split_reference(reference);
    let req = Request::post(format!("/images/create?fromImage={from}&tag={tag}"))
        .header("host", "docker")
        .body(Empty::<Bytes>::new())
        .map_err(|e| e.to_string())?;
    let resp = send(socket, req).await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = Limited::new(resp.into_body(), 16 * 1024 * 1024)
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    if !status.is_success() {
        return Err(format!("상태 {status}"));
    }
    for line in body.split(|b| *b == b'\n') {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line)
            && let Some(e) = v.get("error").and_then(|e| e.as_str())
        {
            return Err(e.chars().take(200).collect());
        }
    }
    Ok(())
}

/// 모든 참조를 받을 때까지(각각 따로 재시도).
pub async fn prefetch(socket: PathBuf, references: Vec<String>) {
    for reference in references {
        let mut wait = Duration::from_secs(10);
        loop {
            match present(&socket, &reference).await {
                Ok(true) => {
                    tracing::info!(image = %reference, "러너 이미지 있음");
                    break;
                }
                Ok(false) => match pull_once(&socket, &reference).await {
                    Ok(()) => {
                        tracing::info!(image = %reference, "러너 이미지 받음");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(image = %reference, error = %e, wait_secs = wait.as_secs(), "러너 이미지 받기 실패 — 다시 한다")
                    }
                },
                Err(e) => {
                    tracing::warn!(image = %reference, error = %e, wait_secs = wait.as_secs(), "docker 소켓 오류 — 다시 한다")
                }
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(600));
        }
    }
}
