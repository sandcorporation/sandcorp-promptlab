//! 응시 앱 내려받기 주소(#22): `GET /download` — 인증 없음. GitHub Release **최신**의 설치 파일
//! (`promptlab-app-setup-<버전>.exe`)로 302 한다.
//!
//! 최신 판은 GitHub API `releases/latest`(토큰 없음, 인증 없는 한도 시간당 60)를 조회해 찾고 결과를 캐시한다.
//! (고정 이름 자산 `releases/latest/download/<이름>`으로 조회 없이 넘기는 길은 Release가 없을 때 우리 안내 화면을
//! 못 띄우고 판 없는 자산을 더 올려야 해서 고르지 않았다.)
//! 조회가 실패하거나 Release가 없으면: 캐시된 마지막 성공 주소가 있으면 그리로, 없으면 안내 화면(200).
//! GitHub 응답 본문·오류 상세는 화면에 싣지 않고 로그(warn)에만 남긴다.
//!
//! | 환경변수 | 기본 |
//! |---|---|
//! | `PROMPTLAB_RELEASES_REPO` | `sandcorporation/sandcorp-promptlab` |
//! | `PROMPTLAB_GITHUB_API_URL` | `https://api.github.com` |

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::AppState;

pub const REPO_ENV: &str = "PROMPTLAB_RELEASES_REPO";
pub const API_ENV: &str = "PROMPTLAB_GITHUB_API_URL";
pub const DEFAULT_REPO: &str = "sandcorporation/sandcorp-promptlab";
pub const DEFAULT_API: &str = "https://api.github.com";
/// 성공한 조회를 믿는 시간(인증 없는 한도 시간당 60을 넘지 않는다).
pub const CACHE_TTL: Duration = Duration::from_secs(600);
/// 조회가 실패했거나 Release가 없을 때 다시 조회하기까지의 최소 간격.
const RETRY_AFTER: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const ASSET_PREFIX: &str = "promptlab-app-setup-";
const ASSET_SUFFIX: &str = ".exe";

#[derive(Debug)]
struct Cache {
    /// 마지막으로 성공해 찾은 설치 파일 주소(조회가 실패해도 이것으로 넘긴다).
    url: Option<String>,
    /// 이 시각 전에는 다시 조회하지 않는다.
    fresh_until: Option<Instant>,
}

/// 내려받기 주소 찾기 + 캐시. 복제하면 같은 캐시를 쓴다.
#[derive(Clone)]
pub struct Download {
    http: reqwest::Client,
    api_url: String,
    repo: String,
    ttl: Duration,
    cache: Arc<Mutex<Cache>>,
}

enum Lookup {
    Found(String),
    NoInstaller,
    Failed(String),
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Download {
    pub fn new(api_url: &str, repo: &str, ttl: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent("promptlab-server")
            .build()
            .unwrap_or_default();
        Self {
            http,
            api_url: api_url.trim_end_matches('/').to_owned(),
            repo: repo.trim_matches('/').to_owned(),
            ttl,
            cache: Arc::new(Mutex::new(Cache {
                url: None,
                fresh_until: None,
            })),
        }
    }

    /// 환경변수로 만든다(없으면 공개 저장소·공개 GitHub API).
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| d.to_owned())
        };
        Self::new(
            &get(API_ENV, DEFAULT_API),
            &get(REPO_ENV, DEFAULT_REPO),
            CACHE_TTL,
        )
    }

    /// 공개 Release 목록 주소(안내 화면의 링크).
    pub fn releases_page(&self) -> String {
        format!("https://github.com/{}/releases", self.repo)
    }

    /// 설치 파일 주소. 없으면 `None`(안내 화면).
    pub async fn installer_url(&self) -> Option<String> {
        // 잠금을 조회 동안 쥐어 동시에 들어온 요청이 GitHub를 여러 번 부르지 않게 한다.
        let mut cache = self.cache.lock().await;
        if cache.fresh_until.is_some_and(|t| Instant::now() < t) {
            return cache.url.clone();
        }
        match self.lookup().await {
            Lookup::Found(url) => {
                cache.url = Some(url);
                cache.fresh_until = Some(Instant::now() + self.ttl);
            }
            Lookup::NoInstaller => {
                tracing::warn!(repo = %self.repo, "내려받기: 최신 Release에 설치 파일이 없다");
                cache.fresh_until = Some(Instant::now() + self.ttl.min(RETRY_AFTER));
            }
            Lookup::Failed(why) => {
                tracing::warn!(repo = %self.repo, %why, "내려받기: 최신 Release를 조회하지 못했다");
                cache.fresh_until = Some(Instant::now() + self.ttl.min(RETRY_AFTER));
            }
        }
        cache.url.clone()
    }

    async fn lookup(&self) -> Lookup {
        let url = format!("{}/repos/{}/releases/latest", self.api_url, self.repo);
        let response = match self
            .http
            .get(url)
            .header("accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Lookup::Failed(format!("요청 실패: {e}")),
        };
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Lookup::NoInstaller; // Release가 아직 없다
        }
        if !status.is_success() {
            return Lookup::Failed(format!("상태 {status}"));
        }
        match response.json::<Release>().await {
            Ok(release) => release
                .assets
                .into_iter()
                .find(|a| a.name.starts_with(ASSET_PREFIX) && a.name.ends_with(ASSET_SUFFIX))
                .map_or(Lookup::NoInstaller, |a| {
                    Lookup::Found(a.browser_download_url)
                }),
            Err(e) => Lookup::Failed(format!("본문 해석 실패: {e}")),
        }
    }
}

/// `GET /download`.
pub async fn download(State(state): State<AppState>) -> Response {
    match state.download.installer_url().await {
        Some(url) => (
            StatusCode::FOUND,
            [
                (header::LOCATION, url),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
        )
            .into_response(),
        None => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            not_ready_page(&state.download.releases_page()),
        )
            .into_response(),
    }
}

fn not_ready_page(releases: &str) -> String {
    format!(
        "<!doctype html><html lang=\"ko\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>PromptLab 응시 앱</title></head><body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem\">\
         <h1>PromptLab 응시 앱</h1>\
         <p>설치 파일을 준비 중입니다. 잠시 뒤 다시 시도해 주세요.</p>\
         <p><a href=\"{releases}\">공개 Release 목록 보기</a></p></body></html>"
    )
}
