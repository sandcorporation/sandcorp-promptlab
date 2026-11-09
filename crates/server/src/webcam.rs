//! 웹캠 감독(#13): 수집 항목 동의, 기준 얼굴 사진, 스냅샷 받기·감독관 열람, 보유 기간 삭제.
//! 규칙은 [`promptlab_shared::webcam`], 저장은 [`promptlab_store::webcam`].
//!
//! - 사람 응시자는 지금 판 안내문에 동의하고 기준 사진을 올려야 문항을 시작한다([`require_ready`]).
//!   스모크 기계 주체(#28)는 면제 — 사람이 아니고, VM에 카메라가 없다. 기계 주체는 이 경로들을 부를 수도 없다
//!   (`auth::machine_may`).
//! - 사진은 `image/jpeg` 한 장씩, 64KB·320×320 이하만 받는다. 영상(전체 녹화)을 받는 경로는 없다.
//! - 열람은 감독 권한(관리자·감독관)만, 인증된 요청으로만(공개 URL 없음, `Cache-Control: no-store`).

use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use promptlab_shared::account::Permission;
use promptlab_shared::api::AssignmentView;
use promptlab_shared::webcam::{
    ConsentRequest, MAX_SNAPSHOTS_PER_ITEM, NOTICE_VERSION, SnapshotReason, SnapshotStored,
    WebcamTerms, WebcamView, check_snapshot,
};
use promptlab_store::exams::{self, Assignment};
use promptlab_store::webcam::{self as store, NewSnapshot, SnapshotOutcome};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Account;
use crate::auth::token::{AccountKind, Clock};
use crate::exams::my_item;
use crate::logs::invite_with;
use crate::{ApiError, AppState};

fn assignment_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 배정이 없다")
}

/// 그 사람의 배정. 남의 것·없는 것은 404.
async fn my_assignment(
    state: &AppState,
    account: &Account,
    id: &str,
) -> Result<Assignment, ApiError> {
    let id: Uuid = id.parse().map_err(|_| assignment_not_found())?;
    exams::assignments_of(&state.pool, &account.sub, Some(id))
        .await?
        .pop()
        .ok_or_else(assignment_not_found)
}

/// 배정의 웹캠 조건.
async fn terms(
    state: &AppState,
    account: &Account,
    invite_id: Uuid,
) -> Result<WebcamTerms, ApiError> {
    let consent = store::consent(&state.pool, invite_id).await?;
    Ok(WebcamTerms {
        notice_version: NOTICE_VERSION.to_owned(),
        retention_days: state.webcam_retention_days,
        consented_at: consent
            .filter(|(version, _)| version == NOTICE_VERSION)
            .map(|(_, at)| at),
        baseline_uploaded: store::baseline(&state.pool, invite_id).await?.is_some(),
        required: account.kind != AccountKind::Machine,
    })
}

/// 배정 목록에 웹캠 조건을 붙인다(`GET /me/assignments`).
pub(crate) async fn with_terms(
    state: &AppState,
    account: &Account,
    mut list: Vec<AssignmentView>,
) -> Result<Vec<AssignmentView>, ApiError> {
    for view in &mut list {
        if let Ok(id) = view.id.parse() {
            view.webcam = Some(terms(state, account, id).await?);
        }
    }
    Ok(list)
}

/// 문항 시작 전 조건: 지금 판 동의 + 기준 사진. 기계 주체(스모크)는 면제.
pub(crate) async fn require_ready(
    state: &AppState,
    account: &Account,
    invite_id: Uuid,
) -> Result<(), ApiError> {
    if account.kind == AccountKind::Machine {
        return Ok(());
    }
    let t = terms(state, account, invite_id).await?;
    if t.consented_at.is_none() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "웹캠·수집 항목 안내에 동의해야 시작할 수 있다",
        ));
    }
    if !t.baseline_uploaded {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "기준 얼굴 사진을 먼저 찍어야 시작할 수 있다",
        ));
    }
    Ok(())
}

/// `POST /assignments/{id}/consent` — 수집 항목 안내 동의(시각·안내문 판을 남긴다).
pub async fn consent(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
    Json(req): Json<ConsentRequest>,
) -> Result<Json<WebcamTerms>, ApiError> {
    let assignment = my_assignment(&state, &account, &id).await?;
    if req.notice_version != NOTICE_VERSION {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "안내문이 바뀌었다 — 새 안내를 읽고 다시 동의해 달라",
        ));
    }
    store::record_consent(
        &state.pool,
        assignment.id,
        NOTICE_VERSION,
        state.clock.now(),
    )
    .await?;
    tracing::info!(invite_id = %assignment.id, notice = NOTICE_VERSION, "웹캠 수집 동의");
    Ok(Json(terms(&state, &account, assignment.id).await?))
}

/// 본문이 받을 수 있는 사진(JPEG 한 장, 저해상도)인가.
fn photo_body(headers: &HeaderMap, body: &Bytes) -> Result<(), ApiError> {
    let jpeg = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("image/jpeg"))
        });
    if !jpeg {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "JPEG 사진 한 장만 받는다(image/jpeg)",
        ));
    }
    check_snapshot(body).map(|_| ()).map_err(|r| {
        let status = match r {
            promptlab_shared::webcam::PhotoRejection::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError::new(status, r.message())
    })
}

