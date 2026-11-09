//! 고객사 관리자의 응시자 데이터 삭제와 감사 로그(#55). 규칙은 [`promptlab_shared::erasure`], 저장은
//! [`promptlab_store::erasure`](#54 삭제 서비스 [`promptlab_store::retention::erase_in`]를 그대로 쓴다).
//!
//! - `POST /orgs/{id}/exams/{exam}/invites/{invite}/erase` — 구성원 관리(관리자)만. 지운 배정을 다시 요청하면 처음의
//!   감사 기록을 그대로 돌려준다(200, 멱등). 그런 배정도 기록도 없으면 404.
//! - `GET /orgs/{id}/erasures[?email=…]` — 구성원 관리(관리자)만. `email`을 주면 그 응시자 식별자 해시로 거른다
//!   (이메일은 저장하지 않고 요청에서 해시만 만든다).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use promptlab_shared::account::Permission;
use promptlab_shared::erasure::{
    AUDIT_RETENTION_DAYS, EraseRequest, ErasureLog, ErasureRecord, normalize_reason, subject_hash,
};
use promptlab_store::erasure::{self as store, EraseBy, EraseOutcome};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Account;
use crate::exams::find_exam;
use crate::orgs::require;
use crate::{ApiError, AppState};

fn invite_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 응시 배정이 없다")
}

/// `POST /orgs/{id}/exams/{exam}/invites/{invite}/erase`
pub async fn erase_assignment(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id, invite_id)): Path<(String, String, String)>,
    Json(request): Json<EraseRequest>,
) -> Result<Json<ErasureRecord>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageMembers).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    let invite_id: Uuid = invite_id.parse().map_err(|_| invite_not_found())?;
    let reason = normalize_reason(request.reason.as_deref())
        .map_err(|m| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, m))?;
    let by = EraseBy {
        sub: &account.sub,
        reason: reason.as_deref(),
        now: state.clock.now(),
    };
    match store::erase_assignment(&state.pool, org.org_id, exam.id, invite_id, by).await? {
        EraseOutcome::Erased(record) => {
            // 로그에는 식별자·건수만(이메일·사유 없음).
            tracing::info!(
                org_id = %org.org_id,
                exam_id = %exam.id,
                %invite_id,
                audit_id = record.id,
                submissions = record.erased.submissions,
                webcam_photos = record.erased.webcam_photos,
                "관리자가 응시자 데이터를 지웠다"
            );
            Ok(Json(record))
        }
        EraseOutcome::Already(record) => Ok(Json(record)),
        EraseOutcome::NotFound => Err(invite_not_found()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ErasureQuery {
    email: Option<String>,
}

/// `GET /orgs/{id}/erasures`
pub async fn erasure_log(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Query(query): Query<ErasureQuery>,
) -> Result<Json<ErasureLog>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageMembers).await?;
    let subject = query
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| subject_hash(&org.org_id.to_string(), e));
    let records = store::list(&state.pool, org.org_id, subject.as_deref()).await?;
    Ok(Json(ErasureLog {
        records,
        retention_days: AUDIT_RETENTION_DAYS,
    }))
}
