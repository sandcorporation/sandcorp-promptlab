//! 서버 API 클라이언트와 설정. 앱은 공개 문제([`PublicCodingProblem`])와 채점 리포트만 받는다.
//!
//! 인증은 sandcorp-auth 계정이다([`crate::auth`]). 처음 API를 부를 때 브라우저로 로그인하고, 이후에는
//! 토큰을 갈아 끼우며 쓴다. `PROMPTLAB_ACCESS_TOKEN`을 주면 그 토큰을 그대로 쓴다(개발·끝에서 끝 확인용).

use crate::webcam::Photo;
use promptlab_shared::webcam::{
    ConsentRequest, NOTICE_VERSION, SnapshotReason, SnapshotStored, WebcamTerms,
};
use std::sync::Arc;

use promptlab_shared::api::{
    AppAuthConfig, AssignmentView, CreateSubmissionRequest, ErrorResponse, ItemStarted, LogHead,
    RegisterLogKeyRequest, StartItemRequest, SubmissionView, SubmitItemRequest, UploadLogRequest,
};
use promptlab_shared::live::{LiveRequest, PostSignal, SignalBatch};
use promptlab_shared::log::SignedEvent;
use promptlab_shared::practice;
use tokio::sync::Mutex;

use crate::auth::{self, AuthError, Tokens};
use crate::session::Submission;
use promptlab_shared::coding::{Language, PublicCodingProblem};
use serde::de::DeserializeOwned;

/// 앱 설정(환경변수).
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// `PROMPTLAB_SERVER_URL`(기본: release는 운영 주소, 디버그는 `http://127.0.0.1:8080` — [`default_server_url`]).
    pub server_url: String,
    /// `PROMPTLAB_ACCESS_TOKEN`(선택) — 이미 받은 액세스 토큰. 없으면 브라우저로 로그인한다.
    pub access_token: Option<String>,
    /// `PROMPTLAB_PROBLEM_ID`(처음 고르는 연습 문제, 기본 [`practice::DEFAULT_PROBLEM_ID`]).
    pub problem_id: String,
}

impl std::fmt::Debug for Config {
    // 토큰은 로그·화면에 찍지 않는다.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("server_url", &self.server_url)
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<숨김>"),
            )
            .field("problem_id", &self.problem_id)
            .finish()
    }
}

/// 운영 서버 주소(서버는 같은 경로를 `/`와 `/api`에서 받는다 — 로그인 리소스·CIMD client_id가 `…/api` 기준이라 `/api`).
pub const PRODUCTION_SERVER_URL: &str = "https://promptlab.sandcorp.co.kr/api";
/// 개발 기본값(`cargo run`·compose 개발 스택·시험).
pub const DEV_SERVER_URL: &str = "http://127.0.0.1:8080";

/// 실행 env가 없을 때의 서버 주소. release는 컴파일 때 굽는 `PROMPTLAB_DEFAULT_SERVER_URL`(없으면 운영 주소),
/// 디버그는 개발 주소 — 로컬 개발 흐름이 운영 서버로 새지 않게.
pub fn default_server_url(release: bool, baked: Option<&str>) -> String {
    if !release {
        return DEV_SERVER_URL.into();
    }
    baked
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(PRODUCTION_SERVER_URL)
        .into()
}

