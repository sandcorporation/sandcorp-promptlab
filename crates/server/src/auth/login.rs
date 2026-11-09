//! 대시보드 로그인(기밀 클라이언트, Authorization Code + PKCE S256 — auth 문서 7절)과
//! 응시 앱이 쓰는 공개 문서들(CIMD·앱 로그인 설정·보호 리소스 메타데이터).
//!
//! 흐름: `GET /auth/login?next=/경로` → state·code_verifier를 서버 쪽(`login_states`)에 두고 브라우저에는
//! 무작위 id 쿠키만 → auth `/authorize`(로그인 화면에서 카카오) → `GET /auth/callback` → state 확인 →
//! 코드 교환(client_secret_basic) → 액세스 토큰을 API와 같은 규칙으로 검증 → 세션 쿠키 → 대시보드로.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use promptlab_shared::api::AppAuthConfig;
use promptlab_store::accounts::{self, LoginState, SessionOwner};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::token::{AccountKind, BoxFuture, Rejection};
use super::{
    API_SCOPE, Account, SESSION_COOKIE, SESSION_SECS, cookie, random_secret, secret_hash,
    set_cookie,
};
use crate::{ApiError, AppState};

/// 로그인 진행 쿠키 이름과 수명(10분).
pub const LOGIN_COOKIE: &str = "pl_login";
pub const LOGIN_SECS: i64 = 10 * 60;
/// 대시보드가 요청하는 스코프. `openid profile`은 표시 이름(ID 토큰)용이다.
pub const DASHBOARD_SCOPE: &str = "openid profile promptlab";

/// 대시보드 기밀 클라이언트. 시크릿은 로그·디버그 출력에 찍지 않는다.
#[derive(Clone)]
pub struct DashboardClient {
    pub client_id: String,
    pub client_secret: String,
}

impl std::fmt::Debug for DashboardClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashboardClient")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<숨김>")
            .finish()
    }
}

/// 코드 교환 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeExchange {
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
}

/// 토큰 응답 중 쓰는 것.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
}

/// auth `/token`. 시험은 가짜를 넣는다.
pub trait TokenEndpoint: Send + Sync {
    fn exchange<'a>(
        &'a self,
        client: &'a DashboardClient,
        request: CodeExchange,
    ) -> BoxFuture<'a, Result<TokenResponse, String>>;
}

/// HTTP로 auth `/token`을 부른다(client_secret_basic).
pub struct HttpTokenEndpoint {
    http: reqwest::Client,
    url: String,
}

impl HttpTokenEndpoint {
    pub fn new(url: impl Into<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            url: url.into(),
        })
    }
}

impl TokenEndpoint for HttpTokenEndpoint {
    fn exchange<'a>(
        &'a self,
        client: &'a DashboardClient,
        request: CodeExchange,
    ) -> BoxFuture<'a, Result<TokenResponse, String>> {
        Box::pin(async move {
            let response = self
                .http
                .post(&self.url)
                .basic_auth(&client.client_id, Some(&client.client_secret))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", &request.code),
                    ("redirect_uri", &request.redirect_uri),
                    ("code_verifier", &request.code_verifier),
                ])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status();
            if !status.is_success() {
                // 본문(RFC 6749 오류)은 비밀이 없지만 짧게만 남긴다.
                let body: Value = response.json().await.unwrap_or(Value::Null);
                return Err(format!("상태 {status}, error={}", body["error"]));
            }
            response.json().await.map_err(|e| e.to_string())
        })
    }
}

/// 로그인 뒤 돌아갈 곳. 이 서비스 안의 경로만(`/`로 시작하고 `//`·`/\`가 아님). 아니면 `/`.
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.starts_with("/\\")
                && !n.chars().any(char::is_control) =>
        {
            n.to_owned()
        }
        _ => "/".into(),
    }
}

/// PKCE S256 code_challenge.
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn redirect(location: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location.to_owned())]).into_response()
}

fn login_failed(state: &AppState, why: &str) -> Response {
    tracing::info!(why, "대시보드 로그인 실패");
    let mut response = redirect(&format!("{}/?login_error=1", state.auth.public_url));
    response
        .headers_mut()
        .append(header::SET_COOKIE, set_cookie(LOGIN_COOKIE, "", 0));
    response
}

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

