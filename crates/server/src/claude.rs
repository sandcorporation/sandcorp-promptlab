//! Anthropic Messages API 호출(#15, AI 리포트). 공식 Rust SDK가 없어 reqwest로 `POST /v1/messages`를 직접 부른다.
//!
//! - 호출은 [`ReportModel`] 트레잇 뒤에 있다 — 서버는 [`AnthropicClient`], 시험은 가짜를 넣는다. 트레잇은 **보낼 요청
//!   본문 JSON 그대로**를 받으므로 가짜가 본 것이 곧 Anthropic에 나갈 것이다(숨은 입력 불변식 시험이 이것을 본다).
//! - 키는 env [`KEY_ENV`]. 없으면 리포트 기능만 꺼진다([`AnthropicClient::from_env`]가 `None`).
//! - 키 값은 `x-api-key` 헤더에만 싣는다. [`ClaudeError`]는 상태 코드·분류만 담고 응답 본문을 싣지 않으며,
//!   [`AnthropicClient`]의 `Debug`도 키를 가린다 — 로그·API 응답·오류 메시지에 키가 나가지 않는다(시험).
//! - 요청마다 시간 제한([`REQUEST_TIMEOUT`]), 일시 오류(연결·시간 초과·408·409·429·5xx)는 [`MAX_ATTEMPTS`]번까지
//!   지수 백오프로 다시 한다. 조직당 동시 호출 한도는 리포트 큐가 건다([`crate::reports`]).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use promptlab_shared::report::TokenUsage;
use serde_json::Value;

/// 키를 읽는 env 이름.
pub const KEY_ENV: &str = "PROMPTLAB_ANTHROPIC_API_KEY";
/// Anthropic API 기본 주소.
pub const API_URL: &str = "https://api.anthropic.com";
/// `anthropic-version` 헤더.
pub const API_VERSION: &str = "2026-10-21";
/// 요청 하나의 시간 제한(사고 포함 응답 하나를 기다린다).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// 일시 오류 때 시도 횟수(처음 포함).
pub const MAX_ATTEMPTS: u32 = 3;

/// 응답 하나.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelReply {
    /// `text` 블록들을 이은 것(사고 블록은 뺀다).
    pub text: String,
    /// 실제로 답한 모델.
    pub model: String,
    /// `end_turn`·`max_tokens`·`refusal` …
    pub stop_reason: String,
    pub usage: TokenUsage,
}

/// 호출 실패. 담당자에게 보이는 짧은 말만 담는다 — 응답 본문·키·요청 본문은 싣지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeError {
    #[error("AI 호출 시간 초과")]
    Timeout,
    #[error("AI 서비스에 연결하지 못했다")]
    Network,
    #[error("AI 서비스 오류({0})")]
    Status(u16),
    #[error("AI 응답을 읽지 못했다")]
    BadResponse,
}

impl ClaudeError {
    /// 다시 해 볼 만한가.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Timeout | Self::Network => true,
            Self::Status(s) => matches!(s, 408 | 409 | 429) || *s >= 500,
            Self::BadResponse => false,
        }
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Messages API 호출 하나. `body`는 `POST /v1/messages` 요청 본문 그대로다.
pub trait ReportModel: Send + Sync {
    fn complete<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<ModelReply, ClaudeError>>;
}

/// 실제 Anthropic API 클라이언트.
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    key: String,
    /// 첫 재시도 전 대기(다음은 두 배씩).
    backoff: Duration,
}

impl fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url)
            .field("key", &"***")
            .finish()
    }
}

impl AnthropicClient {
    /// env [`KEY_ENV`]에서 키를 읽는다. 없거나 비었으면 `None`(리포트 기능 꺼짐).
    pub fn from_env() -> Option<Self> {
        let key = std::env::var(KEY_ENV).ok()?;
        let key = key.trim();
        (!key.is_empty()).then(|| Self::new(key, API_URL, Duration::from_secs(2)))
    }

    /// 주소·첫 재시도 대기를 정해 만든다(시험은 가짜 서버 주소와 짧은 대기).
    pub fn new(key: &str, base_url: &str, backoff: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            backoff,
        }
    }

    async fn once(&self, body: &Value) -> Result<ModelReply, ClaudeError> {
        let response = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ClaudeError::Timeout
                } else {
                    ClaudeError::Network
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClaudeError::Status(status.as_u16()));
        }
        let value: Value = response.json().await.map_err(|e| {
            if e.is_timeout() {
                ClaudeError::Timeout
            } else {
                ClaudeError::BadResponse
            }
        })?;
        parse_reply(&value).ok_or(ClaudeError::BadResponse)
    }
}

impl ReportModel for AnthropicClient {
    fn complete<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<ModelReply, ClaudeError>> {
        Box::pin(async move {
            let mut wait = self.backoff;
            let mut attempt = 1;
            loop {
                match self.once(body).await {
                    Err(e) if e.retryable() && attempt < MAX_ATTEMPTS => {
                        tracing::warn!(attempt, error = %e, "AI 호출 일시 오류 — 다시 한다");
                        tokio::time::sleep(wait).await;
                        wait *= 2;
                        attempt += 1;
                    }
                    other => return other,
                }
            }
        })
    }
}

fn count(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Messages API 응답 JSON에서 텍스트·모델·멈춘 까닭·토큰을 꺼낸다.
pub fn parse_reply(value: &Value) -> Option<ModelReply> {
    let content = value.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<String>();
    let usage = value.get("usage")?;
    Some(ModelReply {
        text,
        model: value.get("model")?.as_str()?.to_owned(),
        stop_reason: value
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        usage: TokenUsage {
            input_tokens: count(usage, "input_tokens"),
            output_tokens: count(usage, "output_tokens"),
            cache_read_tokens: count(usage, "cache_read_input_tokens"),
            cache_write_tokens: count(usage, "cache_creation_input_tokens"),
        },
    })
}
