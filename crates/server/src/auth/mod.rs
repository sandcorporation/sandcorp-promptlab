//! 계정 인증. 모든 사용자는 sandcorp-auth 계정이다(로그인은 카카오 — auth 쪽 화면에서 고른다).
//!
//! - 응시 앱: auth 공개 클라이언트(CIMD, PKCE, 루프백)로 받은 액세스 토큰을 `Authorization: Bearer`로 보낸다.
//! - 대시보드: 이 서버가 기밀 클라이언트로 코드를 교환하고([`login`]) 세션 쿠키를 준다. 액세스 토큰은
//!   검증에만 쓰고 저장하지 않는다(auth 문서 7절).
//!
//! [`require_account`] 미들웨어가 둘 중 하나로 [`Account`]를 요청에 붙인다. 토큰 규칙은 [`token`].
//!
//! 기계 토큰(스모크, #28)은 「스모크」 조직의 응시자로만 행동한다: [`machine_may`]가 허용한 경로(배정 목록·문항
//! 시작·제출·자기 제출 조회) 밖은 조직 경로면 404, 나머지는 403.

pub mod login;
pub mod token;

use std::sync::Arc;

use axum::Json;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use promptlab_shared::api::ErrorResponse;
use promptlab_store::accounts;
use sha2::{Digest, Sha256};

use crate::AppState;
use token::{AccountKind, Rejection, TokenValidator, bearer_token};

/// API 전체가 요구하는 스코프(리소스 `…/api`에 속하고 auth에서 「일반 사용자 허용」이 켜져 있어야 한다).
pub const API_SCOPE: &str = "promptlab";
/// 대시보드 세션 쿠키 이름.
pub const SESSION_COOKIE: &str = "pl_session";
/// 대시보드 세션 수명(12시간 — auth 문서 7절 「12시간 이하」).
pub const SESSION_SECS: i64 = 12 * 60 * 60;

/// 인증 설정. 값의 유래는 `main.rs`(환경변수) 참고.
#[derive(Clone)]
pub struct AuthConfig {
    /// 이 서비스의 공개 원점(예: `https://promptlab.sandcorp.co.kr`, 끝 `/` 없음).
    pub public_url: String,
    /// sandcorp-auth 원점(예: `https://auth.sandcorp.co.kr`). `iss`이자 끝점들의 바탕.
    pub auth_url: String,
    pub validator: Arc<TokenValidator>,
    /// 대시보드 기밀 클라이언트. 없으면 대시보드 로그인이 `503`.
    pub dashboard_client: Option<login::DashboardClient>,
    /// 대시보드 코드 교환.
    pub token_endpoint: Arc<dyn login::TokenEndpoint>,
}

impl AuthConfig {
    /// 이 서비스의 리소스 URL(토큰 `aud`).
    pub fn resource(public_url: &str) -> String {
        format!("{public_url}/api")
    }

    /// RFC 9728 보호 리소스 메타데이터 URL(리소스 경로 `/api`가 뒤에 붙는다).
    pub fn resource_metadata_url(&self) -> String {
        format!(
            "{}/.well-known/oauth-protected-resource/api",
            self.public_url
        )
    }

    /// 대시보드 로그인 콜백(auth에 등록하는 값과 문자 하나까지 같아야 한다).
    pub fn redirect_uri(&self) -> String {
        format!("{}/api/auth/callback", self.public_url)
    }

    /// 응시 앱 공개 클라이언트 id = 이 서버가 내보내는 CIMD 문서 URL.
    pub fn app_client_id(&self) -> String {
        format!("{}/api/oauth/app-client.json", self.public_url)
    }

    pub fn authorization_endpoint(&self) -> String {
        format!("{}/authorize", self.auth_url)
    }

    pub fn token_endpoint_url(&self) -> String {
        format!("{}/token", self.auth_url)
    }
}

/// 로그인한 사람. 핸들러는 `Extension<Account>`로 받는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// sandcorp-auth `sub`(일반 사용자는 `m_`+ULID, 대표는 username).
    pub sub: String,
    pub kind: AccountKind,
}

/// 인증 실패 응답(auth 문서 5절의 `WWW-Authenticate` 그대로, 본문에는 이유를 싣지 않는다).
pub enum AuthFailure {
    Missing,
    Invalid,
    InsufficientScope,
    /// 세션 쿠키로 상태를 바꾸는 요청인데 `Origin`이 이 서비스가 아니다(CSRF).
    CrossOrigin,
}

impl AuthFailure {
    pub fn into_response(self, config: &AuthConfig) -> Response {
        let metadata = config.resource_metadata_url();
        let (status, www, message) = match self {
            AuthFailure::Missing => (
                StatusCode::UNAUTHORIZED,
                Some(format!("Bearer resource_metadata=\"{metadata}\"")),
                "로그인이 필요하다",
            ),
            AuthFailure::Invalid => (
                StatusCode::UNAUTHORIZED,
                Some(format!(
                    "Bearer resource_metadata=\"{metadata}\", error=\"invalid_token\""
                )),
                "로그인이 필요하다",
            ),
            AuthFailure::InsufficientScope => (
                StatusCode::FORBIDDEN,
                Some(format!(
                    "Bearer error=\"insufficient_scope\", scope=\"{API_SCOPE}\""
                )),
                "권한이 없다",
            ),
            AuthFailure::CrossOrigin => (StatusCode::FORBIDDEN, None, "다른 출처의 요청이다"),
        };
        let mut response = (
            status,
            Json(ErrorResponse {
                error: message.into(),
            }),
        )
            .into_response();
        if let Some(www) = www.and_then(|w| HeaderValue::from_str(&w).ok()) {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, www);
        }
        response
    }
}

