//! 조직 문제 편집(#16)·발행 전 점검·발행(#73) API. 경로표는 [`promptlab_shared::authoring`].
//!
//! 모두 자기 조직의 시험 관리 역할(관리자·출제자)만 — 구성원이 아니면 `404`, 역할이 모자라면 `403`
//! ([`crate::orgs::require`]). 문제 편집 응답([`OrgProblem`])에만 히든 테스트·모범 답안이 실린다(출제자 자신의 것) —
//! 점검 결과([`PreviewView`])에는 케이스 입력·출력이 없다(통과 수·발행 가능·짧은 이유만).
//! 저장 전 검증은 [`ProblemSource::validate`](서명·케이스·한도).
//! 점검(모범 답안을 판정기로 예제+히든에 돌리기)은 채점 워커가 한다(점검 큐, 채점이 먼저) — 서버는 넣고 상태만 돌려준다. 발행은 지금 원본의 마지막
//! 점검이 모두 통과일 때만([`promptlab_store::checks::publish`]). 운영자 문항(#19)도 같은 함수([`start_check`]·
//! [`check_view`]·[`publish_in`])를 범위만 바꿔 부른다.
//!
//! HTTP 핸들러와 MCP 도구([`crate::mcp`], #17)가 같은 서비스 함수([`list`]·[`get`]·[`create`]·[`update`]·
//! [`start_preview`]·[`preview`]·[`publish`])를 부른다 — 권한·검증·큐 한도가 한 곳에 있다.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use promptlab_shared::account::Permission;
use promptlab_shared::authoring::{
    OrgProblem, OrgProblemSummary, PreviewRequest, PreviewState, PreviewView, ProblemSource,
    Publication,
};
use promptlab_store::checks::{self, EnqueueError, PublishOutcome, Scope};
use promptlab_store::{accounts, problems};
use uuid::Uuid;

use crate::auth::Account;
use crate::orgs::require;
use crate::{ApiError, AppState};

fn unprocessable(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 문제가 없다")
}

/// 원본 검증([`ProblemSource::validate`] — 제목·설명·서명·케이스·모범 답안 길이). 어긋나면 422.
pub(crate) fn validate(source: ProblemSource) -> Result<ProblemSource, ApiError> {
    source.validate().map(|()| source).map_err(unprocessable)
}

// ── 서비스 함수(HTTP·MCP 공용) ──

/// 조직 문제 목록(히든 테스트 없음).
pub async fn list(
    state: &AppState,
    account: &Account,
    org_id: &str,
) -> Result<Vec<OrgProblemSummary>, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    Ok(problems::list(&state.pool, org.org_id).await?)
}

/// 조직 문제 하나(히든 테스트·모범 답안 포함 — 자기 조직의 시험 관리 역할에게만).
pub async fn get(
    state: &AppState,
    account: &Account,
    org_id: &str,
    problem_id: &str,
) -> Result<OrgProblem, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    problems::get(&state.pool, org.org_id, problem_id)
        .await?
        .ok_or_else(not_found)
}

/// 검증하고 새 조직 문제로 저장한다.
pub async fn create(
    state: &AppState,
    account: &Account,
    org_id: &str,
    source: ProblemSource,
) -> Result<OrgProblem, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    let source = validate(source)?;
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let problem = problems::create(
        &state.pool,
        org.org_id,
        &account.sub,
        &source,
        state.clock.now(),
    )
    .await?;
    tracing::info!(org_id = %org.org_id, problem_id = %problem.id, by = %account.sub, "조직 문제 만듦");
    Ok(problem)
}

/// 검증하고 조직 문제를 고친다.
pub async fn update(
    state: &AppState,
    account: &Account,
    org_id: &str,
    problem_id: &str,
    source: ProblemSource,
) -> Result<OrgProblem, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    // 다른 조직의 문제인지 먼저 본다(검증 오류로 문제가 있는지 알리지 않는다).
    if problems::get(&state.pool, org.org_id, problem_id)
        .await?
        .is_none()
    {
        return Err(not_found());
    }
    let source = validate(source)?;
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let problem = problems::update(
        &state.pool,
        org.org_id,
        problem_id,
        &account.sub,
        &source,
        state.clock.now(),
    )
    .await?
    .ok_or_else(not_found)?;
    tracing::info!(org_id = %org.org_id, problem_id = %problem.id, by = %account.sub, "조직 문제 고침");
    Ok(problem)
}

/// 저장된 문제의 지금 원본으로 발행 전 점검을 큐에 넣는다(채점 큐가 먼저, 범위마다 진행 중
/// [`promptlab_shared::authoring::MAX_ACTIVE_PREVIEWS`]개 — 넘으면 `429`). 범위에 없는 문제는 `404`.
pub(crate) async fn start_check(
    state: &AppState,
    scope: Scope,
    by: &str,
    problem_id: &str,
) -> Result<PreviewView, ApiError> {
    let id = match checks::enqueue(&state.pool, scope, by, problem_id).await? {
        Ok(id) => id,
        Err(EnqueueError::NotFound) => return Err(not_found()),
        Err(EnqueueError::Busy) => {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "진행 중인 발행 전 점검이 많다 — 앞의 점검이 끝난 뒤 다시",
            ));
        }
    };
    tracing::info!(%id, problem_id, by, "발행 전 점검 넣음");
    check_view(state, scope, &id.to_string()).await
}

