//! 데이터 보유·자동 삭제(#54). 규칙은 [`promptlab_shared::retention`], 저장·삭제 서비스는
//! [`promptlab_store::retention`].
//!
//! - 설정 API: 조직 기본값 `GET·PUT /orgs/{id}/retention`(보기는 구성원, 바꾸기는 구성원 관리 = 관리자),
//!   시험별 `GET·PUT /orgs/{id}/exams/{exam}/retention`(보기는 구성원, 바꾸기는 시험 관리 = 관리자·출제자).
//! - 자동 삭제: [`purge_loop`]가 한 시간마다 [`purge_once`] — 기간이 지난 배정과 1년 무활동 연습 기록을 지운다.
//!   로그에는 지운 건수만 남긴다.
//! - 탈퇴: sandcorp-auth `user.deleted` 웹훅([`withdrawal_webhook`], `POST /auth/webhook`)을 받으면 그 `sub`의
//!   개인 연습 기록과 대시보드 세션을 즉시 지운다. 비밀(`PROMPTLAB_AUTH_WEBHOOK_SECRETS`, 쉼표로 여럿 — 클라이언트마다
//!   하나)이 없으면 503(꺼짐). 시험 응시 데이터는 고객사 보관 기간대로 남는다(채용·진단 기록의 주인은 고객사, #55).

use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use promptlab_shared::account::Permission;
use promptlab_shared::retention::{
    DEFAULT_DAYS, Erased, MAX_DAYS, MIN_DAYS, PURGE_INTERVAL_SECS, RetentionView,
    SetRetentionRequest, effective_days, validate_days,
};
use promptlab_store::PgPool;
use promptlab_store::retention::{self as store, EraseTarget};
use serde::Deserialize;
use sha2::Sha256;

use crate::auth::Account;
use crate::auth::token::Clock;
use crate::exams::find_exam;
use crate::orgs::{membership, require};
use crate::{ApiError, AppState};

/// 주기 작업 한 번에 한 트랜잭션으로 지우는 배정 수(트랜잭션을 짧게).
const BATCH: i64 = 200;

fn check_days(days: Option<u32>) -> Result<(), ApiError> {
    match days {
        Some(d) => validate_days(d).map_err(|m| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, m)),
        None => Ok(()),
    }
}

fn view(
    org_days: Option<u32>,
    exam_days: Option<u32>,
    history: Vec<promptlab_shared::retention::RetentionChange>,
) -> RetentionView {
    RetentionView {
        org_days,
        exam_days,
        effective_days: effective_days(exam_days, org_days),
        default_days: DEFAULT_DAYS,
        min_days: MIN_DAYS,
        max_days: MAX_DAYS,
        history,
    }
}

/// `GET /orgs/{id}/retention`
pub async fn get_org_retention(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<RetentionView>, ApiError> {
    let org = membership(&state, &org_id, &account).await?;
    let days = store::org_days(&state.pool, org.org_id).await?;
    let history = store::history(&state.pool, org.org_id, None).await?;
    Ok(Json(view(days, None, history)))
}

/// `PUT /orgs/{id}/retention`
pub async fn set_org_retention(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Json(request): Json<SetRetentionRequest>,
) -> Result<Json<RetentionView>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageMembers).await?;
    check_days(request.days)?;
    store::set_org_days(
        &state.pool,
        org.org_id,
        request.days,
        &account.sub,
        state.clock.now(),
    )
    .await?;
    let history = store::history(&state.pool, org.org_id, None).await?;
    Ok(Json(view(request.days, None, history)))
}

/// `GET /orgs/{id}/exams/{exam}/retention`
pub async fn get_exam_retention(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
) -> Result<Json<RetentionView>, ApiError> {
    let org = membership(&state, &org_id, &account).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    exam_view(&state, org.org_id, exam.id).await.map(Json)
}

async fn exam_view(
    state: &AppState,
    org_id: uuid::Uuid,
    exam_id: uuid::Uuid,
) -> Result<RetentionView, ApiError> {
    let org_days = store::org_days(&state.pool, org_id).await?;
    let exam_days = store::exam_days(&state.pool, exam_id).await?;
    let history = store::history(&state.pool, org_id, Some(exam_id)).await?;
    Ok(view(org_days, exam_days, history))
}

/// `PUT /orgs/{id}/exams/{exam}/retention`
pub async fn set_exam_retention(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
    Json(request): Json<SetRetentionRequest>,
) -> Result<Json<RetentionView>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageExams).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    check_days(request.days)?;
    store::set_exam_days(
        &state.pool,
        org.org_id,
        exam.id,
        request.days,
        &account.sub,
        state.clock.now(),
    )
    .await?;
    exam_view(&state, org.org_id, exam.id).await.map(Json)
}