/// `GET /auth/login` — auth `/authorize`로 보낸다.
pub async fn start(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
) -> Result<Response, ApiError> {
    let config = &state.auth;
    let Some(client) = &config.dashboard_client else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "대시보드 로그인이 설정되지 않았다",
        ));
    };
    let login_id = random_secret();
    let login = LoginState {
        state: random_secret(),
        verifier: random_secret(),
        next_path: safe_next(query.next.as_deref()),
    };
    accounts::save_login_state(&state.pool, &secret_hash(&login_id), &login, LOGIN_SECS).await?;
    let mut url = url::Url::parse(&config.authorization_endpoint())
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "auth 주소가 틀렸다"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client.client_id)
        .append_pair("redirect_uri", &config.redirect_uri())
        .append_pair("state", &login.state)
        .append_pair("code_challenge", &code_challenge(&login.verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", config.validator.rules().audience.as_str())
        .append_pair("scope", DASHBOARD_SCOPE);
    let mut response = redirect(url.as_str());
    response.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(LOGIN_COOKIE, &login_id, LOGIN_SECS),
    );
    Ok(response)
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /auth/callback` — 코드를 교환하고 세션을 만든다.
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    let config = &state.auth;
    let Some(client) = &config.dashboard_client else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "대시보드 로그인이 설정되지 않았다",
        ));
    };
    // 로그인 상태는 성공·실패와 상관없이 한 번 쓰고 지운다.
    let Some(login_id) = cookie(&headers, LOGIN_COOKIE) else {
        return Ok(login_failed(&state, "로그인 쿠키 없음"));
    };
    let Some(login) = accounts::take_login_state(&state.pool, &secret_hash(login_id)).await? else {
        return Ok(login_failed(&state, "로그인 상태 없음·만료"));
    };
    if query.error.is_some() {
        return Ok(login_failed(&state, "auth가 오류로 돌려보냄"));
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return Ok(login_failed(&state, "code·state 없음"));
    };
    if !constant_time_eq(returned_state.as_bytes(), login.state.as_bytes()) {
        return Ok(login_failed(&state, "state 불일치"));
    }
    let tokens = match config
        .token_endpoint
        .exchange(
            client,
            CodeExchange {
                code,
                code_verifier: login.verifier,
                redirect_uri: config.redirect_uri(),
            },
        )
        .await
    {
        Ok(tokens) => tokens,
        Err(e) => {
            tracing::warn!(error = %e, "코드 교환 실패");
            return Ok(login_failed(&state, "코드 교환 실패"));
        }
    };
    let verified = match config
        .validator
        .validate(&tokens.access_token, API_SCOPE)
        .await
    {
        Ok(v) => v,
        Err(Rejection::InvalidToken(rule)) => {
            tracing::info!(rule, "로그인 토큰 거부");
            return Ok(login_failed(&state, "토큰 거부"));
        }
        Err(Rejection::InsufficientScope) => return Ok(login_failed(&state, "스코프 부족")),
    };
    // 기계 토큰은 대시보드 세션을 열지 않는다(#28 — 스모크 조직의 응시자로만).
    if verified.kind == AccountKind::Machine {
        return Ok(login_failed(&state, "기계 계정"));
    }
    let name = match &tokens.id_token {
        Some(id_token) => id_token_name(&state, id_token, &client.client_id, &verified.sub).await,
        None => None,
    };
    accounts::upsert_user(&state.pool, &verified.sub, name.as_deref()).await?;
    let session_id = random_secret();
    accounts::create_session(
        &state.pool,
        &secret_hash(&session_id),
        &SessionOwner {
            sub: verified.sub.clone(),
            is_owner: verified.kind == AccountKind::Owner,
        },
        SESSION_SECS,
    )
    .await?;
    tracing::info!(sub = %verified.sub, "대시보드 로그인");
    let mut response = redirect(&format!("{}{}", config.public_url, login.next_path));
    let cookies = response.headers_mut();
    cookies.append(header::SET_COOKIE, set_cookie(LOGIN_COOKIE, "", 0));
    cookies.append(
        header::SET_COOKIE,
        set_cookie(SESSION_COOKIE, &session_id, SESSION_SECS),
    );
    Ok(response)
}

/// ID 토큰의 표시 이름. 서명(같은 JWKS)·`iss`·`aud`(=client_id)·`sub`(=액세스 토큰과 같음)가 맞을 때만.
async fn id_token_name(
    state: &AppState,
    id_token: &str,
    client_id: &str,
    sub: &str,
) -> Option<String> {
    let claims = state.auth.validator.verified_claims(id_token).await.ok()?;
    let rules = state.auth.validator.rules();
    let ok = claims.get("iss").and_then(Value::as_str) == Some(rules.issuer.as_str())
        && claims.get("aud").and_then(Value::as_str) == Some(client_id)
        && claims.get("sub").and_then(Value::as_str) == Some(sub);
    if !ok {
        tracing::info!("ID 토큰이 맞지 않아 이름을 쓰지 않는다");
        return None;
    }
    claims
        .get("name")
        .and_then(Value::as_str)
        .map(|n| n.trim().chars().take(100).collect::<String>())
        .filter(|n| !n.is_empty())
}

/// `POST /auth/logout` — 세션을 끊는다(auth에는 end-session이 없다).
pub async fn logout(
    State(state): State<AppState>,
    Extension(_account): Extension<Account>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(session_id) = cookie(&headers, SESSION_COOKIE) {
        accounts::delete_session(&state.pool, &secret_hash(session_id)).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, set_cookie(SESSION_COOKIE, "", 0));
    Ok(response)
}

/// `GET /auth/app-config` — 응시 앱의 로그인 값.
pub async fn app_config(State(state): State<AppState>) -> Json<AppAuthConfig> {
    let config = &state.auth;
    Json(AppAuthConfig {
        authorization_endpoint: config.authorization_endpoint(),
        token_endpoint: config.token_endpoint_url(),
        client_id: config.app_client_id(),
        resource: config.validator.rules().audience.clone(),
        scope: API_SCOPE.into(),
    })
}

/// 응시 앱의 루프백 리다이렉트(RFC 8252 — auth가 포트를 무시하고 비교한다).
pub const APP_REDIRECT_URI: &str = "http://127.0.0.1/callback";

/// `GET /oauth/app-client.json` — 응시 앱 공개 클라이언트의 CIMD 문서. `client_id`는 이 문서의 URL이다.
pub async fn app_client_document(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "client_id": state.auth.app_client_id(),
        "client_name": "promptlab 응시 앱",
        "redirect_uris": [APP_REDIRECT_URI],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "scope": API_SCOPE,
    }))
}

/// `GET /.well-known/oauth-protected-resource/api` — RFC 9728(auth 문서 6절).
pub async fn protected_resource_metadata(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "resource": state.auth.validator.rules().audience,
        "authorization_servers": [state.auth.validator.rules().issuer],
        "scopes_supported": [API_SCOPE],
        "bearer_methods_supported": ["header"],
    }))
}

/// 길이가 같으면 내용과 상관없이 같은 시간에 비교한다.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