/// 점검 상태·결과(범위의 것만, 없으면 `404`).
pub(crate) async fn check_view(
    state: &AppState,
    scope: Scope,
    check_id: &str,
) -> Result<PreviewView, ApiError> {
    let missing = || ApiError::new(StatusCode::NOT_FOUND, "그런 점검이 없다");
    let id = Uuid::parse_str(check_id).map_err(|_| missing())?;
    checks::get(&state.pool, scope, id)
        .await?
        .ok_or_else(missing)
}

/// 발행한다. 지금 원본의 마지막 점검이 없거나·안 끝났거나·통과하지 못했으면 `422`(까닭 + 안내).
pub(crate) async fn publish_in(
    state: &AppState,
    scope: Scope,
    by: &str,
    problem_id: &str,
) -> Result<Publication, ApiError> {
    match checks::publish(&state.pool, scope, problem_id, by, state.clock.now()).await? {
        PublishOutcome::NotFound => Err(not_found()),
        PublishOutcome::NotChecked => Err(unprocessable(
            "발행 전 점검을 지금 원본으로 돌려야 한다(점검하지 않았거나 고친 뒤 다시 점검하지 않았다)",
        )),
        PublishOutcome::NotPassed(check) => Err(unprocessable(match check.state {
            PreviewState::Queued | PreviewState::Running => {
                "발행 전 점검이 아직 끝나지 않았다".to_owned()
            }
            PreviewState::Failed => format!(
                "발행 전 점검이 실패했다({}) — 다시 점검하라",
                check.failure_reason.as_deref().unwrap_or("까닭 모름")
            ),
            PreviewState::Done => format!(
                "발행 전 점검을 통과하지 못했다: {}",
                check
                    .reason
                    .as_deref()
                    .unwrap_or("모범 답안이 모든 케이스를 통과해야 한다")
            ),
        })),
        PublishOutcome::Published(publication) => {
            tracing::info!(problem_id, by, "문제 발행");
            Ok(*publication)
        }
    }
}

/// 조직 문제의 발행 전 점검을 큐에 넣는다.
pub async fn start_preview(
    state: &AppState,
    account: &Account,
    org_id: &str,
    request: PreviewRequest,
) -> Result<PreviewView, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    start_check(
        state,
        Scope::Org(org.org_id),
        &account.sub,
        &request.problem_id,
    )
    .await
}

/// 조직 문제 점검 상태·결과.
pub async fn preview(
    state: &AppState,
    account: &Account,
    org_id: &str,
    preview_id: &str,
) -> Result<PreviewView, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    check_view(state, Scope::Org(org.org_id), preview_id).await
}

/// 조직 문제를 발행한다.
pub async fn publish(
    state: &AppState,
    account: &Account,
    org_id: &str,
    problem_id: &str,
) -> Result<Publication, ApiError> {
    let org = require(state, org_id, account, Permission::ManageExams).await?;
    publish_in(state, Scope::Org(org.org_id), &account.sub, problem_id).await
}

// ── HTTP 핸들러 ──

/// `GET /orgs/{id}/problems`
pub async fn list_problems(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<OrgProblemSummary>>, ApiError> {
    list(&state, &account, &org_id).await.map(Json)
}

/// `POST /orgs/{id}/problems`
pub async fn create_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Json(source): Json<ProblemSource>,
) -> Result<(StatusCode, Json<OrgProblem>), ApiError> {
    let problem = create(&state, &account, &org_id, source).await?;
    Ok((StatusCode::CREATED, Json(problem)))
}

/// `GET /orgs/{id}/problems/{problem}`
pub async fn get_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, problem_id)): Path<(String, String)>,
) -> Result<Json<OrgProblem>, ApiError> {
    get(&state, &account, &org_id, &problem_id).await.map(Json)
}

/// `PUT /orgs/{id}/problems/{problem}`
pub async fn update_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, problem_id)): Path<(String, String)>,
    Json(source): Json<ProblemSource>,
) -> Result<Json<OrgProblem>, ApiError> {
    update(&state, &account, &org_id, &problem_id, source)
        .await
        .map(Json)
}

/// `POST /orgs/{id}/problem-previews`
pub async fn create_preview(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<PreviewView>), ApiError> {
    let view = start_preview(&state, &account, &org_id, request).await?;
    Ok((StatusCode::ACCEPTED, Json(view)))
}

/// `GET /orgs/{id}/problem-previews/{preview}`
pub async fn get_preview(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, preview_id)): Path<(String, String)>,
) -> Result<Json<PreviewView>, ApiError> {
    preview(&state, &account, &org_id, &preview_id)
        .await
        .map(Json)
}

/// `POST /orgs/{id}/problems/{problem}/publish`
pub async fn publish_problem(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, problem_id)): Path<(String, String)>,
) -> Result<Json<Publication>, ApiError> {
    publish(&state, &account, &org_id, &problem_id)
        .await
        .map(Json)
}