async fn require_consent(state: &AppState, invite_id: Uuid) -> Result<(), ApiError> {
    match store::consent(&state.pool, invite_id).await? {
        Some((version, _)) if version == NOTICE_VERSION => Ok(()),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "동의 전에는 사진을 받지 않는다",
        )),
    }
}

/// `PUT /assignments/{id}/webcam/baseline` — 기준 얼굴 사진(첫 문항을 시작하기 전까지 바꿀 수 있다).
pub async fn put_baseline(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebcamTerms>, ApiError> {
    let assignment = my_assignment(&state, &account, &id).await?;
    photo_body(&headers, &body)?;
    require_consent(&state, assignment.id).await?;
    if !assignment.started.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "문항을 시작한 뒤에는 기준 사진을 바꿀 수 없다",
        ));
    }
    store::put_baseline(&state.pool, assignment.id, &body, state.clock.now()).await?;
    Ok(Json(terms(&state, &account, assignment.id).await?))
}

/// 스냅샷 업로드 질의.
#[derive(Debug, Deserialize)]
pub struct SnapshotQuery {
    reason: String,
    /// 앱이 붙인 참조(과정 로그의 웹캠 사건이 가리킨다). 같은 참조는 한 번만 저장한다.
    #[serde(rename = "ref")]
    client_ref: Option<String>,
    /// 앱 시계(유닉스 밀리초).
    at_ms: Option<i64>,
}

/// `POST /assignments/{id}/items/{n}/snapshots?reason=…&ref=…&at_ms=…` — 스냅샷 한 장(시작한 문항).
pub async fn upload_snapshot(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((id, position)): Path<(String, String)>,
    Query(q): Query<SnapshotQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<SnapshotStored>), ApiError> {
    let (assignment, position) = my_item(&state, &account, &id, &position).await?;
    let reason = SnapshotReason::parse(&q.reason)
        .ok_or_else(|| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "모르는 스냅샷 까닭"))?;
    if q.client_ref
        .as_deref()
        .is_some_and(|r| r.is_empty() || r.len() > 64)
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "참조는 1~64자",
        ));
    }
    photo_body(&headers, &body)?;
    require_consent(&state, assignment.id).await?;
    if !assignment.started.contains_key(&position) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "시작하지 않은 문항이다",
        ));
    }
    let snap = NewSnapshot {
        invite_id: assignment.id,
        position,
        reason,
        client_ref: q.client_ref.as_deref(),
        app_at_ms: q.at_ms,
        jpeg: &body,
    };
    match store::add_snapshot(
        &state.pool,
        &snap,
        MAX_SNAPSHOTS_PER_ITEM,
        state.clock.now(),
    )
    .await?
    {
        SnapshotOutcome::Stored(id) => Ok((
            StatusCode::CREATED,
            Json(SnapshotStored { id: id.to_string() }),
        )),
        SnapshotOutcome::Duplicate(id) => {
            Ok((StatusCode::OK, Json(SnapshotStored { id: id.to_string() })))
        }
        SnapshotOutcome::Full => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "이 문항의 스냅샷 상한을 넘었다",
        )),
    }
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/webcam` — 감독관이 보는 기준 사진·스냅샷 목록.
pub async fn view(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id)): Path<(String, String, String)>,
) -> Result<Json<WebcamView>, ApiError> {
    let (_, invite, candidate) = invite_with(
        &state,
        &account,
        &org_id,
        &exam_id,
        &invite_id,
        Permission::Proctor,
    )
    .await?;
    Ok(Json(WebcamView {
        candidate,
        consent: store::consent(&state.pool, invite.id).await?,
        baseline: store::baseline(&state.pool, invite.id).await?,
        snapshots: store::snapshots(&state.pool, invite.id).await?,
        retention_days: state.webcam_retention_days,
    }))
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/webcam/photos/{photo}` — 사진 한 장(JPEG).
pub async fn photo(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id, photo_id)): Path<(String, String, String, String)>,
) -> Result<Response, ApiError> {
    let (_, invite, _) = invite_with(
        &state,
        &account,
        &org_id,
        &exam_id,
        &invite_id,
        Permission::Proctor,
    )
    .await?;
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "그런 사진이 없다");
    let photo_id: Uuid = photo_id.parse().map_err(|_| not_found())?;
    let jpeg = store::photo(&state.pool, invite.id, photo_id)
        .await?
        .ok_or_else(not_found)?;
    let mut res = jpeg.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(res)
}

/// 보유 기간이 지난 사진을 한 번 지운다. 지운 장수.
pub async fn purge_once(
    pool: &PgPool,
    clock: &dyn Clock,
    retention_days: u32,
) -> Result<u64, sqlx::Error> {
    let cutoff = clock.now() - i64::from(retention_days) * 86_400;
    store::purge_older_than(pool, cutoff).await
}

/// 한 시간마다 보유 기간이 지난 사진을 지운다(서버 `main`이 띄운다).
pub async fn purge_loop(pool: PgPool, clock: Arc<dyn Clock>, retention_days: u32) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        match purge_once(&pool, clock.as_ref(), retention_days).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                deleted = n,
                retention_days,
                "보유 기간이 지난 웹캠 사진 삭제"
            ),
            Err(e) => tracing::warn!(error = %e, "웹캠 사진 삭제 실패 — 다음 주기에 다시"),
        }
    }
}
