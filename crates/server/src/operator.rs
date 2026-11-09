//! 운영자 화면 API(#19, `/operator/*`). 경로표·결정은 [`promptlab_shared::operator`].
//!
//! 운영자 = 대표 계정([`AccountKind::Owner`]) 하나 — 일반 사용자·기계 주체는 `403`([`require_operator`]).
//! 히든 테스트·모범 답안이 실리는 응답은 운영자 문항 편집([`BankProblem`])뿐이다.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use promptlab_shared::authoring::{PreviewRequest, PreviewView, ProblemSource, Publication};
use promptlab_shared::integrity::{self, Allowlist};
use promptlab_shared::operator::{
    AllowlistChangeRequest, AllowlistView, ApprovalRequest, BankProblem, BankProblemSummary,
    CreateBankProblemRequest, MAX_ALLOW_NOTE_CHARS, MAX_RECENT_FAILURES, MAX_RETIRE_REASON_CHARS,
    METRICS_WINDOW_HOURS, OperatorMetrics, OperatorOrg, RetireRequest,
};
use promptlab_store::checks::Scope;
use promptlab_store::{accounts, bank, operator, usage};
use uuid::Uuid;

use crate::auth::Account;
use crate::auth::token::AccountKind;
use crate::problems::validate;
use crate::{ApiError, AppState};

/// 허용 목록 이력을 몇 개까지 보이나.
pub const ALLOWLIST_HISTORY_LIMIT: i64 = 200;

/// 운영자(대표 계정)만. 아니면 `403`.
pub fn require_operator(account: &Account) -> Result<(), ApiError> {
    if account.kind == AccountKind::Owner {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "운영자만 할 수 있다"))
    }
}

/// 운영자 작업 전에 대표 계정을 사용자 표에 둔다(이력의 `…_by` 외래키).
async fn operator(state: &AppState, account: &Account) -> Result<(), ApiError> {
    require_operator(account)?;
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    Ok(())
}

fn unprocessable(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
}

fn not_found(what: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("그런 {what}이 없다"))
}

/// 승인 전 조직이면 `403` — 시험 만들기와 문항 시작이 부른다. 스모크 조직은 언제나 열려 있다.
pub async fn require_approved(state: &AppState, org_id: Uuid) -> Result<(), ApiError> {
    if operator::org_open(&state.pool, org_id).await? {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "운영자 승인 전인 조직이다 — 승인되면 시험을 열 수 있다",
        ))
    }
}

/// 지금 받는 허용 목록(상수 ∪ 더한 값 − 막은 값). 문항 시작이 부른다.
pub async fn allowlist(state: &AppState) -> Result<Allowlist, ApiError> {
    let overrides = operator::allowlist_overrides(&state.pool).await?;
    Ok(Allowlist::with_overrides(&overrides))
}

// ── 조직 승인 ──

/// `GET /operator/orgs`.
pub async fn orgs(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<Vec<OperatorOrg>>, ApiError> {
    require_operator(&account)?;
    Ok(Json(operator::orgs(&state.pool).await?))
}

/// `PUT /operator/orgs/{id}/approval`.
pub async fn set_approval(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Json(request): Json<ApprovalRequest>,
) -> Result<Json<OperatorOrg>, ApiError> {
    operator(&state, &account).await?;
    let org_id: Uuid = org_id.parse().map_err(|_| not_found("조직"))?;
    let now = state.clock.now();
    let org = operator::set_approval(&state.pool, org_id, &account.sub, request.approved, now)
        .await?
        .ok_or_else(|| not_found("조직"))?;
    tracing::info!(%org_id, approved = request.approved, "조직 승인 바꿈");
    Ok(Json(org))
}

// ── 문제은행 ──

/// `GET /operator/problems`.
pub async fn problems(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<Vec<BankProblemSummary>>, ApiError> {
    require_operator(&account)?;
    Ok(Json(bank::list(&state.pool).await?))
}

/// `GET /operator/problems/{id}` — 운영자 문항만(내장은 저장소 파일이 정본).
pub async fn problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
) -> Result<Json<BankProblem>, ApiError> {
    require_operator(&account)?;
    Ok(Json(
        bank::get(&state.pool, &id)
            .await?
            .ok_or_else(|| not_found("운영자 문항"))?,
    ))
}

/// `POST /operator/problems`.
pub async fn create_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Json(request): Json<CreateBankProblemRequest>,
) -> Result<(StatusCode, Json<BankProblem>), ApiError> {
    operator(&state, &account).await?;
    let source = validate(request.source)?;
    let now = state.clock.now();
    let problem = bank::create(&state.pool, &account.sub, request.practice, &source, now).await?;
    tracing::info!(problem_id = %problem.id, practice = problem.practice, "운영자 문항 만듦");
    Ok((StatusCode::CREATED, Json(problem)))
}