/// 무작위 비밀(32바이트, base64url 43자).
pub fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS 난수를 얻지 못했다");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// DB에 두는 비밀의 해시.
pub fn secret_hash(secret: &str) -> accounts::SecretHash {
    Sha256::digest(secret.as_bytes()).into()
}

/// 요청 쿠키 하나의 값.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// HttpOnly·Secure·SameSite=Lax 쿠키. `max_age` 0이면 지운다.
pub fn set_cookie(name: &str, value: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}"
    ))
    .expect("쿠키 값은 base64url이다")
}

/// `Authorization: Bearer` 토큰만으로 계정을 얻는다(세션 쿠키는 보지 않는다 — MCP(#17)가 이것만 쓴다).
/// 헤더가 없거나 Bearer가 아니면 「토큰 없음」(B1·B2).
pub async fn bearer_account(
    config: &AuthConfig,
    headers: &HeaderMap,
) -> Result<Account, AuthFailure> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(token) = bearer_token(authorization) else {
        return Err(AuthFailure::Missing);
    };
    match config.validator.validate(token, API_SCOPE).await {
        Ok(verified) => Ok(Account {
            sub: verified.sub,
            kind: verified.kind,
        }),
        Err(Rejection::InvalidToken(rule)) => {
            tracing::info!(rule, "토큰 거부");
            Err(AuthFailure::Invalid)
        }
        Err(Rejection::InsufficientScope) => {
            tracing::info!("토큰 거부: 스코프 부족");
            Err(AuthFailure::InsufficientScope)
        }
    }
}

/// 토큰 또는 세션 쿠키로 [`Account`]를 붙인다. 둘 다 없으면 401.
///
/// - `Authorization` 헤더가 있으면 그것만 본다(B1·B2: Bearer가 아니면 「토큰 없음」).
/// - 없으면 세션 쿠키. 쿠키로 상태를 바꾸는 요청(GET·HEAD 밖)은 `Origin`이 이 서비스여야 한다.
pub async fn require_account(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let config = &state.auth;
    let headers = request.headers();
    let account = if headers.contains_key(header::AUTHORIZATION) {
        match bearer_account(config, headers).await {
            Ok(account) => account,
            Err(failure) => return failure.into_response(config),
        }
    } else if let Some(session_id) = cookie(headers, SESSION_COOKIE) {
        let unsafe_method = !matches!(*request.method(), Method::GET | Method::HEAD);
        if unsafe_method {
            let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
            if origin != Some(config.public_url.as_str()) {
                return AuthFailure::CrossOrigin.into_response(config);
            }
        }
        match accounts::session(&state.pool, &secret_hash(session_id)).await {
            Ok(Some(owner)) => Account {
                sub: owner.sub,
                kind: if owner.is_owner {
                    AccountKind::Owner
                } else {
                    AccountKind::Member
                },
            },
            Ok(None) => return AuthFailure::Invalid.into_response(config),
            Err(e) => return crate::ApiError::from(e).into_response(),
        }
    } else {
        return AuthFailure::Missing.into_response(config);
    };
    if account.kind == AccountKind::Machine {
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map(|p| p.as_str().to_owned())
            .unwrap_or_default();
        match machine_may(request.method(), &route) {
            MachineAccess::Allowed => {}
            MachineAccess::NotFound => {
                return crate::ApiError::new(StatusCode::NOT_FOUND, "그런 조직이 없다")
                    .into_response();
            }
            MachineAccess::Forbidden => {
                tracing::info!(route, "기계 토큰 거부: 허용 경로 밖");
                return crate::ApiError::new(StatusCode::FORBIDDEN, "기계 계정은 할 수 없다")
                    .into_response();
            }
        }
    }
    request.extensions_mut().insert(account);
    next.run(request).await
}

/// 기계 주체가 그 경로를 부를 수 있는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineAccess {
    Allowed,
    /// 조직 경로 — 구성원이 아닌 사람과 같이 「없다」.
    NotFound,
    Forbidden,
}

/// 기계 주체(스모크)의 허용 경로. `route`는 axum의 경로 틀(`/api` 접두는 떼고 본다).
/// 응시자로서 필요한 것만: 배정 목록(이때 스모크 배정이 준비된다)·문항 시작·과정 로그 올리기(#9)·제출·자기 제출 조회.
pub fn machine_may(method: &Method, route: &str) -> MachineAccess {
    let route = route.strip_prefix("/api").unwrap_or(route);
    let allowed = match *method {
        Method::GET => matches!(route, "/me/assignments" | "/submissions/{id}"),
        Method::POST => matches!(
            route,
            "/assignments/{id}/items/{n}/start"
                | "/assignments/{id}/items/{n}/submit"
                | "/assignments/{id}/items/{n}/log/keys"
                | "/assignments/{id}/items/{n}/log"
        ),
        _ => false,
    };
    if allowed {
        MachineAccess::Allowed
    } else if route.starts_with("/orgs/") {
        MachineAccess::NotFound
    } else {
        MachineAccess::Forbidden
    }
}