impl Config {
    pub fn from_env() -> Result<Self, ClientError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// 시험용: 환경변수 대신 `lookup`에서 읽는다.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ClientError> {
        let get = |k: &str| lookup(k).filter(|v| !v.trim().is_empty());
        Ok(Self {
            server_url: get("PROMPTLAB_SERVER_URL")
                .unwrap_or_else(|| {
                    default_server_url(
                        !cfg!(debug_assertions),
                        option_env!("PROMPTLAB_DEFAULT_SERVER_URL"),
                    )
                })
                .trim_end_matches('/')
                .to_owned(),
            access_token: get("PROMPTLAB_ACCESS_TOKEN"),
            problem_id: get("PROMPTLAB_PROBLEM_ID")
                .unwrap_or_else(|| practice::DEFAULT_PROBLEM_ID.into()),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("설정 오류: {0}")]
    Config(String),
    #[error("서버에 닿지 못했다: {0}")]
    Network(#[from] reqwest::Error),
    #[error("서버가 거절했다({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("로그인하지 못했다: {0}")]
    Login(#[from] AuthError),
    /// 브라우저 로그인 없이는 부를 수 없다(뒤에서 도는 로그 재전송은 로그인 창을 띄우지 않는다).
    #[error("로그인이 필요하다")]
    NeedsLogin,
}

/// 브라우저로 주소를 여는 방법. 시험은 브라우저 대신 루프백을 직접 부른다.
pub type BrowserOpener = Arc<dyn Fn(&str) + Send + Sync>;

/// promptlab 서버 API.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    config: Config,
    tokens: Arc<Mutex<Option<Tokens>>>,
    browser: BrowserOpener,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ApiClient {
    pub fn new(config: Config) -> Result<Self, ClientError> {
        Self::with_browser(config, Arc::new(auth::open_browser))
    }

    pub fn with_browser(config: Config, browser: BrowserOpener) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            config,
            tokens: Arc::new(Mutex::new(None)),
            browser,
        })
    }

    /// 쓸 액세스 토큰. 없으면 브라우저로 로그인하고, 곧 만료되면 리프레시한다.
    pub async fn access_token(&self) -> Result<String, ClientError> {
        if let Some(token) = &self.config.access_token {
            return Ok(token.clone());
        }
        let mut tokens = self.tokens.lock().await;
        if let Some(current) = tokens.as_ref().filter(|t| t.fresh()) {
            return Ok(current.access_token.clone());
        }
        let auth_config = self.auth_config().await?;
        let refreshed = match tokens.as_ref().and_then(|t| t.refresh_token.clone()) {
            Some(refresh_token) => auth::refresh(&self.http, &auth_config, &refresh_token)
                .await
                .ok(),
            None => None,
        };
        let next = match refreshed {
            Some(t) => t,
            None => {
                let pending = auth::begin(&auth_config).await?;
                (self.browser)(pending.authorize_url());
                pending.finish(&self.http, &auth_config).await?
            }
        };
        let access = next.access_token.clone();
        *tokens = Some(next);
        Ok(access)
    }

    /// 로그아웃(메모리의 토큰을 버린다). 다음 API 호출에서 다시 로그인한다.
    pub async fn forget_login(&self) {
        *self.tokens.lock().await = None;
    }

