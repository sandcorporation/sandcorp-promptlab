//! 오류 수집(GlitchTip, sentry 호환) — 서버·워커가 같이 쓴다. 앱·대시보드는 이 크레이트에 기대지 않는다.
//!
//! - DSN env `SENTRY_DSN`이 비었거나 없으면 SDK를 켜지 않는다(dev·CI·시험은 조용하다).
//! - `release` = env `SENTRY_RELEASE`(배포 커밋 sha), `environment` = `production`, PII 없음, 성능 추적 없음.
//! - `tracing`의 **error 수준만** 이벤트가 된다. warn·info는 breadcrumb로도 보내지 않는다 — 그 줄들은
//!   숨은 입력·이메일 같은 값을 싣지 않도록 훑었지만(PR 본문), error 밖은 훑는 대상이 아니라 아예 나가지 않게 한다.
//! - 이벤트는 [`scrub_event`]를 거친다: 요청의 쿠키·인증 헤더·query string을 지우고 문자열 속 토큰 모양을 가린다.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;
use sentry::ClientInitGuard;
use sentry::protocol::Event;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub const DSN_ENV: &str = "SENTRY_DSN";
pub const RELEASE_ENV: &str = "SENTRY_RELEASE";
pub const REDACTED: &str = "<REDACTED>";
/// 지우는 요청 헤더(소문자).
const SENSITIVE_HEADERS: [&str; 6] = [
    "cookie",
    "set-cookie",
    "authorization",
    "x-api-key",
    "x-sandcorp-signature",
    "proxy-authorization",
];

static TOKEN_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // JWT
        r"eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]*",
        // Authorization 값
        r"(?i)bearer\s+[A-Za-z0-9._~+/=-]+",
        // Anthropic 키
        r"sk-ant-[A-Za-z0-9_-]+",
        // 긴 hex (해시·키)
        r"\b[0-9a-fA-F]{32,}\b",
        // 긴 base64/base64url
        r"[A-Za-z0-9+/_-]{40,}={0,2}",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("정규식"))
    .collect()
});

/// 문자열 속 토큰 모양을 `<REDACTED>`로 바꾼다.
pub fn redact_str(s: &str) -> String {
    let mut out: Cow<str> = Cow::Borrowed(s);
    for re in TOKEN_PATTERNS.iter() {
        if re.is_match(&out) {
            out = Cow::Owned(re.replace_all(&out, REDACTED).into_owned());
        }
    }
    out.into_owned()
}

fn redact_value(v: &mut sentry::protocol::Value) {
    use sentry::protocol::Value;
    match v {
        Value::String(s) => *s = redact_str(s),
        Value::Array(a) => a.iter_mut().for_each(redact_value),
        Value::Object(o) => o.values_mut().for_each(redact_value),
        _ => {}
    }
}

/// `before_send`: 순수 함수. 항상 이벤트를 돌려준다(걸러낸 꼴로).
pub fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    if let Some(req) = event.request.as_mut() {
        req.headers
            .retain(|k, _| !SENSITIVE_HEADERS.contains(&k.to_ascii_lowercase().as_str()));
        req.cookies = None;
        req.query_string = None;
        req.data = None;
        if let Some(url) = req.url.as_mut() {
            // query string·fragment는 지운다(토큰·이메일이 실릴 수 있다).
            url.set_query(None);
            url.set_fragment(None);
        }
    }
    if let Some(m) = event.message.as_mut() {
        *m = redact_str(m);
    }
    if let Some(l) = event.logentry.as_mut() {
        l.message = redact_str(&l.message);
        l.params.iter_mut().for_each(redact_value);
    }
    for ex in event.exception.values.iter_mut() {
        if let Some(v) = ex.value.as_mut() {
            *v = redact_str(v);
        }
    }
    for b in event.breadcrumbs.values.iter_mut() {
        if let Some(m) = b.message.as_mut() {
            *m = redact_str(m);
        }
        b.data.values_mut().for_each(redact_value);
    }
    event.extra.values_mut().for_each(redact_value);
    event.tags.values_mut().for_each(|v| *v = redact_str(v));
    Some(event)
}

/// SDK 옵션. 시험이 같은 옵션을 쓴다.
pub fn client_options(release: Option<String>) -> sentry::ClientOptions {
    // ClientOptions 는 non_exhaustive 라 기본값에서 고쳐 쓴다.
    let mut o = sentry::ClientOptions::default();
    o.release = release.map(Into::into);
    o.environment = Some("production".into());
    o.send_default_pii = false;
    // 성능 추적은 기본값(표본 없음 = 0)을 그대로 둔다(GlitchTip 저장량).
    o.attach_stacktrace = true;
    o.before_send = Some(Arc::new(scrub_event));
    // 요청 본문은 싣지 않는다.
    o.max_request_body_size = sentry::MaxRequestBodySize::None;
    o
}

/// DSN이 비었거나 없으면 `None`(SDK를 켜지 않는다). 있으면 guard — `main`이 끝날 때까지 쥔다.
pub fn init_with(dsn: Option<&str>, release: Option<String>) -> Option<ClientInitGuard> {
    let dsn = dsn.map(str::trim).filter(|d| !d.is_empty())?;
    // 잘못된 DSN을 그대로 init에 넘기면 panic 한다 — 서버가 죽지 않게 먼저 파싱한다(값은 출력하지 않는다).
    let dsn: sentry::types::Dsn = dsn.parse().ok()?;
    let guard = sentry::init((dsn, client_options(release)));
    // DSN이 잘못돼 클라이언트가 안 만들어졌으면 켜진 게 아니다.
    guard.is_enabled().then_some(guard)
}

/// env `SENTRY_DSN`·`SENTRY_RELEASE`로 켠다. 잘못된 DSN 값은 출력하지 않는다.
pub fn init_from_env() -> Option<ClientInitGuard> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    init_with(env(DSN_ENV).as_deref(), env(RELEASE_ENV))
}

/// 로그 출력(fmt) + error만 이벤트로 보내는 sentry 레이어. SDK가 꺼져 있으면 레이어는 아무것도 하지 않는다.
/// breadcrumb·로그 레이어는 두지 않는다(warn·info는 나가지 않는다).
pub fn init_tracing() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .with(sentry_tracing::ErrorLayer::new().with_filter(LevelFilter::ERROR))
        .init();
}

/// 연동 시험용 오류. Debug 출력 첫 단어가 타입 이름이라 GlitchTip에 `GlitchTipOnboardingTest`로 보인다.
#[derive(Debug)]
pub struct GlitchTipOnboardingTest(pub String);

impl std::fmt::Display for GlitchTipOnboardingTest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GlitchTipOnboardingTest {}

/// 한국 시간(UTC+9) 날짜 `YYYY-MM-DD`.
pub fn kst_date(unix_secs: u64) -> String {
    let days = ((unix_secs + 9 * 3600) / 86_400) as i64;
    // Howard Hinnant의 civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// 시험 오류 하나를 보내고 5초 안에 flush한다. 보냈으면 `true`.
pub fn send_onboarding_test(service: &str, unix_secs: u64) -> bool {
    let err = GlitchTipOnboardingTest(format!("연동 시험 {service} {}", kst_date(unix_secs)));
    sentry::capture_error(&err);
    sentry::Hub::current()
        .client()
        .is_some_and(|c| c.flush(Some(Duration::from_secs(5))))
}
