//! 응시 앱 로그인: sandcorp-auth 공개 클라이언트(CIMD) + PKCE(S256) + 루프백 리다이렉트(RFC 8252).
//!
//! 1. 서버 `GET /auth/app-config`에서 auth 끝점·클라이언트 id·리소스·스코프를 받는다.
//! 2. `127.0.0.1`의 빈 포트를 열고 브라우저로 auth `/authorize`를 연다(로그인 화면에서 카카오).
//! 3. auth가 `http://127.0.0.1:<포트>/callback?code=…&state=…`로 돌려보내면 state를 확인하고
//!    `/token`에서 코드를 교환한다(시크릿 없음, `code_verifier`). auth는 루프백의 포트를 무시하고 비교한다.
//! 4. 액세스 토큰(10분)은 만료 1분 전에 리프레시 토큰으로 갈아 끼운다(리프레시 토큰은 매번 회전).
//!
//! 토큰은 메모리에만 둔다(앱을 끄면 다시 로그인).

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use promptlab_shared::api::AppAuthConfig;
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 브라우저가 돌아오기를 기다리는 최대 시간.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// 이만큼 남으면 미리 갈아 끼운다.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("로그인 창을 기다리다 시간이 다 됐다")]
    Timeout,
    #[error("로그인이 거절됐다: {0}")]
    Denied(String),
    #[error("로그인 응답이 이상하다: {0}")]
    Protocol(String),
    #[error("로그인 서버에 닿지 못했다: {0}")]
    Network(#[from] reqwest::Error),
    #[error("로그인 창을 받을 포트를 열지 못했다: {0}")]
    Io(#[from] std::io::Error),
}

/// 받은 토큰. 디버그 출력에 값을 찍지 않는다.
#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Instant,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl Tokens {
    /// 아직 넉넉히 쓸 수 있는가.
    pub fn fresh(&self) -> bool {
        Instant::now() + REFRESH_MARGIN < self.expires_at
    }
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
}

fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS 난수를 얻지 못했다");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE S256.
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 시작한 로그인: 브라우저로 열 주소와, 돌아오기를 기다리는 루프백 포트.
pub struct PendingLogin {
    authorize_url: String,
    redirect_uri: String,
    state: String,
    verifier: String,
    listener: TcpListener,
}

/// 루프백 포트를 열고 인가 URL을 만든다.
pub async fn begin(config: &AppAuthConfig) -> Result<PendingLogin, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let state = random_secret();
    let verifier = random_secret();
    let mut url = Url::parse(&config.authorization_endpoint)
        .map_err(|e| AuthError::Protocol(format!("authorization_endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("state", &state)
        .append_pair("code_challenge", &code_challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", &config.resource)
        .append_pair("scope", &config.scope);
    Ok(PendingLogin {
        authorize_url: url.into(),
        redirect_uri,
        state,
        verifier,
        listener,
    })
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>PromptLab</title>\
<p style=\"font-family:sans-serif\">로그인이 끝났습니다. 이 창을 닫고 PromptLab으로 돌아가세요.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>PromptLab</title>\
<p style=\"font-family:sans-serif\">로그인하지 못했습니다. PromptLab에서 다시 시도하세요.</p>";

impl PendingLogin {
    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// 브라우저가 돌아오기를 기다렸다가 코드를 교환한다.
    pub async fn finish(
        self,
        http: &reqwest::Client,
        config: &AppAuthConfig,
    ) -> Result<Tokens, AuthError> {
        let params = tokio::time::timeout(LOGIN_TIMEOUT, self.wait_for_callback())
            .await
            .map_err(|_| AuthError::Timeout)??;
        let get = |k: &str| {
            params
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        if let Some(error) = get("error") {
            return Err(AuthError::Denied(error));
        }
        if get("state").as_deref() != Some(self.state.as_str()) {
            return Err(AuthError::Protocol("state가 맞지 않는다".into()));
        }
        let code = get("code").ok_or_else(|| AuthError::Protocol("code가 없다".into()))?;
        token_request(
            http,
            config,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", &config.client_id),
                ("code_verifier", &self.verifier),
            ],
        )
        .await
    }

    /// `/callback` 요청 하나의 쿼리. 다른 경로(파비콘 등)는 404로 답하고 계속 기다린다.
    async fn wait_for_callback(&self) -> Result<Vec<(String, String)>, AuthError> {
        loop {
            let (mut stream, _) = self.listener.accept().await?;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 2048];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
                let n = stream.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf);
            let target = head
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("GET "))
                .and_then(|rest| rest.split(' ').next())
                .unwrap_or("");
            let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
                continue;
            };
            if url.path() != "/callback" {
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                continue;
            }
            let params: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            let ok = params.iter().any(|(k, _)| k == "code");
            let page = if ok { DONE_PAGE } else { FAILED_PAGE };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
            return Ok(params);
        }
    }
}

/// 리프레시 토큰으로 새 토큰을 받는다(리프레시 토큰도 새것으로 바뀐다).
pub async fn refresh(
    http: &reqwest::Client,
    config: &AppAuthConfig,
    refresh_token: &str,
) -> Result<Tokens, AuthError> {
    token_request(
        http,
        config,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &config.client_id),
        ],
    )
    .await
}

async fn token_request(
    http: &reqwest::Client,
    config: &AppAuthConfig,
    form: &[(&str, &str)],
) -> Result<Tokens, AuthError> {
    let response = http.post(&config.token_endpoint).form(form).send().await?;
    let status = response.status();
    if !status.is_success() {
        let error = response
            .json::<OAuthError>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| status.to_string());
        return Err(AuthError::Denied(error));
    }
    let body: TokenBody = response.json().await?;
    Ok(Tokens {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        expires_at: Instant::now() + Duration::from_secs(body.expires_in.unwrap_or(600)),
    })
}

/// 시스템 기본 브라우저로 연다.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        eprintln!("브라우저를 열지 못했다({e}). 이 주소를 직접 여세요: {url}");
    }
}
