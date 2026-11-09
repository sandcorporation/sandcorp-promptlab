//! 서버 바이너리. 환경변수:
//! - `DATABASE_URL`(필수): Postgres. 시작할 때 마이그레이션을 돌린다.
//! - `PROMPTLAB_ADDR`: 듣는 주소(기본 `127.0.0.1:8080`, 컨테이너에서는 `0.0.0.0:8080`).
//! - `PROMPTLAB_PUBLIC_URL`: 이 서비스의 공개 원점(기본 `https://promptlab.sandcorp.co.kr`). 리소스 URL(`…/api`,
//!   토큰 `aud`)·대시보드 로그인 콜백(`…/api/auth/callback`)·앱 클라이언트 id가 여기서 나온다.
//! - `PROMPTLAB_AUTH_URL`: sandcorp-auth 원점(기본 `https://auth.sandcorp.co.kr`). `iss`·JWKS·`/authorize`·`/token`.
//! - `PROMPTLAB_AUTH_OWNER_SUB`: 대표 계정 username(기본 `farrar142`, 토큰 규칙 A1).
//! - `PROMPTLAB_AUTH_CLIENT_ID`·`PROMPTLAB_AUTH_CLIENT_SECRET`: 대시보드 기밀 클라이언트. 비면 대시보드 로그인이 꺼진다
//!   (응시 앱 토큰은 그대로 받는다).
//! - `SENTRY_DSN`·`SENTRY_RELEASE`(선택): 오류 수집(GlitchTip). DSN이 없으면 꺼진다. 하위 명령 `glitchtip-test`는
//!   서버를 띄우지 않고 연동 시험 오류 하나를 보낸다.
//! - `PROMPTLAB_DASHBOARD_DIR`: 대시보드 wasm 산출물 폴더(`index.html`이 있어야 한다). 이미지는 `/opt/promptlab/dashboard`.
//!   없으면 API만 서빙한다.
//! - `PROMPTLAB_SMTP_HOST`·`_PORT`·`_USERNAME`·`_PASSWORD`, `PROMPTLAB_MAIL_FROM`: 응시 초대 메일(SMTP). HOST가 없으면
//!   발송이 꺼진다(초대는 만들어지고 대시보드에서 링크를 복사해 전한다). 설명은 `src/mail.rs`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use promptlab_server::auth::AuthConfig;
use promptlab_server::auth::login::{DashboardClient, HttpTokenEndpoint};
use promptlab_server::auth::token::{
    HttpJwksFetcher, SystemClock, TokenRules, TokenValidator, parse_client_ids,
};
use promptlab_server::mail::{self, Mailer, SmtpMailer};
use promptlab_server::{AppState, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 오류 수집(GlitchTip): SENTRY_DSN 이 없으면 꺼진다. guard 는 main 이 끝날 때까지 쥔다.
    let sentry = promptlab_observability::init_from_env();
    // 확인 명령: 서버를 띄우지 않고 시험 오류 하나를 보내 flush 한다(DSN 값은 출력하지 않는다).
    if std::env::args().nth(1).as_deref() == Some("glitchtip-test") {
        return glitchtip_test(sentry.is_some());
    }
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
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let public_url = env("PROMPTLAB_PUBLIC_URL")
        .unwrap_or_else(|| "https://promptlab.sandcorp.co.kr".into())
        .trim_end_matches('/')
        .to_owned();
    let auth_url = env("PROMPTLAB_AUTH_URL")
        .unwrap_or_else(|| "https://auth.sandcorp.co.kr".into())
        .trim_end_matches('/')
        .to_owned();
    let dashboard_client = match (
        env("PROMPTLAB_AUTH_CLIENT_ID"),
        env("PROMPTLAB_AUTH_CLIENT_SECRET"),
    ) {
        (Some(client_id), Some(client_secret)) => Some(DashboardClient {
            client_id,
            client_secret,
        }),
        _ => {
            tracing::warn!("PROMPTLAB_AUTH_CLIENT_ID·SECRET 이 없다 — 대시보드 로그인이 꺼진다");
            None
        }
    };
    // 실시간 감독(#14): TURN 공유 비밀이 없으면 실시간 보기만 꺼진다(값은 로그에 남기지 않는다).
    let turn = promptlab_server::turn::TurnConfig::from_lookup(env, &public_url)?;
    match &turn {
        Some(t) => tracing::info!(
            ttl_secs = t.ttl_secs(),
            "실시간 감독 켜짐(TURN 시간 제한 자격)"
        ),
        None => tracing::warn!("PROMPTLAB_TURN_SECRET 이 없다 — 실시간 감독(WebRTC)이 꺼진다"),
    }
    // 스모크 기계 클라이언트(#28). 없으면 기계 토큰은 전부 거부된다. client_id는 비밀이 아니지만 env로만 받는다.
    let smoke_clients = parse_client_ids(&env("PROMPTLAB_SMOKE_CLIENT_IDS").unwrap_or_default());
    tracing::info!(count = smoke_clients.len(), "스모크 기계 클라이언트");
    let validator = TokenValidator::new(
        TokenRules {
            issuer: auth_url.clone(),
            audience: AuthConfig::resource(&public_url),
            owner_sub: env("PROMPTLAB_AUTH_OWNER_SUB").unwrap_or_else(|| "farrar142".into()),
            machine_client_ids: smoke_clients.clone(),
        },
        Arc::new(HttpJwksFetcher::new(format!(
            "{auth_url}/.well-known/jwks.json"
        ))?),
        Arc::new(SystemClock),
    );
    let auth = AuthConfig {
        token_endpoint: Arc::new(HttpTokenEndpoint::new(format!("{auth_url}/token"))?),
        public_url,
        auth_url,
        validator: Arc::new(validator),
        dashboard_client,
    };
    tracing::info!(resource = %auth.validator.rules().audience, "인증 설정");
    let addr: SocketAddr = std::env::var("PROMPTLAB_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;

    let pool = promptlab_store::connect(&database_url).await?;
    let mailer: Arc<dyn Mailer> = match mail::config_from(env)? {
        Some(config) => {
            tracing::info!(host = %config.host, port = config.port, "초대 메일: SMTP");
            Arc::new(SmtpMailer::new(&config)?)
        }
        None => {
            tracing::warn!("PROMPTLAB_SMTP_HOST 가 없다 — 초대 메일이 꺼진다(링크 복사로 쓴다)");
            Arc::new(mail::Disabled)
        }
    };
    let dashboard_dir = env("PROMPTLAB_DASHBOARD_DIR")
        .map(PathBuf::from)
        .filter(|dir| {
            let ok = dir.join("index.html").is_file();
            if !ok {
                tracing::warn!(dir = %dir.display(), "대시보드 폴더에 index.html 이 없다 — API만 서빙한다");
            }
            ok
        });
    if let Some(dir) = &dashboard_dir {
        tracing::info!(dir = %dir.display(), "대시보드 서빙");
    }
    let webcam_retention_days = env("PROMPTLAB_WEBCAM_RETENTION_DAYS")
        .map(|v| {
            v.parse::<u32>()
                .ok()
                .filter(|d| *d > 0)
                .ok_or("PROMPTLAB_WEBCAM_RETENTION_DAYS 는 1 이상의 정수(일)여야 한다")
        })
        .transpose()?
        .unwrap_or(promptlab_shared::webcam::DEFAULT_RETENTION_DAYS);
    let clock: Arc<dyn promptlab_server::auth::token::Clock> = Arc::new(SystemClock);
    tokio::spawn(promptlab_server::webcam::purge_loop(
        pool.clone(),
        clock.clone(),
        webcam_retention_days,
    ));
    tracing::info!(webcam_retention_days, "웹캠 사진 보유 기간");
    // 데이터 보유·자동 삭제(#54): 한 시간마다(첫 주기는 바로) 기간이 지난 응시·연습 기록을 지운다.
    tokio::spawn(promptlab_server::retention::purge_loop(
        pool.clone(),
        clock.clone(),
    ));
    // 탈퇴 웹훅 비밀(쉼표로 여럿). 없으면 웹훅만 꺼진다. 값은 로그에 남기지 않는다.
    let webhook_secrets = promptlab_server::retention::WebhookSecrets::parse(
        &env("PROMPTLAB_AUTH_WEBHOOK_SECRETS").unwrap_or_default(),
    );
    if webhook_secrets.is_empty() {
        tracing::info!("탈퇴 웹훅 꺼짐 — PROMPTLAB_AUTH_WEBHOOK_SECRETS 가 없다");
    } else {
        tracing::info!(secrets = webhook_secrets.len(), "탈퇴 웹훅 켜짐");
    }
    // AI 리포트(#15): 키가 없으면 기능만 끈다(서버는 뜬다). 키 값은 로그에 남기지 않는다.
    let reports: Option<Arc<dyn promptlab_server::claude::ReportModel>> =
        promptlab_server::claude::AnthropicClient::from_env()
            .map(|c| Arc::new(c) as Arc<dyn promptlab_server::claude::ReportModel>);
    if reports.is_some() {
        tracing::info!(
            model = promptlab_shared::report::REPORT_MODEL,
            "AI 리포트 켜짐"
        );
    } else {
        tracing::info!(
            env = promptlab_server::claude::KEY_ENV,
            "AI 리포트 꺼짐 — API 키가 없다"
        );
    }
    let state = AppState {
        pool,
        auth,
        clock,
        mailer,
        dashboard_dir,
        webcam_retention_days,
        live: promptlab_server::live::Live::new(turn),
        reports: reports.clone(),
        webhook_secrets,
    };
    if let Some(model) = reports {
        tokio::spawn(promptlab_server::reports::report_loop(state.clone(), model));
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "promptlab 서버 시작");
    // sentry-tower: 요청마다 Hub 를 가르고(NewSentryLayer) 요청 정보를 이벤트에 붙인다(SentryHttpLayer — 본문은 안 싣는다).
    let app = router(state).layer(
        tower::ServiceBuilder::new()
            .layer(sentry_tower::NewSentryLayer::new_from_top())
            .layer(sentry_tower::SentryHttpLayer::new()),
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// `promptlab-server glitchtip-test` — DSN 없음이면 종료 코드 1, 보내지 못했으면 2.
fn glitchtip_test(enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !enabled {
        eprintln!("DSN 없음 — SENTRY_DSN 이 비었거나 올바르지 않다");
        std::process::exit(1);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    if promptlab_observability::send_onboarding_test("promptlab", now) {
        println!(
            "GlitchTipOnboardingTest 전송 완료(flush 5초 안에 성공) — 연동 시험 promptlab {}",
            promptlab_observability::kst_date(now)
        );
        Ok(())
    } else {
        eprintln!("전송 실패 — flush 5초 안에 보내지 못했다(네트워크·DSN 확인)");
        std::process::exit(2);
    }
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