    async fn auth_config(&self) -> Result<AppAuthConfig, ClientError> {
        let url = format!("{}/auth/app-config", self.config.server_url);
        let response = self.http.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Rejected {
                status: status.as_u16(),
                message: "로그인 설정을 받지 못했다".into(),
            });
        }
        Ok(response.json().await?)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 연습 문제 하나(공개 보기).
    pub async fn problem(&self, id: &str) -> Result<PublicCodingProblem, ClientError> {
        let url = format!("{}/problems/{id}", self.config.server_url);
        self.send(self.http.get(url)).await
    }

    /// 연습 문제 목록(#18).
    pub async fn practice_problems(&self) -> Result<Vec<PublicCodingProblem>, ClientError> {
        let url = format!("{}/practice/problems", self.config.server_url);
        self.send(self.http.get(url)).await
    }

    pub async fn submit(
        &self,
        problem_id: &str,
        submission: &Submission,
    ) -> Result<SubmissionView, ClientError> {
        let url = format!("{}/submissions", self.config.server_url);
        let body = CreateSubmissionRequest {
            problem_id: problem_id.into(),
            language: submission.language,
            code: submission.code.clone(),
            elapsed_secs: submission.elapsed_secs,
            org_id: None,
        };
        self.send(self.http.post(url).json(&body)).await
    }

    /// 내게 배정된 시험들.
    pub async fn assignments(&self) -> Result<Vec<AssignmentView>, ClientError> {
        let url = format!("{}/me/assignments", self.config.server_url);
        self.send(self.http.get(url)).await
    }

    /// 배정된 시험의 문항 응시를 시작한다(서버가 시작 시각을 남기고 문제·마감을 준다).
    /// 앱 판·모델 sha256을 함께 보내고, 서버가 허용 목록에 없으면 거부한다(`403`, #12).
    pub async fn start_item(
        &self,
        assignment_id: &str,
        position: u32,
        integrity: &StartItemRequest,
    ) -> Result<ItemStarted, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/items/{position}/start",
            self.config.server_url
        );
        self.send(self.http.post(url).json(integrity)).await
    }

    /// 웹캠·수집 항목 안내에 동의한다(#13). 동의 없이는 서버가 문항 시작을 거부한다.
    pub async fn consent(&self, assignment_id: &str) -> Result<WebcamTerms, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/consent",
            self.config.server_url
        );
        let body = ConsentRequest {
            notice_version: NOTICE_VERSION.to_owned(),
        };
        self.send(self.http.post(url).json(&body)).await
    }

    /// 기준 얼굴 사진을 올린다(첫 문항 시작 전까지 바꿀 수 있다).
    pub async fn put_baseline(
        &self,
        assignment_id: &str,
        photo: &Photo,
    ) -> Result<WebcamTerms, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/webcam/baseline",
            self.config.server_url
        );
        self.send(self.jpeg(self.http.put(url), photo)).await
    }

    /// 스냅샷 한 장을 올린다(시작한 문항). 같은 참조는 서버가 한 번만 저장한다.
    /// 뒤에서 도는 일이라 로그인 창을 띄우지 않는다.
    pub async fn upload_snapshot(
        &self,
        assignment_id: &str,
        position: u32,
        reason: SnapshotReason,
        client_ref: &str,
        at_ms: i64,
        photo: &Photo,
    ) -> Result<SnapshotStored, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/items/{position}/snapshots",
            self.config.server_url
        );
        let request = self.http.post(url).query(&[
            ("reason", reason.as_str().to_owned()),
            ("ref", client_ref.to_owned()),
            ("at_ms", at_ms.to_string()),
        ]);
        self.send_quiet(self.jpeg(request, photo)).await
    }

    /// 사진 한 장(JPEG)을 본문으로. [`Photo`]만 받으니 영상은 실을 수 없다.
    fn jpeg(&self, request: reqwest::RequestBuilder, photo: &Photo) -> reqwest::RequestBuilder {
        request
            .header(reqwest::header::CONTENT_TYPE, "image/jpeg")
            .body(photo.bytes().to_vec())
    }

    /// 배정된 시험의 문항을 낸다: 언어·코드와 이 문항에 쓴 사건 수(`log_seq`). 서버가 과정 로그의 마지막 `Submitted`
    /// 사건과 맞춰 본다 — 로그가 덜 올라갔으면 `409`(로그를 올리고 다시 낸다, [`crate::exam::submit`]).
    /// 소요 시간은 서버가 잰다.
    pub async fn submit_item(
        &self,
        assignment_id: &str,
        position: u32,
        language: Language,
        code: &str,
        log_seq: u64,
    ) -> Result<SubmissionView, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/items/{position}/submit",
            self.config.server_url
        );
        let body = SubmitItemRequest {
            language,
            code: code.to_owned(),
            log_seq,
        };
        self.send(self.http.post(url).json(&body)).await
    }

    /// 과정 로그: 이 설치의 공개키를 문항에 등록하고 서버가 아는 체인 머리를 받는다.
    /// 뒤에서 도는 일이라 로그인 창을 띄우지 않는다(로그인이 없으면 [`ClientError::NeedsLogin`]).
    pub async fn register_log_key(
        &self,
        assignment_id: &str,
        position: u32,
        public_key: &str,
    ) -> Result<LogHead, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/items/{position}/log/keys",
            self.config.server_url
        );
        let body = RegisterLogKeyRequest {
            public_key: public_key.to_owned(),
        };
        self.send_quiet(self.http.post(url).json(&body)).await
    }

    /// 과정 로그: 서명한 사건을 순번대로 올린다(이미 받은 것은 서버가 건너뛴다). 로그인 창을 띄우지 않는다.
    pub async fn upload_log(
        &self,
        assignment_id: &str,
        position: u32,
        events: &[SignedEvent],
    ) -> Result<LogHead, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/items/{position}/log",
            self.config.server_url
        );
        let body = UploadLogRequest {
            events: events.to_vec(),
        };
        self.send_quiet(self.http.post(url).json(&body)).await
    }

    pub async fn submission(&self, id: &str) -> Result<SubmissionView, ClientError> {
        let url = format!("{}/submissions/{id}", self.config.server_url);
        self.send(self.http.get(url)).await
    }

    /// 감독관이 실시간 보기를 열었으면 세션(#14). 폴링이라 로그인 창을 띄우지 않는다.
    pub async fn live_request(&self, assignment_id: &str) -> Result<LiveRequest, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/live",
            self.config.server_url
        );
        self.send_quiet(self.http.get(url)).await
    }

    /// 감독관이 보낸 시그널링 메시지(`after` 뒤).
    pub async fn live_signals(
        &self,
        assignment_id: &str,
        session_id: &str,
        after: u64,
    ) -> Result<SignalBatch, ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/live/{session_id}/signals?after={after}",
            self.config.server_url
        );
        self.send_quiet(self.http.get(url)).await
    }

    /// 시그널링 메시지를 보낸다(offer·ICE 후보·bye).
    pub async fn post_signal(
        &self,
        assignment_id: &str,
        session_id: &str,
        signal: &PostSignal,
    ) -> Result<(), ClientError> {
        let url = format!(
            "{}/assignments/{assignment_id}/live/{session_id}/signals",
            self.config.server_url
        );
        let token = self.quiet_token().await?;
        let response = self
            .http
            .post(url)
            .json(signal)
            .bearer_auth(token)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let message = response
            .json::<ErrorResponse>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| status.to_string());
        Err(ClientError::Rejected {
            status: status.as_u16(),
            message,
        })
    }

    /// 로그인 창 없이 쓸 수 있는 토큰(설정 토큰, 아직 유효한 토큰, 또는 리프레시한 토큰).
    async fn quiet_token(&self) -> Result<String, ClientError> {
        if let Some(token) = &self.config.access_token {
            return Ok(token.clone());
        }
        let mut tokens = self.tokens.lock().await;
        if let Some(current) = tokens.as_ref().filter(|t| t.fresh()) {
            return Ok(current.access_token.clone());
        }
        let Some(refresh_token) = tokens.as_ref().and_then(|t| t.refresh_token.clone()) else {
            return Err(ClientError::NeedsLogin);
        };
        let auth_config = self.auth_config().await?;
        let next = auth::refresh(&self.http, &auth_config, &refresh_token).await?;
        let access = next.access_token.clone();
        *tokens = Some(next);
        Ok(access)
    }

    async fn send_quiet<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ClientError> {
        let token = self.quiet_token().await?;
        self.finish(request.bearer_auth(token)).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ClientError> {
        let token = self.access_token().await?;
        self.finish(request.bearer_auth(token)).await
    }

    async fn finish<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ClientError> {
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.json().await?);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // 토큰이 거부됐다(만료·탈퇴 등) — 다음 호출에서 다시 로그인한다.
            self.forget_login().await;
        }
        let message = response
            .json::<ErrorResponse>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| status.to_string());
        Err(ClientError::Rejected {
            status: status.as_u16(),
            message,
        })
    }
}