/// `PUT /operator/problems/{id}` — 운영자 문항만 고친다(연습/시험 구분은 그대로).
pub async fn update_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
    Json(source): Json<ProblemSource>,
) -> Result<Json<BankProblem>, ApiError> {
    operator(&state, &account).await?;
    if bank::get(&state.pool, &id).await?.is_none() {
        return Err(not_found("운영자 문항"));
    }
    let source = validate(source)?;
    let now = state.clock.now();
    let problem = bank::update(&state.pool, &id, &account.sub, &source, now)
        .await?
        .ok_or_else(|| not_found("운영자 문항"))?;
    tracing::info!(problem_id = %id, "운영자 문항 고침");
    Ok(Json(problem))
}

/// `POST /operator/problem-checks` — 운영자 문항의 발행 전 점검(#73)을 큐에 넣는다(내장 문항은 측정 기록이 대신해 `404`).
pub async fn create_check(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<PreviewView>), ApiError> {
    operator(&state, &account).await?;
    let view =
        crate::problems::start_check(&state, Scope::Operator, &account.sub, &request.problem_id)
            .await?;
    Ok((StatusCode::ACCEPTED, Json(view)))
}

/// `GET /operator/problem-checks/{check}`.
pub async fn get_check(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
) -> Result<Json<PreviewView>, ApiError> {
    require_operator(&account)?;
    crate::problems::check_view(&state, Scope::Operator, &id)
        .await
        .map(Json)
}

/// `POST /operator/problems/{id}/publish` — 운영자 문항을 발행한다(점검 미통과면 `422`).
pub async fn publish_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
) -> Result<Json<Publication>, ApiError> {
    operator(&state, &account).await?;
    crate::problems::publish_in(&state, Scope::Operator, &account.sub, &id)
        .await
        .map(Json)
}

/// `PUT /operator/problems/{id}/retirement` — 내리기·되살리기(내장·운영자 문항).
pub async fn set_retirement(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(id): Path<String>,
    Json(request): Json<RetireRequest>,
) -> Result<Json<BankProblemSummary>, ApiError> {
    operator(&state, &account).await?;
    let reason = request.reason.trim();
    if reason.chars().count() > MAX_RETIRE_REASON_CHARS {
        return Err(unprocessable(format!(
            "까닭은 {MAX_RETIRE_REASON_CHARS}자까지다"
        )));
    }
    let now = state.clock.now();
    let summary = bank::set_retired(&state.pool, &id, &account.sub, request.retired, reason, now)
        .await?
        .ok_or_else(|| not_found("문항"))?;
    tracing::info!(problem_id = %id, retired = request.retired, "문항 내리기 바꿈");
    Ok(Json(summary))
}

// ── 지표 ──

/// `GET /operator/metrics`.
pub async fn metrics(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<OperatorMetrics>, ApiError> {
    require_operator(&account)?;
    let now = state.clock.now();
    let queues = operator::queue_stats(&state.pool, now, METRICS_WINDOW_HOURS).await?;
    let failures = operator::recent_failures(&state.pool, MAX_RECENT_FAILURES).await?;
    let costs = usage::operator_costs(&state.pool).await?;
    let total_cost_usd = costs.iter().map(|c| c.cost_usd).sum();
    Ok(Json(OperatorMetrics {
        window_hours: METRICS_WINDOW_HOURS,
        queues,
        failures,
        costs,
        total_cost_usd,
    }))
}

// ── 허용 목록 ──

async fn allowlist_view(state: &AppState) -> Result<AllowlistView, ApiError> {
    Ok(AllowlistView {
        constants: Allowlist::constants(),
        effective: allowlist(state).await?,
        history: operator::allowlist_history(&state.pool, ALLOWLIST_HISTORY_LIMIT).await?,
    })
}

/// `GET /operator/allowlist`.
pub async fn get_allowlist(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<AllowlistView>, ApiError> {
    require_operator(&account)?;
    Ok(Json(allowlist_view(&state).await?))
}

/// `POST /operator/allowlist` — 변경 하나를 이력에 덧붙인다.
pub async fn change_allowlist(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Json(request): Json<AllowlistChangeRequest>,
) -> Result<(StatusCode, Json<AllowlistView>), ApiError> {
    operator(&state, &account).await?;
    let value = integrity::validate_change(request.kind, &request.value, request.action)
        .map_err(unprocessable)?;
    let note = request.note.trim();
    if note.chars().count() > MAX_ALLOW_NOTE_CHARS {
        return Err(unprocessable(format!(
            "메모는 {MAX_ALLOW_NOTE_CHARS}자까지다"
        )));
    }
    let now = state.clock.now();
    operator::record_allowlist_change(
        &state.pool,
        request.kind,
        &value,
        request.action,
        note,
        &account.sub,
        now,
    )
    .await?;
    tracing::info!(kind = request.kind.as_str(), %value, action = request.action.as_str(), "허용 목록 바꿈");
    Ok((StatusCode::CREATED, Json(allowlist_view(&state).await?)))
}