/// 기간이 지난 응시(배정)와 1년 무활동 연습 기록을 모두 지운다. 지운 건수(종류별).
pub async fn purge_once(pool: &PgPool, clock: &dyn Clock) -> Result<Erased, sqlx::Error> {
    let now = clock.now();
    let mut total = Erased::default();
    loop {
        let ids = store::expired_assignments(pool, now, BATCH).await?;
        if ids.is_empty() {
            break;
        }
        total.add(&store::erase(pool, EraseTarget::Assignments(&ids)).await?);
    }
    // 관리자 삭제 감사 기록(#55)도 보관 기간(3년)이 지나면 지운다. 개인 데이터가 아니라 건수에 넣지 않는다.
    let audits = promptlab_store::erasure::purge(pool, now).await?;
    if audits > 0 {
        tracing::info!(audits, "보관 기간이 지난 삭제 감사 기록 삭제");
    }
    loop {
        let subs = store::inactive_practitioners(pool, now, BATCH).await?;
        if subs.is_empty() {
            break;
        }
        for sub in &subs {
            total.add(&store::erase(pool, EraseTarget::PracticeOf(sub)).await?);
        }
    }
    Ok(total)
}

/// 한 시간마다 [`purge_once`](서버 `main`이 띄운다). 첫 주기는 띄우자마자 돈다.
pub async fn purge_loop(pool: PgPool, clock: Arc<dyn Clock>) {
    let mut tick = tokio::time::interval(Duration::from_secs(PURGE_INTERVAL_SECS));
    loop {
        tick.tick().await;
        match purge_once(&pool, clock.as_ref()).await {
            Ok(n) => tracing::info!(
                assignments = n.assignments,
                submissions = n.submissions,
                item_starts = n.item_starts,
                log_events = n.log_events,
                log_keys = n.log_keys,
                log_rejections = n.log_rejections,
                integrity_rejections = n.integrity_rejections,
                webcam_photos = n.webcam_photos,
                webcam_consents = n.webcam_consents,
                reports = n.reports,
                usage_unlinked = n.usage_unlinked,
                "보관 기간이 지난 데이터 삭제"
            ),
            Err(e) => tracing::warn!(error = %e, "보관 기간 삭제 실패 — 다음 주기에 다시"),
        }
    }
}

/// 탈퇴 웹훅 비밀(클라이언트마다 하나, 여럿). 밖으로 꺼내는 메서드가 없고 `Debug`도 가린다.
#[derive(Clone, Default)]
pub struct WebhookSecrets(Vec<String>);

impl std::fmt::Debug for WebhookSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebhookSecrets(<숨김 {}개>)", self.0.len())
    }
}

impl WebhookSecrets {
    /// 쉼표로 이은 비밀들(`PROMPTLAB_AUTH_WEBHOOK_SECRETS`). 빈 칸은 버린다.
    pub fn parse(list: &str) -> Self {
        Self(
            list.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 어느 비밀로든 `HMAC-SHA256(비밀, "<t>." + 본문)`이 `v1`(hex)과 같은가(상수 시간 비교, auth 문서 11절 W3).
    fn verifies(&self, t: &str, body: &[u8], v1: &[u8]) -> bool {
        self.0.iter().any(|secret| {
            let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
                return false;
            };
            mac.update(t.as_bytes());
            mac.update(b".");
            mac.update(body);
            mac.verify_slice(v1).is_ok()
        })
    }
}

/// 서명 시각 허용 차(초, W2).
pub const WEBHOOK_TOLERANCE_SECS: i64 = 300;

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[derive(Deserialize)]
struct WebhookEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    sub: Option<String>,
}

/// `POST /auth/webhook` — sandcorp-auth 탈퇴 알림(`user.deleted`, auth 문서 11절 W1~W5).
/// 처리는 멱등이라(이미 지운 것은 0건) 같은 `id`를 다시 받아도 결과가 같다 — W4의 처리한 `id` 기억을 따로 두지 않는다.
pub async fn withdrawal_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    if state.webhook_secrets.is_empty() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "탈퇴 웹훅이 꺼져 있다",
        ));
    }
    let bad = |m: &str| ApiError::new(StatusCode::BAD_REQUEST, m);
    // W1
    let header = headers
        .get("x-sandcorp-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad("서명 헤더 없음"))?;
    let mut t = None;
    let mut v1 = None;
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", value)) => t = Some(value),
            Some(("v1", value)) => v1 = Some(value),
            _ => {}
        }
    }
    let (Some(t), Some(v1)) = (t, v1) else {
        return Err(bad("서명 헤더 형식"));
    };
    let sent: i64 = t.parse().map_err(|_| bad("서명 헤더 형식"))?;
    let v1 = from_hex(v1).ok_or_else(|| bad("서명 헤더 형식"))?;
    // W2
    if (state.clock.now() - sent).abs() > WEBHOOK_TOLERANCE_SECS {
        return Err(bad("서명 시각이 너무 멀다"));
    }
    // W3
    if !state.webhook_secrets.verifies(t, &body, &v1) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "서명이 맞지 않다"));
    }
    let event: WebhookEvent = serde_json::from_slice(&body).map_err(|_| bad("본문 형식"))?;
    // W5: 모르는 종류는 무시하고 2xx.
    if event.kind != "user.deleted" {
        return Ok(StatusCode::NO_CONTENT);
    }
    let sub = event
        .sub
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("sub 없음"))?;
    let erased = store::erase(&state.pool, EraseTarget::PracticeOf(&sub)).await?;
    let sessions = store::end_sessions_of(&state.pool, &sub).await?;
    tracing::info!(
        submissions = erased.submissions,
        usage_unlinked = erased.usage_unlinked,
        sessions,
        "탈퇴 — 개인 연습 기록·세션 삭제"
    );
    Ok(StatusCode::NO_CONTENT)
}
